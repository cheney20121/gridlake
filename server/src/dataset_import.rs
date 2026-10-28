//! 通用数据集导入：任意 CSV / XLSX → 服务端数据集（大数据表 + DSUMIFS 公式可直接使用）
//!
//! 1. 预览：识别编码、表头所在行（跳过前面的标题 / 说明行）、按样本推断列类型（文本 / 编号 / 整数 / 数值 / 金额 / 日期时间 / 是否），
//!    并与已有数据集按「原始列名」比对，给出可追加的目标（映射模板：同一来源的文件下次自动对齐）
//! 2. 提交：新建 / 追加 / 替换；可指定主键去重（文件内重复与已有数据重复都跳过）；
//!    大文件由 DuckDB read_csv 直接入库，XLSX 先转成 CSV

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use duckdb::{Connection, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::tabular::{decode_text, parse_amount, parse_date_time, read_rows};
use crate::dataset::{self, StoredColumn};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::kinds;
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
    /// 推断类型：见 [`crate::kinds`]
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

/// 列名说明这一列是编号 / 号码（不是数量）。「序号」不算：它要按数字排序
fn id_title(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    !t.contains("序号")
        && ["手机", "电话", "联系方式", "身份证", "证件号", "编号", "编码", "代码", "单号", "流水号", "邮编", "邮政编码", "条码", "卡号", "账号", "帐号", "工号", "学号", "会员号", "sku", "phone", "mobile", "zip"]
            .iter()
            .any(|k| t.contains(k))
}

pub(crate) fn infer_kind(title: &str, values: &[&str]) -> String {
    let non_empty: Vec<&str> = values.iter().copied().filter(|v| !v.is_empty()).collect();
    if non_empty.is_empty() {
        return "text".into();
    }
    let ratio = |pred: &dyn Fn(&str) -> bool| non_empty.iter().filter(|v| pred(v)).count() as f64 / non_empty.len() as f64;
    // 长数字串 / 前导零 / 手机号 / 身份证号：编号，不能当数值（会丢精度、丢前导零、显示成 13,800,138,000）
    let digits = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit());
    let id_like = |v: &str| {
        let phone = v.len() == 11 && v.starts_with('1') && digits(v);
        let idcard = v.len() == 18 && v.is_ascii() && digits(&v[..17]) && v[17..].chars().all(|c| c.is_ascii_digit() || c == 'X' || c == 'x');
        phone || idcard || (digits(v) && (v.len() >= 12 || (v.len() > 1 && v.starts_with('0'))))
    };
    if ratio(&id_like) >= 0.6 {
        return "id".into();
    }
    // 列名说明是编号（手机号、邮编、工号…）、值都是纯数字：也按编号保存（不加千分位、按原样比较）
    if id_title(title) && ratio(&digits) >= 0.9 {
        return "id".into();
    }
    if ratio(&|v| is_numeric_text(v)) >= 0.98 {
        let money_name = ["金额", "价", "费", "款", "额", "收入", "支出", "余额", "成本", "利润", "佣金", "补贴"].iter().any(|k| title.contains(k));
        if money_name {
            return "money".into();
        }
        // 全是整数（数量、件数）：按 BIGINT 精确保存；有小数的是金额（带分）或普通数值
        if non_empty.iter().all(|v| kinds::parse_integer(v).is_some()) {
            return "integer".into();
        }
        let has_cents = non_empty.iter().any(|v| v.contains('.'));
        return if has_cents { "money".into() } else { "number".into() };
    }
    // 是 / 否：只认 true / false 写法（「是 / 否」这类中文按文本保留原样，需要时在预览里手动改成是否列）
    if non_empty.iter().all(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "false")) {
        return "boolean".into();
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
    let hints = crate::import_source::kind_hints(path);
    let data: Vec<&Vec<String>> = rows.iter().skip(header_idx + 1).filter(|r| r.iter().any(|c| !c.is_empty())).take(SAMPLE_ROWS).collect();
    let columns: Vec<PreviewColumn> = titles
        .iter()
        .enumerate()
        .map(|(i, title)| {
            let values: Vec<&str> = data.iter().map(|r| r.get(i).map(String::as_str).unwrap_or("")).collect();
            let filled = values.iter().filter(|v| !v.is_empty()).count();
            let source = headers.get(i).map(|h| h.trim().to_string()).unwrap_or_default();
            // Parquet / JSON 自带的列类型优先；整数列里有前导零或很长的（编号）仍按样本判断为编号
            let inferred = infer_kind(title, &values);
            let kind = match hints.get(&source).and_then(|k| kinds::parse(k)) {
                Some("integer") if inferred == "id" => inferred,
                Some(k) => k.to_string(),
                None => inferred,
            };
            PreviewColumn {
                source,
                title: title.clone(),
                kind,
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
    matches.sort_by_key(|m| std::cmp::Reverse(m.matched * 1000 / m.total.max(1)));

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
    /// 新建时按样本推断的整数 / 是否列，全量数据里有放不下的值，已改成数值 / 文本列（列名）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub widened: Vec<String>,
    /// 不符合校验规则、按设置跳过没有导入的条数（数据集设置了「跳过不合格的行」时）
    pub rejected: i64,
    /// 每条规则拦下了几条
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejected_rules: Vec<crate::validation::ImportHit>,
    pub elapsed_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidCount {
    pub column: String,
    pub count: i64,
}

fn valid_kind(k: &str) -> AppResult<&'static str> {
    kinds::parse(k).ok_or_else(|| AppError::bad(format!("未知的列类型：{k}")))
}

/// 原始文本 → 列值的 SQL 转换（raw 为 VARCHAR 表达式）
pub(crate) fn convert_sql(kind: &str, raw: &str) -> String {
    let cleaned = format!("NULLIF(trim(regexp_replace(regexp_replace(trim({raw}), '^(=\"|''|\\t)+', ''), '\"$', '')), '')");
    match kind {
        "number" | "money" => format!(
            "TRY_CAST(replace(replace(replace(replace(replace({cleaned}, ',', ''), '，', ''), '¥', ''), '￥', ''), '元', '') AS DOUBLE)"
        ),
        "integer" => kinds::integer_sql(&cleaned),
        "boolean" => kinds::bool_sql(&cleaned),
        "datetime" => format!(
            "COALESCE(strftime(TRY_STRPTIME(replace(replace({cleaned}, '年', '-'), '月', '-'), ['%Y-%m-%d %H:%M:%S', '%Y/%m/%d %H:%M:%S', '%Y-%m-%d %H:%M', '%Y/%m/%d %H:%M', '%Y-%m-%d', '%Y/%m/%d', '%Y%m%d', '%Y-%m-%d日']), '%Y-%m-%d %H:%M:%S'), {cleaned})"
        ),
        _ => cleaned,
    }
}

/// 新建数据集时，用全量数据核对按样本（前 2000 行）推断出的整数 / 是否列：
/// 后面出现小数的整数列改成数值列，出现其他写法的是否列改成文本列——不让样本之外的值按空值入库。返回被改动的列名
fn widen_kinds(conn: &Connection, columns: &mut [StoredColumn], source_pos: &HashMap<&str, usize>) -> AppResult<Vec<String>> {
    let mut checks = Vec::new();
    for (i, c) in columns.iter().enumerate() {
        let Some(pos) = source_pos.get(c.source.as_str()) else { continue };
        let raw = format!("s.c{pos}");
        let cleaned = format!("NULLIF(trim({raw}), '')");
        let misfit = match c.kind.as_str() {
            "integer" => format!("{} IS NULL AND {} IS NOT NULL", convert_sql("integer", &raw), convert_sql("number", &raw)),
            "boolean" => format!("{} IS NULL", convert_sql("boolean", &raw)),
            _ => continue,
        };
        checks.push((i, format!("count(*) FILTER (WHERE {cleaned} IS NOT NULL AND {misfit}) > 0")));
    }
    if checks.is_empty() {
        return Ok(vec![]);
    }
    let sql = format!("SELECT {} FROM ds_staging s", checks.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>().join(", "));
    let misfits: Vec<bool> = conn.query_row(&sql, [], |r| (0..checks.len()).map(|i| r.get(i)).collect())?;
    let mut widened = Vec::new();
    for ((i, _), misfit) in checks.into_iter().zip(misfits) {
        if misfit {
            let c = &mut columns[i];
            c.kind = if c.kind == "integer" { "number" } else { "text" }.into();
            widened.push(c.title.clone());
        }
    }
    Ok(widened)
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
    crate::tabular::ensure_utf8(path)?;
    Ok((path.to_path_buf(), header_idx + 1))
}

pub fn commit(conn: &Connection, path: &Path, req: &CommitRequest, dir: &Path) -> AppResult<ImportOutcome> {
    let started = Instant::now();
    let (rows, _, _) = read_head(path, &req.file_name)?;
    let header_idx = detect_header(&rows);
    let headers: Vec<String> = rows.get(header_idx).cloned().unwrap_or_default().iter().map(|h| h.trim().to_string()).collect();
    let source_pos: HashMap<&str, usize> = headers.iter().enumerate().map(|(i, h)| (h.as_str(), i)).collect();

    // 目标列：新建用请求中的列；追加 / 替换用已有数据集的列（按原始列名对齐）
    let (dataset_id, name, table, mut columns): (String, String, String, Vec<StoredColumn>) = match req.mode {
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
        let widened = if req.mode == Mode::Create { widen_kinds(conn, &mut columns, &source_pos)? } else { vec![] };

        // 每列的取值表达式（文件中没有的列为 NULL）
        let exprs: Vec<(String, String, Option<String>)> = columns
            .iter()
            .map(|c| {
                let raw = source_pos.get(c.source.as_str()).map(|i| format!("s.c{i}"));
                let expr = raw.as_ref().map(|r| convert_sql(&c.kind, r)).unwrap_or_else(|| format!("NULL::{}", kinds::sql_type(&c.kind)));
                (c.title.clone(), expr, raw)
            })
            .collect();
        let invalid: Vec<InvalidCount> = {
            let checks: Vec<(String, String)> = exprs
                .iter()
                .zip(&columns)
                .filter(|(_, c)| kinds::is_numeric(&c.kind) || c.kind == "boolean")
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

        let pk: Vec<String> = primary_key.iter().map(|k| ident(k)).collect();
        let col_list = columns.iter().map(|c| ident(&c.title)).collect::<Vec<_>>().join(", ");
        let select_list = exprs.iter().map(|(t, e, _)| format!("{e} AS {}", ident(t))).collect::<Vec<_>>().join(", ");

        // 导入时拦截：数据集设了「跳过 / 拒绝不合格的行」时，按规则检查真正会写入的新数据（主键去重之后的）
        let (rejected, rejected_rules) = match if req.mode == Mode::Create { None } else { crate::validation::import_gate(conn, &dataset_id)? } {
            None => (0, vec![]),
            Some((def, policy, rules)) => {
                let converted = format!("SELECT s.__line, {select_list} FROM ds_staging s");
                let pk_list = pk.join(", ");
                let candidates = match (req.mode, pk.is_empty()) {
                    (_, true) => converted,
                    (Mode::Upsert, false) => format!("SELECT * FROM ({converted}) QUALIFY row_number() OVER (PARTITION BY {pk_list} ORDER BY __line DESC) = 1"),
                    (Mode::Replace, false) => format!("SELECT * FROM ({converted}) QUALIFY row_number() OVER (PARTITION BY {pk_list} ORDER BY __line) = 1"),
                    (_, false) => {
                        let exists = pk.iter().map(|k| format!("d.{k} IS NOT DISTINCT FROM n.{k}")).collect::<Vec<_>>().join(" AND ");
                        format!("SELECT * FROM (SELECT * FROM ({converted}) QUALIFY row_number() OVER (PARTITION BY {pk_list} ORDER BY __line) = 1) n WHERE NOT EXISTS (SELECT 1 FROM {table} d WHERE {exists})")
                    }
                };
                conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE ds_new AS {candidates}"))?;
                // 「不能重复」的范围：替换时只有新数据；增量更新时去掉会被覆盖的旧行
                let universe = match req.mode {
                    Mode::Replace => "ds_new".to_string(),
                    Mode::Upsert => {
                        let matches = pk.iter().map(|k| format!("d.{k} IS NOT DISTINCT FROM n.{k}")).collect::<Vec<_>>().join(" AND ");
                        format!("(SELECT {col_list} FROM {table} d WHERE NOT EXISTS (SELECT 1 FROM ds_new n WHERE {matches}) UNION ALL SELECT {col_list} FROM ds_new)")
                    }
                    _ => format!("(SELECT {col_list} FROM {table} UNION ALL SELECT {col_list} FROM ds_new)"),
                };
                let check = crate::validation::check_import(conn, &def, policy, &rules, "ds_new", &universe);
                let result = match check {
                    Ok(c) if c.bad > 0 && c.policy == crate::validation::OnImport::Reject => Err(crate::validation::reject_error(&name, &c)),
                    Ok(c) if c.bad > 0 => conn
                        .execute(&format!("DELETE FROM ds_staging WHERE __line IN (SELECT __line FROM ds_new WHERE {})", c.condition), params_from_iter(c.args.iter()))
                        .map(|n| (n as i64, c.hits))
                        .map_err(AppError::from),
                    Ok(_) => Ok((0, vec![])),
                    Err(e) => Err(e),
                };
                let _ = conn.execute_batch("DROP TABLE IF EXISTS ds_new");
                result?
            }
        };

        let text_cols: Vec<String> = columns.iter().filter(|c| kinds::is_textual(&c.kind)).map(|c| ident(&c.title)).collect();
        let search = if text_cols.is_empty() { "NULL".into() } else { format!("lower(concat_ws(' ', {}))", text_cols.join(", ")) };

        conn.execute_batch("BEGIN TRANSACTION")?;
        let written = (|| -> AppResult<(i64, i64, i64)> {
            if req.mode == Mode::Create {
                let defs = columns.iter().map(|c| format!("{} {}", ident(&c.title), kinds::sql_type(&c.kind))).collect::<Vec<_>>().join(", ");
                conn.execute_batch(&format!("CREATE TABLE {table} ({defs}, search_text VARCHAR)"))?;
                dataset::register(conn, &dataset_id, &name, &columns, &primary_key)?;
            }
            let replaced = if req.mode == Mode::Replace { conn.execute(&format!("DELETE FROM {table}"), [])? as i64 } else { 0 };
            // 转换后的行（按文件原顺序入库）；有主键时文件内同主键只保留最先出现的一行，并跳过已存在的主键
            let converted = format!("SELECT s.__line, {select_list} FROM ds_staging s");
            let n_text = columns.iter().filter(|c| kinds::is_textual(&c.kind)).map(|c| format!("n.{}", ident(&c.title))).collect::<Vec<_>>();
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
            skipped: rows_read - inserted - updated - rejected,
            updated,
            replaced,
            invalid,
            widened,
            rejected,
            rejected_rules,
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
    let defs = cols.iter().map(|c| format!("{} {}", ident(&c.title), kinds::sql_type(&c.kind))).collect::<Vec<_>>().join(", ");
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
    kinds::default_width(kind)
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

    fn create(conn: &Connection, dir: &Path, csv: &str, kinds: &[(&str, &str)]) -> (ImportPreview, ImportOutcome) {
        let path = dir.join("preview-t.csv");
        std::fs::write(&path, csv).unwrap();
        let p = preview(conn, &path, "t", "t.csv").unwrap();
        let columns = p
            .columns
            .iter()
            .map(|c| CommitColumn {
                source: c.source.clone(),
                title: c.title.clone(),
                kind: kinds.iter().find(|(t, _)| *t == c.title).map(|(_, k)| k.to_string()).unwrap_or_else(|| c.kind.clone()),
                include: true,
            })
            .collect();
        let req = CommitRequest { token: "t".into(), mode: Mode::Create, dataset_id: None, name: Some("订单".into()), columns, primary_key: vec![], file_name: "t.csv".into() };
        let o = commit(conn, &path, &req, dir).unwrap();
        (p, o)
    }

    fn column_types(conn: &Connection, table: &str) -> HashMap<String, String> {
        let mut st = conn.prepare(&format!("DESCRIBE {table}")).unwrap();
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap().map(Result::unwrap).collect()
    }

    #[test]
    fn infers_integer_and_boolean() {
        assert_eq!(infer_kind("数量", &["3", "1,200", "-4"]), "integer");
        assert_eq!(infer_kind("金额", &["3", "4"]), "money");
        assert_eq!(infer_kind("单价", &["3.5", "4"]), "money");
        assert_eq!(infer_kind("得分", &["3", "4.25"]), "money");
        assert_eq!(infer_kind("订单号", &["690000000000000123", "690000000000000124"]), "id");
        assert_eq!(infer_kind("已付款", &["TRUE", "false", ""]), "boolean");
        // 中文的是 / 否保留原样，需要时手动改成是否列
        assert_eq!(infer_kind("已付款", &["是", "否"]), "text");
    }

    #[test]
    fn phone_idcard_and_codes_are_ids() {
        // 手机号不论列名都按编号；身份证号末位可以是 X
        assert_eq!(infer_kind("联系人", &["13800000001", "13912345678", ""]), "id");
        assert_eq!(infer_kind("证件", &["11010119900101123X", "110101199001011234"]), "id");
        // 列名说明是编号、值是短的纯数字
        assert_eq!(infer_kind("工号", &["1001", "1002", "1003"]), "id");
        assert_eq!(infer_kind("邮编", &["100000", "200000"]), "id");
        assert_eq!(infer_kind("会员编号", &["7", "15", "N/A"]), "text");
        // 序号、数量、11 位但不是 1 开头的数字仍是数
        assert_eq!(infer_kind("序号", &["1", "2", "3"]), "integer");
        assert_eq!(infer_kind("数量", &["1001", "1002"]), "integer");
        assert_eq!(infer_kind("销售额", &["23800000001", "5"]), "money");
        assert_eq!(infer_kind("电话", &["010-12345678", "021-87654321"]), "text");
        // 18 个字节的中文不会被当成身份证号（也不会在切字节时出错）
        assert_eq!(infer_kind("地址", &["北京市朝阳区", "上海市浦东区"]), "text");
    }

    #[test]
    fn stores_native_types() {
        let dir = tempfile::tempdir().unwrap();
        let conn = setup();
        let csv = "订单号,数量,金额,已付款,签收,日期\nA1,3,10.5,true,是,2026-10-01\nA2,\"1,200\",20,FALSE,否,2026/10/2\nA3,,7,,,\n";
        let (p, o) = create(&conn, dir.path(), csv, &[("签收", "boolean")]);
        let kinds: Vec<&str> = p.columns.iter().map(|c| c.kind.as_str()).collect();
        assert_eq!(kinds, vec!["text", "integer", "money", "boolean", "text", "datetime"]);
        assert_eq!(o.inserted, 3);
        assert!(o.invalid.is_empty() && o.widened.is_empty());

        let types = column_types(&conn, &o.dataset_id);
        assert_eq!(types["数量"], "BIGINT");
        assert_eq!(types["金额"], "DOUBLE");
        assert_eq!(types["已付款"], "BOOLEAN");
        assert_eq!(types["签收"], "BOOLEAN");
        assert_eq!(types["日期"], "VARCHAR");
        let rows: Vec<(String, Option<i64>, Option<bool>, Option<bool>)> = {
            let mut st = conn.prepare(&format!("SELECT \"订单号\", \"数量\", \"已付款\", \"签收\" FROM {} ORDER BY 1", o.dataset_id)).unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(rows[0], ("A1".into(), Some(3), Some(true), Some(true)));
        assert_eq!(rows[1], ("A2".into(), Some(1200), Some(false), Some(false)));
        assert_eq!(rows[2], ("A3".into(), None, None, None));

        // 登记的列类型原样读回
        let def = dataset::dataset(&conn, &o.dataset_id).unwrap();
        let kind = |t: &str| def.columns.iter().find(|c| c.title == t).unwrap().kind;
        assert_eq!((kind("数量"), kind("已付款")), ("integer", "boolean"));
        // 整数求和得到 HUGEINT，转 JSON 后仍是整数
        let sum = conn.query_row(&format!("SELECT sum(\"数量\") FROM {}", o.dataset_id), [], |r| Ok(crate::tabular::to_json(r.get_ref(0)?))).unwrap();
        assert_eq!(sum, serde_json::json!(1203));
        let flag = conn.query_row(&format!("SELECT \"已付款\" FROM {} WHERE \"订单号\" = 'A1'", o.dataset_id), [], |r| Ok(crate::tabular::to_json(r.get_ref(0)?))).unwrap();
        assert_eq!(flag, serde_json::json!(true));
    }

    #[test]
    fn big_integers_keep_every_digit() {
        let dir = tempfile::tempdir().unwrap();
        let conn = setup();
        // 超过 2^53 的整数用 DOUBLE 存会丢最后几位；手动选整数后按 BIGINT 精确保存
        let (_, o) = create(&conn, dir.path(), "流水号,备注\n9007199254740993,a\n-9007199254740995,b\n", &[("流水号", "integer")]);
        let v: Vec<i64> = {
            let mut st = conn.prepare(&format!("SELECT \"流水号\" FROM {} ORDER BY rowid", o.dataset_id)).unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(v, vec![9_007_199_254_740_993, -9_007_199_254_740_995]);
        // 下发给浏览器时超过 2^53 的按文本给出（JSON 数字在浏览器里会丢位），小的仍是数字
        let json: Vec<serde_json::Value> = {
            let mut st = conn.prepare(&format!("SELECT \"流水号\" FROM {} UNION ALL SELECT 42::BIGINT", o.dataset_id)).unwrap();
            st.query_map([], |r| Ok(crate::tabular::to_json(r.get_ref(0)?))).unwrap().map(Result::unwrap).collect()
        };
        assert!(json.contains(&serde_json::json!("9007199254740993")) && json.contains(&serde_json::json!(42)));
    }

    #[test]
    fn widens_when_rows_after_the_sample_do_not_fit() {
        let dir = tempfile::tempdir().unwrap();
        let conn = setup();
        // 前 2000 行（推断用的样本）都是整数 / true、false，后面才出现小数和别的写法
        let mut csv = String::from("数量,有效\n");
        for i in 0..SAMPLE_ROWS + 50 {
            csv.push_str(&format!("{i},{}\n", i % 2 == 0));
        }
        csv.push_str("2.5,未知\n");
        let (p, o) = create(&conn, dir.path(), &csv, &[]);
        assert_eq!(p.columns.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), vec!["integer", "boolean"]);
        assert_eq!(o.widened, vec!["数量".to_string(), "有效".to_string()]);
        assert!(o.invalid.is_empty(), "{:?}", o.invalid.iter().map(|i| (&i.column, i.count)).collect::<Vec<_>>());
        let types = column_types(&conn, &o.dataset_id);
        assert_eq!((types["数量"].as_str(), types["有效"].as_str()), ("DOUBLE", "VARCHAR"));
        let def = dataset::dataset(&conn, &o.dataset_id).unwrap();
        assert_eq!(def.columns.iter().map(|c| c.kind).collect::<Vec<_>>(), vec!["number", "text"]);
        let last: (f64, String) = conn.query_row(&format!("SELECT \"数量\", \"有效\" FROM {} WHERE \"数量\" = 2.5", o.dataset_id), [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(last, (2.5, "未知".into()));
    }

    #[test]
    fn appending_reports_values_that_do_not_fit() {
        let dir = tempfile::tempdir().unwrap();
        let conn = setup();
        let (_, o) = create(&conn, dir.path(), "数量,有效\n1,true\n", &[]);
        // 追加到已有的整数 / 是否列：不改结构，认不出的值按空值入库并报告
        let path = dir.path().join("preview-u.csv");
        std::fs::write(&path, "数量,有效\n2.5,maybe\n4,false\n").unwrap();
        let req = CommitRequest { token: "u".into(), mode: Mode::Append, dataset_id: Some(o.dataset_id.clone()), name: None, columns: vec![], primary_key: vec![], file_name: "u.csv".into() };
        let r = commit(&conn, &path, &req, dir.path()).unwrap();
        assert_eq!(r.inserted, 2);
        let invalid: Vec<(String, i64)> = r.invalid.iter().map(|i| (i.column.clone(), i.count)).collect();
        assert_eq!(invalid, vec![("数量".to_string(), 1), ("有效".to_string(), 1)]);
    }

    #[test]
    fn source_type_hints_win_over_samples() {
        let dir = tempfile::tempdir().unwrap();
        let conn = setup();
        let path = dir.path().join("preview-h.csv");
        // 像 Parquet 转出的 CSV：金额列碰巧都是整数，按样本会判成整数；提示文件里的类型优先，认不出的类型忽略
        std::fs::write(&path, "amount,user_id,ok,note\n10,123,true,x\n20,456,false,y\n").unwrap();
        std::fs::write(crate::import_source::hints_path(&path), r#"{"amount":"money","user_id":"integer","ok":"boolean","bogus":"nope"}"#).unwrap();
        let p = preview(&conn, &path, "h", "h.csv").unwrap();
        assert_eq!(p.columns.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), vec!["money", "integer", "boolean", "text"]);
    }
}
