//! 外部数据集：直接查询服务器上的 Parquet 文件，不导入
//!
//! - 文件必须在 LEDGER_INBOX_ROOTS 之内（与文件夹连接器同一组允许目录；规范化路径后判断，软链接指到外面的文件不算）
//! - 数据集是一个 DuckDB 视图：`read_parquet(文件列表)` 加三个隐藏列——`__fi`（文件序号）、`__fr`（文件内行号）、
//!   `__rid`（全局行号，关联视图等通用路径用）
//! - 文件列表和每个文件的行数在创建 / 刷新时确定并记在定义里（不读文件就能列出数据集、给出行数）。浏览时把行位置换算成
//!   （文件序号, 行号）的范围，筛选 / 排序后的位置表也存（文件序号, 行号）：条件能下推到 Parquet 扫描，只读要显示的那一屏
//!   （1000 万行上取一屏约 5 ~ 15 毫秒；按计算出来的全局行号取要扫全表，约 150 毫秒，随数据量线性增长）
//! - 列类型按 Parquet 自带的类型；文本列再按样本推断（像 CSV 转出来的 Parquet，日期、编号常是文本）。
//!   日期 / 时间戳在视图里换成「2026-09-15 10:00:00」形式的文本，与导入的数据集约定一致（透视按月分组、切片器照常用）
//! - 只读：不能导入、填报；可以加计算列、建关联视图、写 SQL、做透视。文件有增减或结构变化后，管理员点「刷新文件列表」，
//!   或设置「自动检查」：每隔 N 分钟比对一次文件列表、大小和修改时间（只看文件系统，不读 Parquet），有变化才重新读取（[`schedule`]）

use std::path::{Path, PathBuf};

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::dataset::{self, ColumnDef, DatasetDef, StoredColumn};
use crate::error::{AppError, AppResult};
use crate::kinds;
use crate::schema::{ident, literal};

/// 一个外部数据集最多引用的文件数（视图里是显式的文件列表）
pub const MAX_FILES: usize = 5000;
/// 预览返回的样例行数
const PREVIEW_ROWS: usize = 20;
/// 文本列按这么多行样本推断类型
const SAMPLE_ROWS: usize = 2000;
/// 与隐藏列、内部列重名的列自动改名
const RESERVED: &[&str] = &["search_text", "rowid", "__fi", "__fr", "__rid"];

/// 创建 / 预览的输入
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExternalInput {
    /// 目录（绝对路径，或相对于第一个允许目录）
    pub path: String,
    /// 文件名通配符，多个用 ; 分隔；默认 *.parquet
    #[serde(default = "default_pattern")]
    pub pattern: String,
    /// 包含子目录（按 年=2026/月=09/ 这样分区存放的数据）
    #[serde(default)]
    pub recursive: bool,
    /// 把 key=value 形式的目录名读成列（Hive 分区）
    #[serde(default)]
    pub hive: bool,
    /// 自动检查文件变化的间隔（分钟，1 ~ 1440）；不设表示只手动刷新
    #[serde(default)]
    pub refresh_minutes: Option<u32>,
}

impl ExternalInput {
    /// 读哪些文件、怎么读（不含自动检查的设置）是否相同
    fn same_source(&self, other: &ExternalInput) -> bool {
        (self.path.trim(), self.pattern.trim(), self.recursive, self.hive) == (other.path.trim(), other.pattern.trim(), other.recursive, other.hive)
    }
}

/// 自动检查间隔的上限（分钟）
const MAX_REFRESH_MINUTES: u32 = 1440;

fn default_pattern() -> String {
    "*.parquet".into()
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalFile {
    pub path: String,
    pub rows: i64,
    pub size: u64,
    /// 修改时间（Unix 秒；旧版定义里没有，为 0）
    #[serde(default)]
    pub modified: i64,
}

/// 保存在登记表 definition 里的定义
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExternalDef {
    #[serde(flatten)]
    pub input: ExternalInput,
    pub files: Vec<ExternalFile>,
    pub refreshed_at: String,
    /// 最近一次自动检查的时间与失败原因（目录不见了、刷新会让计算列失效……）
    #[serde(default)]
    pub checked_at: Option<String>,
    #[serde(default)]
    pub check_error: Option<String>,
}

/// 给前端看的摘要（数据集定义里的 external 字段）；offsets 是每个文件第一行的全局位置，浏览时换算用
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExternalInfo {
    pub path: String,
    pub pattern: String,
    pub recursive: bool,
    pub hive: bool,
    pub file_count: usize,
    pub rows: i64,
    pub size: u64,
    pub refreshed_at: String,
    pub refresh_minutes: Option<u32>,
    pub checked_at: Option<String>,
    pub check_error: Option<String>,
    #[serde(skip)]
    pub offsets: Vec<i64>,
}

impl ExternalDef {
    pub fn info(&self) -> ExternalInfo {
        let mut offsets = Vec::with_capacity(self.files.len());
        let mut total = 0;
        for f in &self.files {
            offsets.push(total);
            total += f.rows;
        }
        ExternalInfo {
            path: self.input.path.clone(),
            pattern: self.input.pattern.clone(),
            recursive: self.input.recursive,
            hive: self.input.hive,
            file_count: self.files.len(),
            rows: total,
            size: self.files.iter().map(|f| f.size).sum(),
            refreshed_at: self.refreshed_at.clone(),
            refresh_minutes: self.input.refresh_minutes,
            checked_at: self.checked_at.clone(),
            check_error: self.check_error.clone(),
            offsets,
        }
    }
}

impl ExternalInfo {
    /// 全局行位置区间 [from, to) → 按（文件序号, 行号）的条件（别名 t）；区间为空时为 FALSE
    pub fn range_sql(&self, from: i64, to: i64) -> String {
        let total = self.rows;
        let (from, to) = (from.max(0), to.min(total));
        let mut parts = Vec::new();
        for (i, &start) in self.offsets.iter().enumerate() {
            let end = self.offsets.get(i + 1).copied().unwrap_or(total);
            let (a, b) = (from.max(start), to.min(end));
            if a < b {
                parts.push(format!("(t.__fi = {i} AND t.__fr >= {} AND t.__fr < {})", a - start, b - start));
            }
        }
        if parts.is_empty() { "FALSE".into() } else { format!("({})", parts.join(" OR ")) }
    }
}

// ── 找文件 ────────────────────────────────────────────

fn patterns(input: &ExternalInput) -> AppResult<Vec<String>> {
    let p: Vec<String> = input.pattern.split([';', '；', ',']).map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect();
    if p.is_empty() {
        return Err(AppError::bad("请填写文件名规则，如 *.parquet"));
    }
    Ok(p)
}

/// 匹配的 Parquet 文件（按路径排序，文件序号就是这个顺序）
pub fn find_files(roots: &[PathBuf], input: &ExternalInput) -> AppResult<Vec<PathBuf>> {
    let dir = crate::connector::folder::resolve_dir(roots, &input.path)?;
    let pats = patterns(input)?;
    let allowed: Vec<PathBuf> = roots.iter().filter_map(|r| r.canonicalize().ok()).collect();
    let mut out = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() {
                if input.recursive {
                    stack.push(e.path());
                }
                continue;
            }
            if !name.to_lowercase().ends_with(".parquet") || !pats.iter().any(|p| crate::connector::folder::wildcard(p, &name)) {
                continue;
            }
            // 软链接可能指到允许目录之外
            let Ok(real) = e.path().canonicalize() else { continue };
            if !real.is_file() || !allowed.iter().any(|r| real.starts_with(r)) {
                continue;
            }
            out.push(real);
            if out.len() > MAX_FILES {
                return Err(AppError::bad(format!("匹配的文件超过 {MAX_FILES} 个，请缩小目录或文件名规则")));
            }
        }
    }
    if out.is_empty() {
        return Err(AppError::bad("没有匹配的 Parquet 文件"));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn file_list(files: &[String]) -> String {
    format!("[{}]", files.iter().map(|f| literal(f)).collect::<Vec<_>>().join(", "))
}

fn source_sql(files: &[String], hive: bool) -> String {
    format!("read_parquet({}, union_by_name = true, hive_partitioning = {hive})", file_list(files))
}

/// 文件的大小与修改时间（Unix 秒）；读不到时为 0
fn fs_stamp(p: &Path) -> (u64, i64) {
    std::fs::metadata(p).map_or((0, 0), |m| {
        let modified = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
        (m.len(), modified)
    })
}

/// 文件系统上的文件和定义里记的是否一样（文件列表、大小、修改时间）。只看文件系统，不读 Parquet，所以可以频繁调用
pub fn files_changed(roots: &[PathBuf], def: &ExternalDef) -> AppResult<bool> {
    let files = find_files(roots, &def.input)?;
    if files.len() != def.files.len() {
        return Ok(true);
    }
    Ok(files.iter().zip(&def.files).any(|(p, f)| {
        let (size, modified) = fs_stamp(p);
        p.display().to_string() != f.path || size != f.size || (f.modified != 0 && modified != f.modified)
    }))
}

/// 两份文件清单是否相同；旧版定义没记修改时间（0）的不比较修改时间，免得升级后第一次检查就当成「有变化」
fn same_files(old: &[ExternalFile], new: &[ExternalFile]) -> bool {
    old.len() == new.len() && old.iter().zip(new).all(|(a, b)| a.path == b.path && a.rows == b.rows && a.size == b.size && (a.modified == 0 || a.modified == b.modified))
}

/// 每个文件的行数与大小（只读 Parquet 文件尾的元数据）
fn file_stats(conn: &Connection, files: &[PathBuf]) -> AppResult<Vec<ExternalFile>> {
    let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let mut stmt = conn.prepare(&format!("SELECT file_name, sum(num_rows)::BIGINT FROM parquet_file_metadata({}) GROUP BY file_name", file_list(&names))).map_err(read_err)?;
    let rows: std::collections::HashMap<String, i64> = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<_, _>>()?;
    names
        .iter()
        .zip(files)
        .map(|(n, p)| {
            let rows = rows.get(n).copied().ok_or_else(|| AppError::bad(format!("读不出文件的行数：{n}")))?;
            let (size, modified) = fs_stamp(p);
            Ok(ExternalFile { path: n.clone(), rows, size, modified })
        })
        .collect()
}

fn read_err(e: duckdb::Error) -> AppError {
    AppError::bad(format!("读取 Parquet 文件失败：{e}"))
}

// ── 列 ────────────────────────────────────────────────

/// 一列在视图里的写法与登记的列
#[derive(Clone)]
struct PlannedColumn {
    stored: StoredColumn,
    expr: String,
    sql_type: String,
}

fn title_of(name: &str, seen: &mut std::collections::HashSet<String>) -> String {
    let mut t: String = name.trim().chars().filter(|c| !c.is_control() && *c != '"').collect();
    if t.is_empty() {
        t = "列".into();
    }
    if RESERVED.contains(&t.to_lowercase().as_str()) {
        t = format!("{t}_列");
    }
    let base = t.clone();
    let mut n = 1;
    while !seen.insert(t.clone()) {
        n += 1;
        t = format!("{base}_{n}");
    }
    t
}

/// 按 Parquet 的列类型（文本列再看样本）决定列类型与视图里的表达式
fn plan_columns(conn: &Connection, source: &str) -> AppResult<Vec<PlannedColumn>> {
    let mut stmt = conn.prepare(&format!("DESCRIBE SELECT * FROM {source}")).map_err(read_err)?;
    let desc: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<_, _>>()?;
    if desc.is_empty() {
        return Err(AppError::bad("文件里没有列"));
    }
    // 文本列的样本：一次查询取全部文本列的前若干行
    let text_cols: Vec<&String> = desc.iter().filter(|(_, t)| t == "VARCHAR").map(|(n, _)| n).collect();
    let samples: Vec<Vec<String>> = if text_cols.is_empty() {
        vec![]
    } else {
        let sel = text_cols.iter().map(|n| format!("COALESCE({}, '')", ident(n))).collect::<Vec<_>>().join(", ");
        let mut st = conn.prepare(&format!("SELECT {sel} FROM {source} LIMIT {SAMPLE_ROWS}")).map_err(read_err)?;
        let n = text_cols.len();
        let rows: Vec<Vec<String>> = st.query_map([], |r| (0..n).map(|i| r.get::<_, String>(i)).collect())?.collect::<Result<_, _>>()?;
        (0..n).map(|i| rows.iter().map(|r| r[i].clone()).collect()).collect()
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (name, ty) in &desc {
        let title = title_of(name, &mut seen);
        let c = ident(name);
        let t = ty.to_uppercase();
        let (kind, expr): (&'static str, String) = if t == "VARCHAR" {
            let i = text_cols.iter().position(|x| *x == name).unwrap_or(0);
            let values: Vec<&str> = samples.get(i).map(|v| v.iter().map(String::as_str).collect()).unwrap_or_default();
            let k = kinds::normalize(&crate::dataset_import::infer_kind(&title, &values));
            (k, if kinds::is_textual(k) { c.clone() } else { crate::dataset_import::convert_sql(k, &c) })
        } else {
            match crate::import_source::kind_for_type(name, &t) {
                Some("datetime") if t == "DATE" => ("datetime", format!("strftime({c}, '%Y-%m-%d')")),
                Some("datetime") => ("datetime", format!("strftime({c}, '%Y-%m-%d %H:%M:%S')")),
                Some(k @ ("money" | "number")) => (k, format!("({c})::DOUBLE")),
                Some("integer") if matches!(t.as_str(), "HUGEINT" | "UBIGINT") => ("integer", format!("TRY_CAST({c} AS BIGINT)")),
                Some(k) => (k, c.clone()),
                // 列表、结构体、二进制、时间段等：按文本显示
                None => ("text", format!("CAST({c} AS VARCHAR)")),
            }
        };
        out.push(PlannedColumn {
            stored: StoredColumn { key: title.clone(), title, kind: kind.into(), width: kinds::default_width(kind), source: name.clone() },
            expr,
            sql_type: ty.clone(),
        });
    }
    Ok(out)
}

/// 视图 SQL
fn view_sql(def: &ExternalDef, cols: &[PlannedColumn]) -> String {
    let files: Vec<String> = def.files.iter().map(|f| f.path.clone()).collect();
    let info = def.info();
    let offsets = format!("[{}]::BIGINT[]", info.offsets.iter().map(|o| o.to_string()).collect::<Vec<_>>().join(", "));
    let select = cols.iter().map(|c| format!("{} AS {}", c.expr, ident(&c.stored.title))).collect::<Vec<_>>().join(", ");
    format!(
        "SELECT {select}, file_index::INTEGER AS __fi, file_row_number AS __fr, ({offsets})[file_index::BIGINT + 1] + file_row_number AS __rid FROM {}",
        source_sql(&files, def.input.hive)
    )
}

/// 文件 + 列：预览、创建、刷新共用
fn inspect(conn: &Connection, roots: &[PathBuf], input: &ExternalInput) -> AppResult<(ExternalDef, Vec<PlannedColumn>)> {
    if let Some(m) = input.refresh_minutes
        && !(1..=MAX_REFRESH_MINUTES).contains(&m)
    {
        return Err(AppError::bad(format!("自动检查的间隔应在 1 ~ {MAX_REFRESH_MINUTES} 分钟之间")));
    }
    let files = find_files(roots, input)?;
    let stats = file_stats(conn, &files)?;
    let names: Vec<String> = stats.iter().map(|f| f.path.clone()).collect();
    let cols = plan_columns(conn, &source_sql(&names, input.hive))?;
    let now = crate::db::now_local();
    Ok((ExternalDef { input: input.clone(), files: stats, refreshed_at: now.clone(), checked_at: Some(now), check_error: None }, cols))
}

// ── 接口 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewColumn {
    pub title: String,
    pub source: String,
    pub kind: String,
    pub sql_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPreview {
    pub file_count: usize,
    /// 前几个文件（相对于目录的路径）
    pub files: Vec<String>,
    pub rows: i64,
    pub size: u64,
    pub columns: Vec<PreviewColumn>,
    pub sample_rows: Vec<Vec<Json>>,
}

pub fn preview(conn: &Connection, roots: &[PathBuf], input: &ExternalInput) -> AppResult<ExternalPreview> {
    let (def, cols) = inspect(conn, roots, input)?;
    let sql = view_sql(&def, &cols);
    let titles = cols.iter().map(|c| ident(&c.stored.title)).collect::<Vec<_>>().join(", ");
    let mut stmt = conn.prepare(&format!("SELECT {titles} FROM ({sql}) WHERE __fi = 0 AND __fr < {PREVIEW_ROWS}")).map_err(read_err)?;
    let n = cols.len();
    let sample_rows = stmt.query_map([], |r| (0..n).map(|i| Ok(crate::tabular::to_json(r.get_ref(i)?))).collect::<duckdb::Result<Vec<_>>>())?.collect::<Result<Vec<_>, _>>()?;
    let info = def.info();
    let dir = crate::connector::folder::resolve_dir(roots, &input.path)?;
    Ok(ExternalPreview {
        file_count: info.file_count,
        files: def.files.iter().take(5).map(|f| Path::new(&f.path).strip_prefix(&dir).map(|p| p.display().to_string()).unwrap_or_else(|_| f.path.clone())).collect(),
        rows: info.rows,
        size: info.size,
        columns: cols.into_iter().map(|c| PreviewColumn { title: c.stored.title, source: c.stored.source, kind: c.stored.kind, sql_type: c.sql_type }).collect(),
        sample_rows,
    })
}

/// 新建外部数据集，返回 id
pub fn create(conn: &Connection, roots: &[PathBuf], name: &str, input: &ExternalInput) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::bad("数据集名称不能为空，且不超过 40 个字符"));
    }
    if dataset::datasets(conn)?.iter().any(|d| d.name == name) {
        return Err(AppError::bad(format!("已存在名为「{name}」的数据集")));
    }
    let (def, cols) = inspect(conn, roots, input)?;
    let id = format!("dsx_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    let stored: Vec<StoredColumn> = cols.iter().map(|c| c.stored.clone()).collect();
    let now = crate::db::now_local();
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        conn.execute_batch(&format!("CREATE VIEW {id} AS {}", view_sql(&def, &cols)))?;
        conn.execute(
            "INSERT INTO ledger_dataset (id, name, columns, primary_key, kind, definition, created_at, updated_at)
             VALUES (?, ?, ?, '[]', 'external', ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
            params![id, name, serde_json::to_string(&stored)?, serde_json::to_string(&def)?, now, now],
        )?;
        Ok(())
    })();
    match r {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(id)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RefreshOutcome {
    pub file_count: usize,
    pub rows: i64,
    /// 新出现的文件数 / 不见了的文件数
    pub added_files: usize,
    pub removed_files: usize,
    pub added_columns: Vec<String>,
    pub removed_columns: Vec<String>,
    /// 文件、列或读取方式有没有变化（没有变化时视图不动）
    pub changed: bool,
}

fn load(conn: &Connection, id: &str) -> AppResult<(String, ExternalDef, Vec<StoredColumn>)> {
    let (name, d, c): (String, Option<String>, String) = conn
        .query_row("SELECT name, definition, columns FROM ledger_dataset WHERE id = ? AND kind = 'external' AND deleted_at IS NULL", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|_| AppError::bad("外部数据集不存在"))?;
    let def = d.and_then(|d| serde_json::from_str(&d).ok()).ok_or_else(|| AppError::bad("外部数据集的定义损坏"))?;
    Ok((name, def, serde_json::from_str(&c).unwrap_or_default()))
}

/// 只改定义（自动检查的时间 / 结果、检查间隔），不动视图和列
fn save_def(conn: &Connection, id: &str, def: &ExternalDef) -> AppResult<()> {
    conn.execute("UPDATE ledger_dataset SET definition = ? WHERE id = ?", params![serde_json::to_string(def)?, id])?;
    Ok(())
}

/// 按 input 重新找文件、读结构；有变化时重建视图（会让计算列、关联视图失效时整体取消），没变化时只记下检查时间
fn rebuild(conn: &Connection, roots: &[PathBuf], id: &str, input: &ExternalInput) -> AppResult<RefreshOutcome> {
    let (_, old, old_cols) = load(conn, id)?;
    let (def, cols) = inspect(conn, roots, input)?;
    let stored: Vec<StoredColumn> = cols.iter().map(|c| c.stored.clone()).collect();
    let has = |list: &[ExternalFile], p: &str| list.iter().any(|f| f.path == p);
    let shape = |c: &[StoredColumn]| c.iter().map(|c| (c.title.clone(), c.kind.clone())).collect::<Vec<_>>();
    let changed = !old.input.same_source(input) || !same_files(&old.files, &def.files) || shape(&old_cols) != shape(&stored);
    let outcome = RefreshOutcome {
        file_count: def.files.len(),
        rows: def.info().rows,
        added_files: def.files.iter().filter(|f| !has(&old.files, &f.path)).count(),
        removed_files: old.files.iter().filter(|f| !has(&def.files, &f.path)).count(),
        added_columns: stored.iter().filter(|c| !old_cols.iter().any(|o| o.title == c.title)).map(|c| c.title.clone()).collect(),
        removed_columns: old_cols.iter().filter(|o| !stored.iter().any(|c| c.title == o.title)).map(|c| c.title.clone()).collect(),
        changed,
    };
    if !changed {
        // 记下新的文件信息（补上旧版定义没有的修改时间）和检查时间，视图不动
        save_def(conn, id, &ExternalDef { input: input.clone(), files: def.files, checked_at: def.checked_at, check_error: None, ..old })?;
        return Ok(outcome);
    }
    let sql = view_sql(&def, &cols);
    let (stored_json, def_json) = (serde_json::to_string(&stored)?, serde_json::to_string(&def)?);
    dataset::guarded_change(conn, |conn| {
        conn.execute_batch(&format!("CREATE OR REPLACE VIEW {id} AS {sql}"))?;
        conn.execute(
            "UPDATE ledger_dataset SET columns = ?, definition = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params![stored_json, def_json, crate::db::now_local(), id],
        )?;
        Ok(())
    })?;
    Ok(outcome)
}

/// 手动刷新：重新找文件、读结构并重建视图；会让计算列、关联视图失效时整体取消
pub fn refresh(conn: &Connection, roots: &[PathBuf], id: &str) -> AppResult<RefreshOutcome> {
    let (_, old, _) = load(conn, id)?;
    rebuild(conn, roots, id, &old.input)
}

/// 修改目录、文件名规则、子目录 / 分区选项或自动检查间隔。读取方式变了就按新规则重新读取（同样受计算列 / 关联视图保护）
pub fn update(conn: &Connection, roots: &[PathBuf], id: &str, input: &ExternalInput) -> AppResult<RefreshOutcome> {
    let (_, old, _) = load(conn, id)?;
    if old.input.same_source(input) {
        if let Some(m) = input.refresh_minutes
            && !(1..=MAX_REFRESH_MINUTES).contains(&m)
        {
            return Err(AppError::bad(format!("自动检查的间隔应在 1 ~ {MAX_REFRESH_MINUTES} 分钟之间")));
        }
        let info = old.info();
        let outcome = RefreshOutcome { file_count: info.file_count, rows: info.rows, added_files: 0, removed_files: 0, added_columns: vec![], removed_columns: vec![], changed: false };
        // 从手动改成自动时从现在起算，不要一保存就立即检查
        let checked_at = if old.input.refresh_minutes.is_none() { Some(crate::db::now_local()) } else { old.checked_at.clone() };
        save_def(conn, id, &ExternalDef { input: input.clone(), checked_at, ..old })?;
        return Ok(outcome);
    }
    rebuild(conn, roots, id, input)
}

/// 自动检查一个外部数据集：文件没变只记下检查时间；变了就重新读取。返回（名称，有变化时的结果）；
/// 失败（目录不见了、刷新会让计算列失效……）记在定义里，界面上能看到，下次到点再试
pub fn auto_check(conn: &Connection, roots: &[PathBuf], id: &str) -> AppResult<(String, Option<RefreshOutcome>)> {
    let (name, mut def, _) = load(conn, id)?;
    let result = files_changed(roots, &def).and_then(|changed| if changed { rebuild(conn, roots, id, &def.input.clone()).map(Some) } else { Ok(None) });
    match result {
        Ok(Some(o)) if o.changed => Ok((name, Some(o))),
        Ok(_) => {
            let (_, mut latest, _) = load(conn, id)?;
            latest.checked_at = Some(crate::db::now_local());
            latest.check_error = None;
            save_def(conn, id, &latest)?;
            Ok((name, None))
        }
        Err(e) => {
            def.checked_at = Some(crate::db::now_local());
            def.check_error = Some(e.message());
            save_def(conn, id, &def)?;
            Err(e)
        }
    }
}

/// 到点该自动检查的外部数据集（id）
fn due(conn: &Connection, now: chrono::NaiveDateTime) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id, definition FROM ledger_dataset WHERE kind = 'external' AND deleted_at IS NULL")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, d)| {
            let def: ExternalDef = serde_json::from_str(&d?).ok()?;
            let every = chrono::Duration::minutes(i64::from(def.input.refresh_minutes?));
            let last = def.checked_at.as_deref().and_then(|t| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S").ok());
            last.is_none_or(|t| t + every <= now).then_some(id)
        })
        .collect())
}

/// 后台任务：每 20 秒看一次哪些外部数据集到了自动检查的时间。检查与刷新和导入串行（import_lock）
pub async fn schedule(state: crate::routes::AppState) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(20));
    loop {
        tick.tick().await;
        let st = state.clone();
        let ids = match tokio::task::spawn_blocking(move || -> AppResult<Vec<String>> { due(&st.connection()?, chrono::Local::now().naive_local()) }).await {
            Ok(Ok(ids)) => ids,
            Ok(Err(e)) => {
                tracing::warn!("检查外部数据集的自动刷新计划失败：{}", e.message());
                continue;
            }
            Err(e) => {
                tracing::warn!("检查外部数据集的自动刷新计划失败：{e}");
                continue;
            }
        };
        for id in ids {
            let _guard = state.import_lock.lock().await;
            let (st, roots, did) = (state.clone(), state.config.inbox_roots.clone(), id.clone());
            let r = tokio::task::spawn_blocking(move || -> AppResult<_> { auto_check(&st.connection()?, &roots, &did) }).await;
            match r {
                Ok(Ok((name, Some(o)))) => {
                    tracing::info!("外部数据集「{name}」文件有变化，已自动刷新：{} 个文件、{} 行", o.file_count, o.rows);
                    state.cursors.invalidate();
                    state.checkpoint();
                    crate::webhook::emit(&state, "dataset.refreshed", None, refreshed_event(&id, &name, "auto", &o));
                    crate::sqlq::on_data_changed(&state, &id);
                }
                Ok(Ok((_, None))) => {}
                Ok(Err(e)) => tracing::warn!("外部数据集 {id} 自动检查失败：{}", e.message()),
                Err(e) => tracing::warn!("外部数据集 {id} 自动检查任务异常：{e}"),
            }
        }
    }
}

/// `dataset.refreshed` 事件的内容；trigger：manual 手动刷新 / auto 自动检查 / edit 修改了读取方式
pub fn refreshed_event(id: &str, name: &str, trigger: &str, o: &RefreshOutcome) -> Json {
    serde_json::json!({
        "datasetId": id, "name": name, "trigger": trigger, "fileCount": o.file_count, "rows": o.rows,
        "addedFiles": o.added_files, "removedFiles": o.removed_files, "addedColumns": o.added_columns, "removedColumns": o.removed_columns,
    })
}

/// 登记表里的一行 → 数据集定义（不读文件）
pub fn dataset_def(id: String, name: String, cols: &str, definition: Option<&str>) -> Option<DatasetDef> {
    let def: ExternalDef = serde_json::from_str(definition?).ok()?;
    let cols: Vec<StoredColumn> = serde_json::from_str(cols).ok()?;
    Some(DatasetDef {
        table: id.clone(),
        id,
        name,
        columns: cols
            .into_iter()
            .map(|c| ColumnDef { key: c.key, title: c.title, kind: kinds::normalize(&c.kind), width: c.width, expr: None, computed: false, formula: None, origin: None })
            .collect(),
        // 没有 search_text 列：关键字搜索请用列筛选
        searchable: false,
        supports_rule: false,
        builtin: false,
        primary_key: vec![],
        rid: "__rid".into(),
        kind: "external".into(),
        scope_sig: String::new(),
        view: None,
        summary: None,
        external: Some(def.info()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        conn
    }

    fn write(conn: &Connection, path: &Path, sql: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        conn.execute_batch(&format!("COPY ({sql}) TO {} (FORMAT parquet)", literal(&path.to_string_lossy()))).unwrap();
    }

    fn input(path: &str) -> ExternalInput {
        ExternalInput { path: path.into(), pattern: "*.parquet".into(), recursive: false, hive: false, refresh_minutes: None }
    }

    #[test]
    fn range_maps_positions_to_files() {
        let def = ExternalDef {
            input: input("x"),
            files: vec![ExternalFile { path: "a".into(), rows: 10, size: 0, modified: 0 }, ExternalFile { path: "b".into(), rows: 5, size: 0, modified: 0 }],
            refreshed_at: String::new(),
            checked_at: None,
            check_error: None,
        };
        let info = def.info();
        assert_eq!(info.offsets, vec![0, 10]);
        assert_eq!(info.range_sql(2, 4), "((t.__fi = 0 AND t.__fr >= 2 AND t.__fr < 4))");
        assert_eq!(info.range_sql(8, 12), "((t.__fi = 0 AND t.__fr >= 8 AND t.__fr < 10) OR (t.__fi = 1 AND t.__fr >= 0 AND t.__fr < 2))");
        assert_eq!(info.range_sql(15, 20), "FALSE");
    }

    #[test]
    fn files_must_stay_inside_roots() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let conn = setup();
        write(&conn, &root.path().join("订单/a.parquet"), "SELECT 1 AS x");
        write(&conn, &root.path().join("订单/2026/b.parquet"), "SELECT 2 AS x");
        write(&conn, &outside.path().join("c.parquet"), "SELECT 3 AS x");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("c.parquet"), root.path().join("订单/link.parquet")).unwrap();
        std::fs::write(root.path().join("订单/note.csv"), "x\n1\n").unwrap();
        let roots = vec![root.path().to_path_buf()];
        let names = |i: &ExternalInput| find_files(&roots, i).unwrap().iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect::<Vec<_>>();
        assert_eq!(names(&input("订单")), vec!["a.parquet"]);
        assert_eq!(names(&ExternalInput { recursive: true, ..input("订单") }), vec!["b.parquet", "a.parquet"]);
        assert!(find_files(&roots, &input(&outside.path().display().to_string())).is_err());
        assert!(find_files(&roots, &input("../")).is_err());
        assert!(find_files(&roots, &ExternalInput { pattern: "z*.parquet".into(), ..input("订单") }).is_err());
    }

    #[test]
    fn auto_check_only_rebuilds_on_change() {
        let root = tempfile::tempdir().unwrap();
        let conn = setup();
        let dir = root.path().join("日志");
        write(&conn, &dir.join("d1.parquet"), "SELECT i AS n FROM range(10) t(i)");
        let roots = vec![root.path().to_path_buf()];
        let id = create(&conn, &roots, "日志", &input("日志")).unwrap();
        let def = || load(&conn, &id).unwrap().1;

        // 文件没变：检查不重建
        assert!(!files_changed(&roots, &def()).unwrap());
        let (name, o) = auto_check(&conn, &roots, &id).unwrap();
        assert_eq!((name.as_str(), o.is_none()), ("日志", true));
        let o = refresh(&conn, &roots, &id).unwrap();
        assert!(!o.changed, "手动刷新、没有变化");

        // 计划：不设间隔就不在计划里；设了以后从现在起算
        let now = chrono::Local::now().naive_local();
        assert!(due(&conn, now).unwrap().is_empty());
        let o = update(&conn, &roots, &id, &ExternalInput { refresh_minutes: Some(10), ..input("日志") }).unwrap();
        assert!(!o.changed);
        assert!(due(&conn, now).unwrap().is_empty(), "刚设置，还没到点");
        assert_eq!(due(&conn, now + chrono::Duration::minutes(11)).unwrap(), vec![id.clone()]);
        assert!(update(&conn, &roots, &id, &ExternalInput { refresh_minutes: Some(0), ..input("日志") }).is_err());

        // 新文件：检查发现变化并重建
        write(&conn, &dir.join("d2.parquet"), "SELECT 100 + i AS n FROM range(5) t(i)");
        assert!(files_changed(&roots, &def()).unwrap());
        let (_, o) = auto_check(&conn, &roots, &id).unwrap();
        let o = o.expect("有变化");
        assert_eq!((o.file_count, o.rows, o.added_files, o.changed), (2, 15, 1, true));
        assert_eq!(def().input.refresh_minutes, Some(10), "自动检查的设置保留");
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {id}"), [], |r| r.get(0)).unwrap();
        assert_eq!(n, 15);

        // 目录不见了：检查失败，原因记在定义里，数据集不动
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(auto_check(&conn, &roots, &id).is_err());
        let d = def();
        assert!(d.check_error.is_some() && d.files.len() == 2, "{:?}", d.check_error);

        // 修改读取方式：换目录后按新规则重新读取
        write(&conn, &root.path().join("新日志/x.parquet"), "SELECT 1 AS n, 'a' AS tag");
        let o = update(&conn, &roots, &id, &ExternalInput { refresh_minutes: Some(10), ..input("新日志") }).unwrap();
        assert!(o.changed && o.rows == 1 && o.added_columns == vec!["tag".to_string()]);
        let d = def();
        assert!(d.check_error.is_none() && d.input.path == "新日志");
    }

    #[test]
    fn create_browse_and_refresh() {
        let root = tempfile::tempdir().unwrap();
        let conn = setup();
        let dir = root.path().join("订单");
        write(&conn, &dir.join("p1.parquet"), "SELECT i AS id, 'A' || (i % 3) AS shop, (i * 1.5)::DECIMAL(10,2) AS amount, DATE '2026-09-01' + (i % 30)::INT AS day, i % 2 = 0 AS paid, CAST(i AS VARCHAR) || '号' AS note FROM range(100) t(i)");
        write(&conn, &dir.join("p2.parquet"), "SELECT 100 + i AS id, 'B' AS shop, 1.0::DECIMAL(10,2) AS amount, DATE '2026-10-01' AS day, true AS paid, 'x' AS note FROM range(50) t(i)");
        let roots = vec![root.path().to_path_buf()];

        let p = preview(&conn, &roots, &input("订单")).unwrap();
        assert_eq!((p.file_count, p.rows), (2, 150));
        let kinds: Vec<(&str, &str)> = p.columns.iter().map(|c| (c.title.as_str(), c.kind.as_str())).collect();
        assert_eq!(kinds, vec![("id", "integer"), ("shop", "text"), ("amount", "money"), ("day", "datetime"), ("paid", "boolean"), ("note", "text")]);
        assert_eq!(p.sample_rows[1], vec![serde_json::json!(1), serde_json::json!("A1"), serde_json::json!(1.5), serde_json::json!("2026-09-02"), serde_json::json!(false), serde_json::json!("1号")]);

        let id = create(&conn, &roots, "外部订单", &input("订单")).unwrap();
        let def = dataset::dataset(&conn, &id).unwrap();
        assert_eq!(def.kind, "external");
        let info = def.external.clone().unwrap();
        assert_eq!((info.file_count, info.rows), (2, 150));
        // 按（文件, 行号）范围取跨文件的一屏
        let ids: Vec<i64> = {
            let mut st = conn.prepare(&format!("SELECT id FROM {} t WHERE {} ORDER BY t.__fi, t.__fr", def.table, info.range_sql(98, 103))).unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(ids, vec![98, 99, 100, 101, 102]);
        // 通用路径的全局行号与位置一致
        let rid: i64 = conn.query_row(&format!("SELECT __rid FROM {} WHERE id = 120", def.table), [], |r| r.get(0)).unwrap();
        assert_eq!(rid, 120);
        // 游标：直通与筛选 + 排序后的位置表
        let cache = dataset::CursorCache::new();
        conn.execute_batch("ATTACH ':memory:' AS cursors").unwrap();
        let spec: dataset::QuerySpec = serde_json::from_value(serde_json::json!({"filters": [{"op": "in", "column": "shop", "values": ["B"]}], "sort": [{"column": "id", "desc": true}]})).unwrap();
        let c = dataset::open_cursor(&conn, &cache, &id, &spec).unwrap();
        assert_eq!((c.total, c.direct), (50, false));
        let w = dataset::rows(&conn, &cache, &id, &c.cursor_id, 0, 3, Some(&["id".to_string()])).unwrap();
        assert_eq!(w.rows, vec![vec![serde_json::json!(149)], vec![serde_json::json!(148)], vec![serde_json::json!(147)]]);
        let c = dataset::open_cursor(&conn, &cache, &id, &dataset::QuerySpec::default()).unwrap();
        assert_eq!((c.total, c.direct), (150, true));
        let w = dataset::rows(&conn, &cache, &id, &c.cursor_id, 99, 2, Some(&["id".to_string()])).unwrap();
        assert_eq!(w.rows, vec![vec![serde_json::json!(99)], vec![serde_json::json!(100)]]);
        // 列表给出的行数来自定义，不读文件
        let listed = dataset::list(&conn).unwrap().into_iter().find(|d| d.def.id == id).unwrap();
        assert_eq!(listed.row_count, 150);

        // 新增文件、多一列后刷新
        write(&conn, &dir.join("p3.parquet"), "SELECT 200 AS id, 'C' AS shop, 2.0::DECIMAL(10,2) AS amount, DATE '2026-11-01' AS day, false AS paid, 'y' AS note, 7 AS qty");
        let o = refresh(&conn, &roots, &id).unwrap();
        assert_eq!((o.file_count, o.rows, o.added_files, o.removed_files), (3, 151, 1, 0));
        assert_eq!(o.added_columns, vec!["qty".to_string()]);
        let def = dataset::dataset(&conn, &id).unwrap();
        assert!(def.columns.iter().any(|c| c.title == "qty" && c.kind == "integer"));
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {}", def.table), [], |r| r.get(0)).unwrap();
        assert_eq!(n, 151);

        // 计算列引用的列消失时，刷新整体取消
        dataset::add_calc(&conn, &id, "数量翻倍", "=[qty]*2", None).unwrap();
        std::fs::remove_file(dir.join("p3.parquet")).unwrap();
        let e = refresh(&conn, &roots, &id).err().unwrap();
        assert!(e.message().contains("失效"), "{}", e.message());
        assert_eq!(dataset::dataset(&conn, &id).unwrap().external.unwrap().file_count, 3);
    }
}
