//! 推送内容：读取已保存工作簿里某个仪表盘页的部件（切片器、指标卡、表格卡片、数据集图表），在服务端重新计算
//!
//! 计算规则与前端一致：切片器筛选同一页上含同名字段的数据集部件；日期切片器的相对时间（本月、近 30 天…）
//! 按发送当天计算；指标卡未按日期筛选时取最近一期并与上一期 / 上年同期比较。

use std::io::Read;

use chrono::{Datelike, Duration, NaiveDate};
use duckdb::Connection;
use serde::Serialize;
use serde_json::Value as Json;

use crate::dataset::{self, Filter, QuerySpec};
use crate::error::{AppError, AppResult};
use crate::pivot::{self, PivotField, PivotRequest, PivotResult, PivotSort, PivotValue};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KpiBlock {
    pub title: String,
    pub value: Option<f64>,
    pub money: bool,
    pub delta: Option<f64>,
    pub delta_label: String,
    pub period: Option<String>,
    pub up_is_good: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TableBlock {
    pub title: String,
    pub header: Vec<String>,
    /// 已格式化的单元格
    pub rows: Vec<Vec<String>>,
    pub total: Option<Vec<String>>,
    /// 原始数值（写入 Excel 附件）
    #[serde(skip)]
    pub raw: Vec<(String, Vec<Option<f64>>)>,
    /// 原始合计行
    #[serde(skip)]
    pub raw_total: Option<(String, Vec<Option<f64>>)>,
    /// 各值列的类型（percent / money / number / text）
    #[serde(skip)]
    pub kinds: Vec<String>,
}

/// 推送里数据表只带前几行（完整明细请在平台里看）
const DATATABLE_ROWS: i64 = 10;

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Block {
    Kpi(KpiBlock),
    Table(TableBlock),
    /// 无法计算的部件（如引用的数据集已删除）
    Error { title: String, message: String },
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReportContent {
    pub workbook_name: String,
    pub sheet_name: String,
    /// 生效的筛选说明（如「店铺：旗舰店」「下单时间：本月（2026-10-01 ~ 2026-10-31）」）
    pub filters: Vec<String>,
    pub blocks: Vec<Block>,
    pub generated_at: String,
}

// ── 读取工作簿里的部件 ──

struct Part {
    key: String,
    spec: Json,
    top: f64,
    left: f64,
}

fn gunzip(bytes: &[u8]) -> AppResult<Json> {
    let mut s = String::new();
    flate2::read::GzDecoder::new(bytes).read_to_string(&mut s).map_err(|_| AppError::bad("工作簿内容损坏"))?;
    Ok(serde_json::from_str(&s)?)
}

/// 工作簿中的工作表（ID、名称），供界面选择仪表盘页
pub fn sheets_of(conn: &Connection, workbook_id: &str) -> AppResult<Vec<(String, String, usize)>> {
    let (_, bytes) = crate::workbook::content(conn, workbook_id)?;
    let wb = gunzip(&bytes)?;
    let parts = drawing_parts(&wb);
    let order = wb.get("sheetOrder").and_then(Json::as_array).cloned().unwrap_or_default();
    Ok(order
        .iter()
        .filter_map(Json::as_str)
        .map(|id| {
            let name = wb.pointer(&format!("/sheets/{id}/name")).and_then(Json::as_str).unwrap_or(id).to_string();
            let n = parts.iter().filter(|(s, _)| s == id).count();
            (id.to_string(), name, n)
        })
        .collect())
}

/// 绘图资源里的浮动部件：(sheetId, Part)
fn drawing_parts(wb: &Json) -> Vec<(String, Part)> {
    let res = wb.get("resources").and_then(Json::as_array).and_then(|r| r.iter().find(|x| x.get("name").and_then(Json::as_str) == Some("SHEET_DRAWING_PLUGIN")));
    let Some(data) = res.and_then(|r| r.get("data")).and_then(Json::as_str).and_then(|d| serde_json::from_str::<Json>(d).ok()) else { return vec![] };
    let mut out = Vec::new();
    for (sheet_id, sheet) in data.as_object().into_iter().flatten() {
        for (_, d) in sheet.get("data").and_then(Json::as_object).into_iter().flatten() {
            let Some(key) = d.get("componentKey").and_then(Json::as_str) else { continue };
            if !key.starts_with("webexcel-") {
                continue;
            }
            let Some(spec) = d.get("data").cloned() else { continue };
            let t = d.get("transform");
            let num = |k: &str| t.and_then(|t| t.get(k)).and_then(Json::as_f64).unwrap_or(0.0);
            out.push((sheet_id.clone(), Part { key: key.to_string(), spec, top: num("top"), left: num("left") }));
        }
    }
    out
}

// ── 切片器 ──

fn ymd(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn month_start(y: i32, m: i32) -> NaiveDate {
    let (y, m) = (y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) + 1);
    NaiveDate::from_ymd_opt(y, m as u32, 1).expect("valid month")
}

/// 相对时间 → 起止日期（与前端 presetRange 一致）
pub fn preset_range(preset: &str, today: NaiveDate) -> Option<(String, String, &'static str)> {
    let (y, m) = (today.year(), today.month() as i32);
    let q = (m - 1) / 3 * 3 + 1;
    let last = |d: NaiveDate| d - Duration::days(1);
    Some(match preset {
        "this_month" => (ymd(month_start(y, m)), ymd(last(month_start(y, m + 1))), "本月"),
        "last_month" => (ymd(month_start(y, m - 1)), ymd(last(month_start(y, m))), "上月"),
        "this_quarter" => (ymd(month_start(y, q)), ymd(last(month_start(y, q + 3))), "本季度"),
        "last_quarter" => (ymd(month_start(y, q - 3)), ymd(last(month_start(y, q))), "上季度"),
        "this_year" => (format!("{y}-01-01"), format!("{y}-12-31"), "今年"),
        "last_year" => (format!("{}-01-01", y - 1), format!("{}-12-31", y - 1), "去年"),
        "last_7" => (ymd(today - Duration::days(6)), ymd(today), "近 7 天"),
        "last_30" => (ymd(today - Duration::days(29)), ymd(today), "近 30 天"),
        "last_90" => (ymd(today - Duration::days(89)), ymd(today), "近 90 天"),
        _ => return None,
    })
}

struct Slicer {
    column: String,
    filter: Filter,
    desc: String,
}

fn slicer_of(spec: &Json, today: NaiveDate) -> Option<Slicer> {
    let column = spec.get("column")?.as_str()?.to_string();
    if spec.get("mode").and_then(Json::as_str) == Some("date") {
        let r = spec.get("range")?;
        let (min, max, label) = match r.get("preset").and_then(Json::as_str) {
            Some(p) => {
                let (a, b, l) = preset_range(p, today)?;
                (a, b, Some(l))
            }
            None => {
                let min = r.get("min").and_then(Json::as_str).filter(|s| !s.is_empty());
                let max = r.get("max").and_then(Json::as_str).filter(|s| !s.is_empty());
                if min.is_none() && max.is_none() {
                    return None;
                }
                (min.unwrap_or("0000-01-01").to_string(), max.unwrap_or("9999-12-31").to_string(), None)
            }
        };
        let desc = match label {
            Some(l) => format!("{column}：{l}（{min} ~ {max}）"),
            None => format!("{column}：{min} ~ {max}"),
        };
        return Some(Slicer { filter: Filter::Between { column: column.clone(), min: Some(Json::String(min)), max: Some(Json::String(format!("{max} 23:59:59"))) }, column, desc });
    }
    let values: Vec<String> = spec.get("values")?.as_array()?.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    if values.is_empty() {
        return None;
    }
    let desc = if values.len() <= 3 { format!("{column}：{}", values.join("、")) } else { format!("{column}：{} 等 {} 项", values[..3].join("、"), values.len()) };
    Some(Slicer { filter: Filter::In { column: column.clone(), values }, column, desc })
}

fn has_column(def: &dataset::DatasetDef, name: &str) -> bool {
    def.columns.iter().any(|c| c.title == name || c.key == name)
}

fn same_column(def: &dataset::DatasetDef, f: &Filter, column: &str) -> bool {
    let fc = f.column();
    if fc == column {
        return true;
    }
    def.columns.iter().find(|c| c.key == column || c.title == column).is_some_and(|c| fc == c.key || fc == c.title)
}

fn filters_of(spec: &Json) -> Vec<Filter> {
    spec.get("filters").and_then(|f| serde_json::from_value::<Vec<Filter>>(f.clone()).ok()).unwrap_or_default()
}

// ── 数字格式 ──

fn thousands(v: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v.abs());
    let (int, frac) = s.split_once('.').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((s.clone(), None));
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{}{out}{}", if v < 0.0 { "-" } else { "" }, frac.map(|f| format!(".{f}")).unwrap_or_default())
}

fn trim_num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// 大数用万 / 亿（与前端 compact 一致）
pub fn compact(v: Option<f64>, money: bool) -> String {
    match v {
        None => "—".into(),
        Some(v) if v.abs() >= 1e8 => format!("{}亿", trim_num(v / 1e8)),
        Some(v) if v.abs() >= 1e4 => format!("{}万", trim_num(v / 1e4)),
        Some(v) => thousands(v, if money { 2 } else if v.fract() == 0.0 { 0 } else { 2 }),
    }
}

pub fn percent(v: Option<f64>) -> String {
    match v {
        None => "—".into(),
        Some(v) => format!("{}{:.1}%", if v > 0.0 { "+" } else { "" }, v * 100.0),
    }
}

// ── 部件计算 ──

fn pivot_req(rows: Vec<PivotField>, columns: Vec<PivotField>, values: Vec<PivotValue>, filters: Vec<Filter>) -> PivotRequest {
    PivotRequest {
        rows,
        columns,
        values,
        query: QuerySpec { filters, ..Default::default() },
        sort: PivotSort { by: "key".into(), value_index: 0, desc: false },
        top: None,
        subtotals: false,
        grand_totals: true,
        base_field: None,
        aliases: Default::default(),
        calc_items: vec![],
    }
}

fn value_of(spec: &Json) -> PivotValue {
    let v = spec.get("value");
    PivotValue {
        column: v.and_then(|v| v.get("column")).and_then(Json::as_str).map(String::from),
        agg: v.and_then(|v| v.get("agg")).and_then(Json::as_str).unwrap_or("count").to_string(),
        show: None,
        label: None,
        ..Default::default()
    }
}

fn total_cell(r: &PivotResult) -> Vec<Option<f64>> {
    r.rows.iter().find(|x| x.kind == "total").and_then(|t| t.cells.last().cloned()).unwrap_or_default()
}

fn kpi(conn: &Connection, spec: &Json, extra: Vec<Filter>) -> AppResult<KpiBlock> {
    let ds = spec.get("datasetId").and_then(Json::as_str).unwrap_or("");
    let def = dataset::dataset(conn, ds)?;
    let mut filters = filters_of(spec);
    filters.extend(extra);
    let value = value_of(spec);
    let money = value.column.as_deref().and_then(|c| def.columns.iter().find(|x| x.key == c || x.title == c)).is_some_and(|c| c.kind == "money") && matches!(value.agg.as_str(), "sum" | "avg" | "min" | "max");
    let date_col = spec.get("dateColumn").and_then(Json::as_str).filter(|s| !s.is_empty()).map(String::from);
    let granularity = spec.get("granularity").and_then(Json::as_str).unwrap_or("month").to_string();
    let compare = spec.get("compare").and_then(Json::as_str).unwrap_or("none").to_string();
    let title = spec.get("title").and_then(Json::as_str).unwrap_or("指标").to_string();
    let up_is_good = spec.get("upIsGood").and_then(Json::as_bool).unwrap_or(true);
    let delta_label = if compare == "yoy" {
        "较上年同期"
    } else {
        match granularity.as_str() {
            "day" => "较前一天",
            "year" => "较上年",
            _ => "较上月",
        }
    }
    .to_string();
    let field = |c: &str| PivotField { column: c.to_string(), granularity: Some(granularity.clone()), label: None };

    let mut period = None;
    let mut scoped = filters.clone();
    if let (Some(dc), true) = (&date_col, compare != "none") {
        let has_date_filter = filters.iter().any(|f| same_column(&def, f, dc));
        if !has_date_filter {
            // 最近一期：有数据的最后一个时间键
            let without: Vec<Filter> = filters.iter().filter(|f| !same_column(&def, f, dc)).cloned().collect();
            let mut req = pivot_req(vec![field(dc)], vec![], vec![value.clone()], without);
            req.grand_totals = false;
            crate::usage::hit(ds, crate::usage::REPORT);
            let r = pivot::pivot(conn, ds, &req)?;
            period = r.rows.iter().filter(|x| x.kind == "data").rev().find(|x| x.cells.first().and_then(|c| c.first()).copied().flatten().is_some()).and_then(|x| x.keys.first().cloned().flatten());
            if let Some(p) = &period {
                scoped.push(Filter::Prefix { column: dc.clone(), value: p.clone() });
            }
        }
        let mut req = pivot_req(vec![field(dc)], vec![], vec![value.clone(), PivotValue { show: Some(compare.clone()), ..value.clone() }], scoped);
        req.base_field = Some(dc.clone());
        crate::usage::hit(ds, crate::usage::REPORT);
        let r = pivot::pivot(conn, ds, &req)?;
        let keys: Vec<String> = r.rows.iter().filter(|x| x.kind == "data").filter_map(|x| x.keys.first().cloned().flatten()).collect();
        if period.is_none() && keys.len() == 1 {
            period = keys.first().cloned();
        }
        let cell = total_cell(&r);
        return Ok(KpiBlock { title, value: cell.first().copied().flatten(), money, delta: cell.get(1).copied().flatten(), delta_label, period, up_is_good });
    }
    crate::usage::hit(ds, crate::usage::REPORT);
    let r = pivot::pivot(conn, ds, &pivot_req(vec![], vec![], vec![value], scoped))?;
    Ok(KpiBlock { title, value: total_cell(&r).first().copied().flatten(), money, delta: None, delta_label, period, up_is_good })
}

fn fmt_value(v: Option<f64>, kind: &str) -> String {
    if kind == "percent" { percent(v).trim_start_matches('+').to_string() } else { compact(v, kind == "money") }
}

fn table(conn: &Connection, spec: &Json, extra: Vec<Filter>) -> AppResult<TableBlock> {
    let ds = spec.get("datasetId").and_then(Json::as_str).unwrap_or("");
    let mut filters = filters_of(spec);
    filters.extend(extra);
    let row = spec.get("row");
    let rows = vec![PivotField {
        column: row.and_then(|r| r.get("column")).and_then(Json::as_str).unwrap_or("").to_string(),
        granularity: row.and_then(|r| r.get("granularity")).and_then(Json::as_str).map(String::from),
        label: None,
    }];
    let values: Vec<PivotValue> = spec.get("values").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    let top = spec.get("top").and_then(Json::as_u64).filter(|n| *n > 0).map(|n| n as usize);
    let mut req = pivot_req(rows, vec![], values, filters);
    if spec.get("sort").and_then(Json::as_str) != Some("key") {
        req.sort = PivotSort { by: "value".into(), value_index: 0, desc: true };
    }
    req.top = top;
    crate::usage::hit(ds, crate::usage::REPORT);
    let r = pivot::pivot(conn, ds, &req)?;
    let title = spec.get("title").and_then(Json::as_str).unwrap_or("汇总表").to_string();
    Ok(to_table(title, &r, top.map(|n| format!("前 {n} 项合计")).unwrap_or_else(|| "合计".into()), None))
}

fn to_table(title: String, r: &PivotResult, total_label: String, keep_last: Option<usize>) -> TableBlock {
    let kinds: Vec<String> = r.values.iter().map(|v| v.kind.clone()).collect();
    let mut data: Vec<&pivot::PivotRow> = r.rows.iter().filter(|x| x.kind == "data").collect();
    if let Some(n) = keep_last {
        let skip = data.len().saturating_sub(n);
        data = data.into_iter().skip(skip).collect();
    }
    let header = std::iter::once(r.row_fields.first().cloned().unwrap_or_default()).chain(r.values.iter().map(|v| v.label.clone())).collect();
    let fmt_row = |label: String, cells: &[Option<f64>]| -> Vec<String> { std::iter::once(label).chain(cells.iter().enumerate().map(|(i, v)| fmt_value(*v, &kinds[i]))).collect() };
    let rows = data.iter().map(|x| fmt_row(x.keys.first().cloned().flatten().unwrap_or_else(|| "（空白）".into()), x.cells.last().map(Vec::as_slice).unwrap_or(&[]))).collect();
    let raw = data.iter().map(|x| (x.keys.first().cloned().flatten().unwrap_or_default(), x.cells.last().cloned().unwrap_or_default())).collect();
    let total_row = r.rows.iter().find(|x| x.kind == "total");
    let total = total_row.map(|t| fmt_row(total_label.clone(), t.cells.last().map(Vec::as_slice).unwrap_or(&[])));
    let raw_total = total_row.map(|t| (total_label, t.cells.last().cloned().unwrap_or_default()));
    TableBlock { title, header, rows, total, raw, raw_total, kinds }
}

/// 数据集图表 → 表格（最近 12 期 / 前 12 个分类）
fn chart(conn: &Connection, spec: &Json, extra: Vec<Filter>) -> AppResult<Option<TableBlock>> {
    let src = spec.get("source");
    if src.and_then(|s| s.get("kind")).and_then(Json::as_str) != Some("dataset") {
        return Ok(None);
    }
    let src = src.expect("checked");
    let ds = src.get("datasetId").and_then(Json::as_str).unwrap_or("");
    let x = src.get("x");
    let rows = vec![PivotField {
        column: x.and_then(|v| v.get("column")).and_then(Json::as_str).unwrap_or("").to_string(),
        granularity: x.and_then(|v| v.get("granularity")).and_then(Json::as_str).map(String::from),
        label: None,
    }];
    let mut values: Vec<PivotValue> = src.get("values").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    // 按字段拆分系列：系列作为列（只取第一个数值）
    let split = src.get("splitBy").and_then(Json::as_str).filter(|s| !s.is_empty()).map(String::from);
    let columns = match &split {
        Some(c) => {
            values.truncate(1);
            vec![PivotField { column: c.clone(), granularity: None, label: None }]
        }
        None => vec![],
    };
    let mut filters = filters_of(src);
    filters.extend(extra);
    let by_value = src.get("sort").and_then(Json::as_str) == Some("value");
    let mut req = pivot_req(rows, columns, values, filters);
    if by_value {
        req.sort = PivotSort { by: "value".into(), value_index: 0, desc: true };
        req.top = Some(src.get("top").and_then(Json::as_u64).unwrap_or(12).clamp(1, 12) as usize);
    }
    crate::usage::hit(ds, crate::usage::REPORT);
    let r = pivot::pivot(conn, ds, &req)?;
    let title = spec.get("title").and_then(Json::as_str).filter(|s| !s.is_empty()).unwrap_or("图表").to_string();
    let keep = if by_value { None } else { Some(12) };
    Ok(Some(if split.is_some() { split_table(title, &r, keep) } else { to_table(title, &r, "合计".into(), keep) }))
}

/// 最多列出的系列数，其余合并为「其他」（与前端图表一致）
const MAX_SERIES: usize = 8;

/// 拆分系列的图表 → 宽表：每个系列一列（按合计从大到小，超出的合并为「其他」）+ 合计列
fn split_table(title: String, r: &PivotResult, keep_last: Option<usize>) -> TableBlock {
    let kind = r.values.first().map(|v| v.kind.clone()).unwrap_or_default();
    let n = r.column_keys.len();
    let total_row = r.rows.iter().find(|x| x.kind == "total");
    let col_total = |i: usize| total_row.and_then(|t| t.cells.get(i)).and_then(|c| c.first().copied().flatten()).unwrap_or(0.0);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| col_total(*b).partial_cmp(&col_total(*a)).unwrap_or(std::cmp::Ordering::Equal));
    let (shown, rest) = order.split_at(order.len().min(if n > MAX_SERIES { MAX_SERIES - 1 } else { MAX_SERIES }));
    let name = |i: usize| r.column_keys[i].first().cloned().flatten().unwrap_or_else(|| "（空白）".into());
    let mut header = vec![r.row_fields.first().cloned().unwrap_or_default()];
    header.extend(shown.iter().map(|i| name(*i)));
    if !rest.is_empty() {
        header.push("其他".into());
    }
    header.push("合计".into());
    let line = |cells: &[Vec<Option<f64>>]| -> Vec<Option<f64>> {
        let v = |i: usize| cells.get(i).and_then(|c| c.first().copied().flatten());
        let mut out: Vec<Option<f64>> = shown.iter().map(|i| v(*i)).collect();
        if !rest.is_empty() {
            let vals: Vec<f64> = rest.iter().filter_map(|i| v(*i)).collect();
            out.push(if vals.is_empty() { None } else { Some(vals.iter().sum()) });
        }
        out.push(if r.column_total { v(n) } else { None });
        out
    };
    let mut data: Vec<&pivot::PivotRow> = r.rows.iter().filter(|x| x.kind == "data").collect();
    if let Some(k) = keep_last {
        let skip = data.len().saturating_sub(k);
        data = data.into_iter().skip(skip).collect();
    }
    let raw: Vec<(String, Vec<Option<f64>>)> = data.iter().map(|x| (x.keys.first().cloned().flatten().unwrap_or_else(|| "（空白）".into()), line(&x.cells))).collect();
    let raw_total = total_row.map(|t| ("合计".to_string(), line(&t.cells)));
    let fmt = |(label, vals): &(String, Vec<Option<f64>>)| -> Vec<String> { std::iter::once(label.clone()).chain(vals.iter().map(|v| fmt_value(*v, &kind))).collect() };
    let width = header.len() - 1;
    TableBlock { title, rows: raw.iter().map(fmt).collect(), total: raw_total.as_ref().map(fmt), raw, raw_total, kinds: vec![kind.clone(); width], header }
}

/// 工作表里的数据表：按它的筛选 / 关键字 / 排序取前几行明细
fn datatable(conn: &Connection, spec: &Json, extra: Vec<Filter>) -> AppResult<TableBlock> {
    let ds = spec.get("datasetId").and_then(Json::as_str).unwrap_or("");
    let def = dataset::dataset(conn, ds)?;
    let mut filters = filters_of(spec);
    // 不跟随切片器的数据表只按自己的筛选
    if spec.get("followSlicers").and_then(Json::as_bool) != Some(false) {
        filters.extend(extra);
    }
    let sort: Vec<dataset::SortSpec> = spec.get("sort").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    let keyword = spec.get("keyword").and_then(Json::as_str).filter(|s| !s.trim().is_empty()).map(String::from);
    let q = QuerySpec { filters, keyword, sort, rule: None, violation: None };
    let fields: Vec<String> = spec.get("fields").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    let cols: Vec<&dataset::ColumnDef> = if fields.is_empty() {
        def.columns.iter().collect()
    } else {
        fields
            .iter()
            .map(|f| def.columns.iter().find(|c| &c.title == f || &c.key == f).ok_or_else(|| AppError::bad(format!("数据集「{}」没有列「{f}」", def.name))))
            .collect::<AppResult<_>>()?
    };
    if cols.is_empty() {
        return Err(AppError::bad("数据表没有可显示的列"));
    }
    let (where_sql, args) = dataset::filter_sql(conn, &def, &q)?;
    let order = dataset::order_sql(&def, &q)?;
    let total: i64 = conn.query_row(&format!("SELECT count(*) FROM {} WHERE {where_sql}", def.table), duckdb::params_from_iter(args.iter()), |r| r.get(0))?;
    let select = cols.iter().map(|c| format!("({})", dataset::column_sql(c))).collect::<Vec<_>>().join(", ");
    let mut stmt = conn.prepare(&format!("SELECT {select} FROM {} WHERE {where_sql} ORDER BY {order} LIMIT {DATATABLE_ROWS}", def.table))?;
    let n = cols.len();
    let data: Vec<Vec<Json>> = stmt
        .query_map(duckdb::params_from_iter(args.iter()), |r| (0..n).map(|i| Ok(crate::tabular::to_json(r.get_ref(i)?))).collect::<duckdb::Result<Vec<_>>>())?
        .collect::<Result<_, _>>()?;
    let kinds: Vec<String> = cols.iter().map(|c| if crate::kinds::is_numeric(c.kind) { c.kind.to_string() } else { "text".to_string() }).collect();
    let show = |v: &Json, kind: &str| -> String {
        match v {
            Json::Null => String::new(),
            Json::Number(x) => {
                let f = x.as_f64().unwrap_or(0.0);
                if kind == "money" { thousands(f, 2) } else { trim_num(f) }
            }
            Json::String(s) => s.clone(),
            other => other.to_string(),
        }
    };
    let rows: Vec<Vec<String>> = data.iter().map(|r| r.iter().zip(&kinds).map(|(v, k)| show(v, k)).collect()).collect();
    // 附件：首列作标签，数值列写数值，文本列（raw 为 None）按显示文字写入
    let raw = data
        .iter()
        .zip(&rows)
        .map(|(r, shown)| (shown[0].clone(), r.iter().zip(&kinds).skip(1).map(|(v, k)| if k == "text" { None } else { v.as_f64() }).collect()))
        .collect();
    let name = spec.get("name").and_then(Json::as_str).filter(|s| !s.is_empty()).unwrap_or(&def.name);
    let title = if total > DATATABLE_ROWS { format!("{name}（前 {DATATABLE_ROWS} 行，共 {} 行）", thousands(total as f64, 0)) } else { format!("{name}（共 {total} 行）") };
    Ok(TableBlock { title, header: cols.iter().map(|c| c.title.clone()).collect(), rows, total: None, raw, raw_total: None, kinds: kinds.into_iter().skip(1).collect() })
}

/// 计算一个仪表盘页的推送内容
pub fn build(conn: &Connection, workbook_id: &str, sheet_id: &str, today: NaiveDate) -> AppResult<ReportContent> {
    let (meta, bytes) = crate::workbook::content(conn, workbook_id)?;
    let wb = gunzip(&bytes)?;
    let sheet_name = wb.pointer(&format!("/sheets/{sheet_id}/name")).and_then(Json::as_str).ok_or_else(|| AppError::bad("仪表盘页已被删除或尚未保存"))?.to_string();
    let mut parts: Vec<Part> = drawing_parts(&wb).into_iter().filter(|(s, _)| s == sheet_id).map(|(_, p)| p).collect();
    // 按页面上的位置排序：从上到下、从左到右（同一行的部件高度差在 40px 内视为一行）
    parts.sort_by(|a, b| ((a.top / 40.0).floor(), a.left).partial_cmp(&((b.top / 40.0).floor(), b.left)).unwrap_or(std::cmp::Ordering::Equal));
    let slicers: Vec<Slicer> = parts.iter().filter(|p| p.key == "webexcel-slicer").filter_map(|p| slicer_of(&p.spec, today)).collect();
    let mut blocks = Vec::new();
    let mut defs: std::collections::HashMap<String, Option<dataset::DatasetDef>> = std::collections::HashMap::new();
    for p in parts.iter().filter(|p| p.key != "webexcel-slicer") {
        let ds_id = if p.key == "webexcel-chart" { p.spec.pointer("/source/datasetId") } else { p.spec.get("datasetId") }.and_then(Json::as_str).unwrap_or("").to_string();
        let def = defs.entry(ds_id.clone()).or_insert_with(|| dataset::dataset(conn, &ds_id).ok()).clone();
        let extra: Vec<Filter> = def.as_ref().map(|d| slicers.iter().filter(|s| has_column(d, &s.column)).map(|s| s.filter.clone()).collect()).unwrap_or_default();
        let title = p.spec.get("title").and_then(Json::as_str).unwrap_or("").to_string();
        let r: AppResult<Option<Block>> = match p.key.as_str() {
            "webexcel-kpi" => kpi(conn, &p.spec, extra).map(|k| Some(Block::Kpi(k))),
            "webexcel-table" => table(conn, &p.spec, extra).map(|t| Some(Block::Table(t))),
            "webexcel-datatable" => datatable(conn, &p.spec, extra).map(|t| Some(Block::Table(t))),
            "webexcel-chart" => chart(conn, &p.spec, extra).map(|t| t.map(Block::Table)),
            _ => Ok(None),
        };
        match r {
            Ok(Some(b)) => blocks.push(b),
            Ok(None) => {}
            Err(e) => blocks.push(Block::Error { title: if title.is_empty() { "部件".into() } else { title }, message: e.message() }),
        }
    }
    Ok(ReportContent {
        workbook_name: meta.name,
        sheet_name,
        filters: slicers.into_iter().map(|s| s.desc).collect(),
        blocks,
        generated_at: crate::db::now_local(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_format() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        assert_eq!(preset_range("last_month", d).unwrap().0, "2025-12-01");
        assert_eq!(preset_range("last_month", d).unwrap().1, "2025-12-31");
        assert_eq!(preset_range("this_quarter", d).unwrap().1, "2026-03-31");
        assert_eq!(preset_range("last_quarter", d).unwrap().0, "2025-10-01");
        assert_eq!(preset_range("last_30", d).unwrap().0, "2025-12-17");
        assert_eq!(compact(Some(3313147.54), true), "331.31万");
        assert_eq!(compact(Some(1.25e9), true), "12.5亿");
        assert_eq!(compact(Some(1234.5), true), "1,234.50");
        assert_eq!(compact(Some(-8000.0), false), "-8,000");
        assert_eq!(percent(Some(0.0322)), "+3.2%");
        assert_eq!(percent(Some(-0.1)), "-10.0%");
    }
}
