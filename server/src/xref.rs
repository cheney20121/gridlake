//! 跨工作簿引用（IMPORTRANGE）：从其他工作簿已保存的最新版本里取一块区域的值
//!
//! - 工作簿按 ID 或名称指定（名称重复时要求改用 ID）；区域写法 `Sheet1!A1:C10`、`'销售 明细'!A:C`（整列取到有数据的最后一行）、
//!   `A1:C10`（第一个工作表）或命名区域
//! - 取的是单元格保存的值（公式取保存时的计算结果），与 Excel 打开外部链接时的缓存值一致
//! - 解析后的工作簿按「ID + 版本」缓存在内存里，同一工作簿的多个引用不重复解压解析

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};

use duckdb::{Connection, params};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::error::{AppError, AppResult};

/// 单个区域最多的格子数
const MAX_CELLS: usize = 50_000;
/// 缓存的工作簿个数
const CACHE_SIZE: usize = 6;

#[derive(Deserialize)]
pub struct RangeRequest {
    /// 工作簿 ID 或名称
    pub workbook: String,
    /// 区域或命名区域
    pub range: String,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RangeResult {
    pub workbook_id: Option<String>,
    pub workbook_name: Option<String>,
    pub version: Option<i64>,
    pub values: Vec<Vec<Json>>,
    pub error: Option<String>,
}

type Snapshot = Arc<Json>;

fn cache() -> &'static Mutex<HashMap<String, (i64, Snapshot)>> {
    static C: OnceLock<Mutex<HashMap<String, (i64, Snapshot)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 按 ID 或名称找工作簿（不含回收站）
fn resolve(conn: &Connection, key: &str) -> AppResult<(String, String, i64)> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AppError::bad("缺少工作簿名称"));
    }
    let mut stmt = conn.prepare("SELECT id, name, version FROM ledger_workbook WHERE deleted_at IS NULL AND (id = ? OR name = ?) ORDER BY id = ? DESC")?;
    let rows = stmt.query_map(params![key, key, key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    match rows.as_slice() {
        [] => Err(AppError::bad(format!("找不到工作簿「{key}」"))),
        [only] => Ok(only.clone()),
        [first, ..] if first.0 == key => Ok(first.clone()),
        many => Err(AppError::bad(format!("有 {} 个名为「{key}」的工作簿，请改用工作簿 ID（如 {}）", many.len(), many[0].0))),
    }
}

fn snapshot(conn: &Connection, id: &str, version: i64) -> AppResult<Snapshot> {
    if let Some((v, s)) = cache().lock().ok().and_then(|c| c.get(id).cloned())
        && v == version
    {
        return Ok(s);
    }
    let data: Vec<u8> = conn.query_row("SELECT data FROM ledger_workbook WHERE id = ?", params![id], |r| r.get(0))?;
    let mut json = Vec::new();
    GzDecoder::new(data.as_slice()).read_to_end(&mut json).map_err(|_| AppError::bad("工作簿内容已损坏"))?;
    let parsed: Snapshot = Arc::new(serde_json::from_slice(&json).map_err(|_| AppError::bad("工作簿内容已损坏"))?);
    if let Ok(mut c) = cache().lock() {
        if c.len() >= CACHE_SIZE && !c.contains_key(id) {
            let evict = c.keys().next().cloned();
            if let Some(k) = evict {
                c.remove(&k);
            }
        }
        c.insert(id.to_string(), (version, parsed.clone()));
    }
    Ok(parsed)
}

/// 列字母 → 序号（A=0）
fn col_index(s: &str) -> Option<usize> {
    if s.is_empty() || s.len() > 3 {
        return None;
    }
    s.bytes().try_fold(0usize, |n, b| b.is_ascii_alphabetic().then(|| n * 26 + (b.to_ascii_uppercase() - b'A') as usize + 1)).map(|n| n - 1)
}

/// 一端：列（可无）+ 行（可无），如 A1、A、1，允许 $
fn parse_end(s: &str) -> Option<(Option<usize>, Option<usize>)> {
    let s = s.replace('$', "");
    let split = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    let (letters, digits) = s.split_at(split);
    let col = if letters.is_empty() { None } else { Some(col_index(letters)?) };
    let row = if digits.is_empty() { None } else { Some(digits.parse::<usize>().ok().filter(|r| *r >= 1)? - 1) };
    if col.is_none() && row.is_none() {
        return None;
    }
    Some((col, row))
}

/// 拆出工作表名（可带引号）与区域
fn split_sheet(range: &str) -> (Option<String>, String) {
    match range.rfind('!') {
        Some(i) => {
            let sheet = range[..i].trim();
            let sheet = sheet.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).map(|s| s.replace("''", "'")).unwrap_or_else(|| sheet.to_string());
            (Some(sheet), range[i + 1..].trim().to_string())
        }
        None => (None, range.trim().to_string()),
    }
}

/// 命名区域 → 引用字符串
fn defined_name(snap: &Json, name: &str) -> Option<String> {
    let res = snap.get("resources")?.as_array()?.iter().find(|r| r.get("name").and_then(Json::as_str) == Some("SHEET_DEFINED_NAME_PLUGIN"))?;
    let data: Json = serde_json::from_str(res.get("data")?.as_str()?).ok()?;
    data.as_object()?
        .values()
        .find(|d| d.get("name").and_then(Json::as_str).is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .and_then(|d| d.get("formulaOrRefString").and_then(Json::as_str))
        .map(|s| s.trim_start_matches('=').to_string())
}

fn read_range(snap: &Json, range: &str) -> AppResult<Vec<Vec<Json>>> {
    let range = range.trim();
    if range.is_empty() {
        return Err(AppError::bad("缺少区域"));
    }
    // 不像单元格地址时按命名区域找
    let looks_like_ref = range.contains('!') || range.split(':').all(|p| parse_end(p).is_some());
    let resolved;
    let range = if looks_like_ref {
        range
    } else {
        resolved = defined_name(snap, range).ok_or_else(|| AppError::bad(format!("工作簿里没有命名区域「{range}」")))?;
        resolved.as_str()
    };
    let (sheet_name, addr) = split_sheet(range);
    let sheets = snap.get("sheets").and_then(Json::as_object).ok_or_else(|| AppError::bad("工作簿内容已损坏"))?;
    let sheet = match &sheet_name {
        Some(n) => sheets.values().find(|s| s.get("name").and_then(Json::as_str) == Some(n.as_str())).ok_or_else(|| AppError::bad(format!("没有工作表「{n}」")))?,
        None => {
            let first = snap.get("sheetOrder").and_then(Json::as_array).and_then(|o| o.first()).and_then(Json::as_str);
            first.and_then(|id| sheets.get(id)).or_else(|| sheets.values().next()).ok_or_else(|| AppError::bad("工作簿没有工作表"))?
        }
    };
    let cells = sheet.get("cellData").and_then(Json::as_object);
    let (a, b) = addr.split_once(':').unwrap_or((addr.as_str(), addr.as_str()));
    let (Some(start), Some(end)) = (parse_end(a), parse_end(b)) else {
        return Err(AppError::bad(format!("区域写法不正确：{addr}（应为 A1:C10、A:C 或命名区域）")));
    };
    // 整列 / 整行：取到有数据的最后一行 / 列
    let last_row = cells.and_then(|c| c.keys().filter_map(|k| k.parse::<usize>().ok()).max()).unwrap_or(0);
    let last_col = cells.map(|c| c.values().filter_map(|r| r.as_object()).flat_map(|r| r.keys().filter_map(|k| k.parse::<usize>().ok())).max().unwrap_or(0)).unwrap_or(0);
    let (c1, c2) = (start.0.unwrap_or(0), end.0.unwrap_or(last_col));
    let (r1, r2) = (start.1.unwrap_or(0), end.1.unwrap_or(last_row));
    let (r1, r2) = (r1.min(r2), r1.max(r2));
    let (c1, c2) = (c1.min(c2), c1.max(c2));
    if (r2 - r1 + 1) * (c2 - c1 + 1) > MAX_CELLS {
        return Err(AppError::bad(format!("区域太大（超过 {MAX_CELLS} 个格子），请缩小范围")));
    }
    Ok((r1..=r2)
        .map(|r| {
            let row = cells.and_then(|c| c.get(&r.to_string())).and_then(Json::as_object);
            (c1..=c2).map(|c| row.and_then(|x| x.get(&c.to_string())).and_then(|cell| cell.get("v")).cloned().unwrap_or(Json::Null)).collect()
        })
        .collect())
}

/// 批量取区域：每项单独成功 / 失败
pub fn ranges(conn: &Connection, items: Vec<RangeRequest>, user: &crate::auth::CurrentUser) -> AppResult<Vec<RangeResult>> {
    if items.len() > 200 {
        return Err(AppError::bad("一次最多取 200 个区域"));
    }
    Ok(items
        .into_iter()
        .map(|it| {
            let r = resolve(conn, &it.workbook).and_then(|(id, name, version)| {
                // 只能引用自己有权查看的工作簿
                crate::workbook::require(conn, &id, user, crate::auth::Need::View, false).map_err(|_| AppError::bad(format!("无权引用工作簿「{name}」")))?;
                let snap = snapshot(conn, &id, version)?;
                let values = read_range(&snap, &it.range)?;
                Ok(RangeResult { workbook_id: Some(id), workbook_name: Some(name), version: Some(version), values, error: None })
            });
            r.unwrap_or_else(|e| RangeResult { error: Some(e.message()), ..Default::default() })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap() -> Json {
        json!({
            "sheetOrder": ["s1", "s2"],
            "sheets": {
                "s1": { "name": "汇总", "cellData": { "0": { "0": { "v": "店铺" }, "1": { "v": "金额" } }, "1": { "0": { "v": "A店" }, "1": { "v": 12.5, "f": "=SUM(1)" } }, "2": { "0": { "v": "B店" } } } },
                "s2": { "name": "销售 明细", "cellData": { "3": { "2": { "v": 9 } } } }
            },
            "resources": [{ "name": "SHEET_DEFINED_NAME_PLUGIN", "data": "{\"n1\":{\"id\":\"n1\",\"name\":\"店铺金额\",\"formulaOrRefString\":\"汇总!$A$1:$B$2\"}}" }]
        })
    }

    #[test]
    fn reads_ranges() {
        let s = snap();
        assert_eq!(read_range(&s, "汇总!A1:B2").unwrap(), vec![vec![json!("店铺"), json!("金额")], vec![json!("A店"), json!(12.5)]]);
        assert_eq!(read_range(&s, "A2:A3").unwrap(), vec![vec![json!("A店")], vec![json!("B店")]]);
        assert_eq!(read_range(&s, "'销售 明细'!C4").unwrap(), vec![vec![json!(9)]]);
        assert_eq!(read_range(&s, "汇总!B:B").unwrap().len(), 3);
        assert_eq!(read_range(&s, "店铺金额").unwrap()[1][1], json!(12.5));
        assert!(read_range(&s, "没有的名字").is_err());
        assert!(read_range(&s, "缺失!A1").is_err());
        assert_eq!(col_index("AA"), Some(26));
    }
}
