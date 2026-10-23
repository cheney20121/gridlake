//! 店铺资金流水服务 / 大数据表引擎（Rust + DuckDB）
//!
//! 用法：
//!   gridlake                      启动服务
//!   gridlake check                离线检查数据文件（服务需停止）
//!   gridlake backup [目录]         离线备份（服务运行中用 POST /api/admin/backup）
//!   gridlake restore <文件> [--yes] 从备份恢复（服务需停止）
//!   gridlake verify <文件> [--full] 验证备份文件能用
//!   gridlake version
//!
//! 环境变量见 config.rs（LEDGER_ADDR / LEDGER_DB / LEDGER_WEB_DIR / LEDGER_MEMORY_LIMIT / LEDGER_THREADS /
//! LEDGER_BACKUP_AT / LEDGER_BACKUP_KEEP / LEDGER_BACKUP_DIR / LEDGER_CORS / LEDGER_ADMIN_TOKEN / LEDGER_MAX_UPLOAD_MB /
//! LEDGER_AUTH_MODE / LEDGER_AUTH_URL / LEDGER_ADMINS / LEDGER_AUDIT_DAYS / RUST_LOG）

// 项目有意保留的写法：接口处理函数 / 查询构造函数参数多，行级结果和 SQL 片段的元组类型复杂，from_xxx 是实例方法
#![allow(clippy::too_many_arguments, clippy::type_complexity, clippy::wrong_self_convention)]
// 生产代码不用 unwrap()（出错该返回错误，而不是让线程 panic）；expect("…") 只用于写明了理由的不变量。测试里随意
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod admin;
mod ai;
mod aggregate;
mod anomaly;
mod approval;
mod account;
mod auth;
mod calc;
mod config;
mod connector;
mod comment;
mod collab;
mod dataset;
mod dataset_trash;
mod dataset_import;
mod db;
mod ecommerce;
mod error;
mod import_source;
mod mcp;
mod metrics;
mod msg_i18n;
mod netguard;
mod oidc;
mod pat;
mod share;
mod pivot;
mod pivot_calc;
mod recon;
mod summary;
mod collect;
mod report;
mod routes;
mod schema;
mod scope;
mod sqlq;
mod tabular;
mod template;
mod totp;
mod views;
mod web;
mod workbook;
mod writeback;
mod xref;
mod usage;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use tower_http::compression::predicate::{NotForContentType, Predicate};
use tower_http::compression::{CompressionLayer, CompressionLevel, DefaultPredicate};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;

const USAGE: &str = "用法：
  gridlake                       启动服务
  gridlake check                 离线检查数据文件（服务需停止）
  gridlake backup [目录]          离线备份（服务运行中请用 curl -X POST http://127.0.0.1:8787/api/admin/backup）
  gridlake restore <文件> [--yes]  从备份恢复（服务需停止；原文件改名保留）
  gridlake verify <文件> [--full]  验证备份文件能用（只读打开，服务运行中也可以；--full 全量扫描所有数据块）
  gridlake reset-password <用户名> [--admin]  重置内置账号的密码（服务需停止；新密码从标准输入读取，--admin 同时设为管理员并启用）
  gridlake mcp-bridge --url <地址> --token <令牌>  MCP stdio 转接（给只支持本地 MCP 的 AI 客户端用）
  gridlake version               显示版本";

fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "gridlake=info,tower_http=info".into());
    // LEDGER_LOG_FORMAT=json：每行一个 JSON，便于日志系统采集
    if std::env::var("LEDGER_LOG_FORMAT").is_ok_and(|v| v.trim().eq_ignore_ascii_case("json")) {
        tracing_subscriber::fmt().json().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("serve");
    if matches!(cmd, "version" | "--version" | "-V") {
        println!("gridlake {}", admin::VERSION);
        return Ok(());
    }
    if matches!(cmd, "help" | "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    // MCP 转接不需要数据库与服务配置（在使用者自己的电脑上运行）
    if cmd == "mcp-bridge" {
        return mcp::bridge::run(&args[1..]);
    }
    let config = config::Config::from_env()?;
    match cmd {
        "serve" => tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(serve(config)),
        "check" => admin::cli_check(&config),
        "backup" => admin::cli_backup(&config, args.get(1).map(PathBuf::from)),
        "verify" => {
            let file = args.get(1).filter(|a| !a.starts_with("--")).ok_or_else(|| anyhow::anyhow!("请指定备份文件\n{USAGE}"))?;
            admin::cli_verify(&PathBuf::from(file), args.iter().any(|a| a == "--full"))
        }
        "restore" => {
            let file = args.get(1).filter(|a| !a.starts_with("--")).ok_or_else(|| anyhow::anyhow!("请指定备份文件\n{USAGE}"))?;
            admin::cli_restore(&config, &PathBuf::from(file), args.iter().any(|a| a == "--yes" || a == "-y"))
        }
        "reset-password" => {
            let user = args.get(1).filter(|a| !a.starts_with("--")).ok_or_else(|| anyhow::anyhow!("请指定用户名\n{USAGE}"))?;
            account::cli_reset_password(&config, user, args.iter().any(|a| a == "--admin"))
        }
        other => anyhow::bail!("未知命令：{other}\n{USAGE}"),
    }
}

async fn serve(config: config::Config) -> anyhow::Result<()> {
    let addr = config.addr.clone();
    let max_upload_mb = config.max_upload_mb;
    tracing::info!("gridlake {} 配置：{}", admin::VERSION, config.summary());
    for w in config.exposure_check().map_err(|e| anyhow::anyhow!(e))? {
        tracing::warn!("安全提醒：{w}");
    }

    let state: routes::AppState = Arc::new(db::Db::open(&config)?);
    // 内置账号：还没有任何账号时建管理员（没设置 LEDGER_ADMIN_PASSWORD 时随机生成，只在这里打印一次）
    if config.auth_mode == config::AuthMode::Local {
        let conn = state.connection()?;
        match account::bootstrap(&conn, &config.admin_user, config.admin_password.as_deref()) {
            Ok(Some(generated)) => tracing::warn!("已创建管理员账号「{}」，初始密码：{generated}（请登录后立即修改；不会再次显示）", config.admin_user),
            Ok(None) => {}
            Err(e) => anyhow::bail!("创建管理员账号失败：{}", e.message()),
        }
    }
    let shutdown_state = state.clone();
    tokio::spawn(admin::schedule_backups(state.clone()));
    tokio::spawn(connector::schedule(state.clone()));
    tokio::spawn(report::schedule(state.clone()));
    tokio::spawn(report::alert::schedule(state.clone()));
    tokio::spawn(collect::schedule(state.clone()));
    tokio::spawn(sqlq::schedule(state.clone()));
    tokio::spawn(purge_trash(state.clone()));
    tokio::spawn(flush_usage(state.clone()));
    tokio::spawn(auth::run_audit_writer(state.clone()));

    // 场景包的路由：没启用时不注册，访问得到 404（见 ecommerce/mod.rs）
    let api = Router::new()
        .merge(if ecommerce::enabled() { ecommerce::router(max_upload_mb) } else { Router::new() })
        .route("/ledger/datasets", get(routes::list_datasets))
        .route(
            "/ledger/datasets/import/preview",
            post(routes::dataset_import_preview).layer(DefaultBodyLimit::max(max_upload_mb * 1024 * 1024)),
        )
        .route("/ledger/datasets/import/commit", post(routes::dataset_import_commit))
        .route("/ledger/datasets/import/preview-url", post(routes::dataset_import_preview_url))
        .route("/ledger/datasets/views/preview", post(routes::view_preview))
        .route("/ledger/datasets/views", post(routes::create_view))
        .route("/ledger/datasets/views/{id}", put(routes::update_view))
        .route("/ledger/datasets/trash", get(routes::dataset_trash).delete(routes::empty_dataset_trash))
        .route("/ledger/datasets/input", post(routes::create_input_dataset))
        .route("/ledger/datasets/{id}/writeback", post(routes::dataset_writeback))
        .route("/ledger/datasets/{id}/usage", get(routes::dataset_usage))
        .route("/ledger/datasets/{id}/anomalies", get(routes::dataset_anomalies))
        .route("/ledger/writebacks", get(routes::writebacks))
        .route("/ledger/writebacks/{id}/undo", post(routes::undo_writeback))
        .route("/ledger/datasets/{id}/untrash", post(routes::untrash_dataset))
        .route("/ledger/datasets/{id}/purge", delete(routes::purge_dataset))
        .route("/ledger/datasets/{id}", axum::routing::put(routes::rename_dataset).delete(routes::delete_dataset))
        .route("/ledger/datasets/{id}/cursor", post(routes::open_cursor))
        .route("/ledger/datasets/{id}/rows", get(routes::dataset_rows))
        .route("/ledger/datasets/{id}/stats", post(routes::dataset_stats))
        .route("/ledger/datasets/{id}/distinct", post(routes::dataset_distinct))
        .route("/ledger/datasets/{id}/export", get(routes::dataset_export))
        .route("/ledger/datasets/{id}/aggregate", post(routes::dataset_aggregate))
        .route("/ledger/datasets/{id}/pivot", post(routes::dataset_pivot))
        .route("/ledger/recons/preview", post(routes::recon_preview))
        .route("/ledger/recons", post(routes::create_recon))
        .route("/ledger/datasets/summaries", post(routes::create_summary))
        .route("/ledger/recons/{id}", get(routes::get_recon).put(routes::update_recon))
        .route("/ledger/recons/{id}/summary", get(routes::recon_summary))
        .route("/ledger/recons/{id}/items", get(routes::recon_items))
        .route("/ledger/recons/{id}/groups", get(routes::recon_groups))
        .route("/ledger/recons/{id}/auto", post(routes::recon_auto))
        .route("/ledger/recons/{id}/match", post(routes::recon_match))
        .route("/ledger/recons/{id}/unmatch", post(routes::recon_unmatch))
        .route("/ledger/datasets/{id}/calc/preview", post(routes::calc_preview))
        .route("/ledger/datasets/{id}/calc", post(routes::add_calc))
        .route("/ledger/datasets/{id}/calc/{name}", put(routes::update_calc).delete(routes::remove_calc))
        .route(
            "/ledger/workbooks",
            get(routes::list_workbooks).post(routes::create_workbook).layer(DefaultBodyLimit::max(max_upload_mb * 1024 * 1024)),
        )
        .route("/ledger/workbooks/trash", get(routes::workbook_trash).delete(routes::empty_workbook_trash))
        .route("/ledger/workbooks/move", post(routes::move_workbooks))
        .route("/ledger/workbooks/ranges", post(routes::workbook_ranges))
        .route("/ledger/workbooks/{id}", get(routes::workbook_meta).put(routes::rename_workbook).delete(routes::delete_workbook))
        .route("/ledger/workbooks/{id}/untrash", post(routes::untrash_workbook))
        .route("/ledger/workbooks/{id}/purge", delete(routes::purge_workbook))
        .route("/ledger/workbooks/{id}/star", put(routes::star_workbook))
        .route("/ledger/folders", get(routes::list_folders).post(routes::create_folder))
        .route("/ledger/folders/{id}", put(routes::update_folder).delete(routes::delete_folder))
        .route(
            "/ledger/workbooks/{id}/content",
            get(routes::workbook_content).put(routes::save_workbook).layer(DefaultBodyLimit::max(max_upload_mb * 1024 * 1024)),
        )
        .route("/ledger/workbooks/{id}/duplicate", post(routes::duplicate_workbook))
        .route("/ledger/workbooks/{id}/versions", get(routes::workbook_versions))
        .route("/ledger/workbooks/{id}/versions/{version}/content", get(routes::workbook_version_content))
        .route("/ledger/workbooks/{id}/restore", post(routes::restore_workbook))
        .route("/ledger/connectors", get(routes::list_connectors).post(routes::create_connector))
        .route("/ledger/connectors/test", post(routes::test_connector))
        .route("/ledger/connectors/preview", post(routes::preview_connector))
        .route("/ledger/connectors/{id}", put(routes::update_connector).delete(routes::delete_connector))
        .route("/ledger/connectors/{id}/run", post(routes::run_connector))
        .route("/ledger/connectors/{id}/runs", get(routes::connector_runs))
        .route("/ledger/reports", get(routes::list_reports).post(routes::create_report))
        .route("/ledger/reports/preview", post(routes::preview_report))
        .route("/ledger/reports/sheets", get(routes::report_sheets))
        .route("/ledger/reports/{id}", put(routes::update_report).delete(routes::delete_report))
        .route("/ledger/reports/{id}/send", post(routes::send_report))
        .route("/ledger/reports/{id}/runs", get(routes::report_runs))
        .route("/ledger/collects", get(routes::list_collects).post(routes::create_collect))
        .route("/ledger/collects/mine", get(routes::my_collects))
        .route("/ledger/collects/{id}", get(routes::collect_detail).put(routes::update_collect).delete(routes::delete_collect))
        .route("/ledger/collects/{id}/workbook", post(routes::open_collect_workbook))
        .route("/ledger/collects/{id}/review", post(routes::review_collect))
        .route("/ledger/collects/{id}/remind", post(routes::remind_collect))
        .route("/ledger/collects/{id}/periods", post(routes::open_collect_period))
        .route("/ledger/collects/{id}/periods/{period}", put(routes::set_collect_deadline).delete(routes::delete_collect_period))
        .route("/ledger/alerts", get(routes::list_alerts).post(routes::create_alert))
        .route("/ledger/alerts/preview", post(routes::preview_alert))
        .route("/ledger/alerts/{id}", put(routes::update_alert).delete(routes::delete_alert))
        .route("/ledger/alerts/{id}/evaluate", post(routes::evaluate_alert))
        .route("/ledger/alerts/{id}/runs", get(routes::alert_runs))
        .route("/mcp", get(routes::mcp_get).post(routes::mcp_post).delete(routes::mcp_get))
        .route("/ledger/mcp/tokens", get(routes::mcp_tokens).post(routes::mcp_create_token))
        .route("/ledger/mcp/tokens/{id}", put(routes::mcp_update_token).delete(routes::mcp_revoke_token))
        .route("/ledger/mcp/logs", get(routes::mcp_logs))
        .route("/ledger/sql/run", post(routes::sql_run))
        .route("/ledger/sql/queries", get(routes::sql_queries).post(routes::sql_create_query))
        .route("/ledger/sql/queries/{id}", put(routes::sql_update_query).delete(routes::sql_delete_query))
        .route("/ledger/sql/queries/{id}/refresh", post(routes::sql_refresh_query))
        .route("/ledger/sql/queries/{id}/instances", post(routes::sql_instantiate_query))
        .route("/ledger/ai/status", get(routes::ai_status))
        .route("/ledger/ai/threads", get(routes::ai_threads).post(routes::ai_create_thread))
        .route("/ledger/ai/threads/{id}", get(routes::ai_thread).delete(routes::ai_delete_thread))
        .route("/ledger/ai/threads/{id}/summary", put(routes::ai_set_summary))
        .route("/ledger/ai/threads/{id}/chat", post(routes::ai_chat))
        .route("/ledger/ai/threads/{id}/events", get(routes::ai_events))
        .route("/ledger/ai/threads/{id}/stop", post(routes::ai_stop))
        .route("/ledger/ai/actions/{id}", post(routes::ai_decide))
        .route("/ledger/templates", get(routes::list_templates).post(routes::publish_template))
        .route("/ledger/templates/{id}", put(routes::update_template).delete(routes::delete_template))
        .route("/ledger/templates/{id}/use", post(routes::use_template))
        .route("/ledger/templates/{id}/uses", get(routes::template_uses))
        .route("/ledger/report-channels", post(routes::create_report_channel))
        .route("/ledger/report-channels/{id}", put(routes::update_report_channel).delete(routes::delete_report_channel))
        .route("/ledger/report-channels/{id}/test", post(routes::test_report_channel))
        .route("/ledger/me", get(auth::me))
        .route("/ledger/auth/login", post(account::login_route))
        .route("/ledger/auth/oidc/login", get(oidc::login_route))
        .route("/ledger/auth/oidc/callback", get(oidc::callback_route))
        .route("/ledger/auth/logout", post(account::logout_route))
        .route("/ledger/auth/password", post(account::password_route))
        .route("/ledger/auth/2fa", get(totp::status_route))
        .route("/ledger/auth/2fa/setup", post(totp::setup_route))
        .route("/ledger/auth/2fa/enable", post(totp::enable_route))
        .route("/ledger/auth/2fa/disable", post(totp::disable_route))
        .route("/ledger/tokens", get(pat::list_route).post(pat::create_route))
        .route("/ledger/tokens/{id}", delete(pat::revoke_route))
        .route("/ledger/accounts", get(account::list_route).post(account::create_route))
        .route("/ledger/accounts/{id}", put(account::update_route))
        .route("/ledger/data-scope", get(scope::get_overview))
        .route("/ledger/data-scope/rules", put(scope::put_rule))
        .route("/ledger/data-scope/rules/{id}", delete(scope::remove_rule))
        .route("/ledger/data-scope/datasets/{id}", put(scope::put_binding))
        .route("/ledger/users", get(approval::users))
        .route("/ledger/workbooks/{id}/comments", get(comment::list_comments).post(comment::add_comment))
        .route("/ledger/comments/{id}", put(comment::edit_comment).delete(comment::delete_comment))
        .route("/ledger/comments/{id}/resolve", post(comment::resolve_comment))
        .route("/ledger/collab/{id}/events", get(collab::events))
        .route("/ledger/collab/{id}/send", post(collab::send))
        .route("/ledger/notifications", get(comment::list_notifications))
        .route("/ledger/notifications/read", post(comment::read_notifications))
        .route("/ledger/approvals", get(approval::list_approvals).post(approval::create_approval))
        .route("/ledger/approvals/count", get(approval::count))
        .route("/ledger/approvals/{id}", get(approval::get_approval))
        .route("/ledger/approvals/{id}/decide", post(approval::decide_approval))
        .route("/ledger/approvals/{id}/cancel", post(approval::cancel_approval))
        .route("/ledger/audit", get(auth::audit))
        .route("/ledger/workbooks/{id}/share", put(routes::share_workbook))
        .route("/ledger/share/unlock", post(share::unlock_route))
        .route("/ledger/workbooks/{id}/shares", get(share::list_route).post(share::create_route))
        .route("/ledger/shares/{id}", delete(share::revoke_route))
        .route("/health", get(routes::health))
        .route("/admin/status", get(routes::admin_status))
        .route("/admin/metrics", get(routes::admin_metrics))
        .route("/admin/backup", post(routes::admin_backup))
        .route("/admin/backup/verify", post(routes::admin_backup_verify))
        .fallback(routes::api_not_found)
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::guard));

    // 前端默认按 /ledger-api 访问（开发时由 Vite 代理去掉前缀）：同源部署时两种前缀都可用
    let mut app = Router::new().nest("/api", api.clone()).nest("/ledger-api/api", api);
    if let Some(dir) = &config.web_dir {
        app = app.fallback_service(web::service(dir));
    }
    // 下载类响应（CSV / XLSX、已压缩的工作簿）不再压缩，保留 Content-Length 以显示下载进度
    let compress = DefaultPredicate::new()
        .and(NotForContentType::new("text/csv"))
        .and(NotForContentType::new("application/vnd.openxmlformats"))
        .and(NotForContentType::new("application/octet-stream"));
    let app = app
        .layer(axum::middleware::from_fn(metrics::track))
        // 记下请求语言，错误信息据此翻译（要在认证层外面，认证失败的信息也要翻译）
        .layer(axum::middleware::from_fn(msg_i18n::middleware))
        .layer(CompressionLayer::new().quality(CompressionLevel::Fastest).compress_when(compress))
        .layer(TraceLayer::new_for_http())
        .layer(tower::util::option_layer(cors_layer(config.cors.as_deref())))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("gridlake listening on http://{addr}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(shutdown_signal()).await?;
    // 退出前写入访问计数、合并 WAL，避免下次启动回放
    if let Ok(c) = shutdown_state.connection() {
        let _ = usage::flush(&c);
    }
    shutdown_state.checkpoint();
    tracing::info!("gridlake stopped");
    Ok(())
}

/// 跨域：前端与本服务同源部署（或经 Vite / nginx 代理）时不需要；前后端分开部署时用 LEDGER_CORS 指定前端地址
fn cors_layer(origins: Option<&[String]>) -> Option<CorsLayer> {
    let origins = origins?;
    if origins.iter().any(|o| o == "*") {
        // 任意来源，但不带凭据：接口靠 x-user-token 请求头鉴权，不依赖浏览器自动携带的 cookie / 认证信息，
        // 不用 very_permissive（它会回显请求来源并允许携带凭据）
        return Some(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any).expose_headers(Any));
    }
    let list = origins.iter().filter_map(|o| o.parse().ok()).collect::<Vec<axum::http::HeaderValue>>();
    Some(CorsLayer::new().allow_origin(AllowOrigin::list(list)).allow_methods(Any).allow_headers(Any).expose_headers(Any))
}

/// 回收站自动清理：启动时一次，之后每 6 小时一次（超过 LEDGER_TRASH_DAYS 天的工作簿与数据集彻底删除）
async fn purge_trash(state: routes::AppState) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
    loop {
        tick.tick().await;
        let _guard = state.import_lock.lock().await;
        let st = state.clone();
        let r = tokio::task::spawn_blocking(move || {
            st.connection().map_err(error::AppError::from).and_then(|c| Ok(workbook::purge_expired(&c)? + dataset_trash::purge_expired(&c)?))
        })
        .await;
        match r {
            Ok(Ok(n)) if n > 0 => {
                tracing::info!("回收站自动清理：彻底删除 {n} 个工作簿 / 数据集");
                state.cursors.invalidate();
                state.checkpoint();
            }
            Ok(Err(e)) => tracing::warn!("回收站自动清理失败：{}", e.message()),
            _ => {}
        }
    }
}

/// 数据集访问计数：每分钟从内存写入数据库
async fn flush_usage(state: routes::AppState) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        tick.tick().await;
        let st = state.clone();
        let r = tokio::task::spawn_blocking(move || st.connection().map_err(error::AppError::from).and_then(|c| usage::flush(&c))).await;
        if let Ok(Err(e)) = r {
            tracing::warn!("数据集访问计数写入失败：{}", e.message());
        }
    }
}

/// Ctrl+C 或 SIGTERM：停止接收新请求，等进行中的请求完成后退出
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down…");
}
