//! 通用数据集导入：任意 CSV / XLSX → 服务端数据集（大数据表 + DSUMIFS 公式可直接使用）
//!
//! 1. 预览：识别编码、表头所在行（跳过前面的标题 / 说明行）、按样本推断列类型（文本 / 编号 / 数值 / 金额 / 日期时间），
//!    并与已有数据集按「原始列名」比对，给出可追加的目标（映射模板：同一来源的文件下次自动对齐）
//! 2. 提交：新建 / 追加 / 替换；可指定主键去重（文件内重复与已有数据重复都跳过）；
//!    大文件由 DuckDB read_csv 直接入库，XLSX 先转成 CSV

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::bank::{decode_text, parse_amount, parse_date_time, read_rows};
use crate::dataset::{self, StoredColumn};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::schema::{ident, literal};

const HEADER_SCAN_ROWS: usize = 30;
const SAMPLE_ROWS: usize = 2000;
const PREVIEW_ROWS: usize = 20;
/// 预览只读取文件开头这么多字节（CSV），避免大文件整体解码
const PREVIEW_BYTES: usize = 4 * 1024 * 1024;
/// 列名保留字（与内部列冲突时自动改名）
const RESERVED: &[&str] = &["search_text", "rowid"];

// ── 预览 ──────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PreviewColumn {
    /// 文件中的原始列名
    pub source: String,
    /// 建议的列名（去重、去空白后）
    pub title: String,
    /// 推断类型：text / id / number / money / datetime
    pub kind: String,
    /// 样本中非空值占比
    pub fill_rate: f64,
    pub samples: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetMatch {
    pub dataset_id: String,
    pub name: String,
    /// 文件中能对应上的列数 / 数据集列数
    pub matched: usize,
    pub total: usize,
    /// 数据集有、文件没有的列（追加时为空值）
    pub missing: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub token: String,
    pub file_name: String,
    pub file_size: u64,
    /// 表头所在行（1 起）
    pub header_row: usize,
    pub columns: Vec<PreviewColumn>,
    pub sample_rows: Vec<Vec<String>>,
    /// 估算的数据行数（CSV 按样本平均行长推算；XLSX 为实际行数）
    pub estimated_rows: i64,
    pub matches: Vec<DatasetMatch>,
    /// 多文件合并时各文件的行数与缺少的列
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_files: Vec<crate::import_source::SourceFileInfo>,
}

/// 上传文件暂存路径：保留原扩展名（XLSX 按扩展名识别格式）；清理沿用 preview- 前缀规则
pub fn staged_path(dir: &Path, token: &str, file_name: &str) -> AppResult<PathBuf> {
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(AppError::bad("无效的预览凭证"));
    }
    let ext = file_name.rsplit('.').next().unwrap_or("csv").to_ascii_lowercase();
    let ext = if matches!(ext.as_str(), "csv" | "txt" | "xls" | "xlsx" | "xlsm" | "ods") { ext } else { "csv".into() };
    Ok(dir.join(format!("preview-{token}.{ext}")))
}

fn is_xlsx(name: &str) -> bool {
    matches!(name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str(), "xls" | "xlsx" | "xlsm" | "ods")
}

/// 读取文件开头若干行（CSV 只读前 4 MB；XLSX 读整张表）
fn read_head(path: &Path, file_name: &str) -> AppResult<(Vec<Vec<String>>, bool, i64)> {
    if is_xlsx(file_name) {
        let rows = read_rows(path, file_name)?;
        let n = rows.len() as i64;
        return Ok((rows, true, n));
    }
    let mut buf = Vec::with_capacity(PREVIEW_BYTES);
    std::fs::File::open(path)?.take(PREVIEW_BYTES as u64).read_to_end(&mut buf)?;
    let truncated = buf.len() == PREVIEW_BYTES;
    // 截断处可能落在多字节字符中间：先按字节丢掉最后一个不完整的行再解码，否则 UTF-8 校验失败会被误判为 GBK
    if truncated && let Some(pos) = buf.iter().rposition(|&b| b == b'\n') {
        buf.truncate(pos);
    }
    let text = decode_text(&buf);
    let mut reader = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for rec in reader.records().take(SAMPLE_ROWS + HEADER_SCAN_ROWS + 1) {
        let rec = rec.map_err(|e| AppError::bad(format!("CSV 解析失败：{e}")))?;
        rows.push(rec.iter().map(clean_cell).collect());
    }
    // 估算总行数：文件大小 / 样本平均每行字节数
    let size = std::fs::metadata(path)?.len() as f64;
    let sample_bytes = text.len().max(1) as f64;
    let sample_lines = text.lines().count().max(1) as f64;
    let estimated = if truncated { (size / (sample_bytes / sample_lines)) as i64 } else { sample_lines as i64 };
    Ok((rows, false, estimated))
}

/// 去掉导出工具为防科学计数法加的前缀：'123、="123"、制表符
fn clean_cell(s: &str) -> String {
    let t = s.trim().trim_matches('\t').trim();
    let t = t.strip_prefix("=\"").and_then(|x| x.strip_suffix('"')).unwrap_or(t);
    t.strip_prefix('\'').unwrap_or(t).trim().to_string()
}

fn is_numeric_text(s: &str) -> bool {
    parse_amount(s).is_some()
}

/// 表头行：前 30 行中非空单元格最多、且大部分不是数字的一行
/// 表头所在行（0 起）：跳过文件开头的标题、说明行
pub(crate) fn header_row(path: &Path, file_name: &str) -> AppResult<usize> {
    let (rows, _, _) = read_head(path, file_name)?;
    Ok(detect_header(&rows))
}

pub(crate) fn detect_header(rows: &[Vec<String>]) -> usize {
    let filled = |r: &Vec<String>| r.iter().filter(|c| !c.is_empty()).count();
    let max_filled = rows.iter().take(HEADER_SCAN_ROWS).map(filled).max().unwrap_or(0);
    rows.iter()
        .take(HEADER_SCAN_ROWS)
        .position(|r| {
            let n = filled(r);
            let text = r.iter().filter(|c| !c.is_empty() && !is_numeric_text(c)).count();
            n > 0 && n * 10 >= max_filled * 7 && text * 10 >= n * 7
        })
        .unwrap_or(0)
}

fn infer_kind(title: &str, values: &[&str]) -> String {
    let non_empty: Vec<&str> = values.iter().copied().filter(|v| !v.is_empty()).collect();
    if non_empty.is_empty() {
        return "text".into();
    }
    let ratio = |pred: &dyn Fn(&str) -> bool| non_empty.iter().filter(|v| pred(v)).count() as f64 / non_empty.len() as f64;
    // 长数字串 / 前导零：编号（订单号、手机号），不能当数值（会丢精度、丢前导零）
    let id_like = |v: &str| {
        let digits = v.chars().all(|c| c.is_ascii_digit());
        digits && (v.len() >= 12 || (v.len() > 1 && v.starts_with('0')))
    };
    if ratio(&id_like) >= 0.6 {
        return "id".into();
    }
    if ratio(&|v| is_numeric_text(v)) >= 0.98 {
        let money_name = ["金额", "价", "费", "款", "额", "收入", "支出", "余额", "成本", "利润", "佣金", "补贴"].iter().any(|k| title.contains(k));
        let has_cents = non_empty.iter().any(|v| v.contains('.'));
        return if money_name || has_cents { "money".into() } else { "number".into() };
    }
    if ratio(&|v| v.chars().any(|c| c.is_ascii_digit()) && parse_date_time(v).is_some()) >= 0.95 {
        return "datetime".into();
    }
    "text".into()
}

/// 列名规范化：去空白、去重（重复列加 _2）、空列名用「列N」、避开内部保留字
fn normalize_titles(headers: &[String]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let mut t: String = h.trim().trim_start_matches('\u{feff}').chars().filter(|c| !c.is_control()).collect();
            t = t.replace('"', "");
            if t.is_empty() {
                t = format!("列{}", i + 1);
            }
            if RESERVED.contains(&t.to_lowercase().as_str()) {
                t = format!("{t}_列");
            }
            let n = seen.entry(t.clone()).or_insert(0);
            *n += 1;
            if *n > 1 { format!("{t}_{n}") } else { t }
        })
        .collect()
}

pub fn preview(conn: &Connection, path: &Path, token: &str, file_name: &str) -> AppResult<ImportPreview> {
    let (rows, _, estimated) = read_head(path, file_name)?;
    if rows.is_empty() {
        return Err(AppError::bad("文件是空的"));
    }
    let header_idx = detect_header(&rows);
    let headers = &rows[header_idx];
    let titles = normalize_titles(headers);
    let data: Vec<&Vec<String>> = rows.iter().skip(header_idx + 1).filter(|r| r.iter().any(|c| !c.is_empty())).take(SAMPLE_ROWS).collect();
    let columns: Vec<PreviewColumn> = titles
        .iter()
        .enumerate()
        .map(|(i, title)| {
            let values: Vec<&str> = data.iter().map(|r| r.get(i).map(String::as_str).unwrap_or("")).collect();
            let filled = values.iter().filter(|v| !v.is_empty()).count();
            PreviewColumn {
                source: headers.get(i).map(|h| h.trim().to_string()).unwrap_or_default(),
                title: title.clone(),
                kind: infer_kind(title, &values),
                fill_rate: if data.is_empty() { 0.0 } else { (filled as f64 / data.len() as f64 * 1000.0).round() / 1000.0 },
                samples: values.iter().filter(|v| !v.is_empty()).take(3).map(|v| v.to_string()).collect(),
            }
        })
        .collect();

    // 与已有数据集比对（按原始列名）：给出可追加的目标
    let sources: HashSet<&str> = columns.iter().map(|c| c.source.as_str()).collect();
    let mut matches: Vec<DatasetMatch> = dataset::stored(conn)?
        .into_iter()
        .filter_map(|(id, name, cols)| {
            let matched = cols.iter().filter(|c| sources.contains(c.source.as_str())).count();
            (matched * 2 >= cols.len()).then(|| DatasetMatch {
                dataset_id: id,
                name,
                matched,
                total: cols.len(),
                missing: cols.iter().filter(|c| !sources.contains(c.source.as_str())).map(|c| c.title.clone()).collect(),
            })
        })
        .collect();
    matches.sort_by(|a, b| (b.matched * 1000 / b.total.max(1)).cmp(&(a.matched * 1000 / a.total.max(1))));

    Ok(ImportPreview {
        token: token.to_string(),
        file_name: file_name.to_string(),
        file_size: std::fs::metadata(path)?.len(),
        header_row: header_idx + 1,
        columns,
        sample_rows: data.iter().take(PREVIEW_ROWS).map(|r| (0..titles.len()).map(|i| r.get(i).cloned().unwrap_or_default()).collect()).collect(),
        estimated_rows: (estimated - header_idx as i64 - 1).max(data.len() as i64),
        matches,
        source_files: vec![],
    })
}

// ── 提交 ──────────────────────────────────────────────

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Create,
    Append,
    Replace,
    /// 按主键新增或覆盖（已存在的行用新数据替换）；文件内同主键取最后出现的一行
    Upsert,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CommitColumn {
    pub source: String,
    pub title: String,
    pub kind: String,
    #[serde(default = "default_true")]
    pub include: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub token: String,
    pub mode: Mode,
    /// 追加 / 替换的目标数据集
    pub dataset_id: Option<String>,
    /// 新建时的数据集名称
    pub name: Option<String>,
    /// 新建时使用；追加 / 替换按目标数据集的列（按原始列名对齐）
    #[serde(default)]
    pub columns: Vec<CommitColumn>,
    /// 主键（列名）：用于去重；为空不去重
    #[serde(default)]
    pub primary_key: Vec<String>,
    #[serde(default)]
    pub file_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub dataset_id: String,
    pub name: String,
    pub rows_read: i64,
    pub inserted: i64,
    /// 主键重复被跳过（文件内 + 已有）
    pub skipped: i64,
    /// 增量更新时被新数据覆盖的已有行数
    pub updated: i64,
    /// 替换模式删除的旧行数
    pub replaced: i64,
    /// 数值 / 日期列中无法识别、按空值入库的个数
    pub invalid: Vec<InvalidCount>,
    pub elapsed_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidCount {
    pub column: String,
    pub count: i64,
}

fn valid_kind(k: &str) -> AppResult<&'static str> {
    Ok(match k {
        "text" => "text",
        "id" => "id",
        "number" => "number",
        "money" => "money",
        "datetime" => "datetime",
        other => return Err(AppError::bad(format!("未知的列类型：{other}"))),
    })
}

fn sql_type(kind: &str) -> &'static str {
    if matches!(kind, "number" | "money") { "DOUBLE" } else { "VARCHAR" }
}

/// 原始文本 → 列值的 SQL 转换（raw 为 VARCHAR 表达式）
fn convert_sql(kind: &str, raw: &str) -> String {
    let cleaned = format!("NULLIF(trim(regexp_replace(regexp_replace(trim({raw}), '^(=\"|''|\\t)+', ''), '\"$', '')), '')");
    match kind {
        "number" | "money" => format!(
            "TRY_CAST(replace(replace(replace(replace(replace({cleaned}, ',', ''), '，', ''), '¥', ''), '￥', ''), '元', '') AS DOUBLE)"
        ),
        "datetime" => format!(
            "COALESCE(strftime(TRY_STRPTIME(replace(replace({cleaned}, '年', '-'), '月', '-'), ['%Y-%m-%d %H:%M:%S', '%Y/%m/%d %H:%M:%S', '%Y-%m-%d %H:%M', '%Y/%m/%d %H:%M', '%Y-%m-%d', '%Y/%m/%d', '%Y%m%d', '%Y-%m-%d日']), '%Y-%m-%d %H:%M:%S'), {cleaned})"
        ),
        _ => cleaned,
    }
}

/// XLSX → 临时 CSV（只含数据行，不含表头之前的行）；CSV 直接转为 UTF-8
fn prepare_csv(path: &Path, file_name: &str, dir: &Path) -> AppResult<(PathBuf, usize)> {
    let (rows, _, _) = read_head(path, file_name)?;
    let header_idx = detect_header(&rows);
    let width = rows.get(header_idx).map(Vec::len).unwrap_or(0);
    if is_xlsx(file_name) {
        let out = dir.join(format!("ds-{}.csv", std::process::id() as u64 * 1000 + rows.len() as u64));
        let mut w = csv::Writer::from_path(&out)?;
        for r in rows.iter().skip(header_idx + 1) {
            if r.iter().all(String::is_empty) {
                continue;
            }
            let mut rec: Vec<&str> = r.iter().map(String::as_str).collect();
            rec.resize(width, "");
            w.write_record(&rec)?;
        }
        w.flush()?;
        return Ok((out, 0));
    }
    crate::import::ensure_utf8(path)?;
    Ok((path.to_path_buf(), header_idx + 1))
}

pub fn commit(conn: &Connection, path: &Path, req: &CommitRequest, dir: &Path) -> AppResult<ImportOutcome> {
    let started = Instant::now();
    let (rows, _, _) = read_head(path, &req.file_name)?;
    let header_idx = detect_header(&rows);
    let headers: Vec<String> = rows.get(header_idx).cloned().unwrap_or_default().iter().map(|h| h.trim().to_string()).collect();
    let source_pos: HashMap<&str, usize> = headers.iter().enumerate().map(|(i, h)| (h.as_str(), i)).collect();

    // 目标列：新建用请求中的列；追加 / 替换用已有数据集的列（按原始列名对齐）
    let (dataset_id, name, table, columns): (String, String, String, Vec<StoredColumn>) = match req.mode {
        Mode::Create => {
            let name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| AppError::bad("请填写数据集名称"))?.to_string();
            if dataset::datasets(conn)?.iter().any(|d| d.name == name) {
                return Err(AppError::bad(format!("已存在名为「{name}」的数据集")));
            }
            let mut seen = HashSet::new();
            let mut cols = Vec::new();
            for c in req.columns.iter().filter(|c| c.include) {
                let title = c.title.trim().replace('"', "");
                if title.is_empty() || !seen.insert(title.clone()) {
                    return Err(AppError::bad(format!("列名为空或重复：「{}」", c.title)));
                }
                if RESERVED.contains(&title.to_lowercase().as_str()) || title == "资金净额" {
                    return Err(AppError::bad(format!("「{title}」是保留列名，请换一个")));
                }
                let kind = valid_kind(&c.kind)?;
                cols.push(StoredColumn { key: title.clone(), title, kind: kind.into(), width: default_width(kind), source: c.source.clone() });
            }
            if cols.is_empty() {
                return Err(AppError::bad("至少保留一列"));
            }
            let id = format!("ds_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
            (id.clone(), name, id, cols)
        }
        Mode::Append | Mode::Replace | Mode::Upsert => {
            let id = req.dataset_id.as_deref().ok_or_else(|| AppError::bad("请选择目标数据集"))?;
            let (id, name, cols) = dataset::stored(conn)?
                .into_iter()
                .find(|(i, _, _)| i == id)
                .ok_or_else(|| match dataset::dataset(conn, id) {
                    Err(e) if e.message().contains("回收站") => e,
                    _ => AppError::bad("目标数据集不存在（内置数据集请用对应的导入入口）"),
                })?;
            if !cols.iter().any(|c| source_pos.contains_key(c.source.as_str())) {
                return Err(AppError::bad(format!("文件的列与数据集「{name}」完全对不上")));
            }
            (id.clone(), name, id, cols)
        }
    };
    // 主键：新建用请求中的；追加 / 替换默认沿用数据集登记的主键（保证同一数据集去重口径一致）
    let primary_key: Vec<String> = if req.mode == Mode::Create || !req.primary_key.is_empty() {
        req.primary_key.clone()
    } else {
        dataset::dataset(conn, &dataset_id)?.primary_key
    };
    for k in &primary_key {
        if !columns.iter().any(|c| &c.title == k) {
            return Err(AppError::bad(format!("主键列「{k}」不在数据集中")));
        }
    }
    if req.mode == Mode::Upsert && primary_key.is_empty() {
        return Err(AppError::bad("增量更新需要设置主键"));
    }

    let (csv_path, skip) = prepare_csv(path, &req.file_name, dir)?;
    let cleanup = csv_path != path;
    let result = (|| -> AppResult<ImportOutcome> {
        let names = (0..headers.len().max(1)).map(|i| format!("'c{i}'")).collect::<Vec<_>>().join(", ");
        let source = format!(
            "read_csv({}, header = false, skip = {skip}, all_varchar = true, names = [{names}], delim = ',', quote = '\"', escape = '\"', null_padding = true, strict_mode = false, ignore_errors = true)",
            literal(&csv_path.to_string_lossy())
        );
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE ds_staging AS SELECT row_number() OVER () AS __line, * FROM {source} WHERE NOT (COALESCE(c0, '') = '' AND {all_blank})",
            all_blank = (1..headers.len().max(1)).map(|i| format!("COALESCE(c{i}, '') = ''")).chain(std::iter::once("TRUE".into())).collect::<Vec<_>>().join(" AND ")
        ))?;
        let rows_read: i64 = conn.query_row("SELECT count(*) FROM ds_staging", [], |r| r.get(0))?;

        // 每列的取值表达式（文件中没有的列为 NULL）
        let exprs: Vec<(String, String, Option<String>)> = columns
            .iter()
            .map(|c| {
                let raw = source_pos.get(c.source.as_str()).map(|i| format!("s.c{i}"));
                let expr = raw.as_ref().map(|r| convert_sql(&c.kind, r)).unwrap_or_else(|| format!("NULL::{}", sql_type(&c.kind)));
                (c.title.clone(), expr, raw)
            })
            .collect();
        let invalid: Vec<InvalidCount> = {
            let checks: Vec<(String, String)> = exprs
                .iter()
                .zip(&columns)
                .filter(|(_, c)| matches!(c.kind.as_str(), "number" | "money"))
                .filter_map(|((title, expr, raw), _)| raw.as_ref().map(|r| (title.clone(), format!("count(*) FILTER (WHERE NULLIF(trim({r}), '') IS NOT NULL AND ({expr}) IS NULL)"))))
                .collect();
            if checks.is_empty() {
                vec![]
            } else {
                let sql = format!("SELECT {} FROM ds_staging s", checks.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>().join(", "));
                let counts: Vec<i64> = conn.query_row(&sql, [], |r| (0..checks.len()).map(|i| r.get(i)).collect())?;
                checks.into_iter().zip(counts).filter(|(_, n)| *n > 0).map(|((column, _), count)| InvalidCount { column, count }).collect()
            }
        };

        let text_cols: Vec<String> = columns.iter().filter(|c| matches!(c.kind.as_str(), "text" | "id")).map(|c| ident(&c.title)).collect();
        let search = if text_cols.is_empty() { "NULL".into() } else { format!("lower(concat_ws(' ', {}))", text_cols.join(", ")) };
        let col_list = columns.iter().map(|c| ident(&c.title)).collect::<Vec<_>>().join(", ");
        let select_list = exprs.iter().map(|(t, e, _)| format!("{e} AS {}", ident(t))).collect::<Vec<_>>().join(", ");
        let pk: Vec<String> = primary_key.iter().map(|k| ident(k)).collect();

        conn.execute_batch("BEGIN TRANSACTION")?;
        let written = (|| -> AppResult<(i64, i64, i64)> {
            if req.mode == Mode::Create {
                let defs = columns.iter().map(|c| format!("{} {}", ident(&c.title), sql_type(&c.kind))).collect::<Vec<_>>().join(", ");
                conn.execute_batch(&format!("CREATE TABLE {table} ({defs}, search_text VARCHAR)"))?;
                dataset::register(conn, &dataset_id, &name, &columns, &primary_key)?;
            }
            let replaced = if req.mode == Mode::Replace { conn.execute(&format!("DELETE FROM {table}"), [])? as i64 } else { 0 };
            // 转换后的行（按文件原顺序入库）；有主键时文件内同主键只保留最先出现的一行，并跳过已存在的主键
            let converted = format!("SELECT s.__line, {select_list} FROM ds_staging s");
            let n_text = columns.iter().filter(|c| matches!(c.kind.as_str(), "text" | "id")).map(|c| format!("n.{}", ident(&c.title))).collect::<Vec<_>>();
            let search_n = if n_text.is_empty() { "NULL".into() } else { format!("lower(concat_ws(' ', {}))", n_text.join(", ")) };
            if req.mode == Mode::Upsert {
                // 先删掉主键已存在的旧行，再整体插入新数据：被覆盖的行数 = 删除数
                let pk_list = pk.join(", ");
                let matches = pk.iter().map(|k| format!("d.{k} IS NOT DISTINCT FROM n.{k}")).collect::<Vec<_>>().join(" AND ");
                conn.execute_batch(&format!(
                    "CREATE OR REPLACE TEMP TABLE ds_upsert AS SELECT * FROM ({converted}) QUALIFY row_number() OVER (PARTITION BY {pk_list} ORDER BY __line DESC) = 1"
                ))?;
                // 内容完全相同的行不动（增量同步按 >= 取数会重复读到上次最后一批）：不计为更新，也不改变行的位置
                let same = columns.iter().map(|c| format!("d.{k} IS NOT DISTINCT FROM n.{k}", k = ident(&c.title))).collect::<Vec<_>>().join(" AND ");
                conn.execute(&format!("DELETE FROM ds_upsert n WHERE EXISTS (SELECT 1 FROM {table} d WHERE {same})"), [])?;
                let updated = conn.execute(&format!("DELETE FROM {table} d WHERE EXISTS (SELECT 1 FROM ds_upsert n WHERE {matches})"), [])? as i64;
                let written = conn.execute(&format!("INSERT INTO {table} ({col_list}, search_text) SELECT n.* EXCLUDE (__line), {search_n} FROM ds_upsert n ORDER BY n.__line"), [])? as i64;
                conn.execute_batch("DROP TABLE IF EXISTS ds_upsert")?;
                conn.execute("UPDATE ledger_dataset SET updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now_local(), dataset_id])?;
                return Ok((written - updated, 0, updated));
            }
            let insert = if pk.is_empty() {
                format!("INSERT INTO {table} ({col_list}, search_text) SELECT * EXCLUDE (__line), {search} FROM ({converted}) ORDER BY __line")
            } else {
                let pk_list = pk.join(", ");
                let exists = pk.iter().map(|k| format!("d.{k} IS NOT DISTINCT FROM n.{k}")).collect::<Vec<_>>().join(" AND ");
                format!(
                    "INSERT INTO {table} ({col_list}, search_text)
                     SELECT n.* EXCLUDE (__line), {search_n}
                     FROM (SELECT * FROM ({converted}) QUALIFY row_number() OVER (PARTITION BY {pk_list} ORDER BY __line) = 1) n
                     WHERE NOT EXISTS (SELECT 1 FROM {table} d WHERE {exists})
                     ORDER BY n.__line",
                )
            };
            let inserted = conn.execute(&insert, [])? as i64;
            conn.execute("UPDATE ledger_dataset SET updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now_local(), dataset_id])?;
            Ok((inserted, replaced, 0))
        })();
        let (inserted, replaced, updated) = match written {
            Ok(v) => {
                conn.execute_batch("COMMIT")?;
                v
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        };
        Ok(ImportOutcome {
            dataset_id: dataset_id.clone(),
            name: name.clone(),
            rows_read,
            inserted,
            skipped: rows_read - inserted - updated,
            updated,
            replaced,
            invalid,
            elapsed_ms: started.elapsed().as_millis() as i64,
        })
    })();
    let _ = conn.execute_batch("DROP TABLE IF EXISTS ds_staging");
    if cleanup {
        let _ = std::fs::remove_file(&csv_path);
    }
    result
}

/// 新建一个空数据集（填报数据集）：返回 id
pub fn create_empty(conn: &Connection, name: &str, columns: &[(String, String)], primary_key: &[String]) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::bad("数据集名称不能为空，且不超过 40 个字符"));
    }
    if dataset::datasets(conn)?.iter().any(|d| d.name == name) {
        return Err(AppError::bad(format!("已存在名为「{name}」的数据集")));
    }
    let mut seen = HashSet::new();
    let mut cols = Vec::new();
    for (title, kind) in columns {
        let title = title.trim().replace('"', "");
        if title.is_empty() || !seen.insert(title.clone()) {
            return Err(AppError::bad(format!("列名为空或重复：「{title}」")));
        }
        if RESERVED.contains(&title.to_lowercase().as_str()) || title == "资金净额" {
            return Err(AppError::bad(format!("「{title}」是保留列名，请换一个")));
        }
        let kind = valid_kind(kind)?;
        cols.push(StoredColumn { key: title.clone(), title: title.clone(), kind: kind.into(), width: default_width(kind), source: title });
    }
    if cols.is_empty() {
        return Err(AppError::bad("至少需要一列"));
    }
    for k in primary_key {
        if !cols.iter().any(|c| &c.title == k) {
            return Err(AppError::bad(format!("主键列「{k}」不在列中")));
        }
    }
    let id = format!("ds_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    let defs = cols.iter().map(|c| format!("{} {}", ident(&c.title), sql_type(&c.kind))).collect::<Vec<_>>().join(", ");
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        conn.execute_batch(&format!("CREATE TABLE {id} ({defs}, search_text VARCHAR)"))?;
        dataset::register(conn, &id, name, &cols, primary_key)?;
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

fn default_width(kind: &str) -> u32 {
    match kind {
        "money" | "number" => 110,
        "datetime" => 150,
        "id" => 180,
        _ => 140,
    }
}
