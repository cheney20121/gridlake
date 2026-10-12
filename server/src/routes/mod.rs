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
use crate::workbook::{self, SaveResult, VersionInfo, WorkbookMeta};
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
    let body = Envelope::<()> { code: 404, data: None, msg: crate::msg_i18n::localize(format!("接口不存在：{}", uri.path())) };
    (axum::http::StatusCode::NOT_FOUND, Json(body)).into_response()
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

mod ops;
mod ledger;
mod datasets;
mod workbooks;
mod views;
mod connector;
mod report;
mod template;
mod ai;
mod sql;
mod mcp;
mod recon;
mod writeback;

pub use ops::*;
pub use ledger::*;
pub use datasets::*;
pub use workbooks::*;
pub use views::*;
pub use connector::*;
pub use report::*;
pub use template::*;
pub use ai::*;
pub use sql::*;
pub use mcp::*;
pub use recon::*;
pub use writeback::*;
