//! 表格文件与取值的通用工具：读 CSV / Excel（自动识别 UTF-8 / GBK）、解析金额与日期、DuckDB 取值转 JSON
//!
//! 核心功能（数据集导入、网址导入、定时推送）和各个场景包共用，所以放在这里，不属于任何场景包。

use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto};
use chrono::{Duration, NaiveDate, NaiveDateTime};
use duckdb::types::ValueRef;
use serde_json::Value as Json;

use crate::error::{AppError, AppResult};

pub fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        // 国内网银导出的 CSV 多为 GBK
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

fn cell_text(c: &Data) -> String {
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

pub fn read_rows(path: &Path, file_name: &str) -> AppResult<Vec<Vec<String>>> {
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
        _ => Err(AppError::bad("仅支持 CSV、XLS、XLSX 文件")),
    }
}

pub fn parse_amount(s: &str) -> Option<f64> {
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

pub fn parse_date_time(s: &str) -> Option<NaiveDateTime> {
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
    let t: String = s.replace(['/', '.', '年', '月'], "-").replace(['日', 'T'], " ");
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y%m%d %H:%M:%S", "%Y%m%d %H%M%S", "%Y%m%d%H%M%S"] {
        if let Ok(v) = NaiveDateTime::parse_from_str(&t, f) {
            return Some(v);
        }
    }
    // 带毫秒
    if let Some((head, _)) = t.split_once(['.', ','])
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

/// 非 UTF-8 文件按 GBK 解码后重写（部分导出工具/Excel 另存为生成 GBK 编码的 CSV）
pub fn ensure_utf8(path: &Path) -> AppResult<()> {
    let bytes = std::fs::read(path)?;
    if std::str::from_utf8(&bytes).is_ok() {
        return Ok(());
    }
    let (text, _, had_errors) = encoding_rs::GBK.decode(&bytes);
    if had_errors {
        return Err(AppError::bad("文件编码无法识别，请导出为 UTF-8 或 GBK 编码的 CSV"));
    }
    std::fs::write(path, text.as_bytes())?;
    Ok(())
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

pub fn to_json(v: ValueRef<'_>) -> Json {
    match v {
        ValueRef::Null => Json::Null,
        ValueRef::BigInt(i) => Json::from(i),
        ValueRef::Int(i) => Json::from(i),
        ValueRef::Double(f) => Json::from(round2(f)),
        ValueRef::Text(bytes) => Json::String(String::from_utf8_lossy(bytes).into_owned()),
        // DECIMAL(18,2) 金额列：按十进制文本转数值，不经过二进制浮点运算
        ValueRef::Decimal(d) => d.to_string().parse::<f64>().map(Json::from).unwrap_or(Json::Null),
        other => Json::String(format!("{other:?}")),
    }
}
