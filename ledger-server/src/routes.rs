//! HTTP 处理函数：数据库操作均放到阻塞线程池执行，避免占用 tokio 工作线程

use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use crate::auth::{CurrentUser, Need};
use axum::Extension;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::analysis::{self, AdSpendInput, Breakdown, CrossPeriod, Dimension, QianchuanRow};
use crate::balance::{self, BalanceInput, BalanceRow};
use crate::bank::{self, AutoMatchResult, BankBatch, BankImportResult, BankTxn, MatchInput, WithdrawView};
use crate::close::{self, CloseAction, CloseLog, CloseOverview, PeriodLock};
use crate::dataset::{self, CursorInfo, DatasetInfo, DistinctRequest, DistinctResult, QuerySpec, RowsWindow, SelectionStats, StatsRequest};
use crate::db::Db;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::coverage::{self, Coverage};
use crate::import::{self, ImportMode, ImportResult, PreviewResult};
use crate::shop::{self, ShopInfo, ShopInput, ShopUpdate};
use crate::detail::{self, DetailFilter, DetailSummary, ExportRequest, RowsRequest, RowsResult};
use crate::query::{self, EntityAlias, EntityAliasInput, ImportRecord, PeriodInfo, PivotResult};

pub type AppState = Arc<Db>;

/// 在阻塞线程池中用独立连接执行数据库操作
async fn blocking<T, F>(db: &AppState, f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(&duckdb::Connection) -> AppResult<T> + Send + 'static,
{
    run_blocking(db, f, false).await
}

/// 只读查询：与 `blocking` 相同，但客户端断开（超时、关面板、取消请求）时**中断**正在执行的 DuckDB 查询，
/// 而不是让它在后台白白跑完（亿级数据上一个去重计数可以占满 CPU 几十秒）。写操作不要用它——中断写到一半的多步操作不安全
async fn blocking_read<T, F>(db: &AppState, f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(&duckdb::Connection) -> AppResult<T> + Send + 'static,
{
    run_blocking(db, f, true).await
}

/// 请求体里的查询条件是否为空（无筛选 / 搜索 / 排序 / 规则）
fn spec_is_plain(body: &[u8]) -> bool {
    serde_json::from_slice::<QuerySpec>(body).is_ok_and(|s| s.filters.is_empty() && s.keyword.as_deref().is_none_or(|k| k.trim().is_empty()) && s.sort.is_empty() && s.rule.as_ref().is_none_or(Vec::is_empty))
}

/// 重计算：先过并发闸门（同时进行的个数有限，其余排队），再作为可取消的只读查询执行。排队中客户端断开会自动退出队列
async fn blocking_heavy<T, F>(db: &AppState, f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(&duckdb::Connection) -> AppResult<T> + Send + 'static,
{
    let _permit = db.heavy.acquire().await?;
    blocking_read(db, f).await
}

/// 请求被丢弃（客户端断开）时中断它的查询
struct CancelOnDrop {
    armed: bool,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    handle: Arc<std::sync::Mutex<Option<Arc<duckdb::InterruptHandle>>>>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Ok(mut h) = self.handle.lock()
                && let Some(h) = h.take()
            {
                h.interrupt();
            }
        }
    }
}

/// 正在执行的数据库操作计数（健康检查里能看到，排查「是不是有查询卡着」）
struct Active<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> Active<'a> {
    fn new(c: &'a std::sync::atomic::AtomicUsize) -> Self {
        c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(c)
    }
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn run_blocking<T, F>(db: &AppState, f: F, cancel_on_disconnect: bool) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(&duckdb::Connection) -> AppResult<T> + Send + 'static,
{
    let db = db.clone();
    // 受限用户：数据集定义按其数据范围加载（见 scope.rs）
    let scope = crate::scope::current_task_scope();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handle: Arc<std::sync::Mutex<Option<Arc<duckdb::InterruptHandle>>>> = Arc::new(std::sync::Mutex::new(None));
    let mut guard = CancelOnDrop { armed: cancel_on_disconnect, cancelled: cancelled.clone(), handle: handle.clone() };
    let r = tokio::task::spawn_blocking(move || {
        let _active = Active::new(&db.active_queries);
        // 排队等线程期间客户端就走了：不用开始
        if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AppError::bad("请求已取消"));
        }
        let conn = db.connection()?;
        if let Ok(mut h) = handle.lock() {
            *h = Some(conn.interrupt_handle());
        }
        if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AppError::bad("请求已取消"));
        }
        let out = crate::scope::run_with(scope, || f(&conn));
        if let Ok(mut h) = handle.lock() {
            *h = None;
        }
        out
    })
    .await;
    guard.armed = false;
    r?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    status: &'static str,
    version: &'static str,
    uptime_sec: u64,
    /// 正在执行的数据库操作数（长时间不为 0 说明有查询卡着）
    active_queries: usize,
    /// 重计算并发闸门：上限、正在进行、排队中
    heavy: crate::db::HeavyStatus,
    /// 生效的 DuckDB 内存上限（None 为 DuckDB 默认）
    memory_limit: Option<String>,
}

/// GET /api/health  存活检查（负载均衡 / 容器健康检查用，不需要令牌）
pub async fn health(State(db): State<AppState>) -> Json<Envelope<Health>> {
    ok(Health { status: "ok", version: crate::admin::VERSION, uptime_sec: db.started.elapsed().as_secs(), active_queries: db.active_queries.load(std::sync::atomic::Ordering::SeqCst), heavy: db.heavy.status(), memory_limit: db.config.memory_limit.clone() })
}

/// /api 下没有匹配的接口：返回 JSON 404（避免落到前端页面的 index.html）
pub async fn api_not_found(axum::extract::OriginalUri(uri): axum::extract::OriginalUri) -> axum::response::Response {
    use axum::response::IntoResponse;
    let body = Envelope::<()> { code: 404, data: None, msg: format!("接口不存在：{}", uri.path()) };
    (axum::http::StatusCode::NOT_FOUND, Json(body)).into_response()
}

// ── 运维接口 ──────────────────────────────────────────

/// 运维接口的访问控制：配置了 LEDGER_ADMIN_TOKEN 时校验请求头 x-admin-token；
/// 未配置时只允许本机直接访问（经过反向代理转发的请求带 X-Forwarded-For 等头，一律拒绝）
fn require_admin(db: &AppState, headers: &axum::http::HeaderMap, peer: std::net::SocketAddr) -> AppResult<()> {
    if let Some(token) = &db.config.admin_token {
        let given = headers.get("x-admin-token").and_then(|v| v.to_str().ok()).unwrap_or("");
        // 定长比较，避免按字节提前返回泄露令牌长度 / 前缀
        let same = given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        return if same { Ok(()) } else { Err(AppError::Forbidden("运维令牌无效（请求头 x-admin-token）".into())) };
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

/// 把 multipart 中的文件流式写入指定路径，返回（文件名, 其他文本字段）
async fn receive_upload(mut multipart: Multipart, path: &std::path::Path) -> AppResult<(String, std::collections::HashMap<String, String>)> {
    let mut fields = std::collections::HashMap::new();
    let mut file_name = None;
    while let Some(mut field) = multipart.next_field().await.map_err(|e| AppError::bad(format!("上传数据不完整：{e}")))? {
        let Some(name) = field.name().map(str::to_string) else { continue };
        if name == "file" {
            file_name = Some(field.file_name().unwrap_or("upload.csv").to_string());
            let mut out = tokio::fs::File::create(path).await?;
            while let Some(chunk) = field.chunk().await.map_err(|e| AppError::bad(format!("文件上传中断：{e}")))? {
                out.write_all(&chunk).await?;
            }
            out.flush().await?;
        } else {
            fields.insert(name, field.text().await.map_err(|e| AppError::bad(e.to_string()))?);
        }
    }
    let file_name = file_name.ok_or_else(|| AppError::bad("请选择要导入的 CSV 文件"))?;
    Ok((file_name, fields))
}

fn parse_mode(v: Option<&String>) -> AppResult<ImportMode> {
    match v.map(String::as_str) {
        None | Some("") | Some("merge") => Ok(ImportMode::Merge),
        Some("replace") => Ok(ImportMode::Replace),
        Some(other) => Err(AppError::bad(format!("未知的导入模式：{other}"))),
    }
}

/// POST /api/ledger/imports/preview  multipart: file —— 暂存文件，识别店铺并查重，返回 token
pub async fn preview_import(State(db): State<AppState>, multipart: Multipart) -> AppResult<Json<Envelope<PreviewResult>>> {
    import::cleanup_previews(&db.tmp_dir);
    let token = import::new_preview_token();
    let path = db.tmp_dir.join(format!("preview-{token}.csv"));
    let (file_name, _) = match receive_upload(multipart, &path).await {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    };
    let result = blocking(&db, move |conn| {
        let r = import::preview(conn, &path, &token, &file_name);
        if r.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        r
    })
    .await?;
    Ok(ok(result))
}

#[derive(Deserialize)]
pub struct CommitRequest {
    pub token: String,
    pub shop: String,
    #[serde(default)]
    pub mode: ImportMode,
    #[serde(default, rename = "fileName")]
    pub file_name: String,
}

/// POST /api/ledger/imports/commit  { token, shop, mode: merge | replace }
pub async fn commit_import(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<ImportResult>>> {
    let req: CommitRequest = parse_json(&body)?;
    let path = import::preview_path(&db.tmp_dir, &req.token)?;
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| {
        let r = import::commit(conn, &path, &req.shop, &req.file_name, req.mode);
        if r.is_ok() {
            let _ = std::fs::remove_file(&path);
        }
        r
    })
    .await?;
    log_import(&result);
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(result))
}

/// POST /api/ledger/imports  multipart: shop + file (+ mode) —— 一步导入（脚本/命令行使用），默认按流水号合并
pub async fn import_csv(State(db): State<AppState>, multipart: Multipart) -> AppResult<Json<Envelope<ImportResult>>> {
    let tmp = tempfile::Builder::new().prefix("upload-").suffix(".csv").tempfile_in(&db.tmp_dir)?;
    let (file_name, fields) = receive_upload(multipart, tmp.path()).await?;
    let shop = fields.get("shop").cloned().ok_or_else(|| AppError::bad("请填写店铺名称"))?;
    let mode = parse_mode(fields.get("mode"))?;
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| {
        let r = import::commit(conn, tmp.path(), &shop, &file_name, mode);
        drop(tmp);
        r
    })
    .await?;
    log_import(&result);
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(result))
}

fn log_import(r: &ImportResult) {
    tracing::info!(
        shop = %r.shop, mode = %r.mode, rows = r.row_count, inserted = r.inserted_rows,
        skipped = r.skipped_rows, changed = r.changed_rows, replaced = r.replaced_rows, ms = r.elapsed_ms,
        "import finished"
    );
}

/// GET /api/ledger/shops
pub async fn list_shops(State(db): State<AppState>, Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>) -> AppResult<Json<Envelope<Vec<ShopInfo>>>> {
    let mut shops = blocking(&db, shop::list).await?;
    shops.retain(|s| scope.allows(&s.name));
    Ok(ok(shops))
}

/// POST /api/ledger/shops  { name, douyinShopId?, remark? }
pub async fn create_shop(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<Vec<ShopInfo>>>> {
    let input: ShopInput = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| shop::create(conn, input)).await?))
}

/// PUT /api/ledger/shops  { name, newName?, douyinShopId?, status?, remark? }（改名同步更新历史数据）
pub async fn update_shop(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<Vec<ShopInfo>>>> {
    let input: ShopUpdate = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| shop::update(conn, input)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(result))
}

/// GET /api/ledger/coverage  店铺 × 月份完整性矩阵
pub async fn coverage(State(db): State<AppState>) -> AppResult<Json<Envelope<Coverage>>> {
    Ok(ok(blocking(&db, coverage::coverage).await?))
}

#[derive(Deserialize)]
pub struct PeriodQuery {
    pub period: String,
}

/// GET /api/ledger/pivot?period=2026-09
pub async fn pivot(State(db): State<AppState>, Query(q): Query<PeriodQuery>) -> AppResult<Json<Envelope<PivotResult>>> {
    Ok(ok(blocking_read(&db, move |conn| query::pivot(conn, &q.period)).await?))
}

/// 自行解析 JSON 请求体：格式错误也返回统一信封（axum 的 Json 提取器返回纯文本）
pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> AppResult<T> {
    serde_json::from_slice(body).map_err(|e| AppError::bad(format!("请求格式错误：{e}")))
}

/// POST /api/ledger/details/rows  按偏移量取一段明细（前端虚拟滚动按可视区域分段加载）
pub async fn detail_rows(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<RowsResult>>> {
    let req: RowsRequest = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| detail::rows(conn, &req)).await?))
}

/// POST /api/ledger/details/summary  当前筛选的笔数、金额合计、规则贡献合计与分面统计
pub async fn detail_summary(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<DetailSummary>>> {
    let filter: DetailFilter = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| detail::summary(conn, &filter)).await?))
}

/// POST /api/ledger/details/export  当前筛选结果导出为 CSV
pub async fn detail_export(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;
    let req: ExportRequest = parse_json(&body)?;
    let file_name = format!("资金流水明细_{}.csv", req.filter.period);
    let bytes = blocking(&db, move |conn| detail::export_csv(conn, &req)).await?;
    let disposition = format!("attachment;filename={}", percent_encode(&file_name));
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, disposition)], bytes)
        .into_response())
}

/// 文件名按 UTF-8 百分号编码（前端 request.ts 用 decodeURI 还原）
fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// GET /api/ledger/periods
pub async fn periods(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<PeriodInfo>>>> {
    Ok(ok(blocking(&db, query::periods).await?))
}

/// GET /api/ledger/imports
pub async fn imports(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<ImportRecord>>>> {
    Ok(ok(blocking(&db, query::imports).await?))
}

/// GET /api/ledger/entity-aliases
pub async fn entity_aliases(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<EntityAlias>>>> {
    Ok(ok(blocking(&db, query::entity_aliases).await?))
}

/// PUT /api/ledger/entity-aliases  body: [{ entity, alias }]，alias 为空表示删除
pub async fn save_entity_aliases(
    State(db): State<AppState>,
    body: axum::body::Bytes,
) -> AppResult<Json<Envelope<Vec<EntityAlias>>>> {
    let items: Vec<EntityAliasInput> = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| query::save_entity_aliases(conn, &items)).await?))
}

#[derive(Deserialize)]
pub struct DeleteQuery {
    pub shop: String,
    pub period: String,
}

/// DELETE /api/ledger/data?shop=&period=
pub async fn delete_data(State(db): State<AppState>, Query(q): Query<DeleteQuery>) -> AppResult<Json<Envelope<i64>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| query::delete_period(conn, &q.shop, &q.period)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(n))
}

// ── 第二阶段：月结闭环 ────────────────────────────────

#[derive(Deserialize)]
pub struct PeriodShopQuery {
    pub period: String,
    pub shop: Option<String>,
}

/// GET /api/ledger/balances?period=&shop=  余额勾稽
pub async fn balances(State(db): State<AppState>, Query(q): Query<PeriodShopQuery>) -> AppResult<Json<Envelope<Vec<BalanceRow>>>> {
    Ok(ok(blocking(&db, move |conn| balance::list(conn, &q.period, q.shop.as_deref().filter(|s| !s.is_empty()))).await?))
}

#[derive(Deserialize)]
pub struct SaveBalances {
    #[serde(default)]
    pub operator: String,
    pub items: Vec<BalanceInput>,
}

/// PUT /api/ledger/balances  { operator, items: [{ shop, entity, account, period, opening, closing, remark }] }
pub async fn save_balances(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: SaveBalances = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| balance::save(conn, &req.items, &req.operator)).await?;
    db.checkpoint();
    Ok(ok(true))
}

/// POST /api/ledger/bank/imports  multipart: file + entity (+ accountNo)  银行流水导入
pub async fn import_bank(State(db): State<AppState>, multipart: Multipart) -> AppResult<Json<Envelope<BankImportResult>>> {
    let tmp = tempfile::Builder::new().prefix("bank-").tempfile_in(&db.tmp_dir)?;
    let (file_name, fields) = receive_upload(multipart, tmp.path()).await?;
    let entity = fields.get("entity").cloned().unwrap_or_default();
    let account_no = fields.get("accountNo").cloned();
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| {
        let r = bank::import(conn, tmp.path(), &file_name, &entity, account_no.as_deref());
        drop(tmp);
        r
    })
    .await?;
    db.checkpoint();
    Ok(ok(result))
}

/// GET /api/ledger/bank/imports  已导入的银行流水批次
pub async fn bank_batches(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<BankBatch>>>> {
    Ok(ok(blocking(&db, bank::batches).await?))
}

#[derive(Deserialize)]
pub struct BatchQuery {
    #[serde(rename = "batchId")]
    pub batch_id: String,
}

/// DELETE /api/ledger/bank/imports?batchId=
pub async fn delete_bank_batch(State(db): State<AppState>, Query(q): Query<BatchQuery>) -> AppResult<Json<Envelope<i64>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| bank::delete_batch(conn, &q.batch_id)).await?;
    db.checkpoint();
    Ok(ok(n))
}

/// GET /api/ledger/withdrawals?period=&shop=  提现到账情况
pub async fn withdrawals(State(db): State<AppState>, Query(q): Query<PeriodShopQuery>) -> AppResult<Json<Envelope<WithdrawView>>> {
    Ok(ok(blocking(&db, move |conn| bank::view(conn, &q.period, q.shop.as_deref().filter(|s| !s.is_empty()))).await?))
}

#[derive(Deserialize)]
pub struct AutoMatchRequest {
    pub period: String,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/withdrawals/auto-match  { period, operator }
pub async fn auto_match(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<AutoMatchResult>>> {
    let req: AutoMatchRequest = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| bank::auto_match(conn, &req.period, &req.operator)).await?;
    db.checkpoint();
    Ok(ok(r))
}

#[derive(Deserialize)]
pub struct CandidateQuery {
    pub shop: String,
    pub period: String,
    #[serde(rename = "serialNo")]
    pub serial_no: String,
}

/// GET /api/ledger/withdrawals/candidates?shop=&period=&serialNo=  手工配对候选
pub async fn match_candidates(State(db): State<AppState>, Query(q): Query<CandidateQuery>) -> AppResult<Json<Envelope<Vec<BankTxn>>>> {
    Ok(ok(blocking(&db, move |conn| bank::candidates(conn, &q.shop, &q.period, &q.serial_no)).await?))
}

/// PUT /api/ledger/withdrawals/match  { shop, period, serialNo, txnId, operator }（txnId 为空 = 取消配对）
pub async fn manual_match(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: MatchInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| bank::manual_match(conn, &req)).await?;
    db.checkpoint();
    Ok(ok(true))
}

/// GET /api/ledger/close?period=  各店铺月结状态与体检
pub async fn close_overview(
    State(db): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Envelope<CloseOverview>>> {
    let allow = db.config.allow_self_approve;
    let mut o = blocking(&db, move |conn| close::overview(conn, &q.period, &user, allow)).await?;
    // 受限用户只看到自己范围内的店铺
    o.shops.retain(|s| scope.allows(&s.shop));
    Ok(ok(o))
}

/// POST /api/ledger/close  { shop, period, action: submit | approve | reject | cancel | reopen, operator, approver?, comment, acceptRisk? }
pub async fn close_action(
    State(db): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>,
    body: axum::body::Bytes,
) -> AppResult<Json<Envelope<bool>>> {
    let req: CloseAction = parse_json(&body)?;
    if !scope.allows(&req.shop) {
        return Err(AppError::Forbidden(format!("店铺「{}」不在你的数据范围内", req.shop)));
    }
    let allow = db.config.allow_self_approve;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| close::act(conn, &req, &user, allow)).await?;
    db.checkpoint();
    Ok(ok(true))
}

#[derive(Deserialize)]
pub struct OptionalPeriodQuery {
    pub period: Option<String>,
}

/// GET /api/ledger/close/logs?period=
pub async fn close_logs(
    State(db): State<AppState>,
    Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>,
    Query(q): Query<OptionalPeriodQuery>,
) -> AppResult<Json<Envelope<Vec<CloseLog>>>> {
    let mut logs = blocking(&db, move |conn| close::logs(conn, q.period.as_deref().filter(|s| !s.is_empty()))).await?;
    logs.retain(|l| scope.allows(&l.shop));
    Ok(ok(logs))
}

/// GET /api/ledger/close/locks  已冻结（审批中 / 已锁账）的店铺月份
pub async fn close_locks(State(db): State<AppState>, Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>) -> AppResult<Json<Envelope<Vec<PeriodLock>>>> {
    let mut locks = blocking(&db, close::locks).await?;
    locks.retain(|l| scope.allows(&l.shop));
    Ok(ok(locks))
}

// ── 第三阶段：经营分析 ────────────────────────────────

#[derive(Deserialize)]
pub struct BreakdownQuery {
    pub period: String,
    pub shop: Option<String>,
    pub by: Dimension,
}

/// GET /api/ledger/analysis/breakdown?period=&shop=&by=channel|talent|product|order_month|shop|entity
pub async fn analysis_breakdown(State(db): State<AppState>, Query(q): Query<BreakdownQuery>) -> AppResult<Json<Envelope<Breakdown>>> {
    Ok(ok(blocking_read(&db, move |conn| analysis::breakdown(conn, &q.period, q.shop.as_deref(), q.by)).await?))
}

#[derive(Deserialize)]
pub struct ShopQuery {
    pub shop: Option<String>,
}

/// GET /api/ledger/analysis/cross-period?shop=  下单月份 × 结算月份
pub async fn analysis_cross_period(State(db): State<AppState>, Query(q): Query<ShopQuery>) -> AppResult<Json<Envelope<CrossPeriod>>> {
    Ok(ok(blocking_read(&db, move |conn| analysis::cross_period(conn, q.shop.as_deref())).await?))
}

/// GET /api/ledger/analysis/qianchuan?period=  千川充值 / 消耗 / 千川渠道成交
pub async fn analysis_qianchuan(State(db): State<AppState>, Query(q): Query<PeriodQuery>) -> AppResult<Json<Envelope<Vec<QianchuanRow>>>> {
    Ok(ok(blocking_read(&db, move |conn| analysis::qianchuan(conn, &q.period)).await?))
}

/// PUT /api/ledger/analysis/qianchuan  { shop, period, spend, remark, operator }  录入千川消耗
pub async fn save_ad_spend(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: AdSpendInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| analysis::save_ad_spend(conn, &input)).await?;
    db.checkpoint();
    Ok(ok(true))
}

// ── 大数据表（数据集引擎）────────────────────────────

/// GET /api/ledger/datasets  数据集与列定义
pub async fn list_datasets(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<DatasetInfo>>>> {
    Ok(ok(blocking(&db, dataset::list).await?))
}

/// POST /api/ledger/datasets/{id}/cursor  QuerySpec → 游标（筛选 + 排序物化，按条件缓存）
pub async fn open_cursor(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<CursorInfo>>> {
    let spec: QuerySpec = parse_json(&body)?;
    let db2 = db.clone();
    crate::usage::hit(&id, crate::usage::BROWSE);
    let work = move |conn: &duckdb::Connection| dataset::open_cursor(conn, &db2.cursors, &id, &spec);
    // 整表浏览（没有任何条件）基本是即时的直通游标，不排队、也不等别人的排序；
    // 带筛选 / 搜索 / 排序 / 规则的要物化位置表（可能很重）：先过闸门，再串行建表避免同一查询被并发重复物化
    let plain = spec_is_plain(&body);
    if plain {
        return Ok(ok(blocking_read(&db, work).await?));
    }
    let _permit = db.heavy.acquire().await?;
    let _guard = db.cursors.build_lock.lock().await;
    Ok(ok(blocking_read(&db, work).await?))
}

#[derive(Deserialize)]
pub struct RowsQuery {
    pub cursor: String,
    pub offset: i64,
    pub limit: i64,
    /// 只取这些列（逗号分隔，按此顺序）；省略为全部列
    #[serde(default)]
    pub columns: Option<String>,
}

/// GET /api/ledger/datasets/{id}/rows?cursor=&offset=&limit=&columns=  按位置取一段
pub async fn dataset_rows(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<RowsQuery>) -> AppResult<Json<Envelope<RowsWindow>>> {
    let db2 = db.clone();
    let columns: Option<Vec<String>> = q.columns.as_deref().filter(|s| !s.trim().is_empty()).map(|s| s.split(',').map(|c| c.trim().to_string()).collect());
    Ok(ok(blocking_read(&db, move |conn| dataset::rows(conn, &db2.cursors, &id, &q.cursor, q.offset, q.limit, columns.as_deref())).await?))
}

/// POST /api/ledger/datasets/{id}/stats  选区统计
pub async fn dataset_stats(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<SelectionStats>>> {
    let req: StatsRequest = parse_json(&body)?;
    let db2 = db.clone();
    Ok(ok(blocking_read(&db, move |conn| dataset::stats(conn, &db2.cursors, &id, &req)).await?))
}

/// POST /api/ledger/datasets/{id}/distinct  列值分面（筛选下拉）
pub async fn dataset_distinct(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<DistinctResult>>> {
    let req: DistinctRequest = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| dataset::distinct(conn, &id, &req)).await?))
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub cursor: String,
    /// 逗号分隔的列名（按表格当前列顺序）；为空导出全部列
    #[serde(default)]
    pub columns: String,
    #[serde(default)]
    pub name: String,
    /// csv（默认）/ xlsx
    #[serde(default)]
    pub format: String,
    /// 筛选条件描述（写入 xlsx 汇总页签）
    #[serde(default)]
    pub desc: String,
}

/// GET /api/ledger/datasets/{id}/export?cursor=&columns=&name=&format=csv|xlsx&desc=  按当前筛选 / 排序导出（流式下载）
pub async fn dataset_export(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<ExportQuery>) -> AppResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;
    let columns: Vec<String> = q.columns.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    crate::usage::hit(&id, crate::usage::EXPORT);
    let dir = db.tmp_dir.clone();
    let db2 = db.clone();
    let cursor = q.cursor.clone();
    let xlsx = q.format == "xlsx";
    let desc = q.desc.clone();
    let (path, rows) = blocking(&db, move |conn| {
        if xlsx {
            dataset::export_xlsx(conn, &db2.cursors, &id, &cursor, &columns, &desc, &dir)
        } else {
            dataset::export_csv(conn, &db2.cursors, &id, &cursor, &columns, &dir)
        }
    })
    .await?;
    let file = tokio::fs::File::open(&path).await?;
    // 打开后即删除目录项：句柄仍可读，传输完成或中断时文件自动释放
    let _ = tokio::fs::remove_file(&path).await;
    tracing::info!(rows, xlsx, "dataset export");
    if xlsx {
        let name = if q.name.trim().is_empty() { "数据导出".to_string() } else { q.name.trim().to_string() };
        let disposition = format!("attachment; filename*=UTF-8''{}", percent_encode(&format!("{name}.xlsx")));
        let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
        return Ok((
            [
                (header::CONTENT_TYPE, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()),
                (header::CONTENT_DISPOSITION, disposition),
            ],
            body,
        )
            .into_response());
    }
    // UTF-8 BOM：Excel 直接双击打开不乱码
    let bom = futures_util::stream::once(async { Ok::<_, std::io::Error>(axum::body::Bytes::from_static("\u{feff}".as_bytes())) });
    let body = axum::body::Body::from_stream(futures_util::StreamExt::chain(bom, tokio_util::io::ReaderStream::new(file)));
    let name = if q.name.trim().is_empty() { "数据导出".to_string() } else { q.name.trim().to_string() };
    let disposition = format!("attachment; filename*=UTF-8''{}", percent_encode(&format!("{name}.csv")));
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, disposition)], body).into_response())
}

#[derive(Deserialize)]
pub struct AggregateRequest {
    pub calls: Vec<crate::aggregate::AggregateCall>,
    /// 范围：工作表里数据表的筛选 / 关键字 / 排序（TSUM 等按数据表当前显示的行计算）
    #[serde(default)]
    pub query: Option<dataset::QuerySpec>,
}

/// POST /api/ledger/datasets/{id}/aggregate  { calls: [{ func, column, criteria: [{ column, value }], row? }], query? }  公式下推批量计算
pub async fn dataset_aggregate(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::aggregate::AggregateResult>>> {
    let req: AggregateRequest = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| {
        // 公式里按名称引用：按解析出的数据集计数
        if let Ok(d) = dataset::dataset(conn, &id) {
            crate::usage::hit(&d.id, crate::usage::FORMULA);
        }
        crate::aggregate::aggregate(conn, &id, &req.calls, req.query.as_ref())
    })
    .await?))
}

// ── 通用数据集导入与管理 ──────────────────────────────

/// POST /api/ledger/datasets/import/preview  multipart: file（CSV / XLS / XLSX）
pub async fn dataset_import_preview(State(db): State<AppState>, mut multipart: Multipart) -> AppResult<Json<Envelope<crate::dataset_import::ImportPreview>>> {
    import::cleanup_previews(&db.tmp_dir);
    let token = import::new_preview_token();
    // 可以一次上传多个文件（字段名都是 file），合并为一个数据集
    let mut uploads: Vec<crate::import_source::Upload> = Vec::new();
    let received = async {
        while let Some(mut field) = multipart.next_field().await.map_err(|e| AppError::bad(format!("上传数据不完整：{e}")))? {
            if field.name() != Some("file") {
                continue;
            }
            if uploads.len() >= crate::import_source::MAX_FILES {
                return Err(AppError::bad(format!("一次最多合并 {} 个文件", crate::import_source::MAX_FILES)));
            }
            let name = field.file_name().unwrap_or("upload.csv").to_string();
            let path = db.tmp_dir.join(format!("preview-{token}-{}.upload", uploads.len()));
            uploads.push(crate::import_source::Upload { name, path: path.clone() });
            let mut out = tokio::fs::File::create(&path).await?;
            while let Some(chunk) = field.chunk().await.map_err(|e| AppError::bad(format!("文件上传中断：{e}")))? {
                out.write_all(&chunk).await?;
            }
            out.flush().await?;
        }
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(e) = received {
        for u in &uploads {
            let _ = std::fs::remove_file(&u.path);
        }
        return Err(e);
    }
    Ok(ok(preview_uploads(&db, token, uploads).await?))
}

/// 规整上传 / 下载的文件并预览
async fn preview_uploads(db: &AppState, token: String, uploads: Vec<crate::import_source::Upload>) -> AppResult<crate::dataset_import::ImportPreview> {
    let dir = db.tmp_dir.clone();
    blocking(db, move |conn| {
        let (file_name, infos) = crate::import_source::prepare(conn, &dir, &token, uploads)?;
        let path = crate::dataset_import::staged_path(&dir, &token, &file_name)?;
        let r = crate::dataset_import::preview(conn, &path, &token, &file_name).map(|mut p| {
            p.source_files = infos;
            p
        });
        if r.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        r
    })
    .await
}

#[derive(Deserialize)]
pub struct UrlImportBody {
    pub url: String,
}

/// POST /api/ledger/datasets/import/preview-url  { url }  从网址下载文件并预览（http / https，不访问本机地址）
pub async fn dataset_import_preview_url(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::dataset_import::ImportPreview>>> {
    let req: UrlImportBody = parse_json(&body)?;
    import::cleanup_previews(&db.tmp_dir);
    let token = import::new_preview_token();
    let max = db.config.max_upload_mb as u64 * 1024 * 1024;
    let upload = crate::import_source::download(&req.url, &db.tmp_dir, &token, max).await?;
    Ok(ok(preview_uploads(&db, token, vec![upload]).await?))
}

/// POST /api/ledger/datasets/import/commit  { token, mode: create | append | replace, datasetId?, name?, columns, primaryKey, fileName }
pub async fn dataset_import_commit(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::dataset_import::ImportOutcome>>> {
    let req: crate::dataset_import::CommitRequest = parse_json(&body)?;
    let path = crate::dataset_import::staged_path(&db.tmp_dir, &req.token, &req.file_name)?;
    if !path.exists() {
        return Err(AppError::bad("预览已过期，请重新选择文件"));
    }
    let dir = db.tmp_dir.clone();
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| {
        // 整体替换已有数据集：和改结构一样，只有所有者与管理员
        if let Some(id) = req.dataset_id.as_deref().filter(|_| matches!(req.mode, crate::dataset_import::Mode::Replace)) {
            crate::dataset::require_manage(conn, id, &user)?;
        }
        let r = crate::dataset_import::commit(conn, &path, &req, &dir);
        if let Ok(o) = &r {
            let _ = std::fs::remove_file(&path);
            crate::dataset::set_owner(conn, &o.dataset_id, &user)?;
        }
        r
    })
    .await?;
    tracing::info!(dataset = %result.dataset_id, rows = result.rows_read, inserted = result.inserted, ms = result.elapsed_ms, "dataset import");
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(result))
}

#[derive(Deserialize)]
pub struct RenameDataset {
    pub name: String,
}

/// PUT /api/ledger/datasets/{id}  { name }  改名
pub async fn rename_dataset(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: RenameDataset = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| dataset::rename(conn, &id, &req.name)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

/// DELETE /api/ledger/datasets/{id}?operator=  删除数据集：移到回收站（数据保留，可恢复）
pub async fn delete_dataset(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<OperatorQuery>) -> AppResult<Json<Envelope<i64>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| dataset::delete(conn, &id, &q.operator)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(n))
}

/// GET /api/ledger/datasets/trash  数据集回收站 { days, items }
pub async fn dataset_trash(State(db): State<AppState>) -> AppResult<Json<Envelope<crate::dataset_trash::TrashList>>> {
    Ok(ok(blocking(&db, crate::dataset_trash::list).await?))
}

/// DELETE /api/ledger/datasets/trash  清空数据集回收站（不可恢复）
pub async fn empty_dataset_trash(State(db): State<AppState>) -> AppResult<Json<Envelope<usize>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, crate::dataset_trash::empty).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(n))
}

/// POST /api/ledger/datasets/{id}/untrash  从回收站恢复
pub async fn untrash_dataset(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::dataset_trash::Restored>>> {
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| crate::dataset_trash::restore(conn, &id)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}

/// DELETE /api/ledger/datasets/{id}/purge  彻底删除（连同回收站里基于它的关联视图），返回删除的名称
pub async fn purge_dataset(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Vec<String>>>> {
    let _guard = db.import_lock.lock().await;
    let names = blocking(&db, move |conn| crate::dataset_trash::purge(conn, &id)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(names))
}

// ── 服务端工作簿 ──────────────────────────────────────

use crate::workbook::{self, SaveResult, VersionInfo, WorkbookMeta};

#[derive(Deserialize)]
pub struct CreateWorkbookQuery {
    pub name: String,
    #[serde(default)]
    pub operator: String,
    /// 新建到这个文件夹（空为根目录）
    #[serde(default)]
    pub folder: Option<String>,
}

/// GET /api/ledger/workbooks
pub async fn list_workbooks(State(db): State<AppState>, Extension(user): Extension<CurrentUser>) -> AppResult<Json<Envelope<Vec<WorkbookMeta>>>> {
    Ok(ok(blocking(&db, move |conn| workbook::list_for(conn, &user)).await?))
}

/// POST /api/ledger/workbooks?name=&operator=  body: gzip 压缩的工作簿 JSON
pub async fn create_workbook(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Query(q): Query<CreateWorkbookQuery>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let _guard = db.import_lock.lock().await;
    let m = blocking(&db, move |conn| {
        let m = workbook::create_in(conn, &q.name, &body, &q.operator, q.folder.as_deref().filter(|s| !s.is_empty()))?;
        workbook::set_owner(conn, &m.id, &user)?;
        workbook::meta(conn, &m.id)
    })
    .await?;
    db.checkpoint();
    Ok(ok(m))
}

/// GET /api/ledger/workbooks/{id}  元数据
pub async fn workbook_meta(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    Ok(ok(blocking(&db, move |conn| workbook::meta(conn, &id)).await?))
}

fn gzip_json_response(data: Vec<u8>, version: i64) -> axum::response::Response {
    use axum::http::header;
    use axum::response::IntoResponse;
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8".to_string()),
            // 存储的就是 gzip：原样返回，由浏览器解压
            (header::CONTENT_ENCODING, "gzip".to_string()),
            (header::HeaderName::from_static("x-workbook-version"), version.to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        data,
    )
        .into_response()
}

/// GET /api/ledger/workbooks/{id}/content  工作簿 JSON（Content-Encoding: gzip，X-Workbook-Version 为版本号）
pub async fn workbook_content(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<axum::response::Response> {
    let (m, data) = blocking(&db, move |conn| workbook::content(conn, &id)).await?;
    Ok(gzip_json_response(data, m.version))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveWorkbookQuery {
    pub base_version: i64,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub operator: String,
}

/// PUT /api/ledger/workbooks/{id}/content?baseVersion=&force=&auto=&description=&operator=  body: gzip 压缩的 JSON
/// 冲突时返回 saved: false + conflict（不覆盖）
pub async fn save_workbook(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<SaveWorkbookQuery>, headers: axum::http::HeaderMap, body: axum::body::Bytes) -> AppResult<Json<Envelope<SaveResult>>> {
    let _guard = db.import_lock.lock().await;
    let wb = id.clone();
    let operator = q.operator.clone();
    let r = blocking(&db, move |conn| {
        workbook::save(conn, &id, &body, &workbook::SaveRequest { base_version: q.base_version, force: q.force, operator: &q.operator, description: &q.description, is_auto: q.auto })
    })
    .await?;
    if r.saved {
        db.checkpoint();
        // 协作：保存的快照已包含到哪个序号（没带说明不是协作客户端写的，房间里的人需要重新载入）
        let applied = headers.get("x-collab-seq").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        crate::collab::saved(&wb, r.version, applied, &operator);
    }
    Ok(ok(r))
}

#[derive(Deserialize)]
pub struct WorkbookNameBody {
    pub name: String,
    #[serde(default)]
    pub operator: String,
}

/// PUT /api/ledger/workbooks/{id}  { name }  改名
pub async fn rename_workbook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let req: WorkbookNameBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    Ok(ok(blocking(&db, move |conn| workbook::rename(conn, &id, &req.name)).await?))
}

#[derive(Deserialize, Default)]
pub struct OperatorQuery {
    #[serde(default)]
    pub operator: String,
}

/// DELETE /api/ledger/workbooks/{id}?operator=  移到回收站（可恢复）
pub async fn delete_workbook(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<OperatorQuery>) -> AppResult<Json<Envelope<bool>>> {
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| workbook::move_to_trash(conn, &id, &q.operator)).await?;
    db.checkpoint();
    Ok(ok(true))
}

/// GET /api/ledger/workbooks/trash  回收站
pub async fn workbook_trash(State(db): State<AppState>, Extension(user): Extension<CurrentUser>) -> AppResult<Json<Envelope<workbook::TrashList>>> {
    Ok(ok(blocking(&db, move |conn| workbook::trash_for(conn, &user)).await?))
}

/// DELETE /api/ledger/workbooks/trash  清空回收站（不可恢复）
pub async fn empty_workbook_trash(State(db): State<AppState>) -> AppResult<Json<Envelope<usize>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, workbook::empty_trash).await?;
    db.checkpoint();
    Ok(ok(n))
}

/// POST /api/ledger/workbooks/{id}/untrash  从回收站恢复
pub async fn untrash_workbook(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let _guard = db.import_lock.lock().await;
    let m = blocking(&db, move |conn| workbook::untrash(conn, &id)).await?;
    db.checkpoint();
    Ok(ok(m))
}

/// DELETE /api/ledger/workbooks/{id}/purge  彻底删除（只能删回收站里的，含全部版本）
pub async fn purge_workbook(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| workbook::purge(conn, &id)).await?;
    db.checkpoint();
    Ok(ok(true))
}

#[derive(Deserialize)]
pub struct StarBody {
    pub starred: bool,
}

/// PUT /api/ledger/workbooks/{id}/star  { starred }
pub async fn star_workbook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: StarBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| workbook::set_starred(conn, &id, req.starred)).await?;
    Ok(ok(true))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveBody {
    pub ids: Vec<String>,
    #[serde(default)]
    pub folder_id: Option<String>,
}

/// POST /api/ledger/workbooks/move  { ids, folderId }  移到文件夹（folderId 为空表示根目录）
pub async fn move_workbooks(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<usize>>> {
    let req: MoveBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| {
        for id in &req.ids {
            workbook::require(conn, id, &user, Need::Edit, false)?;
        }
        workbook::move_to(conn, &req.ids, req.folder_id.as_deref().filter(|s| !s.is_empty()))
    })
    .await?;
    db.checkpoint();
    Ok(ok(n))
}

#[derive(Deserialize)]
pub struct RangesBody {
    pub items: Vec<crate::xref::RangeRequest>,
}

/// POST /api/ledger/workbooks/ranges  { items: [{ workbook, range }] }  跨工作簿引用：取其他工作簿已保存版本里的区域值
pub async fn workbook_ranges(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<Vec<crate::xref::RangeResult>>>> {
    let req: RangesBody = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::xref::ranges(conn, req.items, &user)).await?))
}

/// GET /api/ledger/folders
pub async fn list_folders(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<workbook::Folder>>>> {
    Ok(ok(blocking(&db, workbook::folders).await?))
}

/// POST /api/ledger/folders  { name, parentId, operator }
pub async fn create_folder(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<workbook::Folder>>> {
    let req: workbook::FolderInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    Ok(ok(blocking(&db, move |conn| workbook::create_folder(conn, &req)).await?))
}

/// PUT /api/ledger/folders/{id}  { name, parentId }  改名 / 移动
pub async fn update_folder(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: workbook::FolderInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| workbook::update_folder(conn, &id, &req)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/folders/{id}?operator=  删除文件夹（含子文件夹），其中的工作簿移到回收站
pub async fn delete_folder(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<OperatorQuery>) -> AppResult<Json<Envelope<usize>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| workbook::delete_folder(conn, &id, &q.operator)).await?;
    db.checkpoint();
    Ok(ok(n))
}

/// POST /api/ledger/workbooks/{id}/duplicate  { name, operator }  复制为新工作簿
pub async fn duplicate_workbook(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let req: WorkbookNameBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let m = blocking(&db, move |conn| {
        let m = workbook::duplicate(conn, &id, &req.name, &req.operator)?;
        workbook::set_owner(conn, &m.id, &user)?;
        workbook::meta(conn, &m.id)
    })
    .await?;
    db.checkpoint();
    Ok(ok(m))
}

/// GET /api/ledger/workbooks/{id}/versions
pub async fn workbook_versions(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Vec<VersionInfo>>>> {
    Ok(ok(blocking(&db, move |conn| workbook::versions(conn, &id)).await?))
}

/// GET /api/ledger/workbooks/{id}/versions/{version}/content
pub async fn workbook_version_content(State(db): State<AppState>, Path((id, version)): Path<(String, i64)>) -> AppResult<axum::response::Response> {
    let data = blocking(&db, move |conn| workbook::version_content(conn, &id, version)).await?;
    Ok(gzip_json_response(data, version))
}

#[derive(Deserialize)]
pub struct ShareBody {
    pub visibility: String,
}

/// PUT /api/ledger/workbooks/{id}/share  { visibility: private | view | edit }  修改共享范围（所有者与管理员）
pub async fn share_workbook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let req: ShareBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let m = blocking(&db, move |conn| {
        workbook::set_visibility(conn, &id, &req.visibility)?;
        workbook::meta(conn, &id)
    })
    .await?;
    db.checkpoint();
    Ok(ok(m))
}

#[derive(Deserialize)]
pub struct RestoreBody {
    pub version: i64,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/workbooks/{id}/restore  { version, operator }  以历史版本内容生成新版本
pub async fn restore_workbook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let req: RestoreBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let wb = id.clone();
    let operator = req.operator.clone();
    let m = blocking(&db, move |conn| workbook::restore(conn, &id, req.version, &req.operator)).await?;
    db.checkpoint();
    crate::collab::saved(&wb, m.version, None, &operator);
    Ok(ok(m))
}

/// POST /api/ledger/datasets/{id}/pivot  数据透视表（rows / columns / values / filters / sort / top / subtotals / grandTotals）
pub async fn dataset_pivot(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::pivot::PivotResult>>> {
    let req: crate::pivot::PivotRequest = parse_json(&body)?;
    crate::usage::hit(&id, crate::usage::PIVOT);
    Ok(ok(blocking_heavy(&db, move |conn| crate::pivot::pivot(conn, &id, &req)).await?))
}

// ── 关联视图与计算列 ──────────────────────────────────

#[derive(Deserialize)]
pub struct CreateViewBody {
    pub name: String,
    #[serde(flatten)]
    pub def: crate::views::ViewDef,
}

/// POST /api/ledger/datasets/views/preview  { left, right, on: [{ left, right }], columns: [{ source, title }] }  关联预览（匹配率、关联不上的键）
pub async fn view_preview(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::views::JoinPreview>>> {
    let v: crate::views::ViewDef = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::views::preview(conn, &v)).await?))
}

/// POST /api/ledger/datasets/views  { name, left, right, on, columns }  创建关联视图
pub async fn create_view(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let req: CreateViewBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let id = blocking(&db, move |conn| {
        let id = crate::views::create(conn, &req.name, &req.def)?;
        crate::dataset::set_owner(conn, &id, &user)?;
        Ok(id)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(id))
}

/// PUT /api/ledger/datasets/views/{id}  { name, left, right, on, columns }  修改关联视图
pub async fn update_view(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: CreateViewBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| crate::views::update(conn, &id, &req.name, &req.def)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

#[derive(Deserialize)]
pub struct CalcBody {
    #[serde(default)]
    pub name: String,
    pub formula: String,
    pub kind: Option<String>,
}

/// POST /api/ledger/datasets/{id}/calc/preview  { formula }  计算列预览（类型、样例、空值行数）
pub async fn calc_preview(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<dataset::CalcPreview>>> {
    let req: CalcBody = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| dataset::preview_calc(conn, &id, &req.formula)).await?))
}

/// POST /api/ledger/datasets/{id}/calc  { name, formula, kind? }  新增计算列
pub async fn add_calc(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: CalcBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| dataset::add_calc(conn, &id, &req.name, &req.formula, req.kind.as_deref())).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

/// DELETE /api/ledger/datasets/{id}/calc/{name}
pub async fn remove_calc(State(db): State<AppState>, Path((id, name)): Path<(String, String)>) -> AppResult<Json<Envelope<bool>>> {
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| dataset::remove_calc(conn, &id, &name)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

/// PUT /api/ledger/datasets/{id}/calc/{name}  { formula, kind? }  修改计算列
pub async fn update_calc(State(db): State<AppState>, Path((id, name)): Path<(String, String)>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: CalcBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| dataset::update_calc(conn, &id, &name, &req.formula, req.kind.as_deref())).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

// ── 数据连接器 ────────────────────────────────────────

/// GET /api/ledger/connectors  连接器列表（含最近一次同步、下次同步时间）
pub async fn list_connectors(State(db): State<AppState>) -> AppResult<Json<Envelope<serde_json::Value>>> {
    let st = db.clone();
    let (items, roots) = blocking(&db, move |conn| Ok((crate::connector::list(conn, &st.connectors)?, crate::connector::roots(&st.connectors)))).await?;
    Ok(ok(serde_json::json!({ "items": items, "inboxRoots": roots })))
}

/// POST /api/ledger/connectors  新建
pub async fn create_connector(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::connector::ConnectorInput = parse_json(&body)?;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| crate::connector::create(conn, &st.connectors, input)).await?))
}

/// PUT /api/ledger/connectors/{id}  修改（密码留空则沿用已保存的）
pub async fn update_connector(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::connector::ConnectorInput = parse_json(&body)?;
    let st = db.clone();
    blocking(&db, move |conn| crate::connector::update(conn, &st.connectors, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/connectors/{id}  删除连接器（数据集保留）
pub async fn delete_connector(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    let st = db.clone();
    blocking(&db, move |conn| crate::connector::delete(conn, &st.connectors, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/connectors/test  测试来源（数据库连通性 / 文件夹匹配的文件）
pub async fn test_connector(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::connector::TestOutcome>>> {
    let input: crate::connector::ProbeInput = parse_json(&body)?;
    Ok(ok(crate::connector::test(&db, input).await?))
}

/// POST /api/ledger/connectors/preview  预览来源数据（样例行、推断的列）
pub async fn preview_connector(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::connector::PreviewOutcome>>> {
    let input: crate::connector::ProbeInput = parse_json(&body)?;
    Ok(ok(crate::connector::preview(&db, input).await?))
}

/// POST /api/ledger/connectors/{id}/run  立即同步（后台执行，返回同步记录 ID）
pub async fn run_connector(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<String>>> {
    Ok(ok(crate::connector::start(&db, &id, "manual").await?))
}

#[derive(Deserialize)]
pub struct RunsQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

/// GET /api/ledger/connectors/{id}/runs?limit=  同步记录（新的在前）
pub async fn connector_runs(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<RunsQuery>) -> AppResult<Json<Envelope<Vec<crate::connector::RunRecord>>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    Ok(ok(blocking(&db, move |conn| crate::connector::runs(conn, &id, limit)).await?))
}

// ── 定时推送 ──────────────────────────────────────────

/// GET /api/ledger/reports  推送任务、渠道（地址已打码）、邮件是否已配置、看板链接前缀
pub async fn list_reports(State(db): State<AppState>) -> AppResult<Json<Envelope<serde_json::Value>>> {
    let st = db.clone();
    let (items, channels) = blocking(&db, move |conn| Ok((crate::report::list(conn)?, crate::report::list_channels(conn, &st)?))).await?;
    Ok(ok(serde_json::json!({
        "items": items,
        "channels": channels,
        "smtpConfigured": crate::report::channels::smtp_configured(),
        "publicUrl": crate::report::public_url(),
    })))
}

/// POST /api/ledger/reports  新建推送任务
pub async fn create_report(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::report::ReportInput = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::report::create(conn, input)).await?))
}

/// PUT /api/ledger/reports/{id}
pub async fn update_report(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::report::ReportInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::report::update(conn, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/reports/{id}
pub async fn delete_report(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::report::delete(conn, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/reports/{id}/send  立即发送（等待完成，返回本次记录）
pub async fn send_report(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::report::RunRecord>>> {
    Ok(ok(crate::report::send(&db, &id, "manual").await?))
}

/// GET /api/ledger/reports/{id}/runs?limit=  推送记录（新的在前）
pub async fn report_runs(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<RunsQuery>) -> AppResult<Json<Envelope<Vec<crate::report::RunRecord>>>> {
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    Ok(ok(blocking(&db, move |conn| crate::report::runs(conn, &id, limit)).await?))
}

/// POST /api/ledger/reports/preview  { workbookId, sheetId, title? }  预览推送内容
pub async fn preview_report(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::report::PreviewOutcome>>> {
    let input: crate::report::PreviewInput = parse_json(&body)?;
    Ok(ok(crate::report::preview(&db, input).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetsQuery {
    pub workbook_id: String,
}

/// GET /api/ledger/reports/sheets?workbookId=  工作簿里含仪表盘部件的工作表
pub async fn report_sheets(State(db): State<AppState>, Query(q): Query<SheetsQuery>) -> AppResult<Json<Envelope<Vec<crate::report::SheetOption>>>> {
    Ok(ok(blocking(&db, move |conn| crate::report::sheets(conn, &q.workbook_id)).await?))
}

/// POST /api/ledger/report-channels  新建推送渠道
pub async fn create_report_channel(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::report::ChannelInput = parse_json(&body)?;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| crate::report::create_channel(conn, &st, input)).await?))
}

/// PUT /api/ledger/report-channels/{id}  修改（地址留空沿用已保存的）
pub async fn update_report_channel(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::report::ChannelInput = parse_json(&body)?;
    let st = db.clone();
    blocking(&db, move |conn| crate::report::update_channel(conn, &st, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/report-channels/{id}
pub async fn delete_report_channel(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::report::delete_channel(conn, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/report-channels/{id}/test  发送测试消息
pub async fn test_report_channel(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    crate::report::test_channel(&db, &id).await?;
    Ok(ok(true))
}

// ── 模板 ──────────────────────────────────────────────

/// GET /api/ledger/templates  内置与自建模板（需要的数据集与列、概要、使用次数）
pub async fn list_templates(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<crate::template::TemplateView>>>> {
    Ok(ok(blocking(&db, crate::template::list).await?))
}

/// POST /api/ledger/templates  { workbookId, name, description, category, replaceId?, operator? }  把已保存的工作簿发布为模板
pub async fn publish_template(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::template::TemplateView>>> {
    let input: crate::template::PublishInput = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::template::publish(conn, input)).await?))
}

/// PUT /api/ledger/templates/{id}  { name, description, category }
pub async fn update_template(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::template::MetaInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::template::update_meta(conn, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/templates/{id}  删除自建模板（已用它创建的工作簿不受影响）
pub async fn delete_template(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::template::delete(conn, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/templates/{id}/use  { name, bindings: { key: { datasetId, columns } }, clearSlicers?, operator? }  用模板创建工作簿
pub async fn use_template(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::template::UseOutcome>>> {
    let input: crate::template::UseInput = parse_json(&body)?;
    // 填参数会生成 / 重算保存的 SQL 查询（按服务账号运行）：与「生成实例」一样只对完整数据权限的用户开放
    let _guard = if input.params.is_empty() {
        None
    } else {
        if let Some(scope) = crate::scope::current_task_scope() {
            return Err(AppError::Forbidden(format!("按参数生成数据集需要完整的数据权限，请改为选择数据集。{}", crate::scope::restriction_note(&scope).unwrap_or_default())));
        }
        Some(db.import_lock.lock().await)
    };
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let mut out = crate::template::use_template(&st, conn, &id, input)?;
        workbook::set_owner(conn, &out.workbook.id, &user)?;
        out.workbook = workbook::meta(conn, &out.workbook.id)?;
        Ok(out)
    })
    .await?))
}

/// GET /api/ledger/templates/{id}/uses  最近 50 次使用（生成的工作簿）
pub async fn template_uses(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Vec<crate::template::UseRecord>>>> {
    Ok(ok(blocking(&db, move |conn| crate::template::uses(conn, &id)).await?))
}

// ── AI 助手 ───────────────────────────────────────────

/// GET /api/ledger/ai/status  是否已配置模型（模型名 @ 主机，不含密钥）
pub async fn ai_status() -> AppResult<Json<Envelope<crate::ai::Status>>> {
    Ok(ok(crate::ai::status()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiThreadsQuery {
    pub workbook_id: String,
}

/// GET /api/ledger/ai/threads?workbookId=  工作簿的会话（新的在前）
pub async fn ai_threads(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Query(q): Query<AiThreadsQuery>) -> AppResult<Json<Envelope<Vec<crate::ai::Thread>>>> {
    Ok(ok(blocking(&db, move |conn| crate::ai::threads(conn, &q.workbook_id, &user)).await?))
}

/// POST /api/ledger/ai/threads  { workbookId, operator? }  新会话
pub async fn ai_create_thread(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::ai::Thread>>> {
    let mut input: crate::ai::NewThread = parse_json(&body)?;
    input.owner_id = Some(user.id).filter(|_| user.enforced);
    Ok(ok(blocking(&db, move |conn| crate::ai::create_thread(conn, input)).await?))
}

/// GET /api/ledger/ai/threads/{id}  会话内容：提问、回答、工具调用、建议及处理结果
pub async fn ai_thread(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::ai::ThreadDetail>>> {
    Ok(ok(blocking(&db, move |conn| crate::ai::detail(conn, &id)).await?))
}

#[derive(Deserialize)]
pub struct AiSummaryBody {
    #[serde(default)]
    pub summary: Option<String>,
}

/// PUT /api/ledger/ai/threads/{id}/summary  { summary }  手动编辑会话摘要；留空 = 清除，之后提问时重新整理
pub async fn ai_set_summary(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: AiSummaryBody = parse_json(&body)?;
    blocking(&db, move |conn| crate::ai::set_summary(conn, &id, input.summary.as_deref())).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/ai/threads/{id}
pub async fn ai_delete_thread(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::ai::delete_thread(conn, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/ai/threads/{id}/chat  { message, context } 或 { resume: true }（接着回答被中断的问题）  提问；以 SSE 返回事件：
/// start / status{text}（如整理摘要）/ summary{summary} / text{delta} / break / tool{id,name,status,summary,actionId} / action{action} / done / error{message}
pub async fn ai_chat(
    State(db): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AppResult<axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>> {
    let input: crate::ai::ChatInput = parse_json(&body)?;
    let tid = id.clone();
    let input = blocking(&db, move |conn| crate::ai::begin(conn, &tid, &input).map(|()| input)).await?;
    let rx = crate::ai::subscribe(&id).ok_or_else(|| AppError::bad("回答启动失败"))?;
    // 回答在后台任务中进行，与这个连接无关：断开后继续，可经 /events 重新连上
    // 后台任务里不再有请求的任务上下文：带上数据范围，AI 的查询与建议同样受限
    let scope = crate::scope::current_task_scope();
    let (st, tid) = (db.clone(), id);
    tokio::spawn(async move {
        match scope {
            Some(s) => crate::scope::CURRENT.scope(s, crate::ai::chat(st, tid, input)).await,
            None => crate::ai::chat(st, tid, input).await,
        }
    });
    Ok(ai_sse(rx))
}

fn ai_sse(rx: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>) -> axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (Ok(axum::response::sse::Event::default().data(v.to_string())), rx)) });
    axum::response::sse::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

/// GET /api/ledger/ai/threads/{id}/events  重新连上正在进行的回答：先补发本次回答已产生的事件，再继续推送；
/// 没有在回答时只返回 { type: idle }
pub async fn ai_events(Path(id): Path<String>) -> axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let rx = crate::ai::subscribe(&id).unwrap_or_else(|| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = tx.send(serde_json::json!({ "type": "idle" }));
        rx
    });
    ai_sse(rx)
}

/// POST /api/ledger/ai/threads/{id}/stop  停止正在进行的回答（已产生的文字保留）
pub async fn ai_stop(Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    Ok(ok(crate::ai::stop(&id)))
}

/// POST /api/ledger/ai/actions/{id}  { status: applied | dismissed, result?, operator? }  记录建议的处理结果
pub async fn ai_decide(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::ai::DecideInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::ai::decide(conn, &id, input)).await?;
    Ok(ok(true))
}

// ── SQL 查询 ──────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlRunBody {
    pub sql: String,
    #[serde(default)]
    pub limit: Option<usize>,
    /// 参数化查询：参数定义与取值
    #[serde(default)]
    pub params: Vec<crate::sqlq::params::ParamDef>,
    #[serde(default)]
    pub values: std::collections::HashMap<String, String>,
}

/// POST /api/ledger/sql/run  { sql, limit?, params?, values? }  执行只读查询（最多返回 1000 行）
pub async fn sql_run(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::RunResult>>> {
    let b: SqlRunBody = parse_json(&body)?;
    Ok(ok(blocking_heavy(&db, move |conn| crate::sqlq::run_with_params(conn, &b.sql, b.limit.unwrap_or(1000), &b.params, &b.values)).await?))
}

/// GET /api/ledger/sql/queries  已保存的查询（关联数据集、最近一次重算、下次重算时间）
pub async fn sql_queries(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<crate::sqlq::SavedQuery>>>> {
    Ok(ok(blocking(&db, crate::sqlq::list).await?))
}

/// POST /api/ledger/sql/queries  { name, sql, description?, saveAsDataset?, columnKinds?, schedule?, operator? }
pub async fn sql_create_query(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::SavedQuery>>> {
    let input: crate::sqlq::QueryInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let q = crate::sqlq::save(&st, conn, None, input)?;
        // 另存出来的数据集归创建人（与导入、对账、关联视图一致），否则连创建人自己也改不了、删不掉
        if let Some(d) = &q.dataset_id {
            dataset::set_owner(conn, d, &user)?;
        }
        Ok(q)
    })
    .await?))
}

/// PUT /api/ledger/sql/queries/{id}  修改（已关联数据集时立即按新 SQL 重算）
pub async fn sql_update_query(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::SavedQuery>>> {
    let input: crate::sqlq::QueryInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let q = crate::sqlq::save(&st, conn, Some(&id), input)?;
        // 修改时才第一次另存出数据集的，同样归当前用户（已有所有者的不动）
        if let Some(d) = &q.dataset_id {
            dataset::set_owner(conn, d, &user)?;
        }
        Ok(q)
    })
    .await?))
}

/// DELETE /api/ledger/sql/queries/{id}  删除查询（另存的数据集保留）
pub async fn sql_delete_query(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::sqlq::delete(conn, &id)).await?;
    Ok(ok(true))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SqlRefreshBody {
    /// 还没有关联数据集时：新数据集名称
    #[serde(default)]
    pub dataset_name: Option<String>,
}

/// POST /api/ledger/sql/queries/{id}/refresh  { datasetName? }  立即重算并写入数据集
pub async fn sql_refresh_query(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::Materialized>>> {
    let b: SqlRefreshBody = if body.is_empty() { SqlRefreshBody::default() } else { parse_json(&body)? };
    let _guard = db.import_lock.lock().await;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let m = crate::sqlq::refresh_now(&st, conn, &id, b.dataset_name.as_deref())?;
        dataset::set_owner(conn, &m.dataset_id, &user)?;
        Ok(m)
    })
    .await?))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SqlInstanceBody {
    #[serde(default)]
    pub values: std::collections::HashMap<String, String>,
    /// 实例（同时也是数据集）的名称；不填按「查询名（参数=值）」
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub operator: Option<String>,
}

/// POST /api/ledger/sql/queries/{id}/instances  { values, name?, operator? }  用一组参数值生成实例（独立的查询 + 新数据集）
pub async fn sql_instantiate_query(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::SavedQuery>>> {
    let b: SqlInstanceBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let q = crate::sqlq::instantiate(&st, conn, &id, b.values, b.name.as_deref(), b.operator)?;
        if let Some(d) = &q.dataset_id {
            dataset::set_owner(conn, d, &user)?;
        }
        Ok(q)
    })
    .await?))
}

// ── MCP 服务 ──────────────────────────────────────────

fn mcp_json(status: axum::http::StatusCode, body: serde_json::Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    (status, [(axum::http::header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

/// GET / DELETE /api/mcp：本服务不提供服务端推送流，也不维护会话
pub async fn mcp_get() -> axum::response::Response {
    use axum::response::IntoResponse;
    (axum::http::StatusCode::METHOD_NOT_ALLOWED, [(axum::http::header::ALLOW, "POST")], "").into_response()
}

/// POST /api/mcp  MCP Streamable HTTP：JSON-RPC 2.0（单条或批量），需 Authorization: Bearer <令牌>
pub async fn mcp_post(State(db): State<AppState>, headers: axum::http::HeaderMap, body: axum::body::Bytes) -> axum::response::Response {
    use axum::http::StatusCode;
    let auth = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(String::from);
    let client = headers.get(axum::http::header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.chars().take(120).collect::<String>());
    let msg: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return mcp_json(StatusCode::BAD_REQUEST, serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("JSON 解析失败：{e}") } })),
    };
    let r = blocking(&db, move |conn| {
        let caller = crate::mcp::authenticate(conn, auth.as_deref())?;
        let out = match &msg {
            serde_json::Value::Array(items) => {
                let rs: Vec<serde_json::Value> = items.iter().filter_map(|m| crate::mcp::handle(conn, &caller, m, client.as_deref())).collect();
                if rs.is_empty() { None } else { Some(serde_json::Value::Array(rs)) }
            }
            m => crate::mcp::handle(conn, &caller, m, client.as_deref()),
        };
        Ok(out)
    })
    .await;
    match r {
        Ok(Some(v)) => mcp_json(StatusCode::OK, v),
        Ok(None) => mcp_json(StatusCode::ACCEPTED, serde_json::Value::Null).map(|_| axum::body::Body::empty()),
        Err(AppError::Forbidden(m)) => {
            let mut resp = mcp_json(StatusCode::UNAUTHORIZED, serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32001, "message": m } }));
            resp.headers_mut().insert(axum::http::header::WWW_AUTHENTICATE, axum::http::HeaderValue::from_static("Bearer"));
            resp
        }
        Err(e) => mcp_json(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32603, "message": e.message() } })),
    }
}

/// GET /api/ledger/mcp/tokens  有效的 MCP 令牌（不含明文）
pub async fn mcp_tokens(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<crate::mcp::TokenView>>>> {
    Ok(ok(blocking(&db, crate::mcp::list_tokens).await?))
}

/// POST /api/ledger/mcp/tokens  { name, allowSql?, datasets?, operator? } → { token, secret }（明文只返回这一次）
pub async fn mcp_create_token(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<serde_json::Value>>> {
    let input: crate::mcp::TokenInput = parse_json(&body)?;
    let (view, secret) = blocking(&db, move |conn| crate::mcp::create_token(conn, input)).await?;
    Ok(ok(serde_json::json!({ "token": view, "secret": secret })))
}

/// PUT /api/ledger/mcp/tokens/{id}  修改名称 / 是否允许 SQL / 可访问的数据集
pub async fn mcp_update_token(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::mcp::TokenInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::mcp::update_token(conn, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/mcp/tokens/{id}  吊销（立即失效）
pub async fn mcp_revoke_token(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::mcp::revoke_token(conn, &id)).await?;
    Ok(ok(true))
}

/// GET /api/ledger/mcp/logs?limit=  调用审计日志（新的在前）
pub async fn mcp_logs(State(db): State<AppState>, Query(q): Query<RunsQuery>) -> AppResult<Json<Envelope<Vec<crate::mcp::LogView>>>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    Ok(ok(blocking(&db, move |conn| crate::mcp::logs(conn, limit)).await?))
}

// ── 逐笔对账 ──────────────────────────────────────────

/// 勾对人：登录用户的名字；没有登录体系时用请求里的 operator
fn who(user: &CurrentUser, fallback: &str) -> String {
    if user.enforced && !user.name.is_empty() { user.name.clone() } else { fallback.trim().to_string() }
}

#[derive(Deserialize)]
pub struct ReconBody {
    #[serde(default)]
    pub name: String,
    pub def: crate::recon::ReconDef,
    /// 修改时：换了数据集 / 金额列后确认清空已有勾对
    #[serde(default, rename = "resetMatches")]
    pub reset_matches: bool,
}

/// POST /api/ledger/recons/preview  { def }  新建前预览：两侧行数、日期范围、容易填错的地方
pub async fn recon_preview(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::recon::Preview>>> {
    let req: ReconBody = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| crate::recon::preview(conn, &req.def)).await?))
}

/// POST /api/ledger/recons  { name, def }  创建对账（同时得到一个同名的结果数据集），返回数据集 id
pub async fn create_recon(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let req: ReconBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let id = blocking(&db, move |conn| {
        let id = crate::recon::create(conn, &req.name, &req.def)?;
        dataset::set_owner(conn, &id, &user)?;
        Ok(id)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(id))
}

/// GET /api/ledger/recons/{id}  对账的定义
pub async fn get_recon(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::recon::Info>>> {
    Ok(ok(blocking_read(&db, move |conn| crate::recon::info(conn, &id)).await?))
}

/// PUT /api/ledger/recons/{id}  { name, def, resetMatches? }  修改（所有者或管理员）
pub async fn update_recon(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: ReconBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| crate::recon::update(conn, &id, &req.name, &req.def, req.reset_matches)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconSummaryQuery {
    #[serde(default)]
    pub as_of: Option<String>,
}

/// GET /api/ledger/recons/{id}/summary?asOf=  汇总：两侧笔数与金额、截至日的未达账项、勾对组统计
pub async fn recon_summary(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<ReconSummaryQuery>) -> AppResult<Json<Envelope<crate::recon::Summary>>> {
    Ok(ok(blocking_read(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::summary(conn, &ctx, q.as_of.as_deref())
    })
    .await?))
}

/// GET /api/ledger/recons/{id}/items?side=left|right&status=open|matched|all&q=&amount=&from=&to=&offset=&limit=  行列表
pub async fn recon_items(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<crate::recon::ItemQuery>) -> AppResult<Json<Envelope<crate::recon::ItemPage>>> {
    Ok(ok(blocking_read(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::items(conn, &ctx, &q)
    })
    .await?))
}

/// GET /api/ledger/recons/{id}/groups?method=&withDiff=&broken=&q=&offset=&limit=  勾对组（含组内的行）
pub async fn recon_groups(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<crate::recon::GroupQuery>) -> AppResult<Json<Envelope<crate::recon::GroupPage>>> {
    Ok(ok(blocking_read(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::groups(conn, &ctx, &q)
    })
    .await?))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReconAutoBody {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/recons/{id}/auto  { dryRun? }  自动勾对（dryRun 只试算不保存）
pub async fn recon_auto(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::recon::AutoOutcome>>> {
    let req: ReconAutoBody = if body.is_empty() { ReconAutoBody::default() } else { parse_json(&body)? };
    let _guard = db.import_lock.lock().await;
    let operator = who(&user, &req.operator);
    let dry = req.dry_run;
    let r = blocking(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::auto_match(conn, &ctx, &operator, dry)
    })
    .await?;
    if !dry {
        db.cursors.invalidate();
        db.checkpoint();
    }
    Ok(ok(r))
}

#[derive(Deserialize)]
pub struct ReconManualBody {
    #[serde(flatten)]
    pub req: crate::recon::ManualRequest,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/recons/{id}/match  { left: [行标识], right: [行标识], note?, force? }  手工勾对（任意多对多）
pub async fn recon_match(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::recon::ManualOutcome>>> {
    let req: ReconManualBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let operator = who(&user, &req.operator);
    let r = blocking(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::manual_match(conn, &ctx, &req.req, &operator)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}

/// POST /api/ledger/recons/{id}/unmatch  { groups: [勾对组] } 或 { mode: auto | all }  取消勾对
pub async fn recon_unmatch(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::recon::UnmatchOutcome>>> {
    let req: crate::recon::UnmatchRequest = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::unmatch(conn, &ctx, &req)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}

// ── 填报回写 ──────────────────────────────────────────

/// POST /api/ledger/datasets/{id}/writeback  { mode: upsert|append, columns, rows, keys?, operator?, source? }
/// 整批校验，有错误时不写入并返回出错的格子（errors）
pub async fn dataset_writeback(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::writeback::WritebackOutcome>>> {
    let req: crate::writeback::WritebackRequest = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| crate::writeback::submit(conn, &id, &req)).await?;
    if r.batch_id.is_some() {
        tracing::info!(dataset = %r.dataset_name, inserted = r.inserted, updated = r.updated, "writeback");
        db.cursors.invalidate();
        db.checkpoint();
    }
    Ok(ok(r))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WritebackListQuery {
    #[serde(default)]
    pub dataset_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// GET /api/ledger/writebacks?datasetId=&limit=  提交记录（新的在前），附带能否撤销
pub async fn writebacks(State(db): State<AppState>, Query(q): Query<WritebackListQuery>) -> AppResult<Json<Envelope<Vec<crate::writeback::WritebackRecord>>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    Ok(ok(blocking(&db, move |conn| crate::writeback::list(conn, q.dataset_id.as_deref().filter(|s| !s.is_empty()), limit)).await?))
}

/// POST /api/ledger/writebacks/{id}/undo?operator=  撤销一次提交（数据集之后没有别的改动时）
pub async fn undo_writeback(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<OperatorQuery>) -> AppResult<Json<Envelope<crate::writeback::UndoOutcome>>> {
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| crate::writeback::undo(conn, &id, &q.operator)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}

/// POST /api/ledger/datasets/input  { name, columns: [{ title, kind }], primaryKey }  新建空的填报数据集，返回 id
pub async fn create_input_dataset(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::writeback::NewInputDataset = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let id = blocking(&db, move |conn| {
        let id = crate::writeback::create_dataset(conn, &input)?;
        crate::dataset::set_owner(conn, &id, &user)?;
        Ok(id)
    })
    .await?;
    db.checkpoint();
    Ok(ok(id))
}

/// GET /api/ledger/datasets/{id}/anomalies  异常发现：最近 7 天相对此前 28 天明显偏离的走势与分组（受限用户只扫描自己范围内的数据）
pub async fn dataset_anomalies(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::anomaly::ScanResult>>> {
    Ok(ok(blocking_heavy(&db, move |conn| crate::anomaly::scan_dataset(conn, &id)).await?))
}

/// GET /api/ledger/datasets/{id}/usage  使用情况：引用它的工作簿 / 模板 / 视图 / SQL / 连接 / 推送，近 30 天访问次数
pub async fn dataset_usage(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::usage::DatasetUsage>>> {
    Ok(ok(blocking(&db, move |conn| crate::usage::usage(conn, &id)).await?))
}
