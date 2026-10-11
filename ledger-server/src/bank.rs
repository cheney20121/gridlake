//! 提现—银行到账对账
//!
//! 1. 按主体导入银行流水（CSV / XLS / XLSX）。各银行导出格式不一：前若干行常为账户信息，表头名也不同，
//!    这里扫描前 30 行找表头，按同义词识别「交易时间 / 收入 / 支出 / 金额 + 借贷标志 / 对方户名 / 摘要 / 流水号」
//! 2. 抖店「提现」出账流水按 主体 + 金额相等 + 到账日期在提现后 0~3 天 自动一对一配对，其余可手工配对
//! 3. 未配对的提现：仍在到账窗口内 = 在途；银行流水未覆盖到账窗口 = 待导入银行流水；否则 = 逾期未到账（需追查）

use std::collections::{HashMap, HashSet};
use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto};
use chrono::{Duration, NaiveDate, NaiveDateTime};
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::close;
use crate::db::{now_local, today_local};
use crate::error::{AppError, AppResult};
use crate::schema::ident;

/// 提现到账窗口：提现当天 ~ 之后 3 天
pub const WINDOW_DAYS: i64 = 3;
const HEADER_SCAN_ROWS: usize = 30;
/// 对方户名 / 摘要含这些词的银行收入视为平台打款（用于提示「银行有、平台无」）
const PLATFORM_KEYWORDS: &[&str] = &["抖音", "字节", "微播", "抖店", "头条", "合众易宝", "提现"];

// ── 文件解析 ──────────────────────────────────────────

pub(crate) fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        // 国内网银导出的 CSV 多为 GBK
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

pub(crate) fn cell_text(c: &Data) -> String {
    match c {
        Data::Empty => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.trim().to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 { format!("{}", *f as i64) } else { format!("{f}") }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => d.as_datetime().map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
        Data::Error(_) => String::new(),
    }
}

pub(crate) fn read_rows(path: &Path, file_name: &str) -> AppResult<Vec<Vec<String>>> {
    let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "xls" | "xlsx" | "xlsm" | "ods" => {
            let mut wb = open_workbook_auto(path).map_err(|e| AppError::bad(format!("无法读取 Excel 文件：{e}")))?;
            let range = wb
                .worksheet_range_at(0)
                .ok_or_else(|| AppError::bad("Excel 文件中没有工作表"))?
                .map_err(|e| AppError::bad(format!("无法读取工作表：{e}")))?;
            Ok(range.rows().map(|r| r.iter().map(cell_text).collect()).collect())
        }
        "csv" | "txt" => {
            let text = decode_text(&std::fs::read(path)?);
            let mut reader = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(text.as_bytes());
            let mut rows = Vec::new();
            for rec in reader.records() {
                let rec = rec.map_err(|e| AppError::bad(format!("CSV 解析失败：{e}")))?;
                // 去掉制表符与「="…"」防科学计数法写法
                rows.push(rec.iter().map(|s| s.trim().trim_start_matches('=').trim_matches('"').trim_matches('\t').trim().to_string()).collect());
            }
            Ok(rows)
        }
        _ => Err(AppError::bad("银行流水仅支持 CSV、XLS、XLSX 文件")),
    }
}

// ── 表头识别 ──────────────────────────────────────────

#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub date: Option<String>,
    pub time: Option<String>,
    pub inflow: Option<String>,
    pub outflow: Option<String>,
    pub amount: Option<String>,
    pub direction: Option<String>,
    pub counterparty: Option<String>,
    pub summary: Option<String>,
    pub serial_no: Option<String>,
    pub account_no: Option<String>,
}

#[derive(Default, Clone, Copy)]
struct Cols {
    date: Option<usize>,
    time: Option<usize>,
    inflow: Option<usize>,
    outflow: Option<usize>,
    amount: Option<usize>,
    direction: Option<usize>,
    counterparty: Option<usize>,
    summary: Option<usize>,
    serial_no: Option<usize>,
    account_no: Option<usize>,
}

fn norm(h: &str) -> String {
    h.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 按关键字优先级找列：先找完全相等，再找包含；exclude 中任一词出现则跳过
fn find(headers: &[String], used: &HashSet<usize>, keys: &[&str], exclude: &[&str]) -> Option<usize> {
    let ok = |i: usize, h: &String| !used.contains(&i) && !h.is_empty() && !exclude.iter().any(|x| h.contains(x));
    for k in keys {
        if let Some(i) = headers.iter().enumerate().position(|(i, h)| ok(i, h) && h == k) {
            return Some(i);
        }
    }
    for k in keys {
        if let Some(i) = headers.iter().enumerate().position(|(i, h)| ok(i, h) && h.contains(k)) {
            return Some(i);
        }
    }
    None
}

fn detect_cols(raw: &[String]) -> Cols {
    let headers: Vec<String> = raw.iter().map(|h| norm(h)).collect();
    let mut used = HashSet::new();
    let take = |keys: &[&str], exclude: &[&str], used: &mut HashSet<usize>| {
        let r = find(&headers, used, keys, exclude);
        if let Some(i) = r {
            used.insert(i);
        }
        r
    };
    let mut c = Cols::default();
    c.inflow = take(&["收入金额", "贷方发生额", "贷方金额", "存入金额", "收款金额", "入账金额", "收入", "贷方", "存入"], &["余额", "笔数"], &mut used);
    c.outflow = take(&["支出金额", "借方发生额", "借方金额", "取出金额", "付款金额", "支出", "借方", "取出"], &["余额", "笔数"], &mut used);
    c.amount = take(&["交易金额", "发生额", "金额"], &["余额", "笔数"], &mut used);
    c.direction = take(&["借贷标志", "借贷方向", "收支方向", "收付方向", "借贷", "收支", "方向"], &[], &mut used);
    c.counterparty = take(
        &["对方户名", "对方账户名称", "对方账户名", "对方名称", "对方单位", "收付款人名称", "付款人名称", "付款方名称", "对方"],
        &["账号", "开户", "行名", "银行"],
        &mut used,
    );
    c.serial_no = take(&["交易流水号", "银行流水号", "流水号", "交易序号", "凭证号", "交易号", "业务编号"], &[], &mut used);
    c.summary = take(&["摘要", "用途", "附言", "交易摘要", "备注", "交易类型"], &[], &mut used);
    c.account_no = take(&["本方账号", "本方账户", "账号"], &["对方"], &mut used);
    c.date = take(&["交易日期", "记账日期", "入账日期", "发生日期", "日期"], &[], &mut used);
    c.time = take(&["交易时间", "记账时间", "入账时间", "时间"], &[], &mut used);
    c
}

fn score(c: &Cols) -> usize {
    let has_time = c.date.is_some() || c.time.is_some();
    let has_amount = c.inflow.is_some() || c.amount.is_some();
    if !has_time || !has_amount {
        return 0;
    }
    [c.date, c.time, c.inflow, c.outflow, c.amount, c.direction, c.counterparty, c.summary, c.serial_no, c.account_no]
        .iter()
        .filter(|v| v.is_some())
        .count()
}

// ── 值解析 ────────────────────────────────────────────

pub(crate) fn parse_amount(s: &str) -> Option<f64> {
    let t: String = s.chars().filter(|c| !matches!(c, ',' | '，' | '¥' | '￥' | ' ' | '元' | '\u{a0}')).collect();
    if t.is_empty() || t == "-" || t == "--" {
        return None;
    }
    let (neg, t) = match t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(inner) => (true, inner.to_string()),
        None => (false, t),
    };
    let v: f64 = t.trim_start_matches('+').parse().ok()?;
    Some(if neg { -v } else { v })
}

pub(crate) fn parse_date_time(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Excel 日期序列号（单元格未设日期格式时）
    if let Ok(serial) = s.parse::<f64>()
        && (20000.0..80000.0).contains(&serial)
    {
        let base = NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
        return Some(base + Duration::seconds((serial * 86400.0).round() as i64));
    }
    let t: String = s.replace(['/', '.', '年', '月'], "-").replace('日', " ").replace('T', " ");
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y%m%d %H:%M:%S", "%Y%m%d %H%M%S", "%Y%m%d%H%M%S"] {
        if let Ok(v) = NaiveDateTime::parse_from_str(&t, f) {
            return Some(v);
        }
    }
    // 带毫秒
    if let Some((head, _)) = t.split_once(|c| c == '.' || c == ',')
        && let Ok(v) = NaiveDateTime::parse_from_str(head, "%Y-%m-%d %H:%M:%S")
    {
        return Some(v);
    }
    for f in ["%Y-%m-%d", "%Y%m%d"] {
        if let Ok(v) = NaiveDate::parse_from_str(&t, f) {
            return v.and_hms_opt(0, 0, 0);
        }
    }
    None
}

/// 只有时分秒的「交易时间」列
fn parse_time_only(s: &str) -> Option<(u32, u32, u32)> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    let parts: Vec<u32> = if s.contains(':') {
        s.trim().split(':').filter_map(|p| p.trim().parse().ok()).collect()
    } else if digits.len() == 6 {
        vec![digits[0..2].parse().ok()?, digits[2..4].parse().ok()?, digits[4..6].parse().ok()?]
    } else {
        return None;
    };
    match parts.as_slice() {
        [h, m] if *h < 24 && *m < 60 => Some((*h, *m, 0)),
        [h, m, sec] if *h < 24 && *m < 60 && *sec < 60 => Some((*h, *m, *sec)),
        _ => None,
    }
}

fn row_time(row: &[String], c: &Cols) -> Option<NaiveDateTime> {
    let get = |i: Option<usize>| i.and_then(|i| row.get(i)).map(String::as_str).unwrap_or("");
    let (d, t) = (get(c.date), get(c.time));
    // 时间列本身就是完整日期时间
    if let Some(v) = parse_date_time(t).filter(|_| !t.is_empty() && parse_time_only(t).is_none()) {
        return Some(v);
    }
    let date = parse_date_time(d)?;
    match parse_time_only(t) {
        Some((h, m, s)) => date.date().and_hms_opt(h, m, s),
        None => Some(date),
    }
}

fn is_inflow_flag(s: &str) -> Option<bool> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if ["贷", "收", "入", "存"].iter().any(|k| s.contains(k)) || s.eq_ignore_ascii_case("C") || s.eq_ignore_ascii_case("CR") {
        return Some(true);
    }
    if ["借", "支", "出", "付", "取"].iter().any(|k| s.contains(k)) || s.eq_ignore_ascii_case("D") || s.eq_ignore_ascii_case("DR") {
        return Some(false);
    }
    None
}

fn row_amount(row: &[String], c: &Cols) -> Option<f64> {
    let get = |i: Option<usize>| i.and_then(|i| row.get(i)).map(String::as_str).unwrap_or("");
    if c.inflow.is_some() || c.outflow.is_some() {
        let inflow = parse_amount(get(c.inflow)).unwrap_or(0.0);
        let outflow = parse_amount(get(c.outflow)).unwrap_or(0.0);
        if inflow != 0.0 {
            return Some(inflow.abs());
        }
        if outflow != 0.0 {
            return Some(-outflow.abs());
        }
        if c.amount.is_none() {
            return None;
        }
    }
    let v = parse_amount(get(c.amount))?;
    match is_inflow_flag(get(c.direction)) {
        Some(true) => Some(v.abs()),
        Some(false) => Some(-v.abs()),
        None => Some(v),
    }
}

// ── 导入 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BankImportResult {
    pub batch_id: String,
    pub file_name: String,
    pub entity: String,
    /// 表头所在行（1 起）
    pub header_row: usize,
    pub mapping: Mapping,
    /// 识别为交易的行数
    pub parsed_rows: i64,
    pub inserted_rows: i64,
    /// 以前导入过（重复）
    pub skipped_rows: i64,
    /// 无日期或无金额（合计行、空行等）被忽略
    pub ignored_rows: i64,
    pub inflow_count: i64,
    pub inflow_amount: f64,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
}

pub fn import(conn: &Connection, path: &Path, file_name: &str, entity: &str, account_no: Option<&str>) -> AppResult<BankImportResult> {
    let entity = entity.trim();
    if entity.is_empty() {
        return Err(AppError::bad("请选择银行流水所属的商户主体"));
    }
    let rows = read_rows(path, file_name)?;
    let (header_idx, cols) = rows
        .iter()
        .take(HEADER_SCAN_ROWS)
        .enumerate()
        .map(|(i, r)| (i, detect_cols(r)))
        .max_by_key(|(i, c)| (score(c), std::cmp::Reverse(*i)))
        .filter(|(_, c)| score(c) > 0)
        .ok_or_else(|| {
            let sample = rows.iter().take(5).map(|r| r.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" | ")).collect::<Vec<_>>().join("；");
            AppError::bad(format!("没有识别到表头（需要包含交易日期/时间和收入/金额列）。文件前几行：{sample}"))
        })?;
    let headers = &rows[header_idx];
    let name_of = |i: Option<usize>| i.and_then(|i| headers.get(i)).cloned();
    let mapping = Mapping {
        date: name_of(cols.date),
        time: name_of(cols.time),
        inflow: name_of(cols.inflow),
        outflow: name_of(cols.outflow),
        amount: name_of(cols.amount),
        direction: name_of(cols.direction),
        counterparty: name_of(cols.counterparty),
        summary: name_of(cols.summary),
        serial_no: name_of(cols.serial_no),
        account_no: name_of(cols.account_no),
    };

    let batch_id = format!("bank-{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    let get = |row: &Vec<String>, i: Option<usize>| i.and_then(|i| row.get(i)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut ignored = 0i64;
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut txns = Vec::new();
    for row in rows.iter().skip(header_idx + 1) {
        let (Some(time), Some(amount)) = (row_time(row, &cols), row_amount(row, &cols)) else {
            if row.iter().any(|s| !s.is_empty()) {
                ignored += 1;
            }
            continue;
        };
        if amount == 0.0 {
            ignored += 1;
            continue;
        }
        let amount = (amount * 100.0).round() / 100.0;
        let account = get(row, cols.account_no).or_else(|| account_no.map(str::trim).filter(|s| !s.is_empty()).map(String::from));
        let time = time.format("%Y-%m-%d %H:%M:%S").to_string();
        let counterparty = get(row, cols.counterparty);
        let summary = get(row, cols.summary);
        let serial = get(row, cols.serial_no);
        // 重复导入同一份流水按 txn_id 去重：有流水号用流水号，否则用交易要素 + 文件内出现次序
        let base = match &serial {
            Some(s) => format!("{entity}|{}|S:{s}", account.as_deref().unwrap_or("")),
            None => format!(
                "{entity}|{}|{time}|{amount:.2}|{}|{}",
                account.as_deref().unwrap_or(""),
                counterparty.as_deref().unwrap_or(""),
                summary.as_deref().unwrap_or("")
            ),
        };
        let n = seen.entry(base.clone()).or_insert(0);
        *n += 1;
        let txn_id = if *n == 1 { base } else { format!("{base}#{n}") };
        txns.push((txn_id, account, time, amount, counterparty, summary, serial));
    }
    if txns.is_empty() {
        return Err(AppError::bad("文件中没有识别到交易记录（每行需要有日期和非零金额）"));
    }

    conn.execute_batch("BEGIN TRANSACTION")?;
    let written = (|| -> anyhow::Result<i64> {
        let mut stmt = conn.prepare(
            "INSERT INTO ledger_bank_txn (txn_id, entity, account_no, txn_time, amount, counterparty, summary, serial_no, batch_id, file_name, imported_at)
             VALUES (?, ?, ?, ?, CAST(? AS DECIMAL(18,2)), ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP)) ON CONFLICT DO NOTHING",
        )?;
        let mut inserted = 0i64;
        let now = now_local();
        for (id, account, time, amount, cp, summary, serial) in &txns {
            inserted += stmt.execute(params![id, entity, account, time, amount, cp, summary, serial, batch_id, file_name, now])? as i64;
        }
        Ok(inserted)
    })();
    let inserted = match written {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            v
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e.into());
        }
    };
    let inflows: Vec<f64> = txns.iter().map(|t| t.3).filter(|a| *a > 0.0).collect();
    Ok(BankImportResult {
        batch_id,
        file_name: file_name.to_string(),
        entity: entity.to_string(),
        header_row: header_idx + 1,
        mapping,
        parsed_rows: txns.len() as i64,
        inserted_rows: inserted,
        skipped_rows: txns.len() as i64 - inserted,
        ignored_rows: ignored,
        inflow_count: inflows.len() as i64,
        inflow_amount: (inflows.iter().sum::<f64>() * 100.0).round() / 100.0,
        time_from: txns.iter().map(|t| t.2.clone()).min(),
        time_to: txns.iter().map(|t| t.2.clone()).max(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BankBatch {
    pub batch_id: String,
    pub entity: String,
    pub file_name: Option<String>,
    pub row_count: i64,
    pub time_from: String,
    pub time_to: String,
    pub matched: i64,
    pub imported_at: String,
}

pub fn batches(conn: &Connection) -> AppResult<Vec<BankBatch>> {
    let mut stmt = conn.prepare(
        "SELECT b.batch_id, any_value(b.entity), any_value(b.file_name), count(*), min(b.txn_time), max(b.txn_time),
                count(m.txn_id), strftime(min(b.imported_at), '%Y-%m-%d %H:%M:%S')
         FROM ledger_bank_txn b LEFT JOIN ledger_withdraw_match m ON m.txn_id = b.txn_id
         GROUP BY b.batch_id ORDER BY min(b.imported_at) DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BankBatch {
                batch_id: r.get(0)?,
                entity: r.get(1)?,
                file_name: r.get(2)?,
                row_count: r.get(3)?,
                time_from: r.get(4)?,
                time_to: r.get(5)?,
                matched: r.get(6)?,
                imported_at: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 删除一批银行流水（导错主体时使用）；其中已配对到锁账月份的不能删
pub fn delete_batch(conn: &Connection, batch_id: &str) -> AppResult<i64> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT m.shop, d.period FROM ledger_withdraw_match m
         JOIN ledger_bank_txn b ON b.txn_id = m.txn_id
         JOIN ledger_detail d ON d.shop = m.shop AND d.period = m.period AND d.{serial} = m.serial_no
         WHERE b.batch_id = ?"
            .replace("{serial}", &ident("动帐流水号"))
            .as_str(),
    )?;
    let used = stmt.query_map(params![batch_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    for (shop, period) in &used {
        close::ensure_editable(conn, shop, period)?;
    }
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> anyhow::Result<i64> {
        conn.execute("DELETE FROM ledger_withdraw_match WHERE txn_id IN (SELECT txn_id FROM ledger_bank_txn WHERE batch_id = ?)", params![batch_id])?;
        Ok(conn.execute("DELETE FROM ledger_bank_txn WHERE batch_id = ?", params![batch_id])? as i64)
    })();
    match r {
        Ok(n) => {
            conn.execute_batch("COMMIT")?;
            if n == 0 {
                return Err(AppError::bad("该批银行流水不存在"));
            }
            Ok(n)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e.into())
        }
    }
}

// ── 对账 ──────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BankTxn {
    pub txn_id: String,
    pub entity: String,
    pub account_no: Option<String>,
    pub time: String,
    pub amount: f64,
    pub counterparty: Option<String>,
    pub summary: Option<String>,
    pub serial_no: Option<String>,
    /// 对方户名 / 摘要像平台打款
    pub platform_like: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawRow {
    pub shop: String,
    pub period: String,
    pub serial_no: String,
    pub time: String,
    pub amount: f64,
    pub entity: String,
    pub remark: Option<String>,
    /// matched 已到账 / in_transit 在途 / no_bank 银行流水未覆盖 / overdue 逾期未到账
    pub status: String,
    pub method: Option<String>,
    pub matched_by: Option<String>,
    pub txn: Option<BankTxn>,
    /// 到账日 - 提现日（天）
    pub lag_days: Option<i64>,
    /// 到账金额 - 提现金额（手工配对可能有手续费差异）
    pub diff: Option<f64>,
    /// 到账截止日（提现日 + 窗口）
    pub due_date: String,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawStats {
    pub count: i64,
    pub amount: f64,
    pub matched: i64,
    pub matched_amount: f64,
    pub in_transit: i64,
    pub in_transit_amount: f64,
    pub no_bank: i64,
    pub no_bank_amount: f64,
    pub overdue: i64,
    pub overdue_amount: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BankCoverage {
    pub entity: String,
    pub row_count: i64,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawView {
    pub period: String,
    pub today: String,
    pub window_days: i64,
    pub rows: Vec<WithdrawRow>,
    pub stats: WithdrawStats,
    /// 涉及主体的银行流水覆盖范围
    pub banks: Vec<BankCoverage>,
    /// 到账窗口内、未配对、像平台打款的银行收入（银行有、平台无）
    pub unmatched_bank: Vec<BankTxn>,
}

fn platform_like(cp: &Option<String>, summary: &Option<String>) -> bool {
    [cp, summary].iter().filter_map(|v| v.as_deref()).any(|s| PLATFORM_KEYWORDS.iter().any(|k| s.contains(k)))
}

fn date_of(time: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(time.get(0..10)?, "%Y-%m-%d").ok()
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn withdraw_where() -> String {
    format!("{} LIKE '%提现%' AND {} = '出账'", ident("动账场景"), ident("动账方向"))
}

fn period_range(period: &str) -> AppResult<(NaiveDate, NaiveDate)> {
    let start = NaiveDate::parse_from_str(&format!("{period}-01"), "%Y-%m-%d").map_err(|_| AppError::bad(format!("月份格式应为 YYYY-MM：{period}")))?;
    let next = start.checked_add_months(chrono::Months::new(1)).ok_or_else(|| AppError::bad("月份超出范围"))?;
    Ok((start, next - Duration::days(1)))
}


fn read_txn(r: &duckdb::Row, base: usize) -> duckdb::Result<Option<BankTxn>> {
    let id: Option<String> = r.get(base)?;
    let Some(txn_id) = id else { return Ok(None) };
    let counterparty: Option<String> = r.get(base + 5)?;
    let summary: Option<String> = r.get(base + 6)?;
    Ok(Some(BankTxn {
        txn_id,
        entity: r.get(base + 1)?,
        account_no: r.get(base + 2)?,
        time: r.get(base + 3)?,
        amount: r.get(base + 4)?,
        platform_like: platform_like(&counterparty, &summary),
        counterparty,
        summary,
        serial_no: r.get(base + 7)?,
    }))
}

const TXN_COLS: &str = "b.txn_id, b.entity, b.account_no, b.txn_time, b.amount::DOUBLE, b.counterparty, b.summary, b.serial_no";

/// 某月提现到账情况；shop 为空表示全部店铺
pub fn view(conn: &Connection, period: &str, shop: Option<&str>) -> AppResult<WithdrawView> {
    let (start, end) = period_range(period)?;
    let today = today_local();
    let (serial, time, amt, entity, remark) = (ident("动帐流水号"), ident("动账时间"), ident("动账金额"), ident("商户主体名称"), ident("备注"));
    let sql = format!(
        "SELECT d.shop, d.{serial}, d.{time}, d.{amt}::DOUBLE, d.{entity}, d.{remark}, m.method, m.matched_by, {TXN_COLS}
         FROM ledger_detail d
         LEFT JOIN ledger_withdraw_match m ON m.shop = d.shop AND m.period = d.period AND m.serial_no = d.{serial}
         LEFT JOIN ledger_bank_txn b ON b.txn_id = m.txn_id
         WHERE d.period = $1 AND {w} AND (CAST($2 AS VARCHAR) IS NULL OR d.shop = $2)
         ORDER BY d.{time}",
        w = withdraw_where()
    );
    let mut stmt = conn.prepare(&sql)?;
    let raw = stmt
        .query_map(params![period, shop], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                read_txn(r, 8)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let entities: Vec<String> = raw.iter().map(|r| r.4.clone()).collect::<HashSet<_>>().into_iter().collect();
    let banks = bank_coverage(conn, &entities)?;
    let cover: HashMap<&str, Option<NaiveDate>> = banks.iter().map(|b| (b.entity.as_str(), b.time_to.as_deref().and_then(date_of))).collect();

    let mut stats = WithdrawStats::default();
    let mut rows = Vec::with_capacity(raw.len());
    for (shop, serial_no, time, amount, entity, remark, method, matched_by, txn) in raw {
        let w_date = date_of(&time).unwrap_or(start);
        let due = w_date + Duration::days(WINDOW_DAYS);
        let status = if txn.is_some() {
            "matched"
        } else if today <= due {
            "in_transit"
        } else if !cover.get(entity.as_str()).copied().flatten().is_some_and(|to| to >= due) {
            // 银行流水没覆盖到到账截止日，无法判断钱到没到
            "no_bank"
        } else {
            "overdue"
        };
        stats.count += 1;
        stats.amount += amount;
        let (n, a) = match status {
            "matched" => (&mut stats.matched, &mut stats.matched_amount),
            "in_transit" => (&mut stats.in_transit, &mut stats.in_transit_amount),
            "no_bank" => (&mut stats.no_bank, &mut stats.no_bank_amount),
            _ => (&mut stats.overdue, &mut stats.overdue_amount),
        };
        *n += 1;
        *a += amount;
        rows.push(WithdrawRow {
            lag_days: txn.as_ref().and_then(|t| Some((date_of(&t.time)? - w_date).num_days())),
            diff: txn.as_ref().map(|t| round2(t.amount - amount)),
            due_date: due.to_string(),
            shop,
            period: period.to_string(),
            serial_no,
            time,
            amount,
            entity,
            remark,
            status: status.into(),
            method,
            matched_by,
            txn,
        });
    }
    for v in [
        &mut stats.amount,
        &mut stats.matched_amount,
        &mut stats.in_transit_amount,
        &mut stats.no_bank_amount,
        &mut stats.overdue_amount,
    ] {
        *v = round2(*v);
    }

    let unmatched_bank = unmatched_inflows(conn, &entities, start, end + Duration::days(WINDOW_DAYS))?
        .into_iter()
        .filter(|t| t.platform_like)
        .collect();
    Ok(WithdrawView { period: period.to_string(), today: today.to_string(), window_days: WINDOW_DAYS, rows, stats, banks, unmatched_bank })
}

fn bank_coverage(conn: &Connection, entities: &[String]) -> AppResult<Vec<BankCoverage>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare("SELECT count(*), min(txn_time), max(txn_time) FROM ledger_bank_txn WHERE entity = ?")?;
    let mut sorted = entities.to_vec();
    sorted.sort();
    for e in sorted {
        let (row_count, time_from, time_to) = stmt.query_row(params![e], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        out.push(BankCoverage { entity: e, row_count, time_from, time_to });
    }
    Ok(out)
}

fn unmatched_inflows(conn: &Connection, entities: &[String], from: NaiveDate, to: NaiveDate) -> AppResult<Vec<BankTxn>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {TXN_COLS} FROM ledger_bank_txn b
         WHERE b.entity = ? AND b.amount > 0 AND b.txn_time >= ? AND b.txn_time < ?
           AND NOT EXISTS (SELECT 1 FROM ledger_withdraw_match m WHERE m.txn_id = b.txn_id)
         ORDER BY b.txn_time"
    ))?;
    let mut out = Vec::new();
    let to_exclusive = (to + Duration::days(1)).to_string();
    for e in entities {
        let rows = stmt.query_map(params![e, from.to_string(), to_exclusive], |r| read_txn(r, 0))?.collect::<Result<Vec<_>, _>>()?;
        out.extend(rows.into_iter().flatten());
    }
    out.sort_by(|a, b| a.time.cmp(&b.time));
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoMatchResult {
    pub matched: i64,
    pub unmatched: i64,
}

/// 自动配对某月未配对的提现：同主体、金额相等、到账日在提现后 0~3 天，取最早到账的一笔；已锁账的店铺月份跳过
pub fn auto_match(conn: &Connection, period: &str, operator: &str) -> AppResult<AutoMatchResult> {
    let (start, end) = period_range(period)?;
    let (serial, time, amt, entity) = (ident("动帐流水号"), ident("动账时间"), ident("动账金额"), ident("商户主体名称"));
    let mut stmt = conn.prepare(&format!(
        "SELECT d.shop, d.{serial}, d.{time}, d.{amt}::DOUBLE, d.{entity} FROM ledger_detail d
         WHERE d.period = ? AND {w}
           AND NOT EXISTS (SELECT 1 FROM ledger_withdraw_match m WHERE m.shop = d.shop AND m.period = d.period AND m.serial_no = d.{serial})
         ORDER BY d.{time}",
        w = withdraw_where()
    ))?;
    let pending = stmt
        .query_map(params![period], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?, r.get::<_, String>(4)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let locked = close::locked_shops(conn, period)?;
    let pending: Vec<_> = pending.into_iter().filter(|p| !locked.contains(&p.0)).collect();
    let entities: Vec<String> = pending.iter().map(|p| p.4.clone()).collect::<HashSet<_>>().into_iter().collect();
    let candidates = unmatched_inflows(conn, &entities, start, end + Duration::days(WINDOW_DAYS))?;

    let mut used = HashSet::new();
    let mut pairs = Vec::new();
    for (shop, serial_no, time, amount, entity) in &pending {
        let Some(w_date) = date_of(time) else { continue };
        let best = candidates
            .iter()
            .filter(|t| !used.contains(&t.txn_id) && &t.entity == entity && (t.amount - amount).abs() < 0.005)
            .filter_map(|t| {
                let lag = (date_of(&t.time)? - w_date).num_days();
                (0..=WINDOW_DAYS).contains(&lag).then_some((lag, t))
            })
            .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.time.cmp(&b.1.time)));
        if let Some((_, t)) = best {
            used.insert(t.txn_id.clone());
            pairs.push((shop.clone(), serial_no.clone(), t.txn_id.clone()));
        }
    }

    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> anyhow::Result<()> {
        let mut ins = conn.prepare(
            "INSERT INTO ledger_withdraw_match (shop, period, serial_no, txn_id, method, matched_by, matched_at)
             VALUES (?, ?, ?, ?, 'auto', ?, CAST(? AS TIMESTAMP))",
        )?;
        let now = now_local();
        for (shop, serial_no, txn_id) in &pairs {
            ins.execute(params![shop, period, serial_no, txn_id, operator, now])?;
        }
        Ok(())
    })();
    match r {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e.into());
        }
    }
    Ok(AutoMatchResult { matched: pairs.len() as i64, unmatched: (pending.len() - pairs.len()) as i64 })
}

/// 手工配对的候选：同主体、提现前 1 天 ~ 后 10 天的未配对银行收入，金额最接近的在前
pub fn candidates(conn: &Connection, shop: &str, period: &str, serial_no: &str) -> AppResult<Vec<BankTxn>> {
    let (time, amount, entity) = withdrawal(conn, shop, period, serial_no)?;
    let d = date_of(&time).ok_or_else(|| AppError::bad("提现时间格式不正确"))?;
    let mut list = unmatched_inflows(conn, &[entity], d - Duration::days(1), d + Duration::days(10))?;
    list.sort_by(|a, b| (a.amount - amount).abs().total_cmp(&(b.amount - amount).abs()).then_with(|| a.time.cmp(&b.time)));
    list.truncate(50);
    Ok(list)
}

/// (time, amount, entity)
fn withdrawal(conn: &Connection, shop: &str, period: &str, serial_no: &str) -> AppResult<(String, f64, String)> {
    let sql = format!(
        "SELECT {time}, {amt}::DOUBLE, {entity} FROM ledger_detail WHERE shop = ? AND period = ? AND {serial} = ? AND {w} LIMIT 1",
        time = ident("动账时间"),
        amt = ident("动账金额"),
        entity = ident("商户主体名称"),
        serial = ident("动帐流水号"),
        w = withdraw_where()
    );
    conn.query_row(&sql, params![shop, period, serial_no], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|_| AppError::bad(format!("没有找到「{shop}」的提现流水 {serial_no}")))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchInput {
    pub shop: String,
    pub period: String,
    pub serial_no: String,
    /// 为空表示取消配对
    pub txn_id: Option<String>,
    #[serde(default)]
    pub operator: String,
}

pub fn manual_match(conn: &Connection, input: &MatchInput) -> AppResult<()> {
    let (_, _, entity) = withdrawal(conn, &input.shop, &input.period, &input.serial_no)?;
    close::ensure_editable(conn, &input.shop, &input.period)?;
    let Some(txn_id) = input.txn_id.as_deref().filter(|s| !s.is_empty()) else {
        conn.execute(
            "DELETE FROM ledger_withdraw_match WHERE shop = ? AND period = ? AND serial_no = ?",
            params![input.shop, input.period, input.serial_no],
        )?;
        return Ok(());
    };
    let (txn_entity, txn_amount): (String, f64) = conn
        .query_row("SELECT entity, amount::DOUBLE FROM ledger_bank_txn WHERE txn_id = ?", params![txn_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| AppError::bad("银行流水不存在"))?;
    if txn_entity != entity {
        return Err(AppError::bad(format!("银行流水属于「{txn_entity}」，与提现主体「{entity}」不一致")));
    }
    if txn_amount <= 0.0 {
        return Err(AppError::bad("只能配对银行收入"));
    }
    let taken: i64 = conn.query_row(
        "SELECT count(*) FROM ledger_withdraw_match WHERE txn_id = ? AND NOT (shop = ? AND period = ? AND serial_no = ?)",
        params![txn_id, input.shop, input.period, input.serial_no],
        |r| r.get(0),
    )?;
    if taken > 0 {
        return Err(AppError::bad("该笔银行流水已配对其他提现"));
    }
    conn.execute(
            "DELETE FROM ledger_withdraw_match WHERE shop = ? AND period = ? AND serial_no = ?",
            params![input.shop, input.period, input.serial_no],
        )?;
    conn.execute(
        "INSERT INTO ledger_withdraw_match (shop, period, serial_no, txn_id, method, matched_by, matched_at)
         VALUES (?, ?, ?, ?, 'manual', ?, CAST(? AS TIMESTAMP))",
        params![input.shop, input.period, input.serial_no, txn_id, input.operator, now_local()],
    )?;
    Ok(())
}
