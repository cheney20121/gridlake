//! 运维接口：状态、指标、备份

use super::*;

/// 运维接口的访问控制：配置了 LEDGER_ADMIN_TOKEN 时校验请求头 x-admin-token；
/// 未配置时只允许本机直接访问（经过反向代理转发的请求带 X-Forwarded-For 等头，一律拒绝）
fn require_admin(db: &AppState, headers: &axum::http::HeaderMap, peer: std::net::SocketAddr) -> AppResult<()> {
    if let Some(token) = &db.config.admin_token {
        // x-admin-token 请求头，或 Authorization: Bearer <令牌>（Prometheus 抓取指标时用后者）
        let bearer = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
        let given = headers.get("x-admin-token").and_then(|v| v.to_str().ok()).or(bearer).map(str::trim).unwrap_or("");
        // 定长比较，避免按字节提前返回泄露令牌长度 / 前缀
        let same = given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        return if same { Ok(()) } else { Err(AppError::Forbidden("运维令牌无效（请求头 x-admin-token 或 Authorization: Bearer）".into())) };
    }
    let proxied = ["x-forwarded-for", "x-real-ip", "forwarded"].iter().any(|h| headers.contains_key(*h));
    if peer.ip().is_loopback() && !proxied {
        Ok(())
    } else {
        Err(AppError::Forbidden("运维接口只允许在服务器本机访问；如需远程访问请配置 LEDGER_ADMIN_TOKEN".into()))
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminStatus {
    #[serde(flatten)]
    status: crate::admin::Status,
    uptime_sec: u64,
    config: String,
    last_backup: Option<crate::admin::BackupRecord>,
}

/// GET /api/admin/status  运行状态：版本、数据文件大小、各数据集行数、失效的计算列 / 视图、备份
pub async fn admin_status(
    State(db): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
) -> AppResult<Json<Envelope<AdminStatus>>> {
    require_admin(&db, &headers, peer)?;
    let st = db.clone();
    let status = blocking(&db, move |conn| Ok(crate::admin::status(conn, &st.config.db_path, &st.config.backup_dir)?)).await?;
    let last_backup = db.last_backup.lock().ok().and_then(|l| l.clone());
    Ok(ok(AdminStatus { status, uptime_sec: db.started.elapsed().as_secs(), config: db.config.summary(), last_backup }))
}

/// GET /api/admin/metrics  Prometheus 文本格式的运行指标（见 metrics.rs）
pub async fn admin_metrics(
    State(db): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
) -> AppResult<axum::response::Response> {
    use axum::response::IntoResponse;
    require_admin(&db, &headers, peer)?;
    let size = |p: &std::path::Path| std::fs::metadata(p).map_or(0, |m| m.len());
    let mut wal = db.config.db_path.clone().into_os_string();
    wal.push(".wal");
    let heavy = db.heavy.status();
    let backup = db.last_backup.lock().ok().and_then(|l| l.clone()).map(|b| {
        let age = chrono::NaiveDateTime::parse_from_str(&b.at, "%Y-%m-%d %H:%M:%S")
            .ok()
            .and_then(|t| t.and_local_timezone(chrono::Local).single())
            .map_or(0, |t| (chrono::Local::now() - t).num_seconds());
        (b.error.is_none() && b.remote_error.is_none(), age, b.size)
    });
    let snap = crate::metrics::Snapshot {
        version: crate::admin::VERSION,
        uptime_secs: db.started.elapsed().as_secs(),
        active_queries: db.active_queries.load(std::sync::atomic::Ordering::SeqCst),
        heavy_limit: heavy.limit,
        heavy_running: heavy.running,
        heavy_waiting: heavy.waiting,
        db_bytes: size(&db.config.db_path),
        wal_bytes: size(std::path::Path::new(&wal)),
        backup,
    };
    let body = crate::metrics::render(&crate::metrics::METRICS, &snap);
    Ok(([(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body).into_response())
}

#[derive(serde::Deserialize)]
pub struct VerifyQuery {
    /// local（默认）：本地最新的备份；remote：对象存储上最新的备份（会下载回来）
    source: Option<String>,
    /// 全量扫描每张表的每一列，校验所有数据块（亿级数据要几分钟）
    full: Option<bool>,
}

/// POST /api/admin/backup/verify?source=local|remote&full=1  恢复演练：用恢复时相同的方式验证最新的备份真的能用
pub async fn admin_backup_verify(
    State(db): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<VerifyQuery>,
) -> AppResult<Json<Envelope<crate::admin::VerifyReport>>> {
    require_admin(&db, &headers, peer)?;
    let remote = match q.source.as_deref() {
        None | Some("local") => false,
        Some("remote") => true,
        Some(_) => return Err(AppError::bad("source 应为 local 或 remote")),
    };
    crate::admin::run_verify(&db, remote, q.full.unwrap_or(false)).await.map(ok).map_err(|e| AppError::bad(format!("恢复演练未通过：{e}")))
}

/// POST /api/admin/backup  立即备份（与导入等写操作串行）
pub async fn admin_backup(
    State(db): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
) -> AppResult<Json<Envelope<crate::admin::BackupRecord>>> {
    require_admin(&db, &headers, peer)?;
    let r = crate::admin::run_backup(&db).await;
    match r.error {
        Some(e) => Err(AppError::Internal(anyhow::anyhow!("备份失败：{e}"))),
        None => Ok(ok(r)),
    }
}

