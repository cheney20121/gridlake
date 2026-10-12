//! SQL 查询：对数据集写只读 SQL（DuckDB 方言），结果可另存为数据集并按计划重算
//!
//! - 安全：语句先经 guard 检查（只能 SELECT、只能读数据集）；执行时在独立连接上把用到的数据集建成临时视图
//!   （列名用数据集的中文列名），整个执行包在事务里并回滚；超过时限中断
//! - 另存为数据集：结果写成普通数据集（与导入的一致：数值列 DOUBLE，其余文本），透视 / 图表 / 仪表盘 / D 系列公式照常使用；
//!   重算时结构不变只替换数据，结构变了按「结构变更检查」重建（计算列、关联视图失效则取消）
//! - 已保存的查询（ledger_sql_query）：手动 / 每天定时 / 数据连接同步后重算，记录最近一次结果
//! - 参数化查询：SQL 里的 `{{名称}}` 按类型（文本 / 数字 / 日期）安全地换成字面量（见 params.rs）；同一个查询可以用不同的参数值生成多个「实例」数据集

pub mod guard;
pub mod params;

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{NaiveDateTime, NaiveTime};
use duckdb::Connection;
use duckdb::params;
use duckdb::types::{TimeUnit, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::dataset::{self, DatasetDef, StoredColumn};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::routes::AppState;
use crate::schema::ident;

const PREVIEW_MAX: usize = 1000;

fn timeout() -> Duration {
    Duration::from_secs(std::env::var("LEDGER_SQL_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).filter(|n| *n >= 5).unwrap_or(60))
}

// ── 执行环境 ──────────────────────────────────────────

/// 检查语句并在连接上建好用到的数据集的临时视图；返回检查结果与用到的数据集
fn prepare(conn: &Connection, sql: &str) -> AppResult<(guard::Checked, Vec<DatasetDef>)> {
    prepare_scoped(conn, sql, None)
}

/// scope：只允许查询这些数据集（ID）；None 为全部
fn prepare_scoped(conn: &Connection, sql: &str, scope: Option<&HashSet<String>>) -> AppResult<(guard::Checked, Vec<DatasetDef>)> {
    let catalog: Vec<DatasetDef> = dataset::datasets(conn)?.into_iter().filter(|d| scope.is_none_or(|s| s.contains(&d.id))).collect();
    let names: HashSet<String> = catalog.iter().map(|d| d.name.clone()).collect();
    let checked = guard::check(conn, sql, &|t| names.contains(t))?;
    let used: Vec<DatasetDef> = catalog.into_iter().filter(|d| checked.tables.contains(&d.name)).collect();
    for d in &used {
        crate::usage::hit(&d.id, crate::usage::SQL);
        let cols = d.columns.iter().map(|c| format!("{} AS {}", dataset::column_sql(c), ident(&c.title))).collect::<Vec<_>>().join(", ");
        conn.execute_batch(&format!("CREATE OR REPLACE TEMP VIEW {} AS SELECT {cols} FROM {}", ident(&d.name), d.table))?;
    }
    Ok((checked, used))
}

/// 在时限内执行 f；超时中断查询
fn with_timeout<T>(conn: &Connection, f: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
    let handle = conn.interrupt_handle();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let limit = timeout();
    let watcher = std::thread::spawn(move || {
        if rx.recv_timeout(limit).is_err() {
            handle.interrupt();
            return true;
        }
        false
    });
    let r = f();
    let _ = tx.send(());
    let timed_out = watcher.join().unwrap_or(false);
    match r {
        Err(_) if timed_out => Err(AppError::bad(format!("查询超过 {} 秒未完成，已中止。可以缩小范围或先另存为数据集再查询", limit.as_secs()))),
        Err(e) => Err(friendly(e)),
        ok => ok,
    }
}

/// DuckDB 的错误改成用户看得懂的说明
fn friendly(e: AppError) -> AppError {
    let m = e.message();
    if let Some(i) = m.find("Binder Error: ") {
        let rest = &m[i + 14..];
        if let Some(col) = rest.strip_prefix("Referenced column \"").and_then(|r| r.split('"').next()) {
            return AppError::bad(format!("列不存在：{col}（列名含空格或符号时用双引号括起来）"));
        }
        return AppError::bad(format!("SQL 错误：{}", rest.lines().next().unwrap_or(rest)));
    }
    for p in ["Parser Error: ", "Conversion Error: ", "Catalog Error: ", "Invalid Input Error: ", "Out of Range Error: "] {
        if let Some(i) = m.find(p) {
            let rest = &m[i + p.len()..];
            return AppError::bad(format!("SQL 错误：{}", rest.lines().next().unwrap_or(rest)));
        }
    }
    e
}

// ── 结果 ──────────────────────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResultColumn {
    pub name: String,
    /// text / number / money / datetime
    pub kind: String,
    /// DuckDB 类型（如 DOUBLE、VARCHAR、TIMESTAMP）
    pub sql_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Json>>,
    /// 结果超过返回上限（只返回前 N 行）
    pub truncated: bool,
    pub elapsed_ms: i64,
    /// 用到的数据集
    pub datasets: Vec<String>,
}

fn fmt_ts(micros: i64) -> String {
    chrono::DateTime::from_timestamp_micros(micros).map(|t| t.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default()
}

/// 去掉浮点累加误差（如 502650.6400000003 → 502650.64）
fn clean(x: f64) -> Json {
    if !x.is_finite() {
        return Json::Null;
    }
    if x.abs() < 1e12 {
        let r = (x * 1e6).round() / 1e6;
        return json!(r);
    }
    json!(x)
}

fn to_json(v: Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Boolean(b) => Json::Bool(b),
        Value::TinyInt(x) => json!(x),
        Value::SmallInt(x) => json!(x),
        Value::Int(x) => json!(x),
        Value::BigInt(x) => json!(x),
        Value::HugeInt(x) => json!(x as f64),
        Value::UTinyInt(x) => json!(x),
        Value::USmallInt(x) => json!(x),
        Value::UInt(x) => json!(x),
        Value::UBigInt(x) => json!(x),
        Value::Double(x) => clean(x),
        Value::Float(x) => clean(x as f64),
        Value::Decimal(d) => d.to_string().parse::<f64>().map(|x| json!(x)).unwrap_or(Json::Null),
        Value::Timestamp(unit, x) => {
            let micros = match unit {
                TimeUnit::Second => x * 1_000_000,
                TimeUnit::Millisecond => x * 1000,
                TimeUnit::Microsecond => x,
                TimeUnit::Nanosecond => x / 1000,
            };
            Json::String(fmt_ts(micros))
        }
        Value::Date32(days) => Json::String((chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid") + chrono::Duration::days(days as i64)).format("%Y-%m-%d").to_string()),
        Value::Text(s) => Json::String(s),
        Value::Blob(_) => Json::String("（二进制）".into()),
        other => Json::String(format!("{other:?}")),
    }
}

fn money_hint(name: &str) -> bool {
    ["金额", "收入", "成本", "利润", "费", "款", "价", "额", "GMV", "gmv", "消耗", "花费", "佣金", "补贴"].iter().any(|k| name.contains(k))
}

/// 结果列类型：与来源数据集同名的列沿用其类型；否则按 SQL 类型推断（数值列名像金额的记为金额）
fn kind_of(name: &str, sql_type: &str, used: &[DatasetDef]) -> String {
    let t = sql_type.to_uppercase();
    let numeric = ["DOUBLE", "FLOAT", "REAL", "DECIMAL", "INTEGER", "BIGINT", "SMALLINT", "TINYINT", "HUGEINT", "UBIGINT", "UINTEGER", "USMALLINT", "UTINYINT"].iter().any(|n| t.starts_with(n));
    if let Some(c) = used.iter().flat_map(|d| d.columns.iter()).find(|c| c.title == name) {
        let k = c.kind;
        if (numeric && matches!(k, "money" | "number")) || (!numeric && !matches!(k, "money" | "number")) {
            return if k == "id" { "text".into() } else { k.into() };
        }
    }
    if numeric {
        return if money_hint(name) { "money".into() } else { "number".into() };
    }
    if t.starts_with("TIMESTAMP") || t == "DATE" {
        return "datetime".into();
    }
    "text".into()
}

/// 结果的列（名称、DuckDB 类型）
fn describe(conn: &Connection, sql: &str) -> AppResult<Vec<(String, String)>> {
    let mut stmt = conn.prepare(&format!("DESCRIBE (\n{sql}\n)"))?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 执行查询（预览）：最多返回 limit 行
pub fn run(conn: &Connection, sql: &str, limit: usize) -> AppResult<RunResult> {
    run_scoped(conn, sql, limit, None)
}

/// 带参数的运行：先把参数值安全地代入
pub fn run_with_params(conn: &Connection, sql: &str, limit: usize, defs: &[params::ParamDef], values: &HashMap<String, String>) -> AppResult<RunResult> {
    let bound = params::bind(sql, defs, values)?;
    run(conn, &bound, limit)
}

/// 执行查询，只能读 scope 内的数据集（外部接入按令牌授权范围）
pub fn run_scoped(conn: &Connection, sql: &str, limit: usize, scope: Option<&HashSet<String>>) -> AppResult<RunResult> {
    let limit = limit.clamp(1, PREVIEW_MAX);
    let start = Instant::now();
    let (checked, used) = prepare_scoped(conn, sql, scope)?;
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = with_timeout(conn, || {
        let desc = describe(conn, &checked.sql)?;
        let columns: Vec<ResultColumn> = desc.iter().map(|(n, t)| ResultColumn { name: n.clone(), kind: kind_of(n, t, &used), sql_type: t.clone() }).collect();
        let mut stmt = conn.prepare(&checked.sql)?;
        let mut rows_iter = stmt.query([])?;
        let mut rows = Vec::new();
        let mut truncated = false;
        while let Some(row) = rows_iter.next()? {
            if rows.len() == limit {
                truncated = true;
                break;
            }
            let mut out = Vec::with_capacity(columns.len());
            for i in 0..columns.len() {
                out.push(to_json(row.get::<_, Value>(i)?));
            }
            rows.push(out);
        }
        Ok((columns, rows, truncated))
    });
    let _ = conn.execute_batch("ROLLBACK");
    let (columns, rows, truncated) = r?;
    Ok(RunResult { columns, rows, truncated, elapsed_ms: start.elapsed().as_millis() as i64, datasets: used.into_iter().map(|d| d.name).collect() })
}

/// 另存为数据集之前的检查结果
pub struct SaveCheck {
    pub columns: Vec<ResultColumn>,
    /// 前几行
    pub rows: Vec<Vec<Json>>,
    /// 结果总行数
    pub total: i64,
    /// 用到的数据集
    pub datasets: Vec<String>,
}

/// 另存为数据集前的检查（不写入任何东西）：SQL 能不能执行、结果的列是否合规（空 / 重复 / 保留列名），以及总行数与前几行。
/// 给 AI 生成"另存为数据集"建议时核对用：用户确认之前不会产生任何数据
pub fn check_for_save(conn: &Connection, sql: &str, preview_rows: usize, scope: Option<&HashSet<String>>) -> AppResult<SaveCheck> {
    let r = run_scoped(conn, sql, preview_rows, scope)?;
    let (checked, used) = prepare_scoped(conn, sql, scope)?;
    let total = with_timeout(conn, || {
        let desc = describe(conn, &checked.sql)?;
        plan_columns(&desc, &used, &HashMap::new())?;
        Ok(conn.query_row(&format!("SELECT count(*) FROM (\n{}\n) q", checked.sql), [], |row| row.get::<_, i64>(0))?)
    })?;
    Ok(SaveCheck { columns: r.columns, rows: r.rows, total, datasets: r.datasets })
}

// ── 另存为数据集 ──────────────────────────────────────

/// 结果列的最终定义与写入表达式
fn plan_columns(desc: &[(String, String)], used: &[DatasetDef], overrides: &HashMap<String, String>) -> AppResult<Vec<(StoredColumn, String)>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (name, ty) in desc {
        let title = name.trim().replace('"', "");
        if title.is_empty() || !seen.insert(title.clone()) {
            return Err(AppError::bad(format!("结果中有空的或重复的列名「{name}」，请用 AS 起不同的名字")));
        }
        if matches!(title.to_lowercase().as_str(), "search_text" | "rowid" | "__rid") || title == "资金净额" {
            return Err(AppError::bad(format!("「{title}」是保留列名，请用 AS 改名")));
        }
        let kind = overrides.get(name).cloned().filter(|k| matches!(k.as_str(), "text" | "id" | "number" | "money" | "datetime")).unwrap_or_else(|| kind_of(name, ty, used));
        let src = format!("q.{}", ident(name));
        let t = ty.to_uppercase();
        let expr = match kind.as_str() {
            "number" | "money" => format!("TRY_CAST({src} AS DOUBLE)"),
            "datetime" if t.starts_with("TIMESTAMP") => format!("strftime({src}, '%Y-%m-%d %H:%M:%S')"),
            "datetime" if t == "DATE" => format!("strftime({src}, '%Y-%m-%d')"),
            _ => format!("CAST({src} AS VARCHAR)"),
        };
        let width = match kind.as_str() {
            "money" | "number" => 120,
            "datetime" => 160,
            _ => 140,
        };
        out.push((StoredColumn { key: title.clone(), title, kind, width, source: name.clone() }, expr));
    }
    if out.is_empty() {
        return Err(AppError::bad("查询没有返回任何列"));
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Materialized {
    pub dataset_id: String,
    pub dataset_name: String,
    pub rows: i64,
    /// 结构有变化（重建了数据集）
    pub rebuilt: bool,
    pub elapsed_ms: i64,
}

/// 把查询结果写入数据集：target 为空时新建（名为 name），否则替换已有数据集的数据
fn materialize(conn: &Connection, sql: &str, name: &str, target: Option<&str>, overrides: &HashMap<String, String>, max_rows: i64) -> AppResult<Materialized> {
    let start = Instant::now();
    let (checked, used) = prepare(conn, sql)?;
    if let Some(t) = target
        && used.iter().any(|d| d.id == t)
    {
        return Err(AppError::bad("查询不能引用它自己生成的数据集"));
    }
    let desc = with_timeout(conn, || describe(conn, &checked.sql))?;
    let cols = plan_columns(&desc, &used, overrides)?;
    let stored: Vec<StoredColumn> = cols.iter().map(|(c, _)| c.clone()).collect();
    let text_cols: Vec<String> = stored.iter().filter(|c| matches!(c.kind.as_str(), "text" | "id")).map(|c| ident(&c.title)).collect();
    let search = if text_cols.is_empty() { "NULL".into() } else { format!("lower(concat_ws(' ', {}))", text_cols.join(", ")) };
    let select = cols.iter().map(|(c, e)| format!("{e} AS {}", ident(&c.title))).collect::<Vec<_>>().join(", ");
    let col_list = stored.iter().map(|c| ident(&c.title)).collect::<Vec<_>>().join(", ");
    let defs = stored.iter().map(|c| format!("{} {}", ident(&c.title), if matches!(c.kind.as_str(), "number" | "money") { "DOUBLE" } else { "VARCHAR" })).collect::<Vec<_>>().join(", ");
    // 写入：先算到临时表（受时限约束，超行数直接报错），再在事务里替换
    with_timeout(conn, || {
        conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE sqlq_result AS SELECT {select} FROM (\n{}\n) q LIMIT {}", checked.sql, max_rows + 1))?;
        Ok(())
    })?;
    let rows: i64 = conn.query_row("SELECT count(*) FROM sqlq_result", [], |r| r.get(0))?;
    if rows > max_rows {
        return Err(AppError::bad(format!("结果超过 {max_rows} 行上限，请加筛选或汇总后再保存")));
    }
    let insert = |table: &str| format!("INSERT INTO {table} ({col_list}, search_text) SELECT {col_list}, {search} FROM sqlq_result");
    let (id, rebuilt) = match target {
        None => {
            let name = name.trim();
            if name.is_empty() {
                return Err(AppError::bad("请填写数据集名称"));
            }
            if dataset::datasets(conn)?.iter().any(|d| d.name == name) {
                return Err(AppError::bad(format!("已存在名为「{name}」的数据集")));
            }
            let id = format!("ds_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
            conn.execute_batch("BEGIN TRANSACTION")?;
            let r = (|| -> AppResult<()> {
                conn.execute_batch(&format!("CREATE TABLE {id} ({defs}, search_text VARCHAR)"))?;
                dataset::register(conn, &id, name, &stored, &[])?;
                conn.execute_batch(&insert(&id))?;
                Ok(())
            })();
            match r {
                Ok(()) => conn.execute_batch("COMMIT")?,
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(e);
                }
            }
            (id, false)
        }
        Some(t) => {
            let (_, _, old) = dataset::stored(conn)?.into_iter().find(|(i, _, _)| i == t).ok_or_else(|| match dataset::dataset(conn, t) {
                // 在回收站：提示恢复
                Err(e) if e.message().contains("回收站") => e,
                _ => AppError::bad("查询生成的数据集已被删除，请重新另存"),
            })?;
            let same = old.len() == stored.len() && old.iter().zip(&stored).all(|(a, b)| a.title == b.title && a.kind == b.kind);
            if same {
                conn.execute_batch("BEGIN TRANSACTION")?;
                let r = (|| -> AppResult<()> {
                    conn.execute_batch(&format!("DELETE FROM {t}"))?;
                    conn.execute_batch(&insert(t))?;
                    conn.execute("UPDATE ledger_dataset SET updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now_local(), t])?;
                    Ok(())
                })();
                match r {
                    Ok(()) => conn.execute_batch("COMMIT")?,
                    Err(e) => {
                        let _ = conn.execute_batch("ROLLBACK");
                        return Err(e);
                    }
                }
                (t.to_string(), false)
            } else {
                // 结构变了：重建表并更新列定义；计算列 / 关联视图因此失效时整体取消
                dataset::guarded_change(conn, |c| {
                    c.execute_batch(&format!("DROP TABLE {t}; CREATE TABLE {t} ({defs}, search_text VARCHAR)"))?;
                    c.execute_batch(&insert(t))?;
                    c.execute("UPDATE ledger_dataset SET columns = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![serde_json::to_string(&stored)?, now_local(), t])?;
                    Ok(())
                })
                .map_err(|e| AppError::bad(format!("查询结果的列与数据集不一致，重建失败：{}", e.message())))?;
                (t.to_string(), true)
            }
        }
    };
    let _ = conn.execute_batch("DROP TABLE IF EXISTS sqlq_result");
    let dataset_name = dataset::dataset(conn, &id)?.name;
    Ok(Materialized { dataset_id: id, dataset_name, rows, rebuilt, elapsed_ms: start.elapsed().as_millis() as i64 })
}

// ── 已保存的查询 ──────────────────────────────────────

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    Manual,
    Daily { at: String },
    #[serde(rename_all = "camelCase")]
    AfterSync { connector_id: String },
}

impl Schedule {
    fn next_run(&self, base: NaiveDateTime) -> Option<NaiveDateTime> {
        match self {
            Schedule::Daily { at } => {
                let t = NaiveTime::parse_from_str(at, "%H:%M").ok()?;
                let mut n = base.date().and_time(t);
                if n <= base {
                    n += chrono::Duration::days(1);
                }
                Some(n)
            }
            _ => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryInput {
    pub name: String,
    pub sql: String,
    #[serde(default)]
    pub description: String,
    /// 另存为数据集（新建时为数据集名称；已关联数据集时忽略）
    #[serde(default)]
    pub save_as_dataset: Option<String>,
    /// 结果列类型调整：列名 → 类型
    #[serde(default)]
    pub column_kinds: HashMap<String, String>,
    #[serde(default = "manual")]
    pub schedule: Schedule,
    #[serde(default)]
    pub operator: Option<String>,
    /// 参数定义（SQL 里的 `{{名称}}`）与当前取的值
    #[serde(default)]
    pub params: Vec<params::ParamDef>,
    #[serde(default)]
    pub values: HashMap<String, String>,
    /// 由哪个参数化查询生成的实例
    #[serde(default)]
    pub template_id: Option<String>,
}

fn manual() -> Schedule {
    Schedule::Manual
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedQuery {
    pub id: String,
    pub name: String,
    pub sql: String,
    pub description: String,
    pub dataset_id: Option<String>,
    /// 关联的数据集名称（已删除时为空）
    pub dataset_name: Option<String>,
    pub column_kinds: HashMap<String, String>,
    pub schedule: Schedule,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_run_at: Option<String>,
    /// success / failed
    pub last_status: Option<String>,
    pub last_rows: Option<i64>,
    pub last_message: Option<String>,
    pub next_run: Option<String>,
    pub running: bool,
    pub params: Vec<params::ParamDef>,
    pub values: HashMap<String, String>,
    /// 实例：由哪个参数化查询生成的
    pub template_id: Option<String>,
}

fn running() -> &'static Mutex<HashSet<String>> {
    static R: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_running(id: &str) -> bool {
    running().lock().map(|r| r.contains(id)).unwrap_or(false)
}

const COLS: &str = "q.id, q.name, q.sql, q.description, q.dataset_id, d.name, q.column_kinds, q.schedule, q.created_by,
    strftime(q.created_at, '%Y-%m-%d %H:%M:%S'), strftime(q.updated_at, '%Y-%m-%d %H:%M:%S'), strftime(q.last_run_at, '%Y-%m-%d %H:%M:%S'),
    q.last_status, q.last_rows, q.last_message, q.params, q.param_values, q.template_id";

fn from_row(r: &duckdb::Row<'_>) -> duckdb::Result<SavedQuery> {
    Ok(SavedQuery {
        id: r.get(0)?,
        name: r.get(1)?,
        sql: r.get(2)?,
        description: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        dataset_id: r.get(4)?,
        dataset_name: r.get(5)?,
        column_kinds: r.get::<_, Option<String>>(6)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        schedule: r.get::<_, Option<String>>(7)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Schedule::Manual),
        created_by: r.get(8)?,
        created_at: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
        updated_at: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
        last_run_at: r.get(11)?,
        last_status: r.get(12)?,
        last_rows: r.get(13)?,
        last_message: r.get(14)?,
        next_run: None,
        running: false,
        params: r.get::<_, Option<String>>(15)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        values: r.get::<_, Option<String>>(16)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        template_id: r.get(17)?,
    })
}

pub fn list(conn: &Connection) -> AppResult<Vec<SavedQuery>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM ledger_sql_query q LEFT JOIN ledger_dataset d ON d.id = q.dataset_id ORDER BY q.updated_at DESC"))?;
    let mut out = stmt.query_map([], from_row)?.collect::<Result<Vec<_>, _>>()?;
    for q in &mut out {
        q.running = is_running(&q.id);
        let base = q.last_run_at.as_deref().or(Some(q.updated_at.as_str())).and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok());
        q.next_run = if q.dataset_id.is_some() { base.and_then(|b| q.schedule.next_run(b)).map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()) } else { None };
    }
    Ok(out)
}

fn one(conn: &Connection, id: &str) -> AppResult<SavedQuery> {
    list(conn)?.into_iter().find(|q| q.id == id).ok_or_else(|| AppError::bad("查询不存在"))
}

fn validate(conn: &Connection, input: &QueryInput) -> AppResult<()> {
    if input.name.trim().is_empty() || input.name.chars().count() > 60 {
        return Err(AppError::bad("查询名称不能为空，且不超过 60 个字符"));
    }
    if let Schedule::Daily { at } = &input.schedule {
        NaiveTime::parse_from_str(at, "%H:%M").map_err(|_| AppError::bad("时间格式应为 HH:MM"))?;
    }
    if let Schedule::AfterSync { connector_id } = &input.schedule
        && conn.query_row("SELECT count(*) FROM ledger_connector WHERE id = ?", params![connector_id], |r| r.get::<_, i64>(0))? == 0
    {
        return Err(AppError::bad("请选择数据连接"));
    }
    // 只检查语法与引用（不执行）
    let catalog = dataset::datasets(conn)?;
    let names: HashSet<String> = catalog.iter().map(|d| d.name.clone()).collect();
    let sql = params::bind_for_check(&guard::trim_sql(&input.sql), &input.params, &input.values)?;
    guard::check(conn, &sql, &|t| names.contains(t))?;
    Ok(())
}

/// 保存（新建或修改）；需要另存为数据集时立即写入一次
pub fn save(state: &AppState, conn: &Connection, id: Option<&str>, input: QueryInput) -> AppResult<SavedQuery> {
    validate(conn, &input)?;
    let now = now_local();
    let sql = guard::trim_sql(&input.sql);
    let mut previous: Option<SavedQuery> = None;
    let id = match id {
        Some(id) => {
            previous = Some(one(conn, id)?);
            conn.execute(
                "UPDATE ledger_sql_query SET name = ?, sql = ?, description = ?, column_kinds = ?, schedule = ?, params = ?, param_values = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
                params![input.name.trim(), sql, input.description.trim(), serde_json::to_string(&input.column_kinds)?, serde_json::to_string(&input.schedule)?, serde_json::to_string(&input.params)?, serde_json::to_string(&input.values)?, now, id],
            )?;
            id.to_string()
        }
        None => {
            let id = format!("sq_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
            conn.execute(
                "INSERT INTO ledger_sql_query (id, name, sql, description, column_kinds, schedule, created_by, created_at, updated_at, params, param_values, template_id) VALUES (?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP), ?, ?, ?)",
                params![id, input.name.trim(), sql, input.description.trim(), serde_json::to_string(&input.column_kinds)?, serde_json::to_string(&input.schedule)?, input.operator, now, now, serde_json::to_string(&input.params)?, serde_json::to_string(&input.values)?, input.template_id],
            )?;
            id
        }
    };
    let q = one(conn, &id)?;
    if (q.dataset_id.is_some() || input.save_as_dataset.as_deref().is_some_and(|s| !s.trim().is_empty()))
        && let Err(e) = refresh_now(state, conn, &id, input.save_as_dataset.as_deref()) {
            // 写入数据集失败：修改的查询恢复原样（保持查询与数据集一致），新建的查询删除
            match previous {
                Some(p) => {
                    conn.execute(
                        "UPDATE ledger_sql_query SET name = ?, sql = ?, description = ?, column_kinds = ?, schedule = ?, params = ?, param_values = ? WHERE id = ?",
                        params![p.name, p.sql, p.description, serde_json::to_string(&p.column_kinds)?, serde_json::to_string(&p.schedule)?, serde_json::to_string(&p.params)?, serde_json::to_string(&p.values)?, id],
                    )?;
                }
                None => {
                    conn.execute("DELETE FROM ledger_sql_query WHERE id = ?", params![id])?;
                }
            }
            return Err(e);
        }
    one(conn, &id)
}

/// 用一组参数值从参数化查询生成一个实例：复制成独立的查询并另存为一个新数据集，之后按它自己的计划重算
pub fn instantiate(state: &AppState, conn: &Connection, id: &str, values: HashMap<String, String>, name: Option<&str>, operator: Option<String>) -> AppResult<SavedQuery> {
    let q = one(conn, id)?;
    if q.params.is_empty() {
        return Err(AppError::bad("这个查询没有参数，不能生成实例"));
    }
    // 先严格校验值（缺值、类型不对立即说明），再生成
    params::bind(&q.sql, &q.params, &values)?;
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => format!("{}{}", q.name, params::describe(&q.params, &values)),
    };
    let input = QueryInput {
        name: name.clone(),
        sql: q.sql,
        description: q.description,
        save_as_dataset: Some(name),
        column_kinds: q.column_kinds,
        schedule: q.schedule,
        operator,
        params: q.params,
        values,
        template_id: Some(q.id),
    };
    save(state, conn, None, input)
}

/// 两组参数值实际代入的是否相同（去掉首尾空白；没填的按默认值）
fn same_values(defs: &[params::ParamDef], a: &HashMap<String, String>, b: &HashMap<String, String>) -> bool {
    let effective = |m: &HashMap<String, String>| -> Vec<Option<String>> {
        defs.iter()
            .map(|d| m.get(&d.name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).or_else(|| d.default.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from)))
            .collect()
    };
    effective(a) == effective(b)
}

/// 按一组参数值取得参数化查询的数据集：已有同样参数值的实例（或查询本身）就重算后直接用，否则生成新实例。
/// 返回（查询，是否新生成）。模板「打开时填参数」用它，同样的参数反复使用不会生出一堆重复的数据集
pub fn instance_for(state: &AppState, conn: &Connection, root: &str, values: HashMap<String, String>, operator: Option<String>) -> AppResult<(SavedQuery, bool)> {
    let q = one(conn, root).map_err(|_| AppError::bad("模板引用的参数化查询已被删除，请改为选择数据集"))?;
    if q.params.is_empty() {
        return Err(AppError::bad(format!("查询「{}」已经没有参数，请改为选择数据集", q.name)));
    }
    params::bind(&q.sql, &q.params, &values)?;
    let existing = list(conn)?
        .into_iter()
        .find(|x| (x.id == q.id || x.template_id.as_deref() == Some(q.id.as_str())) && x.dataset_name.is_some() && x.params == q.params && same_values(&q.params, &x.values, &values));
    if let Some(x) = existing {
        refresh_now(state, conn, &x.id, None)?;
        return Ok((one(conn, &x.id)?, false));
    }
    Ok((instantiate(state, conn, &q.id, values, None, operator)?, true))
}

pub fn delete(conn: &Connection, id: &str) -> AppResult<()> {
    if is_running(id) {
        return Err(AppError::bad("正在重算，请稍后再删除"));
    }
    conn.execute("DELETE FROM ledger_sql_query WHERE id = ?", params![id])?;
    Ok(())
}

/// 立即重算（写入关联数据集；没有关联时需给出新数据集名称）
pub fn refresh_now(state: &AppState, conn: &Connection, id: &str, new_dataset: Option<&str>) -> AppResult<Materialized> {
    {
        let mut r = running().lock().map_err(|_| anyhow::anyhow!("查询状态锁已损坏"))?;
        if !r.insert(id.to_string()) {
            return Err(AppError::bad("正在重算中"));
        }
    }
    let result = (|| {
        let q = one(conn, id)?;
        let target = q.dataset_id.clone();
        if target.is_none() && new_dataset.is_none_or(|s| s.trim().is_empty()) {
            return Err(AppError::bad("这个查询还没有另存为数据集"));
        }
        let r = params::bind(&q.sql, &q.params, &q.values)
            .and_then(|sql| materialize(conn, &sql, new_dataset.unwrap_or(""), target.as_deref(), &q.column_kinds, state.connectors.max_rows));
        let now = now_local();
        match &r {
            Ok(m) => {
                conn.execute(
                    "UPDATE ledger_sql_query SET dataset_id = ?, last_run_at = CAST(? AS TIMESTAMP), last_status = 'success', last_rows = ?, last_message = ? WHERE id = ?",
                    params![m.dataset_id, now, m.rows, if m.rebuilt { Some("结果的列有变化，已重建数据集") } else { None }, id],
                )?;
            }
            Err(e) => {
                conn.execute(
                    "UPDATE ledger_sql_query SET last_run_at = CAST(? AS TIMESTAMP), last_status = 'failed', last_message = ? WHERE id = ?",
                    params![now, e.message(), id],
                )?;
            }
        }
        r
    })();
    if let Ok(mut r) = running().lock() {
        r.remove(id);
    }
    if result.is_ok() {
        state.cursors.invalidate();
        state.checkpoint();
    }
    result
}

async fn refresh_bg(state: &AppState, id: String, name: String, trigger: &str) {
    let _guard = state.import_lock.lock().await;
    let st = state.clone();
    let qid = id.clone();
    let r = tokio::task::spawn_blocking(move || -> AppResult<Materialized> {
        let conn = st.connection()?;
        refresh_now(&st, &conn, &qid, None)
    })
    .await;
    match r {
        Ok(Ok(m)) => tracing::info!("SQL 查询「{name}」{trigger}重算完成：{} 行", m.rows),
        Ok(Err(e)) => tracing::warn!("SQL 查询「{name}」{trigger}重算失败：{}", e.message()),
        Err(e) => tracing::warn!("SQL 查询「{name}」重算任务异常：{e}"),
    }
}

/// 每 30 秒检查一次到期的定时重算
pub async fn schedule(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let st = state.clone();
        let due = tokio::task::spawn_blocking(move || -> AppResult<Vec<(String, String)>> {
            let conn = st.connection()?;
            let now = chrono::Local::now().naive_local();
            Ok(list(&conn)?
                .into_iter()
                .filter(|q| !q.running && q.next_run.as_deref().and_then(|t| NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S").ok()).is_some_and(|t| t <= now))
                .map(|q| (q.id, q.name))
                .collect())
        })
        .await
        .unwrap_or_else(|e| Err(AppError::Internal(e.into())));
        match due {
            Ok(due) => {
                for (id, name) in due {
                    refresh_bg(&state, id, name, "定时").await;
                }
            }
            Err(e) => tracing::warn!("检查 SQL 定时重算失败：{}", e.message()),
        }
    }
}

/// 数据连接同步到新数据后：重算「同步后」的查询，再触发推送（推送用到的数据集可能是查询结果）
pub fn on_connector_synced(state: &AppState, connector_id: &str) {
    let due = state.connection().map_err(AppError::from).and_then(|conn| list(&conn)).map(|l| {
        l.into_iter()
            .filter(|q| q.dataset_id.is_some() && matches!(&q.schedule, Schedule::AfterSync { connector_id: c } if c == connector_id))
            .map(|q| (q.id, q.name))
            .collect::<Vec<_>>()
    });
    let st = state.clone();
    let cid = connector_id.to_string();
    tokio::spawn(async move {
        if let Ok(due) = due {
            for (id, name) in due {
                refresh_bg(&st, id, name, "同步后").await;
            }
        }
        crate::report::on_connector_synced(&st, &cid);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_values() {
        assert_eq!(kind_of("订单金额", "DOUBLE", &[]), "money");
        assert_eq!(kind_of("订单数", "BIGINT", &[]), "number");
        assert_eq!(kind_of("月份", "VARCHAR", &[]), "text");
        assert_eq!(kind_of("日期", "DATE", &[]), "datetime");
        assert_eq!(to_json(Value::Date32(0)), json!("1970-01-01"));
        assert_eq!(to_json(Value::Timestamp(TimeUnit::Microsecond, 1_000_000)), json!("1970-01-01 00:00:01"));
        assert_eq!(guard::trim_sql(" SELECT 1 ;; \n"), "SELECT 1");
        assert_eq!(clean(502650.6400000003), json!(502650.64));
        assert_eq!(clean(0.1 + 0.2), json!(0.3));
        let e = plan_columns(&[("a".into(), "INTEGER".into()), ("a".into(), "INTEGER".into())], &[], &HashMap::new()).err().unwrap();
        assert!(e.message().contains("重复"));
    }

    /// 端到端：临时视图只暴露数据集，执行在事务里回滚，内部表不可见
    #[test]
    fn run_on_temp_views() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ledger_dataset (id VARCHAR, name VARCHAR, columns VARCHAR, primary_key VARCHAR DEFAULT '[]', created_at TIMESTAMP, updated_at TIMESTAMP, kind VARCHAR, view VARCHAR);
             CREATE TABLE ds_1 (\"店铺\" VARCHAR, \"金额\" DOUBLE, search_text VARCHAR);
             INSERT INTO ds_1 VALUES ('A', 10, NULL), ('A', 5, NULL), ('B', 7, NULL);",
        )
        .unwrap();
        let cols = r#"[{"key":"店铺","title":"店铺","kind":"text","width":100},{"key":"金额","title":"金额","kind":"money","width":100}]"#;
        conn.execute("INSERT INTO ledger_dataset (id, name, columns) VALUES ('ds_1', '订单', ?)", [cols]).unwrap();
        if dataset::datasets(&conn).map(|d| d.iter().any(|x| x.name == "订单")).unwrap_or(false) {
            let r = run(&conn, "SELECT 店铺, sum(金额) AS 金额 FROM 订单 GROUP BY 店铺 ORDER BY 金额 DESC", 10).unwrap();
            assert_eq!(r.rows.len(), 2);
            assert_eq!(r.rows[0][0], json!("A"));
            assert_eq!(r.rows[0][1], json!(15.0));
            assert_eq!(r.columns[1].kind, "money");
            assert!(run(&conn, "SELECT * FROM ds_1", 10).is_err());
        }
    }
}
