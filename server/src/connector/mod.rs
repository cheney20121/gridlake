//! 数据连接器：按计划把外部数据（文件夹 / 数据库）自动同步进数据集
//!
//! 所有来源先落成表格文件，再走数据集导入管线（类型识别、数值清洗、主键去重 / 覆盖），口径与手工导入一致。
//! 取数阶段不加锁；写入阶段与其他导入串行（import_lock），单个文件 / 单次查询在一个事务里写入，失败不留半截数据，
//! 增量位置只在写入成功后推进。设计见 web/docs/connectors-design.md

mod clickhouse;
pub mod database;
pub mod folder;
pub mod doudian;
pub mod http;
pub mod s3;
pub mod secret;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{NaiveDateTime, NaiveTime};
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::dataset_import::{self, CommitColumn, CommitRequest, Mode};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::routes::AppState;

use database::DbSource;
use http::HttpSource;
use folder::FolderSource;
use s3::S3Source;

/// 单次同步整体超时
const RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 每个连接器保留的同步记录条数
const KEEP_RUNS: i64 = 200;
/// 预览取的行数
const PREVIEW_LIMIT: u32 = 200;

// ── 定义 ──────────────────────────────────────────────

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    Folder(FolderSource),
    Database(DbSource),
    Http(HttpSource),
    S3(S3Source),
}

impl Source {
    fn kind(&self) -> &'static str {
        match self {
            Source::Folder(_) => "folder",
            Source::Database(_) => "database",
            Source::Http(_) => "http",
            Source::S3(_) => "s3",
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// 每次用最新数据整体替换
    Replace,
    /// 只加入新数据（有主键时跳过已有的行）
    Append,
    /// 按主键新增或覆盖
    Upsert,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// 已有数据集；为空时首次同步按 name + columns 新建，之后记录在这里
    #[serde(default)]
    pub dataset_id: Option<String>,
    /// 新建数据集的名称
    #[serde(default)]
    pub name: Option<String>,
    pub mode: SyncMode,
    #[serde(default)]
    pub primary_key: Vec<String>,
    /// 新建时的列（来自预览，可改名 / 改类型 / 不导入）
    #[serde(default)]
    pub columns: Vec<CommitColumn>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Schedule {
    Manual,
    /// 每 N 分钟（≥ 5）
    Interval { minutes: u32 },
    /// 每天固定时间 HH:MM
    Daily { at: String },
}

impl Schedule {
    fn validate(&self) -> AppResult<()> {
        match self {
            Schedule::Manual => Ok(()),
            Schedule::Interval { minutes } if *minutes >= 5 && *minutes <= 7 * 24 * 60 => Ok(()),
            Schedule::Interval { .. } => Err(AppError::bad("同步间隔应在 5 分钟到 7 天之间")),
            Schedule::Daily { at } => NaiveTime::parse_from_str(at, "%H:%M").map(|_| ()).map_err(|_| AppError::bad("每天同步时间格式应为 HH:MM")),
        }
    }

    /// 下次同步时间（last：上次开始时间）
    fn next_run(&self, last: Option<NaiveDateTime>, created: NaiveDateTime) -> Option<NaiveDateTime> {
        match self {
            Schedule::Manual => None,
            Schedule::Interval { minutes } => Some(last.map(|l| l + chrono::Duration::minutes(*minutes as i64)).unwrap_or(created)),
            Schedule::Daily { at } => {
                let t = NaiveTime::parse_from_str(at, "%H:%M").ok()?;
                // 新建后不立即补跑当天已过的时间点，从下一个时间点开始
                let base = last.unwrap_or(created);
                let mut next = base.date().and_time(t);
                if next <= base {
                    next += chrono::Duration::days(1);
                }
                Some(next)
            }
        }
    }
}

/// 新建 / 修改请求
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorInput {
    pub name: String,
    pub source: Source,
    pub target: Target,
    pub schedule: Schedule,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub connector_id: String,
    pub trigger: String,
    /// running / success / failed / skipped
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub rows_read: i64,
    pub inserted: i64,
    pub updated: i64,
    pub skipped: i64,
    pub replaced: i64,
    pub files: Vec<String>,
    pub watermark_from: Option<String>,
    pub watermark_to: Option<String>,
    pub message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorView {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub source: Source,
    /// 数据库来源是否已保存密码（密码本身永不返回）
    pub has_password: bool,
    pub target: Target,
    /// 目标数据集当前名称（已删除时为空）
    pub dataset_name: Option<String>,
    pub schedule: Schedule,
    pub enabled: bool,
    pub watermark: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_run: Option<RunRecord>,
    pub next_run: Option<String>,
    pub running: bool,
}

struct Stored {
    id: String,
    name: String,
    source: Source,
    target: Target,
    schedule: Schedule,
    enabled: bool,
    watermark: Option<String>,
    created_at: String,
    updated_at: String,
}

// ── 运行时状态 ────────────────────────────────────────

pub struct Runtime {
    pub secret: secret::Secret,
    pub inbox_roots: Vec<PathBuf>,
    pub max_rows: i64,
    /// 从对象存储下载单个文件的大小上限（同上传上限）
    pub max_file_bytes: u64,
    s3_client: reqwest::Client,
    running: Mutex<HashSet<String>>,
}

impl Runtime {
    pub fn new(config: &crate::config::Config) -> anyhow::Result<Self> {
        let data_dir = config.db_path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        for r in &config.inbox_roots {
            if let Err(e) = std::fs::create_dir_all(r) {
                tracing::warn!("无法创建收件目录 {}：{e}", r.display());
            }
        }
        Ok(Self {
            secret: secret::Secret::load(&data_dir)?,
            inbox_roots: config.inbox_roots.clone(),
            max_rows: config.connector_max_rows,
            max_file_bytes: config.max_upload_mb as u64 * 1024 * 1024,
            s3_client: s3::client().map_err(|e| anyhow::anyhow!(e.message()))?,
            running: Mutex::new(HashSet::new()),
        })
    }

    fn is_running(&self, id: &str) -> bool {
        self.running.lock().map(|r| r.contains(id)).unwrap_or(false)
    }
}

// ── 存取 ──────────────────────────────────────────────

fn parse_ts(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()
}

fn load_all(conn: &Connection) -> AppResult<Vec<Stored>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, source, target, schedule, enabled, watermark, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(updated_at, '%Y-%m-%d %H:%M:%S')
         FROM ledger_connector ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, bool>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, name, source, target, schedule, enabled, watermark, created, updated)| {
            Ok(Stored {
                source: serde_json::from_str(&source).map_err(|e| AppError::bad(format!("连接器「{name}」来源配置损坏：{e}")))?,
                target: serde_json::from_str(&target).map_err(|e| AppError::bad(format!("连接器「{name}」目标配置损坏：{e}")))?,
                schedule: serde_json::from_str(&schedule).unwrap_or(Schedule::Manual),
                id,
                name,
                enabled,
                watermark,
                created_at: created.unwrap_or_default(),
                updated_at: updated.unwrap_or_default(),
            })
        })
        .collect()
}

fn load_one(conn: &Connection, id: &str) -> AppResult<Stored> {
    load_all(conn)?.into_iter().find(|c| c.id == id).ok_or_else(|| AppError::bad("连接器不存在"))
}

fn run_from_row(r: &duckdb::Row<'_>) -> duckdb::Result<RunRecord> {
    let files: Option<String> = r.get(11)?;
    Ok(RunRecord {
        id: r.get(0)?,
        connector_id: r.get(1)?,
        trigger: r.get(2)?,
        status: r.get(3)?,
        started_at: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        finished_at: r.get(5)?,
        rows_read: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
        inserted: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
        updated: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
        skipped: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
        replaced: r.get::<_, Option<i64>>(10)?.unwrap_or(0),
        files: files.and_then(|f| serde_json::from_str(&f).ok()).unwrap_or_default(),
        watermark_from: r.get(12)?,
        watermark_to: r.get(13)?,
        message: r.get(14)?,
    })
}

const RUN_COLUMNS: &str = "id, connector_id, trigger_by, status, strftime(started_at, '%Y-%m-%d %H:%M:%S'), strftime(finished_at, '%Y-%m-%d %H:%M:%S'),
    rows_read, inserted, updated, skipped, replaced, files, watermark_from, watermark_to, message";

/// 写入该数据集的数据连接：(id, 名称, 同步方式说明, 是否启用)
pub fn targeting(conn: &Connection, dataset_id: &str) -> AppResult<Vec<(String, String, String, bool)>> {
    Ok(load_all(conn)?
        .into_iter()
        .filter(|c| c.target.dataset_id.as_deref() == Some(dataset_id))
        .map(|c| {
            let mode = match c.target.mode {
                SyncMode::Replace => "整体替换",
                SyncMode::Append => "追加",
                SyncMode::Upsert => "按主键更新",
            };
            (c.id, c.name, mode.to_string(), c.enabled)
        })
        .collect())
}

/// 以「整体替换」方式写入该数据集的数据连接名称（填报回写时用来拦截：写入的数据会在下次同步时丢失）
pub fn replacing_connector(conn: &Connection, dataset_id: &str) -> AppResult<Option<String>> {
    Ok(load_all(conn)?.into_iter().find(|c| c.target.dataset_id.as_deref() == Some(dataset_id) && c.target.mode == SyncMode::Replace).map(|c| c.name))
}

pub fn runs(conn: &Connection, connector_id: &str, limit: i64) -> AppResult<Vec<RunRecord>> {
    let mut stmt = conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM ledger_connector_run WHERE connector_id = ? ORDER BY started_at DESC, id DESC LIMIT ?"))?;
    Ok(stmt.query_map(params![connector_id, limit], run_from_row)?.collect::<Result<Vec<_>, _>>()?)
}

pub fn list(conn: &Connection, rt: &Runtime) -> AppResult<Vec<ConnectorView>> {
    let datasets = crate::dataset::datasets(conn)?;
    load_all(conn)?
        .into_iter()
        .map(|c| {
            let last_run = runs(conn, &c.id, 1)?.into_iter().next();
            let last_start = last_run.as_ref().and_then(|r| parse_ts(&r.started_at));
            let next_run = if c.enabled {
                c.schedule.next_run(last_start, parse_ts(&c.created_at).unwrap_or_else(|| chrono::Local::now().naive_local())).map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            } else {
                None
            };
            let (source, has_password) = redact(c.source);
            Ok(ConnectorView {
                kind: source.kind().into(),
                dataset_name: c.target.dataset_id.as_ref().and_then(|id| datasets.iter().find(|d| &d.id == id)).map(|d| d.name.clone()),
                running: rt.is_running(&c.id),
                id: c.id,
                name: c.name,
                source,
                has_password,
                target: c.target,
                schedule: c.schedule,
                enabled: c.enabled,
                watermark: c.watermark,
                created_at: c.created_at,
                updated_at: c.updated_at,
                last_run,
                next_run,
            })
        })
        .collect()
}

/// 去掉密码（含密文），返回是否已保存密码
fn redact(source: Source) -> (Source, bool) {
    match source {
        Source::Database(mut d) => {
            let has = d.password_enc.is_some();
            d.password = None;
            d.password_enc = None;
            (Source::Database(d), has)
        }
        Source::Http(mut h) => {
            let has = h.secret_enc.is_some();
            h.secret = None;
            h.secret_enc = None;
            (Source::Http(h), has)
        }
        Source::S3(mut x) => {
            let has = x.secret_enc.is_some();
            x.secret_key = None;
            x.secret_enc = None;
            (Source::S3(x), has)
        }
        other => (other, false),
    }
}

/// 请求中的明文密码 → 密文；未填写时沿用已保存的
fn seal(rt: &Runtime, source: Source, previous: Option<&Source>) -> AppResult<Source> {
    match source {
        Source::Database(mut d) => {
            match d.password.take().filter(|p| !p.is_empty()) {
                Some(p) => d.password_enc = Some(rt.secret.encrypt(&p)?),
                None => {
                    d.password_enc = match previous {
                        Some(Source::Database(old)) => old.password_enc.clone(),
                        _ => None,
                    }
                }
            }
            Ok(Source::Database(d))
        }
        Source::Http(mut h) => {
            match h.secret.take().filter(|p| !p.is_empty()) {
                Some(p) => h.secret_enc = Some(rt.secret.encrypt(&p)?),
                None => {
                    h.secret_enc = match previous {
                        Some(Source::Http(old)) => old.secret_enc.clone(),
                        _ => None,
                    }
                }
            }
            Ok(Source::Http(h))
        }
        Source::S3(mut x) => {
            match x.secret_key.take().filter(|p| !p.is_empty()) {
                Some(p) => x.secret_enc = Some(rt.secret.encrypt(&p)?),
                None => {
                    x.secret_enc = match previous {
                        Some(Source::S3(old)) => old.secret_enc.clone(),
                        _ => None,
                    }
                }
            }
            Ok(Source::S3(x))
        }
        other => Ok(other),
    }
}

fn password_of(rt: &Runtime, d: &DbSource) -> AppResult<String> {
    if let Some(p) = d.password.as_deref().filter(|p| !p.is_empty()) {
        return Ok(p.to_string());
    }
    match &d.password_enc {
        Some(enc) => Ok(rt.secret.decrypt(enc).map_err(|e| AppError::bad(e.to_string()))?),
        None => Ok(String::new()),
    }
}

fn secret_of(rt: &Runtime, h: &HttpSource) -> AppResult<String> {
    if let Some(p) = h.secret.as_deref().filter(|p| !p.is_empty()) {
        return Ok(p.to_string());
    }
    match &h.secret_enc {
        Some(enc) => Ok(rt.secret.decrypt(enc).map_err(|e| AppError::bad(e.to_string()))?),
        None => Ok(String::new()),
    }
}

fn s3_secret_of(rt: &Runtime, x: &S3Source) -> AppResult<String> {
    if let Some(p) = x.secret_key.as_deref().filter(|p| !p.is_empty()) {
        return Ok(p.to_string());
    }
    match &x.secret_enc {
        Some(enc) => Ok(rt.secret.decrypt(enc).map_err(|e| AppError::bad(e.to_string()))?),
        None => Err(AppError::bad("请填写访问密钥（Secret Key）")),
    }
}

/// 对象存储来源必须有密钥（新填的，或编辑时沿用已保存的）；要在 seal 之后检查
fn ensure_secret(source: &Source) -> AppResult<()> {
    match source {
        Source::S3(x) if x.secret_enc.is_none() => Err(AppError::bad("请填写访问密钥（Secret Key）")),
        _ => Ok(()),
    }
}

fn validate(conn: &Connection, rt: &Runtime, input: &ConnectorInput, self_id: Option<&str>) -> AppResult<()> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::bad("连接器名称不能为空，且不超过 40 个字符"));
    }
    if load_all(conn)?.iter().any(|c| c.name == name && Some(c.id.as_str()) != self_id) {
        return Err(AppError::bad(format!("已存在名为「{name}」的连接器")));
    }
    input.schedule.validate()?;
    match &input.source {
        Source::Folder(f) => {
            folder::resolve_dir(&rt.inbox_roots, &f.path)?;
            if f.pattern.trim().is_empty() {
                return Err(AppError::bad("请填写文件名规则，如 *.csv"));
            }
        }
        Source::Database(d) => {
            database::check_sql(&d.sql)?;
            if d.host.trim().is_empty() || d.database.trim().is_empty() || d.user.trim().is_empty() {
                return Err(AppError::bad("请填写数据库地址、库名与账号"));
            }
            if d.incremental_column.as_deref().is_some_and(|c| !c.trim().is_empty()) && input.target.primary_key.is_empty() {
                return Err(AppError::bad("增量同步需要设置主键（用于去重 / 覆盖重复读到的行）"));
            }
        }
        Source::S3(x) => {
            s3::check(x)?;
        }
        Source::Http(h) => {
            http::check(h)?;
            if h.incremental_field.as_deref().is_some_and(|c| !c.trim().is_empty()) && input.target.primary_key.is_empty() {
                return Err(AppError::bad("增量同步需要设置主键（用于去重 / 覆盖重复读到的行）"));
            }
        }
    }
    let t = &input.target;
    if t.mode == SyncMode::Upsert && t.primary_key.is_empty() {
        return Err(AppError::bad("「增量更新」需要设置主键"));
    }
    match &t.dataset_id {
        Some(id) => {
            if !crate::dataset::stored(conn)?.iter().any(|(i, _, _)| i == id) {
                return Err(AppError::bad("目标数据集不存在，或不是导入的数据表（内置数据集、关联视图不能作为同步目标）"));
            }
        }
        None => {
            let n = t.name.as_deref().map(str::trim).unwrap_or("");
            if n.is_empty() {
                return Err(AppError::bad("请填写新数据集的名称，或选择已有数据集"));
            }
            if crate::dataset::datasets(conn)?.iter().any(|d| d.name == n) {
                return Err(AppError::bad(format!("已存在名为「{n}」的数据集；要同步到它请选择「已有数据集」")));
            }
            if !t.columns.iter().any(|c| c.include) {
                return Err(AppError::bad("请先预览数据，确定要导入的列"));
            }
            for k in &t.primary_key {
                if !t.columns.iter().any(|c| c.include && c.title.trim() == k) {
                    return Err(AppError::bad(format!("主键列「{k}」不在要导入的列中")));
                }
            }
        }
    }
    Ok(())
}

pub fn create(conn: &Connection, rt: &Runtime, input: ConnectorInput) -> AppResult<String> {
    validate(conn, rt, &input, None)?;
    let id = format!("cn_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    let source = seal(rt, input.source, None)?;
    ensure_secret(&source)?;
    let now = now_local();
    conn.execute(
        "INSERT INTO ledger_connector (id, name, kind, source, target, schedule, enabled, watermark, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, NULL, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![id, input.name.trim(), source.kind(), serde_json::to_string(&source)?, serde_json::to_string(&input.target)?, serde_json::to_string(&input.schedule)?, input.enabled, now, now],
    )?;
    Ok(id)
}

pub fn update(conn: &Connection, rt: &Runtime, id: &str, input: ConnectorInput) -> AppResult<()> {
    let old = load_one(conn, id)?;
    if rt.is_running(id) {
        return Err(AppError::bad("正在同步中，请稍后再修改"));
    }
    validate(conn, rt, &input, Some(id))?;
    let source = seal(rt, input.source, Some(&old.source))?;
    ensure_secret(&source)?;
    // 来源查询或目标数据集变了：增量位置从头开始
    let same_source = match (&old.source, &source) {
        (Source::Database(a), Source::Database(b)) => a.same_query(b),
        (Source::Folder(a), Source::Folder(b)) => a == b,
        (Source::Http(a), Source::Http(b)) => a.same_query(b),
        (Source::S3(a), Source::S3(b)) => a.same_location(b),
        _ => false,
    };
    let keep_watermark = same_source && old.target.dataset_id == input.target.dataset_id;
    conn.execute(
        "UPDATE ledger_connector SET name = ?, kind = ?, source = ?, target = ?, schedule = ?, enabled = ?,
           watermark = CASE WHEN ? THEN watermark ELSE NULL END, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
        params![
            input.name.trim(),
            source.kind(),
            serde_json::to_string(&source)?,
            serde_json::to_string(&input.target)?,
            serde_json::to_string(&input.schedule)?,
            input.enabled,
            keep_watermark,
            now_local(),
            id
        ],
    )?;
    Ok(())
}

/// 删除连接器（数据集与已同步的数据保留）
pub fn delete(conn: &Connection, rt: &Runtime, id: &str) -> AppResult<()> {
    if rt.is_running(id) {
        return Err(AppError::bad("正在同步中，请稍后再删除"));
    }
    load_one(conn, id)?;
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        for t in ["ledger_connector_file", "ledger_connector_run"] {
            conn.execute(&format!("DELETE FROM {t} WHERE connector_id = ?"), params![id])?;
        }
        conn.execute("DELETE FROM ledger_connector WHERE id = ?", params![id])?;
        Ok(())
    })();
    match r {
        Ok(()) => Ok(conn.execute_batch("COMMIT")?),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 服务启动时：上次未结束的同步标为中断
pub fn recover(conn: &Connection) -> AppResult<()> {
    let n = conn.execute(
        "UPDATE ledger_connector_run SET status = 'failed', finished_at = CAST(? AS TIMESTAMP), message = '服务重启，同步被中断（下次会自动重试）' WHERE status = 'running'",
        params![now_local()],
    )?;
    if n > 0 {
        tracing::warn!("{n} 个同步因服务重启被中断");
    }
    Ok(())
}

// ── 测试与预览 ────────────────────────────────────────

/// 测试 / 预览请求：来源配置；编辑已有连接器时带上 id，未填写密码时用已保存的
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInput {
    #[serde(default)]
    pub connector_id: Option<String>,
    pub source: Source,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestOutcome {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<database::TestResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<folder::FileEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<http::HttpTestResult>,
    pub inbox_roots: Vec<String>,
}

fn imported_checker(conn: &Connection, connector_id: Option<&str>) -> AppResult<impl Fn(&str, u64, &str) -> bool + use<>> {
    let set: HashSet<(String, i64, String)> = match connector_id {
        Some(id) => {
            let mut stmt = conn.prepare("SELECT path, size, mtime FROM ledger_connector_file WHERE connector_id = ?")?;
            stmt.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?
        }
        None => HashSet::new(),
    };
    Ok(move |p: &str, size: u64, mtime: &str| set.contains(&(p.to_string(), size as i64, mtime.to_string())))
}

fn resolve_probe(conn: &Connection, rt: &Runtime, input: ProbeInput) -> AppResult<Source> {
    let previous = match &input.connector_id {
        Some(id) => Some(load_one(conn, id)?.source),
        None => None,
    };
    seal(rt, input.source, previous.as_ref())
}

pub fn roots(rt: &Runtime) -> Vec<String> {
    rt.inbox_roots.iter().map(|r| r.display().to_string()).collect()
}

pub async fn test(state: &AppState, input: ProbeInput) -> AppResult<TestOutcome> {
    let rt = &state.connectors;
    let conn = state.connection()?;
    let connector_id = input.connector_id.clone();
    let source = resolve_probe(&conn, rt, input)?;
    match source {
        Source::Folder(f) => {
            let files = folder::list(&rt.inbox_roots, &f, imported_checker(&conn, connector_id.as_deref())?)?;
            let new = files.iter().filter(|x| !x.imported).count();
            Ok(TestOutcome {
                message: if files.is_empty() { "目录可以访问，但没有匹配的文件".into() } else { format!("匹配到 {} 个文件，其中 {new} 个未导入", files.len()) },
                database: None,
                files: Some(files),
                http: None,
                inbox_roots: roots(rt),
            })
        }
        Source::S3(x) => {
            let imported = imported_checker(&conn, connector_id.as_deref())?;
            drop(conn);
            let files = s3::list(&x, &s3_secret_of(rt, &x)?, &rt.s3_client, imported).await?;
            let new = files.iter().filter(|f| !f.imported).count();
            Ok(TestOutcome {
                message: if files.is_empty() { "存储桶可以访问，但没有匹配的文件".into() } else { format!("匹配到 {} 个文件，其中 {new} 个未导入", files.len()) },
                database: None,
                files: Some(files),
                http: None,
                inbox_roots: roots(rt),
            })
        }
        Source::Database(d) => {
            drop(conn);
            let password = password_of(rt, &d)?;
            let r = database::test(&d, &password).await?;
            Ok(TestOutcome {
                message: if r.columns.is_empty() { format!("连接成功：{}", r.server_version) } else { format!("连接成功：{}，查询结果 {} 列", r.server_version, r.columns.len()) },
                database: Some(r),
                files: None,
                http: None,
                inbox_roots: roots(rt),
            })
        }
        Source::Http(h) => {
            drop(conn);
            let secret = secret_of(rt, &h)?;
            let r = http::test(&h, &secret).await?;
            Ok(TestOutcome {
                message: if r.records == 0 { "接口可以访问，第一页没有记录".into() } else { format!("接口可以访问：第一页 {} 条记录，{} 个字段", r.records, r.fields.len()) },
                database: None,
                files: None,
                http: Some(r),
                inbox_roots: roots(rt),
            })
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOutcome {
    /// 预览所用的文件（文件夹来源：最新的匹配文件）
    pub sample_of: String,
    #[serde(flatten)]
    pub preview: dataset_import::ImportPreview,
}

/// 预览：取一部分数据，推断列名与类型，并给出可同步到的已有数据集
pub async fn preview(state: &AppState, input: ProbeInput) -> AppResult<PreviewOutcome> {
    let rt = &state.connectors;
    let token = format!("cnp{}", chrono::Local::now().format("%H%M%S%3f"));
    let source = {
        let conn = state.connection()?;
        resolve_probe(&conn, rt, input)?
    };
    match &source {
        Source::Folder(f) => {
            let files = folder::list(&rt.inbox_roots, f, |_, _, _| false)?;
            let latest = files.last().ok_or_else(|| AppError::bad("目录中没有匹配的文件，放入一个样例文件后再预览"))?.clone();
            let staged = stage_inbox_file(state, &token, Path::new(&latest.path), &latest.name).await?;
            let st = state.clone();
            let name = latest.name.clone();
            let p = staged.clone();
            let r = tokio::task::spawn_blocking(move || -> AppResult<_> {
                let conn = st.connection()?;
                dataset_import::preview(&conn, &p, &token, &name)
            })
            .await?;
            let _ = std::fs::remove_file(&staged);
            Ok(PreviewOutcome { sample_of: latest.name, preview: r? })
        }
        Source::S3(x) => {
            let secret = s3_secret_of(rt, x)?;
            let files = s3::list(x, &secret, &rt.s3_client, |_, _, _| false).await?;
            let latest = files.last().ok_or_else(|| AppError::bad("存储桶里没有匹配的文件，放入一个样例文件后再预览"))?.clone();
            let staged = stage_s3_object(state, &token, x, &secret, &latest).await?;
            let st = state.clone();
            let name = latest.name.clone();
            let p = staged.clone();
            let r = tokio::task::spawn_blocking(move || -> AppResult<_> {
                let conn = st.connection()?;
                dataset_import::preview(&conn, &p, &token, &name)
            })
            .await?;
            let _ = std::fs::remove_file(&staged);
            Ok(PreviewOutcome { sample_of: latest.path, preview: r? })
        }
        Source::Database(_) | Source::Http(_) => {
            let staged = dataset_import::staged_path(&state.tmp_dir, &token, "preview.csv")?;
            let r = match &source {
                Source::Database(d) => database::extract(d, &password_of(rt, d)?, None, Some(PREVIEW_LIMIT), PREVIEW_LIMIT as i64 + 1, &staged).await,
                Source::Http(h) => http::extract(h, &secret_of(rt, h)?, None, Some(PREVIEW_LIMIT), PREVIEW_LIMIT as i64 + 1, &staged).await,
                Source::Folder(_) | Source::S3(_) => unreachable!(),
            };
            if let Err(e) = r {
                let _ = std::fs::remove_file(&staged);
                return Err(e);
            }
            let st = state.clone();
            let p = staged.clone();
            let r = tokio::task::spawn_blocking(move || -> AppResult<_> {
                let conn = st.connection()?;
                dataset_import::preview(&conn, &p, &token, "preview.csv")
            })
            .await?;
            let _ = std::fs::remove_file(&staged);
            let what = if matches!(source, Source::Http(_)) { "接口返回的前" } else { "查询结果前" };
            Ok(PreviewOutcome { sample_of: format!("{what} {PREVIEW_LIMIT} 行"), preview: r? })
        }
    }
}

// ── 同步 ──────────────────────────────────────────────

#[derive(Default)]
struct Tally {
    rows_read: i64,
    inserted: i64,
    updated: i64,
    skipped: i64,
    replaced: i64,
    files: Vec<String>,
    watermark_to: Option<String>,
    notes: Vec<String>,
}

impl Tally {
    fn add(&mut self, o: &dataset_import::ImportOutcome) {
        self.rows_read += o.rows_read;
        self.inserted += o.inserted;
        self.updated += o.updated;
        self.skipped += o.skipped;
        self.replaced += o.replaced;
        for i in &o.invalid {
            self.notes.push(format!("「{}」有 {} 个值无法识别为数字，按空值导入", i.column, i.count));
        }
    }
}

/// 立即同步：后台执行，返回同步记录 ID
pub async fn start(state: &AppState, id: &str, trigger: &str) -> AppResult<String> {
    {
        let mut running = state.connectors.running.lock().map_err(|_| anyhow::anyhow!("连接器状态锁已损坏"))?;
        if !running.insert(id.to_string()) {
            return Err(AppError::bad("正在同步中"));
        }
    }
    let prepared = (|| -> AppResult<(Stored, String)> {
        let conn = state.connection()?;
        let c = load_one(&conn, id)?;
        let run_id = format!("run_{}_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"), &id[id.len().saturating_sub(4)..]);
        conn.execute(
            "INSERT INTO ledger_connector_run (id, connector_id, trigger_by, status, started_at, watermark_from) VALUES (?, ?, ?, 'running', CAST(? AS TIMESTAMP), ?)",
            params![run_id, id, trigger, now_local(), c.watermark],
        )?;
        Ok((c, run_id))
    })();
    let (c, run_id) = match prepared {
        Ok(v) => v,
        Err(e) => {
            if let Ok(mut r) = state.connectors.running.lock() {
                r.remove(id);
            }
            return Err(e);
        }
    };
    let st = state.clone();
    let rid = run_id.clone();
    tokio::spawn(async move {
        let cid = c.id.clone();
        let name = c.name.clone();
        let outcome = match tokio::time::timeout(RUN_TIMEOUT, execute(&st, c, &rid)).await {
            Ok(r) => r,
            Err(_) => Err(AppError::bad("同步超过 30 分钟未完成，已中止")),
        };
        let (status, tally, message) = match outcome {
            Ok((t, status)) => {
                let msg = if t.notes.is_empty() { None } else { Some(t.notes.join("；")) };
                (status, t, msg)
            }
            Err(e) => ("failed", Tally::default(), Some(e.message())),
        };
        match &message {
            Some(m) if status == "failed" => tracing::warn!("连接器「{name}」同步失败：{m}"),
            _ => tracing::info!("连接器「{name}」同步{}：读取 {} 行，新增 {}，更新 {}", if status == "skipped" { "（无新数据）" } else { "完成" }, tally.rows_read, tally.inserted, tally.updated),
        }
        let finish = st.connection().and_then(|conn| {
            conn.execute(
                "UPDATE ledger_connector_run SET status = ?, finished_at = CAST(? AS TIMESTAMP), rows_read = ?, inserted = ?, updated = ?, skipped = ?, replaced = ?,
                   files = ?, watermark_to = ?, message = ? WHERE id = ?",
                params![
                    status,
                    now_local(),
                    tally.rows_read,
                    tally.inserted,
                    tally.updated,
                    tally.skipped,
                    tally.replaced,
                    serde_json::to_string(&tally.files).unwrap_or_default(),
                    tally.watermark_to,
                    message,
                    rid
                ],
            )?;
            // 只保留最近的记录
            conn.execute(
                "DELETE FROM ledger_connector_run WHERE connector_id = ? AND id NOT IN (SELECT id FROM ledger_connector_run WHERE connector_id = ? ORDER BY started_at DESC, id DESC LIMIT ?)",
                params![cid, cid, KEEP_RUNS],
            )?;
            Ok(())
        });
        if let Err(e) = finish {
            tracing::error!("记录同步结果失败：{e:#}");
        }
        if let Ok(mut r) = st.connectors.running.lock() {
            r.remove(&cid);
        }
        // 有新数据：触发「同步后推送」的任务
        if status == "success" {
            // 先重算依赖它的 SQL 查询，再推送（推送内容可能用到查询结果）
            crate::sqlq::on_connector_synced(&st, &cid);
        }
    });
    Ok(run_id)
}

/// 收件文件 → 导入暂存位置：先复制到临时目录（不改动源文件，GBK 会转码；也避免读到导入过程中被覆盖的文件），
/// Parquet / JSON 转成 CSV
async fn stage_inbox_file(state: &AppState, token: &str, src: &Path, name: &str) -> AppResult<PathBuf> {
    let st = state.clone();
    let (token, src, name) = (token.to_string(), src.to_path_buf(), name.to_string());
    tokio::task::spawn_blocking(move || -> AppResult<PathBuf> {
        let tmp = st.tmp_dir.join(format!("preview-{token}-src.upload"));
        std::fs::copy(&src, &tmp).map_err(|e| AppError::bad(format!("读取文件失败：{name}（{e}）")))?;
        prepare_staged(&st, &token, name, tmp)
    })
    .await?
}

/// 从对象存储下载一个对象，再按上传文件的同一套规则整理成可导入的表格文件
async fn stage_s3_object(state: &AppState, token: &str, src: &S3Source, secret: &str, entry: &folder::FileEntry) -> AppResult<PathBuf> {
    let tmp = state.tmp_dir.join(format!("preview-{token}-src.upload"));
    if let Err(e) = s3::download(src, secret, &state.connectors.s3_client, entry, &tmp, state.connectors.max_file_bytes).await {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let (st, token, name) = (state.clone(), token.to_string(), entry.name.clone());
    tokio::task::spawn_blocking(move || prepare_staged(&st, &token, name, tmp)).await?
}

/// 把拿到手的原始文件（压缩包解开、GBK 转码、xlsx 取工作表……）整理成导入用的暂存文件
fn prepare_staged(st: &AppState, token: &str, name: String, tmp: PathBuf) -> AppResult<PathBuf> {
    let conn = st.connection()?;
    let (file_name, _) = crate::import_source::prepare(&conn, &st.tmp_dir, token, vec![crate::import_source::Upload { name, path: tmp }])?;
    dataset_import::staged_path(&st.tmp_dir, token, &file_name)
}

/// 本次写入的导入方式：目标数据集还没建时为新建
fn commit_mode(t: &Target) -> Mode {
    match (&t.dataset_id, t.mode) {
        (None, _) => Mode::Create,
        (Some(_), SyncMode::Replace) => Mode::Replace,
        (Some(_), SyncMode::Append) => Mode::Append,
        (Some(_), SyncMode::Upsert) => Mode::Upsert,
    }
}

/// 写入一个表格文件（与其他导入串行）；新建数据集后把 ID 记回连接器
async fn commit_file(state: &AppState, connector_id: &str, target: &mut Target, path: PathBuf, file_name: String) -> AppResult<dataset_import::ImportOutcome> {
    let _guard = state.import_lock.lock().await;
    let st = state.clone();
    let req = CommitRequest {
        token: String::new(),
        mode: commit_mode(target),
        dataset_id: target.dataset_id.clone(),
        name: target.name.clone(),
        columns: target.columns.clone(),
        primary_key: target.primary_key.clone(),
        file_name,
    };
    let cid = connector_id.to_string();
    let mut t = target.clone();
    let (outcome, t) = tokio::task::spawn_blocking(move || -> AppResult<_> {
        let conn = st.connection()?;
        let o = dataset_import::commit(&conn, &path, &req, &st.tmp_dir)?;
        if t.dataset_id.is_none() {
            t.dataset_id = Some(o.dataset_id.clone());
            conn.execute("UPDATE ledger_connector SET target = ? WHERE id = ?", params![serde_json::to_string(&t)?, cid])?;
        }
        Ok((o, t))
    })
    .await??;
    *target = t;
    state.cursors.invalidate();
    state.checkpoint();
    Ok(outcome)
}

/// 返回（统计，状态 success / skipped）
async fn execute(state: &AppState, c: Stored, run_id: &str) -> AppResult<(Tally, &'static str)> {
    let rt = &state.connectors;
    let mut target = c.target.clone();
    let mut tally = Tally::default();
    match &c.source {
        Source::Folder(_) | Source::S3(_) => {
            let files = match &c.source {
                Source::S3(x) => {
                    let imported = imported_checker(&state.connection()?, Some(&c.id))?;
                    s3::list(x, &s3_secret_of(rt, x)?, &rt.s3_client, imported).await?
                }
                Source::Folder(f) => {
                    let conn = state.connection()?;
                    folder::list(&rt.inbox_roots, f, imported_checker(&conn, Some(&c.id))?)?
                }
                _ => unreachable!(),
            };
            let settling = files.iter().filter(|x| !x.imported && x.settling).count();
            let mut pending: Vec<folder::FileEntry> = files.into_iter().filter(|x| !x.imported && !x.settling).collect();
            // 全量替换：只要最新的一个文件
            if c.target.mode == SyncMode::Replace && pending.len() > 1 {
                let latest = pending.pop().expect("non-empty");
                tally.notes.push(format!("全量替换方式只导入最新的文件，跳过了 {} 个较早的文件", pending.len()));
                pending = vec![latest];
            }
            if settling > 0 {
                tally.notes.push(format!("{settling} 个文件刚刚修改过、可能仍在写入，下次再导入"));
            }
            if pending.is_empty() {
                return Ok((tally, "skipped"));
            }
            for (i, file) in pending.iter().enumerate() {
                // 先复制到临时目录：不改动源文件（GBK 会转码），也避免读到导入过程中被覆盖的文件
                let token = format!("cn{}_{i}", run_id.replace(|ch: char| !ch.is_ascii_alphanumeric(), ""));
                let staged = match &c.source {
                    Source::S3(x) => stage_s3_object(state, &token, x, &s3_secret_of(rt, x)?, file).await?,
                    _ => stage_inbox_file(state, &token, Path::new(&file.path), &file.name).await?,
                };
                let r = commit_file(state, &c.id, &mut target, staged.clone(), file.name.clone()).await;
                let _ = std::fs::remove_file(&staged);
                let outcome = r.map_err(|e| {
                    let done = if tally.files.is_empty() { String::new() } else { format!("（之前的 {} 个文件已导入）", tally.files.len()) };
                    AppError::bad(format!("文件「{}」导入失败：{}{done}", file.name, e.message()))
                })?;
                tally.add(&outcome);
                tally.files.push(file.name.clone());
                let conn = state.connection()?;
                conn.execute(
                    "INSERT INTO ledger_connector_file (connector_id, path, size, mtime, run_id, imported_at) VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
                    params![c.id, file.path, file.size as i64, file.mtime, run_id, now_local()],
                )?;
            }
            Ok((tally, "success"))
        }
        Source::Database(_) | Source::Http(_) => {
            let incremental = match &c.source {
                Source::Database(d) => d.incremental_column.as_deref().is_some_and(|x| !x.trim().is_empty()),
                Source::Http(h) => h.incremental_field.as_deref().is_some_and(|x| !x.trim().is_empty()),
                Source::Folder(_) | Source::S3(_) => false,
            };
            let watermark = if incremental { c.watermark.clone() } else { None };
            let staged = dataset_import::staged_path(&state.tmp_dir, &format!("cn{}", run_id.replace(|ch: char| !ch.is_ascii_alphanumeric(), "")), "query.csv")?;
            let extracted = match &c.source {
                Source::Database(d) => database::extract(d, &password_of(rt, d)?, watermark.as_deref(), None, rt.max_rows, &staged).await,
                Source::Http(h) => http::extract(h, &secret_of(rt, h)?, watermark.as_deref(), None, rt.max_rows, &staged).await,
                Source::Folder(_) | Source::S3(_) => unreachable!(),
            };
            let extracted = match extracted {
                Ok(x) => x,
                Err(e) => {
                    let _ = std::fs::remove_file(&staged);
                    return Err(e);
                }
            };
            if extracted.rows == 0 && c.target.mode != SyncMode::Replace {
                let _ = std::fs::remove_file(&staged);
                tally.watermark_to = watermark;
                return Ok((tally, "skipped"));
            }
            let r = commit_file(state, &c.id, &mut target, staged.clone(), "query.csv".into()).await;
            let _ = std::fs::remove_file(&staged);
            tally.add(&r?);
            if tally.inserted == 0 && tally.updated == 0 && tally.replaced == 0 {
                tally.notes.push(format!("读取 {} 行，与已有数据相同", tally.rows_read));
            }
            if incremental {
                tally.watermark_to = extracted.watermark.clone();
                let conn = state.connection()?;
                conn.execute("UPDATE ledger_connector SET watermark = ? WHERE id = ?", params![extracted.watermark, c.id])?;
            }
            let changed = tally.inserted > 0 || tally.updated > 0 || tally.replaced > 0;
            Ok((tally, if changed { "success" } else { "skipped" }))
        }
    }
}

/// 定时调度：每 20 秒检查一次到期的连接器
pub async fn schedule(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(20));
    loop {
        tick.tick().await;
        let due = (|| -> AppResult<Vec<(String, String)>> {
            let conn = state.connection()?;
            let now = chrono::Local::now().naive_local();
            Ok(list(&conn, &state.connectors)?
                .into_iter()
                .filter(|c| c.enabled && !c.running)
                .filter(|c| c.next_run.as_deref().and_then(parse_ts).is_some_and(|t| t <= now))
                .map(|c| (c.id, c.name))
                .collect())
        })();
        match due {
            Ok(due) => {
                for (id, name) in due {
                    if let Err(e) = start(&state, &id, "schedule").await {
                        tracing::warn!("连接器「{name}」定时同步未能开始：{}", e.message());
                    }
                }
            }
            Err(e) => tracing::warn!("检查定时同步失败：{}", e.message()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> NaiveDateTime {
        parse_ts(s).unwrap()
    }

    #[test]
    fn next_run_times() {
        let created = ts("2026-10-04 10:00:00");
        let interval = Schedule::Interval { minutes: 30 };
        assert_eq!(interval.next_run(None, created), Some(created));
        assert_eq!(interval.next_run(Some(ts("2026-10-04 11:00:00")), created), Some(ts("2026-10-04 11:30:00")));

        let daily = Schedule::Daily { at: "02:30".into() };
        // 从没同步过：今天 02:30 已过 → 明天
        assert_eq!(daily.next_run(None, created), Some(ts("2026-10-05 02:30:00")));
        // 今天 02:30 已同步 → 明天
        assert_eq!(daily.next_run(Some(ts("2026-10-04 02:30:05")), created), Some(ts("2026-10-05 02:30:00")));
        // 昨天手动同步过、今天还没到点 → 今天
        assert_eq!(daily.next_run(Some(ts("2026-10-03 15:00:00")), created), Some(ts("2026-10-04 02:30:00")));
        assert_eq!(Schedule::Manual.next_run(None, created), None);
    }

    #[test]
    fn schedule_validation() {
        assert!(Schedule::Interval { minutes: 4 }.validate().is_err());
        assert!(Schedule::Interval { minutes: 10 }.validate().is_ok());
        assert!(Schedule::Daily { at: "25:00".into() }.validate().is_err());
        assert!(Schedule::Daily { at: "07:05".into() }.validate().is_ok());
    }
}
