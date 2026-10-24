//! 数据透视表（服务端计算，支撑千万行）
//!
//! 一条 SQL：`GROUP BY GROUPING SETS` 一次扫表算出所有层级 —— 明细（行 × 列）、各级行小计、行总计（跨列）、
//! 列总计、全部总计；再在内存里组装层级、排序（按名称 / 按值）、展开为表格行。
//! 每个格子都对应一组确定的筛选条件，前端据此下钻到明细并核对

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use duckdb::types::Value;
use duckdb::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::pivot_calc;

mod items;
pub use items::CalcItem;
use crate::dataset::{self, DatasetDef, QuerySpec};
use crate::error::{AppError, AppResult};

/// 行（叶子组合）上限：超过时提示加筛选或使用「前 N 项」
const MAX_LEAF_ROWS: i64 = 20_000;
/// 列组合上限
const MAX_COLUMN_KEYS: i64 = 200;
/// 分组子查询里的行顺序列（首个 / 末个按它取值）
const ORD: &str = "__ord";

/// 原样保留到分的汇总方式（金额列上取的是原始值或其和）
pub(crate) fn keeps_cents(agg: &str) -> bool {
    matches!(agg, "sum" | "min" | "max" | "first" | "last")
}

#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PivotField {
    pub column: String,
    /// 日期分组：year / quarter / month / week / day
    /// （年 / 月 / 日取文本前 4 / 7 / 10 位，适用于日期时间列与 YYYY-MM 之类的月份列；季度为 2026-Q4，周为 ISO 周 2026-W41）
    #[serde(default)]
    pub granularity: Option<String>,
    /// 表头别名（为空时用列名 + 分组）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PivotValue {
    /// count 时可省略
    #[serde(default)]
    pub column: Option<String>,
    /// sum / count / avg / min / max / first / last / count_distinct / approx_distinct / median / approx_median / approx_p90 / approx_p95 / approx_p99 / p10 / p25 / p75 / p90 / p95 / p99 / stddev / var
    pub agg: String,
    /// 值显示方式：raw（默认）/ pct_total / pct_row / pct_col / pct_parent / yoy / yoy_diff / mom / mom_diff
    #[serde(default)]
    pub show: Option<String>,
    /// 表头别名（为空时自动生成，如「订单金额（求和）」）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// agg 为 calc（计算值）时的公式，如 `([金额（求和）]-[成本（求和）])/[金额（求和）]`，见 `pivot_calc`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// 计算值的数字格式：money / number（默认）/ percent（结果是小数，如 0.25 显示为 25%）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PivotSort {
    /// key：按名称；value：按某个值字段（看该层级的小计）
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub value_index: usize,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PivotRequest {
    #[serde(default)]
    pub rows: Vec<PivotField>,
    #[serde(default)]
    pub columns: Vec<PivotField>,
    pub values: Vec<PivotValue>,
    /// 筛选（与大数据表的筛选同构）
    #[serde(default, flatten)]
    pub query: QuerySpec,
    #[serde(default)]
    pub sort: PivotSort,
    /// 只保留第一个行字段按第一个值排名的前 N 项（如「退款最多的前 20 个达人」）
    #[serde(default)]
    pub top: Option<usize>,
    #[serde(default = "yes")]
    pub subtotals: bool,
    #[serde(default = "yes")]
    pub grand_totals: bool,
    /// 同比 / 环比的时间字段（行或列字段的列名）；为空时自动取第一个日期类字段
    #[serde(default)]
    pub base_field: Option<String>,
    /// 行 / 列标签别名：字段列名 → 原值（空值为 ""）→ 别名。只影响按名称排序，显示由前端替换
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub aliases: HashMap<String, HashMap<String, String>>,
    /// 计算项：在某个行 / 列字段的取值之间做运算（如列字段「单据类型」里新增「期末 = 入库 - 出库」），见 `items`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calc_items: Vec<CalcItem>,
}

fn yes() -> bool {
    true
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueInfo {
    pub label: String,
    pub agg: String,
    pub column: Option<String>,
    /// money / number / percent（前端决定数字格式）
    pub kind: String,
    /// 显示方式（同请求）
    pub show: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PivotRow {
    /// 行键（长度 = 已展开的层级数）；None 表示该字段为空值
    pub keys: Vec<Option<String>>,
    /// data：明细；subtotal：某一级小计；total：总计
    pub kind: &'static str,
    /// cells[列下标][值下标]；有列字段时最后一列为「总计」。按值的显示方式换算后的值（占比为小数，如 0.25）
    pub cells: Vec<Vec<Option<f64>>>,
    /// 原始汇总值（有值使用了占比 / 同比 / 环比时提供，下钻核对用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Vec<Vec<Option<f64>>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PivotResult {
    pub row_fields: Vec<String>,
    pub column_fields: Vec<String>,
    pub values: Vec<ValueInfo>,
    /// 列组合（不含总计列）
    pub column_keys: Vec<Vec<Option<String>>>,
    /// 是否附加了「总计」列
    pub column_total: bool,
    pub rows: Vec<PivotRow>,
    /// 叶子行数（不含小计 / 总计）
    pub leaf_rows: usize,
    /// 应用「前 N 项」前第一个行字段的取值数
    pub first_level_count: Option<i64>,
    /// 同比 / 环比所用的时间字段
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_field: Option<String>,
    pub elapsed_ms: i64,
}

/// 值显示方式
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Show {
    Raw,
    /// 占总计
    PctTotal,
    /// 占行总计（同一行跨列）
    PctRow,
    /// 占列总计
    PctCol,
    /// 占上一级行汇总
    PctParent,
    /// 同比增长率 / 增减额（与上年同期比）
    Yoy,
    YoyDiff,
    /// 环比增长率 / 增减额（与上一期比）
    Mom,
    MomDiff,
}

impl Show {
    fn parse(s: Option<&str>) -> AppResult<Self> {
        Ok(match s.unwrap_or("raw") {
            "" | "raw" => Self::Raw,
            "pct_total" => Self::PctTotal,
            "pct_row" => Self::PctRow,
            "pct_col" => Self::PctCol,
            "pct_parent" => Self::PctParent,
            "yoy" => Self::Yoy,
            "yoy_diff" => Self::YoyDiff,
            "mom" => Self::Mom,
            "mom_diff" => Self::MomDiff,
            other => return Err(AppError::bad(format!("未知的值显示方式：{other}"))),
        })
    }

    fn code(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::PctTotal => "pct_total",
            Self::PctRow => "pct_row",
            Self::PctCol => "pct_col",
            Self::PctParent => "pct_parent",
            Self::Yoy => "yoy",
            Self::YoyDiff => "yoy_diff",
            Self::Mom => "mom",
            Self::MomDiff => "mom_diff",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Raw => "",
            Self::PctTotal => "占总计",
            Self::PctRow => "占行合计",
            Self::PctCol => "占列合计",
            Self::PctParent => "占上级",
            Self::Yoy => "同比",
            Self::YoyDiff => "同比增减",
            Self::Mom => "环比",
            Self::MomDiff => "环比增减",
        }
    }

    fn is_pct(self) -> bool {
        matches!(self, Self::PctTotal | Self::PctRow | Self::PctCol | Self::PctParent)
    }

    /// 需要上一期数据：Some(true) 上年同期，Some(false) 上一期
    fn period(self) -> Option<bool> {
        match self {
            Self::Yoy | Self::YoyDiff => Some(true),
            Self::Mom | Self::MomDiff => Some(false),
            _ => None,
        }
    }
}

/// 时间键的粒度：由透视结果里该字段的取值判断（年 2026 / 月 2026-10 / 日 2026-10-05）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PeriodFmt {
    Year,
    /// 2026-Q4
    Quarter,
    Month,
    /// ISO 周 2026-W41
    Week,
    Day,
}

fn period_fmt(keys: &[&str]) -> Option<PeriodFmt> {
    let is = |k: &str, n: usize| k.len() == n && k.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' || b == b'/' } else { b.is_ascii_digit() });
    if keys.is_empty() {
        return None;
    }
    let quarter = |k: &str| k.len() == 7 && k[..4].bytes().all(|b| b.is_ascii_digit()) && &k[4..6] == "-Q" && matches!(&k[6..], "1" | "2" | "3" | "4");
    let week = |k: &str| k.len() == 8 && k[..4].bytes().all(|b| b.is_ascii_digit()) && &k[4..6] == "-W" && k[6..].bytes().all(|b| b.is_ascii_digit());
    if keys.iter().all(|k| is(k, 4)) {
        Some(PeriodFmt::Year)
    } else if keys.iter().all(|k| quarter(k)) {
        Some(PeriodFmt::Quarter)
    } else if keys.iter().all(|k| week(k)) {
        Some(PeriodFmt::Week)
    } else if keys.iter().all(|k| is(k, 7)) {
        Some(PeriodFmt::Month)
    } else if keys.iter().all(|k| is(k, 10)) {
        Some(PeriodFmt::Day)
    } else {
        None
    }
}

/// 把「上一期」的时间键平移到本期：e 为分组键表达式，yearly 为同比（+1 年）否则环比（+1 期）
fn shift_sql(e: &str, fmt: PeriodFmt, yearly: bool) -> String {
    let y = format!("TRY_CAST(substr({e}, 1, 4) AS INTEGER)");
    match (fmt, yearly) {
        (PeriodFmt::Year, _) => format!("CAST(TRY_CAST({e} AS INTEGER) + 1 AS VARCHAR)"),
        // 季度、周的同比：年份 + 1，季度号 / 周号不变（没有第 53 周的年份自然对不上）
        (PeriodFmt::Quarter | PeriodFmt::Week, true) => format!("CAST({y} + 1 AS VARCHAR) || substr({e}, 5)"),
        (PeriodFmt::Quarter, false) => {
            let q = format!("TRY_CAST(substr({e}, 7, 1) AS INTEGER)");
            format!("CASE WHEN {q} = 4 THEN CAST({y} + 1 AS VARCHAR) || '-Q1' ELSE CAST({y} AS VARCHAR) || '-Q' || CAST({q} + 1 AS VARCHAR) END")
        }
        (PeriodFmt::Week, false) => {
            // ISO 周一 = 当年 1 月 4 日所在周的周一 + (周号 - 1) × 7；下一周再加 7 天
            let w = format!("TRY_CAST(substr({e}, 7, 2) AS INTEGER)");
            let jan4 = format!("make_date({y}, 1, 4)");
            week_key(&format!("({jan4} - CAST(isodow({jan4}) - 1 AS INTEGER) + CAST({w} * 7 AS INTEGER))"))
        }
        (PeriodFmt::Month, y) => format!(
            "strftime(TRY_STRPTIME(replace({e}, '/', '-') || '-01', '%Y-%m-%d') + INTERVAL 1 {}, '%Y-%m')",
            if y { "YEAR" } else { "MONTH" }
        ),
        (PeriodFmt::Day, y) => format!(
            "strftime(TRY_STRPTIME(replace({e}, '/', '-'), '%Y-%m-%d') + INTERVAL 1 {}, '%Y-%m-%d')",
            if y { "YEAR" } else { "DAY" }
        ),
    }
}

/// 日期 → ISO 周键（2026-W41）
fn week_key(d: &str) -> String {
    format!("CAST(isoyear({d}) AS VARCHAR) || '-W' || lpad(CAST(week({d}) AS VARCHAR), 2, '0')")
}

/// 增长率：(本期 - 基期) / |基期|；基期为空或 0 时无意义
fn growth(cur: Option<f64>, base: Option<f64>) -> Option<f64> {
    match (cur, base) {
        (Some(c), Some(b)) if b != 0.0 => Some((c - b) / b.abs()),
        _ => None,
    }
}

fn diff(cur: Option<f64>, base: Option<f64>) -> Option<f64> {
    if cur.is_none() && base.is_none() {
        return None;
    }
    Some(((cur.unwrap_or(0.0) - base.unwrap_or(0.0)) * 100.0).round() / 100.0 + 0.0)
}

fn ratio(v: Option<f64>, total: Option<f64>) -> Option<f64> {
    match (v, total) {
        (Some(v), Some(t)) if t != 0.0 => Some(v / t),
        _ => None,
    }
}

fn agg_label(agg: &str) -> &'static str {
    match agg {
        "sum" => "求和",
        "count" => "计数",
        "avg" => "平均",
        "min" => "最小",
        "max" => "最大",
        "first" => "首个",
        "last" => "末个",
        "count_distinct" => "去重计数",
        "approx_distinct" => "近似去重计数",
        "median" => "中位数",
        "approx_median" => "近似中位数",
        "approx_p90" => "近似 P90",
        "approx_p95" => "近似 P95",
        "approx_p99" => "近似 P99",
        "p10" => "P10",
        "p25" => "P25",
        "p75" => "P75",
        "p90" => "P90",
        "p95" => "P95",
        "p99" => "P99",
        "stddev" => "标准差",
        "var" => "方差",
        _ => "",
    }
}

/// 近似分位数（DuckDB 的 approx_quantile，t-digest，流式、占内存很小）：approx_p95 → 0.95
fn approx_percentile(agg: &str) -> Option<f64> {
    match agg {
        "approx_median" => Some(0.5),
        "approx_p90" => Some(0.9),
        "approx_p95" => Some(0.95),
        "approx_p99" => Some(0.99),
        _ => None,
    }
}

/// 分位数汇总：p90 → 0.9
fn percentile(agg: &str) -> Option<f64> {
    match agg {
        "median" => Some(0.5),
        "p10" => Some(0.1),
        "p25" => Some(0.25),
        "p75" => Some(0.75),
        "p90" => Some(0.9),
        "p95" => Some(0.95),
        "p99" => Some(0.99),
        _ => None,
    }
}

pub(crate) fn dim_sql(def: &DatasetDef, f: &PivotField) -> AppResult<(String, String)> {
    let col = def.columns.iter().find(|c| c.key == f.column || c.title == f.column).ok_or_else(|| AppError::bad(format!("列不存在：{}", f.column)))?;
    let base = format!("CAST(({}) AS VARCHAR)", dataset::column_sql(col));
    let expr = match f.granularity.as_deref() {
        Some("year") => format!("substr({base}, 1, 4)"),
        Some("quarter") => format!("substr({base}, 1, 4) || '-Q' || CAST((TRY_CAST(substr({base}, 6, 2) AS INTEGER) - 1) // 3 + 1 AS VARCHAR)"),
        Some("month") => format!("substr({base}, 1, 7)"),
        Some("week") => week_key(&format!("TRY_CAST(substr(replace({base}, '/', '-'), 1, 10) AS DATE)")),
        Some("day") => format!("substr({base}, 1, 10)"),
        None | Some("") => base,
        Some(g) => return Err(AppError::bad(format!("未知的日期分组：{g}"))),
    };
    let name = match (f.label.as_deref().map(str::trim).filter(|s| !s.is_empty()), f.granularity.as_deref()) {
        (Some(l), _) => l.to_string(),
        (_, Some("year")) => format!("{}（年）", col.title),
        (_, Some("quarter")) => format!("{}（季度）", col.title),
        (_, Some("month")) => format!("{}（月）", col.title),
        (_, Some("week")) => format!("{}（周）", col.title),
        (_, Some("day")) => format!("{}（日）", col.title),
        _ => col.title.clone(),
    };
    Ok((expr, name))
}

pub(crate) fn value_sql(def: &DatasetDef, v: &PivotValue) -> AppResult<(String, ValueInfo)> {
    let agg = v.agg.as_str();
    let custom = v.label.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    if agg == "calc" {
        // 计算值不查数据库：占一个空列，汇总完成后按公式补上（见 apply_calcs）
        let kind = match v.format.as_deref() {
            Some("money") => "money",
            Some("percent") => "percent",
            None | Some("" | "number") => "number",
            Some(f) => return Err(AppError::bad(format!("计算值的格式只能是 money / number / percent，不是「{f}」"))),
        };
        return Ok(("NULL::DOUBLE".into(), ValueInfo { label: custom.unwrap_or_default(), agg: "calc".into(), column: None, kind: kind.into(), show: "raw".into() }));
    }
    if agg == "count" && v.column.is_none() {
        return Ok(("count(*)::DOUBLE".into(), ValueInfo { label: custom.unwrap_or_else(|| "计数".into()), agg: "count".into(), column: None, kind: "number".into(), show: "raw".into() }));
    }
    let name = v.column.as_deref().ok_or_else(|| AppError::bad("值字段缺少列"))?;
    let col = def.columns.iter().find(|c| c.key == name || c.title == name).ok_or_else(|| AppError::bad(format!("列不存在：{name}")))?;
    let numeric = crate::kinds::is_numeric(col.kind);
    let e = dataset::column_sql(col);
    let sql = match agg {
        "count" => format!("count({e})::DOUBLE"),
        "count_distinct" => format!("count(DISTINCT {e})::DOUBLE"),
        // 近似去重（HyperLogLog，误差通常在 5% 以内）：高基数列（订单号、用户 ID）上比精确去重快几倍到几十倍、占内存少得多
        "approx_distinct" => format!("approx_count_distinct({e})::DOUBLE"),
        // 按列的原生类型聚合（金额列为 DECIMAL，求和无浮点误差），最后再转 DOUBLE
        "sum" | "avg" | "min" | "max" if numeric => format!("{agg}({e})::DOUBLE"),
        // 首个 / 末个：按行顺序（导入顺序）第一个 / 最后一个非空值，如期初 / 期末余额、最新单价；行顺序列 __ord 由 run_grid 的子查询提供
        "first" if numeric => format!("arg_min({e}, {ORD}) FILTER (WHERE ({e}) IS NOT NULL)::DOUBLE"),
        "last" if numeric => format!("arg_max({e}, {ORD}) FILTER (WHERE ({e}) IS NOT NULL)::DOUBLE"),
        // 统计类在 DOUBLE 上算（DECIMAL 的分位数插值、方差会溢出精度）
        a if numeric && percentile(a).is_some() => format!("quantile_cont(({e})::DOUBLE, {})", percentile(a).unwrap_or(0.5)),
        // 近似分位数：精确分位数要把整列数值放进内存（1 亿行 × 每个值约 0.8 GB，且不受内存上限约束），近似版常数内存
        a if numeric && approx_percentile(a).is_some() => format!("approx_quantile(({e})::DOUBLE, {})", approx_percentile(a).unwrap_or(0.5)),
        "stddev" if numeric => format!("stddev_samp(({e})::DOUBLE)"),
        "var" if numeric => format!("var_samp(({e})::DOUBLE)"),
        a if agg_label(a).is_empty() => return Err(AppError::bad(format!("未知的汇总方式：{a}"))),
        _ => return Err(AppError::bad(format!("「{}」不是数值列，只能计数或去重计数", col.title))),
    };
    // 与原列同单位的（求和、平均、最值、分位数、标准差）沿用金额格式；方差、计数为普通数字
    // 整数列只有求和、最值、首末个还是整数，平均、分位数、标准差带小数
    let kind = match col.kind {
        "integer" if !matches!(agg, "sum" | "min" | "max" | "first" | "last") => "number".into(),
        k if numeric && !matches!(agg, "count" | "count_distinct" | "approx_distinct" | "var") => k.to_string(),
        _ => "number".into(),
    };
    let label = custom.unwrap_or_else(|| format!("{}（{}）", col.title, agg_label(agg)));
    Ok((sql, ValueInfo { label, agg: agg.into(), column: Some(col.title.clone()), kind, show: "raw".into() }))
}

type Key = Vec<Option<String>>;

/// 某一行前缀的汇总：列键（None = 跨列总计）→ 各值
#[derive(Default)]
struct Bucket {
    by_col: HashMap<Option<Key>, Vec<Option<f64>>>,
}

/// 一次 GROUPING SETS 查询的结果：行前缀 → 汇总；以及出现过的列组合
struct Grid {
    buckets: HashMap<Key, Bucket>,
    column_keys: BTreeSet<Key>,
}

/// 执行分组集合查询（主查询与同比 / 环比的基期查询共用）
#[allow(clippy::too_many_arguments)]
fn run_grid(conn: &Connection, table: &str, rid: &str, dims: &[String], k: usize, m: usize, vals: &[(String, ValueInfo)], sets: &[String], where_sql: &str, args: &[Value]) -> AppResult<Grid> {
    let d = |i: usize| format!("d{i}");
    let all_dims: Vec<String> = (0..k + m).map(d).collect();
    let grouping = if all_dims.is_empty() { "0".to_string() } else { format!("GROUPING({})", all_dims.join(", ")) };
    // 维度表达式在子查询里算好（d0、d1…），值字段在外层对原始列聚合
    let inner = dims.iter().enumerate().map(|(i, e)| format!(", {e} AS {}", d(i))).collect::<String>();
    let dsel = all_dims.iter().map(|x| format!("{x}, ")).collect::<String>();
    let sql = format!(
        "SELECT {dsel}{grouping}::BIGINT AS g, {vsel}
         FROM (SELECT *, {rid} AS {ORD}{inner} FROM {table} WHERE {where_sql}) s
         GROUP BY GROUPING SETS ({sets})",
        vsel = vals.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>().join(", "),
        sets = sets.join(", "),
    );
    let mut grid = Grid { buckets: HashMap::new(), column_keys: BTreeSet::new() };
    let mut stmt = conn.prepare(&sql)?;
    let n_dims = k + m;
    let mut rs = stmt.query(params_from_iter(args.iter()))?;
    while let Some(r) = rs.next()? {
        let keys: Vec<Option<String>> = (0..n_dims).map(|i| r.get::<_, Option<String>>(i)).collect::<duckdb::Result<_>>()?;
        let g: i64 = r.get(n_dims)?;
        let values: Vec<Option<f64>> = (0..vals.len())
            .map(|i| {
                // 金额（求和 / 最值）保留到分，消除 DOUBLE 列求和的尾差
                let money = vals[i].1.kind == "money" && keeps_cents(&vals[i].1.agg);
                r.get::<_, Option<f64>>(n_dims + 1 + i).map(|v| v.map(|x| if money { (x * 100.0).round() / 100.0 + 0.0 } else { x }))
            })
            .collect::<duckdb::Result<_>>()?;
        // GROUPING 位：最高位对应第一个维度，1 表示该维度被汇总掉
        let rolled = |i: usize| (g >> (n_dims - 1 - i)) & 1 == 1;
        let prefix_len = (0..k).take_while(|&i| !rolled(i)).count();
        let col_key = if m > 0 && !rolled(k) { Some(keys[k..].to_vec()) } else { None };
        if let Some(ck) = &col_key {
            grid.column_keys.insert(ck.clone());
        }
        grid.buckets.entry(keys[..prefix_len].to_vec()).or_default().by_col.insert(col_key, values);
    }
    Ok(grid)
}

/// 编译好的计算值：（值的下标，公式）
type Calcs = Vec<(usize, pivot_calc::Expr)>;

/// 把请求里的计算值编译成表达式：`[名称]` 按表头文字匹配，其次按列名（取第一个同列的普通值）；只能引用普通值
fn compile_calcs(req: &PivotRequest, vals: &mut [(String, ValueInfo)]) -> AppResult<Calcs> {
    let mut calcs = Vec::new();
    let mut n = 0;
    for (i, v) in req.values.iter().enumerate() {
        if v.agg != "calc" {
            continue;
        }
        n += 1;
        if !matches!(v.show.as_deref(), None | Some("" | "raw")) {
            return Err(AppError::bad("计算值不能再设置占比 / 同比 / 环比（公式里直接写比率即可）"));
        }
        if vals[i].1.label.is_empty() {
            vals[i].1.label = format!("计算值{n}");
        }
        let formula = v.formula.as_deref().map(str::trim).filter(|f| !f.is_empty()).ok_or_else(|| AppError::bad(format!("「{}」缺少公式", vals[i].1.label)))?;
        let plain: Vec<(usize, &ValueInfo)> = vals.iter().enumerate().filter(|(j, _)| req.values[*j].agg != "calc").map(|(j, x)| (j, &x.1)).collect();
        let resolve = |name: &str| -> AppResult<usize> {
            if let Some((j, _)) = plain.iter().find(|(_, info)| info.label == name) {
                return Ok(*j);
            }
            if let Some((j, _)) = plain.iter().find(|(_, info)| info.column.as_deref() == Some(name)) {
                return Ok(*j);
            }
            let calc_named = req.values.iter().enumerate().any(|(j, x)| x.agg == "calc" && vals[j].1.label == name);
            let have = plain.iter().map(|(_, info)| format!("[{}]", info.label)).collect::<Vec<_>>().join("、");
            Err(AppError::bad(if calc_named {
                format!("公式里的「{name}」是另一个计算值，计算值只能引用普通的汇总值（可用：{have}）")
            } else {
                format!("公式里的「{name}」不是透视表里的值（可用：{}）", if have.is_empty() { "还没有普通的汇总值，请先添加".into() } else { have })
            }))
        };
        calcs.push((i, pivot_calc::parse(formula, &resolve)?));
    }
    Ok(calcs)
}

impl Grid {
    /// 按公式补上每个格子里的计算值（明细、小计、总计、每一列都单独算）
    fn apply_calcs(&mut self, calcs: &Calcs, money: &[bool]) {
        if calcs.is_empty() {
            return;
        }
        for bucket in self.buckets.values_mut() {
            for values in bucket.by_col.values_mut() {
                for (i, e) in calcs {
                    let r = pivot_calc::eval(e, values);
                    values[*i] = if money[*i] { r.map(|x| (x * 100.0).round() / 100.0 + 0.0) } else { r };
                }
            }
        }
    }

    fn cell(&self, prefix: &Key, col: &Option<Key>, vi: usize) -> Option<f64> {
        self.buckets.get(prefix).and_then(|b| b.by_col.get(col)).and_then(|v| v.get(vi).copied().flatten())
    }
}

/// 字符串列表 → SQL 参数占位与参数值
fn in_list(keys: &[String]) -> (String, Vec<Value>) {
    (keys.iter().map(|_| "?").collect::<Vec<_>>().join(", "), keys.iter().map(|k| Value::Text(k.clone())).collect())
}

pub fn pivot(conn: &Connection, dataset_id: &str, req: &PivotRequest) -> AppResult<PivotResult> {
    let started = Instant::now();
    let def = dataset::dataset(conn, dataset_id)?;
    if req.values.is_empty() {
        return Err(AppError::bad("请至少添加一个值字段"));
    }
    if req.rows.len() + req.columns.len() > 6 {
        return Err(AppError::bad("行字段与列字段合计最多 6 个"));
    }
    let rows: Vec<(String, String)> = req.rows.iter().map(|f| dim_sql(&def, f)).collect::<AppResult<_>>()?;
    let cols: Vec<(String, String)> = req.columns.iter().map(|f| dim_sql(&def, f)).collect::<AppResult<_>>()?;
    let shows: Vec<Show> = req.values.iter().map(|v| Show::parse(v.show.as_deref())).collect::<AppResult<_>>()?;
    let mut vals: Vec<(String, ValueInfo)> = req.values.iter().map(|v| value_sql(&def, v)).collect::<AppResult<_>>()?;
    let calcs = compile_calcs(req, &mut vals)?;
    let items = items::compile(req)?;
    if req.top.is_some() && req.values[0].agg == "calc" {
        return Err(AppError::bad("「前 N 项」按第一个值排名，第一个值不能是计算值，请把一个普通的汇总值放在最前面"));
    }
    if calcs.len() == req.values.len() {
        return Err(AppError::bad("计算值要引用普通的汇总值，请至少添加一个普通的值字段"));
    }
    let calc_money: Vec<bool> = vals.iter().map(|(_, i)| i.kind == "money").collect();
    let (where_sql, args) = dataset::filter_sql(conn, &def, &req.query)?;
    let (k, m) = (rows.len(), cols.len());
    let dims: Vec<String> = rows.iter().chain(cols.iter()).map(|(e, _)| e.clone()).collect();
    let d = |i: usize| format!("d{i}");

    // 占比需要的汇总层级即使不显示也要算出来
    let need_totals = shows.iter().any(|s| s.is_pct());
    let need_subtotals = shows.contains(&Show::PctParent);
    if shows.iter().any(|s| matches!(s, Show::PctRow)) && m == 0 {
        return Err(AppError::bad("「占行合计」需要有列字段；没有列字段时请用「占总计」"));
    }
    if shows.iter().any(|s| matches!(s, Show::PctParent)) && k == 0 {
        return Err(AppError::bad("「占上级」需要有行字段"));
    }

    // 规模检查：列组合、叶子行数
    let count_distinct = |range: std::ops::Range<usize>| -> AppResult<i64> {
        if range.is_empty() {
            return Ok(1);
        }
        let tuple = range.map(|i| format!("COALESCE({}, '\u{1}')", dims[i])).collect::<Vec<_>>().join(", ");
        Ok(conn.query_row(
            &format!("SELECT count(*) FROM (SELECT DISTINCT {tuple} FROM {} WHERE {where_sql})", def.table),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )?)
    };
    if m > 0 {
        let n = count_distinct(k..k + m)?;
        if n > MAX_COLUMN_KEYS {
            return Err(AppError::bad(format!("列字段共有 {n} 种取值，超过 {MAX_COLUMN_KEYS} 列上限，请换用取值较少的字段或先筛选")));
        }
    }

    // 「前 N 项」：第一个行字段按第一个值排名
    let mut top_filter = String::new();
    let mut first_level_count = None;
    if let (Some(n), true) = (req.top, k > 0) {
        first_level_count = Some(count_distinct(0..1)?);
        top_filter = format!(
            " AND ({first}) IN (SELECT {first} FROM (SELECT *, {rid} AS {ORD} FROM {t} WHERE {where_sql}) s GROUP BY 1 ORDER BY {v} DESC NULLS LAST LIMIT {n})",
            first = dims[0],
            rid = def.rid,
            t = def.table,
            v = vals[0].0,
        );
    }
    let top_args: Vec<Value> = if top_filter.is_empty() { vec![] } else { args.clone() };

    if k > 0 && req.top.is_none() {
        let n = count_distinct(0..k)?;
        if n > MAX_LEAF_ROWS {
            return Err(AppError::bad(format!("行字段组合共 {n} 行，超过 {MAX_LEAF_ROWS} 行上限，请先筛选或使用「前 N 项」")));
        }
    }

    // 分组集合：行前缀（含小计层级）× {列字段, 跨列}
    let with_subtotals = req.subtotals || need_subtotals;
    let with_totals = req.grand_totals || need_totals;
    let mut sets: Vec<String> = Vec::new();
    let row_levels: Vec<usize> = (0..=k).rev().filter(|&i| i == k || (i > 0 && with_subtotals) || (i == 0 && with_totals)).collect();
    for &i in &row_levels {
        let prefix: Vec<String> = (0..i).map(d).collect();
        if m > 0 {
            let with_cols: Vec<String> = prefix.iter().cloned().chain((k..k + m).map(d)).collect();
            sets.push(format!("({})", with_cols.join(", ")));
            if with_totals {
                sets.push(format!("({})", prefix.join(", ")));
            }
        } else {
            sets.push(format!("({})", prefix.join(", ")));
        }
    }
    let mut all_args = args.clone();
    all_args.extend(top_args);
    let mut grid = run_grid(conn, &def.table, &def.rid, &dims, k, m, &vals, &sets, &format!("{where_sql}{top_filter}"), &all_args)?;
    // 计算项先补进来（只算普通的汇总值），再算计算值，这样计算项的行 / 列上计算值也是对的
    if !items.is_empty() {
        let round: Vec<bool> = vals.iter().map(|(_, i)| i.kind == "money" && keeps_cents(&i.agg)).collect();
        grid.apply_items(&items, k, vals.len(), &round)?;
    }
    grid.apply_calcs(&calcs, &calc_money);
    let buckets = &grid.buckets;
    let mut column_keys: Vec<Key> = grid.column_keys.iter().cloned().collect();
    // 列标签有别名时按别名排序
    let alias_of = |field: &str, key: &Option<String>| -> Option<String> { req.aliases.get(field).and_then(|m| m.get(key.as_deref().unwrap_or(""))).cloned() };
    if req.columns.iter().any(|f| req.aliases.contains_key(&f.column)) {
        column_keys.sort_by_cached_key(|ck| ck.iter().enumerate().map(|(i, x)| alias_of(&req.columns[i].column, x).or_else(|| x.clone())).collect::<Vec<_>>());
    }
    // 计算项排在同级取值的最后（稳定排序：其余顺序不变）
    if !items.is_empty() && m > 0 {
        let calc_cols = items::col_names(&items, k, m);
        column_keys.sort_by_cached_key(|ck| ck.iter().enumerate().map(|(i, x)| calc_cols[i].contains(x.as_deref().unwrap_or("\u{0}"))).collect::<Vec<_>>());
    }
    let calc_rows = items::row_names(&items, k);
    let column_total = m > 0 && req.grand_totals;

    // ── 同比 / 环比：基期查询 ──
    // 取本期显示的各时间键，把基期数据的时间键平移到本期后按同样的分组汇总；
    // 时间字段上的筛选不作用于基期（筛选 2026-10 时基期是 2025-10），其他筛选照常
    let mut base_grids: HashMap<bool, Grid> = HashMap::new();
    let mut base_field_name = None;
    let periods: BTreeSet<bool> = shows.iter().filter_map(|s| s.period()).collect();
    // 本期没有任何数据（如数据集还是空的、筛选后为空）时无需基期，同比 / 环比为空值
    let no_data = !periods.is_empty() && {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM (SELECT 1 FROM {} WHERE {where_sql} LIMIT 1)", def.table), duckdb::params_from_iter(args.iter()), |r| r.get(0))?;
        n == 0
    };
    if !periods.is_empty() && !no_data {
        let fields: Vec<&PivotField> = req.rows.iter().chain(req.columns.iter()).collect();
        let keys_at = |t: usize| -> Vec<String> {
            let mut set: BTreeSet<String> = BTreeSet::new();
            if t < k {
                for p in buckets.keys().filter(|p| p.len() > t) {
                    if let Some(x) = &p[t] {
                        set.insert(x.clone());
                    }
                }
            } else {
                for ck in &column_keys {
                    if let Some(x) = &ck[t - k] {
                        set.insert(x.clone());
                    }
                }
            }
            set.into_iter().collect()
        };
        // 时间字段：指定的，或第一个取值形如年 / 月 / 日的字段（列字段优先，同比常放在列上）
        let order: Vec<usize> = (k..k + m).chain(0..k).collect();
        let t = match &req.base_field {
            Some(f) => fields.iter().position(|x| &x.column == f).ok_or_else(|| AppError::bad(format!("时间字段「{f}」不在行或列中")))?,
            None => *order
                .iter()
                .find(|&&i| period_fmt(&keys_at(i).iter().map(String::as_str).collect::<Vec<_>>()).is_some())
                .ok_or_else(|| AppError::bad("同比 / 环比需要把日期字段放到行或列（按年 / 月 / 日分组，或取值为 2026-10 这类月份）"))?,
        };
        let shown = keys_at(t);
        let fmt = period_fmt(&shown.iter().map(String::as_str).collect::<Vec<_>>())
            .ok_or_else(|| AppError::bad(format!("「{}」的取值不是年 / 月 / 日，无法计算同比 / 环比", if t < k { &rows[t].1 } else { &cols[t - k].1 })))?;
        base_field_name = Some(if t < k { rows[t].1.clone() } else { cols[t - k].1.clone() });
        let time_col = def.columns.iter().find(|c| c.key == fields[t].column || c.title == fields[t].column);
        let mut base_query = req.query.clone();
        base_query.filters.retain(|f| time_col.is_none_or(|c| f.column() != c.key && f.column() != c.title));
        let (base_where, base_args) = dataset::filter_sql(conn, &def, &base_query)?;
        for yearly in periods {
            let mut bdims = dims.clone();
            bdims[t] = shift_sql(&dims[t], fmt, yearly);
            let (ph, mut extra) = in_list(&shown);
            let mut w = format!("{base_where} AND ({}) IN ({ph})", bdims[t]);
            let mut a = base_args.clone();
            a.append(&mut extra);
            // 有「前 N 项」时基期只取本期显示的那些第一级行，合计口径才一致
            if req.top.is_some() && k > 0 && t != 0 {
                let firsts: Vec<String> = buckets.keys().filter(|p| !p.is_empty()).filter_map(|p| p[0].clone()).collect::<BTreeSet<_>>().into_iter().collect();
                let (ph, mut extra) = in_list(&firsts);
                w.push_str(&format!(" AND ({}) IN ({ph})", dims[0]));
                a.append(&mut extra);
            }
            base_grids.insert(yearly, run_grid(conn, &def.table, &def.rid, &bdims, k, m, &vals, &sets, &w, &a)?);
        }
    }

    let col_of = |ci: usize| -> Option<Key> { column_keys.get(ci).cloned() };
    let raw_of = |b: Option<&Bucket>| -> Vec<Vec<Option<f64>>> {
        let mut out: Vec<Vec<Option<f64>>> = column_keys
            .iter()
            .map(|ck| b.and_then(|b| b.by_col.get(&Some(ck.clone())).cloned()).unwrap_or_else(|| vec![None; vals.len()]))
            .collect();
        if m == 0 || column_total {
            out.push(b.and_then(|b| b.by_col.get(&None).cloned()).unwrap_or_else(|| vec![None; vals.len()]));
        }
        out
    };
    let any_show = shows.iter().any(|s| *s != Show::Raw);
    // 按显示方式换算一行的格子
    let display = |prefix: &Key, raw: &[Vec<Option<f64>>]| -> Vec<Vec<Option<f64>>> {
        raw.iter()
            .enumerate()
            .map(|(ci, cell)| {
                let col = col_of(ci);
                cell.iter()
                    .enumerate()
                    .map(|(vi, &v)| match shows[vi] {
                        Show::Raw => v,
                        Show::PctTotal => ratio(v, grid.cell(&vec![], &None, vi)),
                        Show::PctRow => ratio(v, grid.cell(prefix, &None, vi)),
                        Show::PctCol => ratio(v, grid.cell(&vec![], &col, vi)),
                        Show::PctParent => {
                            if prefix.is_empty() {
                                ratio(v, v)
                            } else {
                                ratio(v, grid.cell(&prefix[..prefix.len() - 1].to_vec(), &col, vi))
                            }
                        }
                        s @ (Show::Yoy | Show::YoyDiff | Show::Mom | Show::MomDiff) => {
                            let base = base_grids.get(&s.period().unwrap_or(true)).and_then(|g| g.cell(prefix, &col, vi));
                            if matches!(s, Show::Yoy | Show::Mom) { growth(v, base) } else { diff(v, base) }
                        }
                    })
                    .collect()
            })
            .collect()
    };
    let make_row = |prefix: &Key, kind: &'static str| -> PivotRow {
        let raw = raw_of(buckets.get(prefix));
        if any_show {
            PivotRow { keys: prefix.clone(), kind, cells: display(prefix, &raw), raw: Some(raw) }
        } else {
            PivotRow { keys: prefix.clone(), kind, cells: raw, raw: None }
        }
    };
    // 排序依据：该前缀跨列总计（无总计时用各列之和）的某个值（按原始值排序）
    let sort_value = |prefix: &Key| -> f64 {
        let b = buckets.get(prefix);
        let vi = req.sort.value_index.min(vals.len() - 1);
        b.and_then(|b| b.by_col.get(&None).and_then(|v| v[vi]))
            .or_else(|| b.map(|b| b.by_col.values().filter_map(|v| v[vi]).sum()))
            .unwrap_or(0.0)
    };

    // 叶子行前缀
    let leaves: Vec<Key> = buckets.keys().filter(|p| p.len() == k).cloned().collect();
    // 构建层级：每一级的子节点
    let mut children: HashMap<Key, BTreeSet<Option<String>>> = HashMap::new();
    for leaf in &leaves {
        for i in 0..k {
            children.entry(leaf[..i].to_vec()).or_default().insert(leaf[i].clone());
        }
    }
    let mut out_rows: Vec<PivotRow> = Vec::new();
    fn walk(
        prefix: &Key,
        k: usize,
        children: &HashMap<Key, BTreeSet<Option<String>>>,
        req: &PivotRequest,
        sort_value: &dyn Fn(&Key) -> f64,
        make_row: &dyn Fn(&Key, &'static str) -> PivotRow,
        calc_rows: &[std::collections::HashSet<String>],
        out: &mut Vec<PivotRow>,
    ) {
        if prefix.len() == k {
            out.push(make_row(prefix, "data"));
            return;
        }
        let mut kids: Vec<Option<String>> = children.get(prefix).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        let key_of = |c: &Option<String>| {
            let mut p = prefix.clone();
            p.push(c.clone());
            p
        };
        if req.sort.by == "value" {
            kids.sort_by(|a, b| sort_value(&key_of(a)).total_cmp(&sort_value(&key_of(b))));
        } else if let Some(map) = req.rows.get(prefix.len()).and_then(|f| req.aliases.get(&f.column)) {
            // 按名称排序时用别名（空值仍排最后）
            let text = |c: &Option<String>| map.get(c.as_deref().unwrap_or("")).cloned().or_else(|| c.clone());
            kids.sort_by_key(|a| (a.is_none(), text(a)));
        }
        // 名称排序：BTreeSet 已升序（空值在最前，挪到最后）
        if req.sort.by != "value" && kids.first().is_some_and(|x| x.is_none()) {
            let blank = kids.remove(0);
            kids.push(blank);
        }
        if req.sort.desc {
            kids.reverse();
        }
        // 计算项排在同级最后（稳定划分，其余顺序不变）
        if let Some(names) = calc_rows.get(prefix.len()).filter(|n| !n.is_empty()) {
            kids.sort_by_key(|c| c.as_deref().is_some_and(|x| names.contains(x)));
        }
        for c in kids {
            let p = key_of(&c);
            walk(&p, k, children, req, sort_value, make_row, calc_rows, out);
            if req.subtotals && p.len() < k {
                out.push(make_row(&p, "subtotal"));
            }
        }
    }
    if k > 0 {
        walk(&vec![], k, &children, req, &sort_value, &make_row, &calc_rows, &mut out_rows);
    }
    if req.grand_totals || k == 0 {
        out_rows.push(make_row(&vec![], "total"));
    }

    // 显示方式写进值的说明（放在计算之后：汇总时金额按原类型保留到分）
    for ((v, s), rv) in vals.iter_mut().zip(&shows).zip(&req.values) {
        if *s != Show::Raw {
            // 自定义了表头的不再追加显示方式
            if rv.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
                v.1.label = format!("{} · {}", v.1.label, s.label());
            }
            v.1.show = s.code().into();
            if s.is_pct() || matches!(s, Show::Yoy | Show::Mom) {
                v.1.kind = "percent".into();
            }
        }
    }

    Ok(PivotResult {
        row_fields: rows.into_iter().map(|(_, n)| n).collect(),
        column_fields: cols.into_iter().map(|(_, n)| n).collect(),
        values: vals.into_iter().map(|(_, v)| v).collect(),
        column_keys,
        column_total,
        leaf_rows: leaves.len(),
        rows: out_rows,
        first_level_count,
        base_field: base_field_name,
        elapsed_ms: started.elapsed().as_millis() as i64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_formats() {
        assert_eq!(period_fmt(&["2025", "2026"]), Some(PeriodFmt::Year));
        assert_eq!(period_fmt(&["2026-09", "2026-10"]), Some(PeriodFmt::Month));
        assert_eq!(period_fmt(&["2026-10-01", "2026/10/02"]), Some(PeriodFmt::Day));
        assert_eq!(period_fmt(&["2026-10", "旗舰店"]), None);
        assert_eq!(period_fmt(&["2026-Q3", "2026-Q4"]), Some(PeriodFmt::Quarter));
        assert_eq!(period_fmt(&["2026-W01", "2026-W41"]), Some(PeriodFmt::Week));
        assert_eq!(period_fmt(&["2026-Q5"]), None);
        assert_eq!(period_fmt(&[]), None);
    }

    #[test]
    fn growth_math() {
        assert_eq!(growth(Some(120.0), Some(100.0)), Some(0.2));
        // 基期为负：按绝对值计算方向（亏损收窄为正增长）
        assert_eq!(growth(Some(-50.0), Some(-100.0)), Some(0.5));
        assert_eq!(growth(Some(10.0), Some(0.0)), None);
        assert_eq!(growth(Some(10.0), None), None);
        assert_eq!(diff(Some(10.0), None), Some(10.0));
        assert_eq!(diff(None, Some(3.5)), Some(-3.5));
        assert_eq!(diff(None, None), None);
    }

    #[test]
    fn shift_expressions() {
        let conn = Connection::open_in_memory().unwrap();
        let q = |sql: String| -> String { conn.query_row(&format!("SELECT {sql}"), [], |r| r.get(0)).unwrap() };
        assert_eq!(q(shift_sql("'2025'", PeriodFmt::Year, true)), "2026");
        assert_eq!(q(shift_sql("'2025-10'", PeriodFmt::Month, true)), "2026-10");
        assert_eq!(q(shift_sql("'2026-12'", PeriodFmt::Month, false)), "2027-01");
        assert_eq!(q(shift_sql("'2026-02-28'", PeriodFmt::Day, false)), "2026-03-01");
        // 闰日的同比基期并入 2 月 28 日（DuckDB 日期加一年会落到月末）
        assert_eq!(q(shift_sql("'2024-02-29'", PeriodFmt::Day, true)), "2025-02-28");
        assert_eq!(q(shift_sql("'2025-Q4'", PeriodFmt::Quarter, true)), "2026-Q4");
        assert_eq!(q(shift_sql("'2025-Q4'", PeriodFmt::Quarter, false)), "2026-Q1");
        assert_eq!(q(shift_sql("'2026-Q2'", PeriodFmt::Quarter, false)), "2026-Q3");
        assert_eq!(q(shift_sql("'2025-W41'", PeriodFmt::Week, true)), "2026-W41");
        assert_eq!(q(shift_sql("'2026-W40'", PeriodFmt::Week, false)), "2026-W41");
        // 2020 年有第 53 周：2020-W53 的下一周是 2021-W01；2025-W52 的下一周是 2026-W01
        assert_eq!(q(shift_sql("'2020-W53'", PeriodFmt::Week, false)), "2021-W01");
        assert_eq!(q(shift_sql("'2025-W52'", PeriodFmt::Week, false)), "2026-W01");
    }

    #[test]
    fn date_grouping_expressions() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (d TIMESTAMP, m VARCHAR, s VARCHAR); INSERT INTO t VALUES ('2026-10-05 13:00:00', '2026-11', '2026/01/01'), ('2021-01-03 00:00:00', '2026-03', NULL)").unwrap();
        let def = |col: &str| format!("CAST(({col}) AS VARCHAR)");
        let q = |sql: String| -> Vec<Option<String>> {
            let mut st = conn.prepare(&format!("SELECT {sql} FROM t ORDER BY rowid")).unwrap();
            st.query_map([], |r| r.get::<_, Option<String>>(0)).unwrap().map(Result::unwrap).collect()
        };
        let quarter = |b: String| format!("substr({b}, 1, 4) || '-Q' || CAST((TRY_CAST(substr({b}, 6, 2) AS INTEGER) - 1) // 3 + 1 AS VARCHAR)");
        let week = |b: String| week_key(&format!("TRY_CAST(substr(replace({b}, '/', '-'), 1, 10) AS DATE)"));
        assert_eq!(q(quarter(def("d"))), vec![Some("2026-Q4".into()), Some("2021-Q1".into())]);
        assert_eq!(q(quarter(def("m"))), vec![Some("2026-Q4".into()), Some("2026-Q1".into())]);
        // 2026-10-05 是周一（ISO 第 41 周）；2021-01-03 属于 2020 年第 53 周
        assert_eq!(q(week(def("d"))), vec![Some("2026-W41".into()), Some("2020-W53".into())]);
        assert_eq!(q(week(def("s"))), vec![Some("2026-W01".into()), None]);
    }
}
