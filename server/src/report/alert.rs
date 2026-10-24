//! 阈值告警：指标（可按某一列分组）超过阈值时，推送到企业微信 / 钉钉 / 飞书群或邮箱
//!
//! - 规则（ledger_alert）：数据集 + 指标（汇总值或两个汇总值的比率）+ 分组列 + 筛选 + 时间窗口 + 条件 + 最小样本量 + 计划 + 渠道。
//!   整条规则存成 JSON（`spec`），以后加字段不用改表
//! - 评估用透视引擎（与仪表盘、公式同一套计算）：每个分组得到一个值，再套条件。受数据范围限制的用户不能建告警（见 auth.rs），
//!   告警按服务账号读数据，不受创建人范围限制
//! - **去重**：`ledger_alert_state` 记录当前处于触发中的分组。新触发才通知；持续触发按「重复」策略决定是否提醒；
//!   恢复（评估过、不再满足条件）后删除状态（可选先发一条「已恢复」通知），再次触发重新通知。样本量不足 / 没有数据的分组不下结论，状态保持不变
//! - **判断依据**（`Condition.basis`）：当前值，或较上期的变化（相对百分比 / 绝对差）。上期是紧挨着本期之前、长度相同的一段，
//!   所以要设时间窗口；本期没有数据的分组，对「求和 / 计数」类指标按 0 处理（销量掉到 0 正是要抓的情况）
//! - **数据停更**（`Metric::Freshness`）：指标是「某日期列的最新一条距今几天」，可按分组（每家店铺多久没有新订单了）
//! - 通知发送失败（所有渠道都没发出去）不记状态，下次评估重试
//! - 渠道、计划、消息渲染复用定时推送（本模块是 `report` 的子模块，直接使用它的私有项）
//! - 每次评估写一条记录（ledger_alert_run），含各渠道的结果

use std::collections::HashMap;

use serde_json::Value as Json;

use super::*;
use crate::auth::CurrentUser;
use crate::dataset::{self, ColumnDef, DatasetDef, Filter, QuerySpec};
use crate::pivot::{self, PivotField, PivotRequest, PivotSort, PivotValue};
use super::content::TableBlock;

/// 消息里最多列出的分组数
const MAX_LISTED: usize = 20;
/// 试算最多返回的分组数
const MAX_PREVIEW_ROWS: usize = 200;
/// 单条规则一次评估的总时限（含发送）
const RUN_TIMEOUT: Duration = Duration::from_secs(300);
const AGGS: &[&str] = &["sum", "count", "avg", "min", "max", "count_distinct"];

fn on() -> bool {
    true
}

// ── 规则 ──────────────────────────────────────────────

/// 一个汇总值：对某列求和 / 计数…（count 可不选列）
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(default)]
    pub column: Option<String>,
    pub agg: String,
}

/// 被监控的指标
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Metric {
    /// 单个汇总值，如订单金额求和
    Agg {
        #[serde(default)]
        column: Option<String>,
        agg: String,
    },
    /// 两个汇总值的比率（汇总之后再相除，不是各行比率的平均），如 退款额 / 成交额；分母为 0 时该分组没有值
    Ratio { numerator: Part, denominator: Part },
    /// 数据停更：某日期列的最新一条距今天几天（按分组时逐组算），如「A 店已 5 天没有新订单」
    Freshness {
        #[serde(rename = "dateColumn")]
        date_column: String,
    },
}

/// 时间窗口：只统计这段时间内的数据。预设按评估当天计算，且**不含今天**（今天的数据还没跑完）
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Window {
    pub date_column: Option<String>,
    /// none / yesterday / last_7 / last_30 / this_month / last_month
    pub preset: String,
}

impl Default for Window {
    fn default() -> Self {
        Self { date_column: None, preset: "none".into() }
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Gt,
    Gte,
    Lt,
    Lte,
}

impl Op {
    fn holds(self, v: f64, threshold: f64) -> bool {
        match self {
            Op::Gt => v > threshold,
            Op::Gte => v >= threshold,
            Op::Lt => v < threshold,
            Op::Lte => v <= threshold,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Op::Gt => ">",
            Op::Gte => "≥",
            Op::Lt => "<",
            Op::Lte => "≤",
        }
    }

    /// 越大越糟（超过上限）还是越小越糟（低于下限）：决定列表里谁排前面
    fn bigger_is_worse(self) -> bool {
        matches!(self, Op::Gt | Op::Gte)
    }
}

/// 条件判断的对象
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// 本期的指标值
    #[default]
    Value,
    /// 较上期的相对变化：(本期 − 上期) / |上期|，如下降 20% 写 −0.2；上期为 0 时不判断
    ChangePct,
    /// 较上期的绝对变化：本期 − 上期（比率指标是百分点，如上升 3 个百分点写 0.03）
    ChangeAbs,
}

impl Basis {
    fn is_change(self) -> bool {
        self != Basis::Value
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Condition {
    pub op: Op,
    /// 阈值（比率用小数，如 5% 写 0.05；变化率同理）
    pub threshold: f64,
    #[serde(default)]
    pub basis: Basis,
}

/// 最小样本量：分母太小的分组不判断（2 单退 1 单不该算退款率 50%）
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Guard {
    #[serde(default)]
    pub column: Option<String>,
    pub agg: String,
    pub min: f64,
}

/// 持续触发时是否重复提醒
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Repeat {
    /// 只在新触发时通知一次，恢复后再触发才重新通知
    #[default]
    UntilRecovered,
    /// 每次评估仍然触发就通知
    EveryRun,
    /// 距上次通知超过 N 小时仍在触发才再提醒
    EveryHours { hours: u32 },
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AlertInput {
    pub name: String,
    pub dataset_id: String,
    pub metric: Metric,
    /// 指标显示名（如「退款率」）；为空时按指标自动生成
    #[serde(default)]
    pub metric_label: Option<String>,
    /// 分组列：按它拆开，逐组判断；为空则整体一个值
    #[serde(default)]
    pub group_by: Option<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub window: Window,
    pub condition: Condition,
    #[serde(default)]
    pub guard: Option<Guard>,
    pub schedule: Schedule,
    pub channel_ids: Vec<String>,
    #[serde(default)]
    pub repeat: Repeat,
    /// 恢复正常（之前通知过的分组不再满足条件）时，也发一条「已恢复」通知
    #[serde(default)]
    pub notify_recovery: bool,
    #[serde(default = "on")]
    pub enabled: bool,
}

#[derive(Clone)]
struct StoredAlert {
    id: String,
    input: AlertInput,
    created_by: String,
    created_by_name: String,
    created_at: String,
    updated_at: String,
}

/// 决定「要不要把已有的触发状态作废」的那部分定义：这些变了，旧状态就不再有意义
fn definition_key(i: &AlertInput) -> String {
    serde_json::json!([i.dataset_id, i.metric, i.group_by, i.filters, i.window, i.condition, i.guard]).to_string()
}

// ── 编译与评估 ────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fmt {
    Money,
    Percent,
    Number,
    /// 天数（数据停更）
    Days,
}

fn fmt_value(v: f64, fmt: Fmt) -> String {
    match fmt {
        Fmt::Percent => format!("{:.1}%", v * 100.0),
        Fmt::Money => content::compact(Some(v), true),
        Fmt::Number => content::compact(Some(v), false),
        Fmt::Days => format!("{} 天", v.round() as i64),
    }
}

/// 绝对变化量，带正负号；比率的变化是「百分点」
fn fmt_delta(v: f64, fmt: Fmt) -> String {
    match fmt {
        Fmt::Percent => format!("{:+.1} 个百分点", v * 100.0),
        Fmt::Days => format!("{:+} 天", v.round() as i64),
        Fmt::Money | Fmt::Number => format!("{}{}", if v < 0.0 { "-" } else { "+" }, content::compact(Some(v.abs()), fmt == Fmt::Money)),
    }
}

/// 条件里的阈值 / 变化量怎么写
fn fmt_threshold(v: f64, basis: Basis, fmt: Fmt) -> String {
    match basis {
        Basis::Value => fmt_value(v, fmt),
        Basis::ChangePct => content::percent(Some(v)),
        Basis::ChangeAbs => fmt_delta(v, fmt),
    }
}


fn agg_cn(agg: &str) -> &'static str {
    match agg {
        "sum" => "求和",
        "count" => "计数",
        "avg" => "平均",
        "min" => "最小",
        "max" => "最大",
        "count_distinct" => "去重计数",
        _ => "",
    }
}

fn find_col<'a>(def: &'a DatasetDef, name: &str) -> AppResult<&'a ColumnDef> {
    def.columns.iter().find(|c| c.key == name || c.title == name).ok_or_else(|| AppError::bad(format!("数据集「{}」没有「{name}」列", def.name)))
}

fn blank(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// 一个汇总值 → 透视值；返回 (值, 是否金额, 显示名)
fn part_value(def: &DatasetDef, column: &Option<String>, agg: &str, label: Option<&str>) -> AppResult<(PivotValue, bool, String)> {
    if !AGGS.contains(&agg) {
        return Err(AppError::bad(format!("不支持的汇总方式：{agg}")));
    }
    let col = blank(column).map(|c| find_col(def, c)).transpose()?;
    if col.is_none() && agg != "count" {
        return Err(AppError::bad(format!("「{}」需要选择一列", agg_cn(agg))));
    }
    let money = col.is_some_and(|c| c.kind == "money") && matches!(agg, "sum" | "avg" | "min" | "max");
    let shown = format!("{}（{}）", col.map(|c| c.title.as_str()).unwrap_or("行数"), agg_cn(agg));
    let value = PivotValue { column: col.map(|c| c.key.clone()), agg: agg.to_string(), label: label.map(String::from), ..Default::default() };
    Ok((value, money, shown))
}

/// 「量」类指标：求和 / 计数 / 去重计数（某段时间没有数据就是 0）
fn is_volume(metric: &Metric) -> bool {
    matches!(metric, Metric::Agg { agg, .. } if matches!(agg.as_str(), "sum" | "count" | "count_distinct"))
}

struct Compiled {
    values: Vec<PivotValue>,
    /// 被判断的值在 values 里的下标
    metric: usize,
    guard: Option<usize>,
    fmt: Fmt,
    label: String,
    /// 数据停更：日期列（不走透视，单独算）
    freshness: Option<String>,
    /// 「量」类指标（求和 / 计数 / 去重计数）：本期没有数据就是 0，比较上期时样本量只看上期
    volume: bool,
}

fn compile(def: &DatasetDef, input: &AlertInput) -> AppResult<Compiled> {
    if let Metric::Freshness { date_column } = &input.metric {
        let col = find_col(def, date_column)?;
        if col.kind != "datetime" {
            return Err(AppError::bad(format!("「{}」不是日期列，不能监控数据停更", col.title)));
        }
        let label = blank(&input.metric_label).map(String::from).unwrap_or_else(|| format!("「{}」最新数据距今", col.title));
        return Ok(Compiled { values: vec![], metric: 0, guard: None, fmt: Fmt::Days, label, freshness: Some(col.key.clone()), volume: false });
    }
    let volume = is_volume(&input.metric);
    let (mut values, metric, fmt, auto_label) = match &input.metric {
        Metric::Agg { column, agg } => {
            let (v, money, shown) = part_value(def, column, agg, None)?;
            (vec![v], 0, if money { Fmt::Money } else { Fmt::Number }, shown)
        }
        Metric::Ratio { numerator, denominator } => {
            let (n, _, n_shown) = part_value(def, &numerator.column, &numerator.agg, Some("分子"))?;
            let (d, _, d_shown) = part_value(def, &denominator.column, &denominator.agg, Some("分母"))?;
            let ratio = PivotValue { agg: "calc".into(), formula: Some("[分子]/[分母]".into()), format: Some("percent".into()), label: Some("指标".into()), ..Default::default() };
            (vec![n, d, ratio], 2, Fmt::Percent, format!("{n_shown} / {d_shown}"))
        }
        Metric::Freshness { .. } => unreachable!("数据停更在上面已处理"),
    };
    let mut guard = None;
    if let Some(g) = &input.guard {
        let (v, _, _) = part_value(def, &g.column, &g.agg, Some("样本量"))?;
        values.push(v);
        guard = Some(values.len() - 1);
    }
    let label = blank(&input.metric_label).map(String::from).unwrap_or(auto_label);
    Ok(Compiled { values, metric, guard, fmt, label, freshness: None, volume })
}

/// 时间窗口预设 → (起, 止, 说明)。都不含今天，除了「本月」（到今天为止的这个月，月底前是不完整的）
pub fn window_range(preset: &str, today: NaiveDate) -> Option<(String, String, String)> {
    let d = |n: i64| (today - chrono::Duration::days(n)).format("%Y-%m-%d").to_string();
    Some(match preset {
        "yesterday" => (d(1), d(1), "昨天".into()),
        "last_7" => (d(7), d(1), "近 7 天（不含今天）".into()),
        "last_30" => (d(30), d(1), "近 30 天（不含今天）".into()),
        "this_month" | "last_month" => {
            let (a, b, l) = content::preset_range(preset, today)?;
            (a, b, l.to_string())
        }
        _ => return None,
    })
}

/// 能做「较上期」比较的时间窗口（本月不行：月初几天本期太短，和整月比没有意义）
const COMPARABLE: &[&str] = &["yesterday", "last_7", "last_30", "last_month"];

/// 「较上期」：本期和紧挨着它之前、长度相同的一段 → ((本期起, 本期止), (上期起, 上期止))
pub fn compare_windows(preset: &str, today: NaiveDate) -> Option<((String, String), (String, String))> {
    let d = |n: i64| (today - chrono::Duration::days(n)).format("%Y-%m-%d").to_string();
    Some(match preset {
        "yesterday" => ((d(1), d(1)), (d(2), d(2))),
        "last_7" => ((d(7), d(1)), (d(14), d(8))),
        "last_30" => ((d(30), d(1)), (d(60), d(31))),
        "last_month" => {
            let (a, b, _) = content::preset_range("last_month", today)?;
            let first = NaiveDate::parse_from_str(&a, "%Y-%m-%d").ok()?;
            let prev_last = first - chrono::Duration::days(1);
            let prev_first = prev_last.with_day(1)?;
            ((a, b), (prev_first.format("%Y-%m-%d").to_string(), prev_last.format("%Y-%m-%d").to_string()))
        }
        _ => return None,
    })
}

fn between(column: &str, (min, max): &(String, String)) -> Filter {
    Filter::Between { column: column.to_string(), min: Some(Json::String(min.clone())), max: Some(Json::String(format!("{max} 23:59:59"))) }
}

/// 一条筛选的说明（如「状态：已退款」），写进消息；取值多时只列前几个
fn filter_desc(def: &DatasetDef, f: &Filter) -> String {
    let name = |c: &str| def.columns.iter().find(|x| x.key == c || x.title == c).map(|x| x.title.clone()).unwrap_or_else(|| c.to_string());
    let list = |values: &[String]| {
        let shown: Vec<&str> = values.iter().take(3).map(|v| if v.is_empty() { "（空）" } else { v.as_str() }).collect();
        if values.len() > 3 { format!("{} 等 {} 项", shown.join("、"), values.len()) } else { shown.join("、") }
    };
    let bound = |v: &Option<Json>| v.as_ref().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).unwrap_or_default();
    match f {
        Filter::In { column, values } => format!("{}：{}", name(column), list(values)),
        Filter::Between { column, min, max } => format!("{}：{} ~ {}", name(column), bound(min), bound(max)),
        Filter::Contains { column, value } => format!("{} 包含「{value}」", name(column)),
        Filter::Prefix { column, value } => format!("{}：{value}…", name(column)),
        Filter::Prefixes { column, values } => format!("{}：{}…", name(column), list(values)),
    }
}

/// 规则的筛选 + 时间窗口 → 实际筛选条件与窗口说明
fn effective_filters(input: &AlertInput, today: NaiveDate) -> AppResult<(Vec<Filter>, Option<String>)> {
    let mut filters = input.filters.clone();
    if input.window.preset == "none" {
        return Ok((filters, None));
    }
    let column = blank(&input.window.date_column).ok_or_else(|| AppError::bad("设置了时间窗口，请选择日期列"))?;
    let (min, max, label) = window_range(&input.window.preset, today).ok_or_else(|| AppError::bad(format!("未知的时间窗口：{}", input.window.preset)))?;
    let desc = format!("{column}：{label}（{min} ~ {max}）");
    filters.push(Filter::Between { column: column.to_string(), min: Some(Json::String(min)), max: Some(Json::String(format!("{max} 23:59:59"))) });
    Ok((filters, Some(desc)))
}

#[derive(Clone, Debug)]
pub struct GroupValue {
    pub key: String,
    pub value: Option<f64>,
    pub guard: Option<f64>,
    /// 上期的值和样本量（只在「较上期变化」时有）
    pub prev: Option<f64>,
    pub prev_guard: Option<f64>,
    /// 被拿去和阈值比较的数：当前值，或较上期的变化
    pub judged: Option<f64>,
    /// 附加说明（数据停更：最新数据的日期）
    pub note: Option<String>,
}

impl GroupValue {
    fn new(key: String, value: Option<f64>, guard: Option<f64>) -> Self {
        Self { key, value, guard, prev: None, prev_guard: None, judged: value, note: None }
    }

    /// 按判断依据算出要和阈值比较的数
    fn judge(&mut self, basis: Basis) {
        self.judged = match basis {
            Basis::Value => self.value,
            Basis::ChangePct => self.value.zip(self.prev).and_then(|(c, p)| (p != 0.0).then(|| (c - p) / p.abs())),
            Basis::ChangeAbs => self.value.zip(self.prev).map(|(c, p)| c - p),
        };
    }
}

struct Evaluation {
    groups: Vec<GroupValue>,
    dataset_name: String,
    metric_label: String,
    fmt: Fmt,
    basis: Basis,
    window_desc: Option<String>,
    /// 规则里设置的筛选，写进消息里，让收到的人知道统计的是哪部分数据
    filter_descs: Vec<String>,
    elapsed_ms: i64,
}

impl Evaluation {
    /// 表头里「值」的那几列：判断当前值时一列，比较上期时三列
    fn value_headers(&self) -> Vec<String> {
        if self.basis.is_change() { vec!["本期".into(), "上期".into(), "变化".into()] } else { vec![self.metric_label.clone()] }
    }

    fn value_cells(&self, g: &GroupValue) -> Vec<String> {
        let fv = |v: Option<f64>| v.map(|v| fmt_value(v, self.fmt)).unwrap_or_else(|| "—".into());
        if self.basis.is_change() {
            vec![fv(g.value), fv(g.prev), g.judged.map(|v| fmt_threshold(v, self.basis, self.fmt)).unwrap_or_else(|| "—".into())]
        } else {
            let mut v = fv(g.value);
            if let Some(n) = &g.note {
                v.push_str(&format!("（{n}）"));
            }
            vec![v]
        }
    }
}

/// 一段时间里各分组的指标值（透视引擎算）
fn eval_groups(conn: &Connection, def: &DatasetDef, c: &Compiled, group: Option<&str>, filters: Vec<Filter>) -> AppResult<Vec<GroupValue>> {
    let req = PivotRequest {
        rows: group.map(|g| vec![PivotField { column: g.to_string(), granularity: None, label: None }]).unwrap_or_default(),
        columns: vec![],
        values: c.values.clone(),
        query: QuerySpec { filters, ..Default::default() },
        sort: PivotSort { by: "key".into(), value_index: 0, desc: false },
        top: None,
        subtotals: false,
        // 不分组时读总计行；分组时读各组的明细行
        grand_totals: group.is_none(),
        base_field: None,
        aliases: Default::default(),
        calc_items: vec![],
    };
    let r = pivot::pivot(conn, &def.id, &req)?;
    let want = if group.is_some() { "data" } else { "total" };
    Ok(r
        .rows
        .iter()
        .filter(|row| row.kind == want)
        .map(|row| {
            let cells = row.cells.last().cloned().unwrap_or_default();
            GroupValue::new(
                if group.is_some() { row.keys.first().cloned().flatten().unwrap_or_else(|| "（空）".into()) } else { String::new() },
                cells.get(c.metric).copied().flatten(),
                c.guard.and_then(|i| cells.get(i).copied().flatten()),
            )
        })
        .collect())
}

/// 把上期的值并进本期。只在上期有的分组：「量」类指标按 0 处理（销量掉到 0 正是要抓的），其他指标没有可比的本期，不判断
fn merge_periods(cur: Vec<GroupValue>, prev: Vec<GroupValue>, volume: bool) -> Vec<GroupValue> {
    let mut prev: Vec<(String, GroupValue)> = prev.into_iter().map(|g| (g.key.clone(), g)).collect();
    let mut out = Vec::with_capacity(cur.len());
    for mut g in cur {
        match prev.iter().position(|(k, _)| *k == g.key) {
            Some(i) => {
                let (_, p) = prev.swap_remove(i);
                g.prev = p.value;
                g.prev_guard = p.guard;
            }
            // 上期没有这个分组：「量」类指标上期就是 0
            None if volume => {
                g.prev = Some(0.0);
                g.prev_guard = Some(0.0);
            }
            None => {}
        }
        out.push(g);
    }
    if volume {
        prev.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, p) in prev {
            let mut g = GroupValue::new(key, Some(0.0), None);
            g.prev = p.value;
            g.prev_guard = p.guard;
            out.push(g);
        }
    }
    out
}

/// 数据停更：各分组日期列的最新一条距今几天
fn eval_freshness(conn: &Connection, def: &DatasetDef, date_column: &str, group: Option<&str>, filters: &[Filter], today: NaiveDate) -> AppResult<Vec<GroupValue>> {
    /// 分组数上限，和分组透视一致的量级
    const MAX_FRESH_GROUPS: usize = 50_000;
    let date_expr = dataset::column_sql(find_col(def, date_column)?);
    let (where_sql, args) = dataset::filter_sql(conn, def, &QuerySpec { filters: filters.to_vec(), ..Default::default() })?;
    let latest = format!("max(TRY_CAST(substr(CAST(({date_expr}) AS VARCHAR), 1, 10) AS DATE))::VARCHAR");
    let sql = match group {
        Some(g) => format!(
            "SELECT CAST(({}) AS VARCHAR), {latest} FROM {} WHERE {where_sql} GROUP BY 1 LIMIT {}",
            dataset::column_sql(find_col(def, g)?),
            def.table,
            MAX_FRESH_GROUPS + 1
        ),
        None => format!("SELECT '', {latest} FROM {} WHERE {where_sql}", def.table),
    };
    let rows: Vec<(Option<String>, Option<String>)> = {
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_map(duckdb::params_from_iter(args.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?
    };
    if rows.len() > MAX_FRESH_GROUPS {
        return Err(AppError::bad("分组列的取值太多（超过 5 万），请换一个取值较少的列"));
    }
    let mut out: Vec<GroupValue> = rows
        .into_iter()
        .map(|(key, latest)| {
            let date = latest.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
            let mut g = GroupValue::new(if group.is_some() { key.unwrap_or_else(|| "（空）".into()) } else { String::new() }, date.map(|d| (today - d).num_days() as f64), None);
            g.note = date.map(|d| format!("最新 {d}"));
            g
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

fn evaluate(conn: &Connection, input: &AlertInput, today: NaiveDate) -> AppResult<Evaluation> {
    let started = std::time::Instant::now();
    let def = dataset::dataset(conn, &input.dataset_id)?;
    let c = compile(&def, input)?;
    let group = blank(&input.group_by);
    if let Some(g) = group {
        find_col(&def, g)?;
    }
    let basis = input.condition.basis;
    crate::usage::hit(&def.id, crate::usage::REPORT);
    let (mut groups, window_desc) = if let Some(date_column) = &c.freshness {
        (eval_freshness(conn, &def, date_column, group, &input.filters, today)?, None)
    } else if basis.is_change() {
        let column = blank(&input.window.date_column).ok_or_else(|| AppError::bad("比较上期需要设置时间窗口，请选择日期列"))?;
        let (cur, prev) = compare_windows(&input.window.preset, today).ok_or_else(|| AppError::bad("「较上期」只支持：昨天、近 7 天、近 30 天、上月"))?;
        let with = |range: &(String, String)| input.filters.iter().cloned().chain(std::iter::once(between(column, range))).collect::<Vec<_>>();
        let cur_groups = eval_groups(conn, &def, &c, group, with(&cur))?;
        let prev_groups = eval_groups(conn, &def, &c, group, with(&prev))?;
        let desc = format!("{column}：本期 {} ~ {}，上期 {} ~ {}", cur.0, cur.1, prev.0, prev.1);
        (merge_periods(cur_groups, prev_groups, c.volume), Some(desc))
    } else {
        let (filters, window_desc) = effective_filters(input, today)?;
        (eval_groups(conn, &def, &c, group, filters)?, window_desc)
    };
    groups.iter_mut().for_each(|g| g.judge(basis));
    let filter_descs = input.filters.iter().map(|f| filter_desc(&def, f)).collect();
    Ok(Evaluation { groups, dataset_name: def.name, metric_label: c.label, fmt: c.fmt, basis, window_desc, filter_descs, elapsed_ms: started.elapsed().as_millis() as i64 })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// 满足条件（超标）
    Hit,
    /// 评估过，不满足条件
    Clear,
    /// 没有值或样本量不足：不下结论
    Unknown,
}

/// 样本量够不够：判断当前值时看本期；比较上期时，「量」类指标只看上期（基数够大），其他指标本期和上期都要够
fn guard_passes(g: &GroupValue, gd: &Guard, basis: Basis, volume: bool) -> bool {
    let ok = |x: Option<f64>| x.is_some_and(|x| x >= gd.min);
    match (basis.is_change(), volume) {
        (false, _) => ok(g.guard),
        (true, true) => ok(g.prev_guard),
        (true, false) => ok(g.guard) && ok(g.prev_guard),
    }
}

fn verdict(g: &GroupValue, cond: &Condition, guard: Option<&Guard>, volume: bool) -> Verdict {
    let Some(v) = g.judged.filter(|v| v.is_finite()) else { return Verdict::Unknown };
    if let Some(gd) = guard
        && !guard_passes(g, gd, cond.basis, volume)
    {
        return Verdict::Unknown;
    }
    if cond.op.holds(v, cond.threshold) { Verdict::Hit } else { Verdict::Clear }
}

// ── 触发状态与通知决策 ────────────────────────────────

#[derive(Clone, Debug)]
struct Prev {
    since: NaiveDateTime,
    last_notified: Option<NaiveDateTime>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Notice {
    /// 新触发
    New,
    /// 仍在触发，按重复策略再次提醒
    Reminder,
}

/// 处于触发中的分组，这次要不要通知
fn next_notice(prev: Option<&Prev>, repeat: &Repeat, now: NaiveDateTime) -> Option<Notice> {
    let Some(p) = prev else { return Some(Notice::New) };
    match repeat {
        Repeat::UntilRecovered => None,
        Repeat::EveryRun => Some(Notice::Reminder),
        Repeat::EveryHours { hours } => {
            let last = p.last_notified.unwrap_or(p.since);
            (now - last >= chrono::Duration::hours(i64::from(*hours))).then_some(Notice::Reminder)
        }
    }
}

struct Hit {
    group: GroupValue,
    notice: Option<Notice>,
    prev: Option<Prev>,
}

/// 评估过、已恢复的分组（原来处于触发中）
struct Cleared {
    group: GroupValue,
    since: NaiveDateTime,
}

struct Plan {
    ev: Evaluation,
    /// 满足条件的分组（越糟越靠前）
    hits: Vec<Hit>,
    clear: Vec<Cleared>,
    unknown: usize,
}

fn load_states(conn: &Connection, alert_id: &str) -> AppResult<HashMap<String, Prev>> {
    let mut stmt = conn.prepare(
        "SELECT group_key, strftime(since, '%Y-%m-%d %H:%M:%S'), strftime(last_notified, '%Y-%m-%d %H:%M:%S') FROM ledger_alert_state WHERE alert_id = ?",
    )?;
    let rows = stmt
        .query_map(params![alert_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(k, since, last)| Some((k, Prev { since: since.as_deref().and_then(parse_ts)?, last_notified: last.as_deref().and_then(parse_ts) })))
        .collect())
}

fn sort_worst_first(groups: &mut [(f64, usize)], op: Op) {
    groups.sort_by(|a, b| {
        let o = a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal);
        if op.bigger_is_worse() { o.reverse() } else { o }
    });
}

fn make_plan(conn: &Connection, alert: &StoredAlert, now: NaiveDateTime) -> AppResult<Plan> {
    let input = &alert.input;
    let ev = evaluate(conn, input, now.date())?;
    let volume = is_volume(&input.metric);
    let prev = load_states(conn, &alert.id)?;
    let mut hits = Vec::new();
    let mut clear = Vec::new();
    let mut unknown = 0;
    for g in &ev.groups {
        match verdict(g, &input.condition, input.guard.as_ref(), volume) {
            Verdict::Hit => {
                let p = prev.get(&g.key);
                hits.push(Hit { group: g.clone(), notice: next_notice(p, &input.repeat, now), prev: p.cloned() });
            }
            Verdict::Clear => {
                if let Some(p) = prev.get(&g.key) {
                    clear.push(Cleared { group: g.clone(), since: p.since });
                }
            }
            Verdict::Unknown => unknown += 1,
        }
    }
    let mut order: Vec<(f64, usize)> = hits.iter().enumerate().map(|(i, h)| (h.group.judged.unwrap_or(0.0), i)).collect();
    sort_worst_first(&mut order, input.condition.op);
    let mut sorted: Vec<Option<Hit>> = hits.into_iter().map(Some).collect();
    let hits = order.into_iter().filter_map(|(_, i)| sorted[i].take()).collect();
    Ok(Plan { ev, hits, clear, unknown })
}

// ── 消息 ──────────────────────────────────────────────

fn condition_text(input: &AlertInput, label: &str, fmt: Fmt) -> String {
    let c = &input.condition;
    let subject = if c.basis.is_change() { format!("{label} 较上期变化") } else { label.to_string() };
    format!("{subject} {} {}", c.op.symbol(), fmt_threshold(c.threshold, c.basis, fmt))
}

/// 消息里的「规则」说明：条件 + 样本量
fn rule_text(input: &AlertInput, ev: &Evaluation) -> String {
    let mut r = condition_text(input, &ev.metric_label, ev.fmt);
    if let Some(g) = &input.guard {
        r.push_str(&format!("；{}", guard_text(g)));
    }
    r
}

fn group_label(input: &AlertInput, g: &GroupValue) -> String {
    if input.group_by.is_some() { g.key.clone() } else { "整体".into() }
}

fn guard_text(g: &Guard) -> String {
    let col = blank(&g.column).unwrap_or("行数");
    format!("样本量（{col}{}）≥ {}", agg_cn(&g.agg), content::compact(Some(g.min), false))
}

fn duration_text(d: chrono::Duration) -> String {
    if d.num_hours() < 48 { format!("{} 小时", d.num_hours().max(1)) } else { format!("{} 天", d.num_days()) }
}

fn build_content(alert: &StoredAlert, plan: &Plan, now: NaiveDateTime) -> ReportContent {
    let input = &alert.input;
    let ev = &plan.ev;
    let mut filters = Vec::new();
    filters.extend(ev.window_desc.clone());
    filters.extend(ev.filter_descs.iter().cloned());
    let rule = rule_text(input, ev);
    let shown: Vec<&Hit> = plan.hits.iter().filter(|h| h.notice.is_some()).take(MAX_LISTED).collect();
    let notices = plan.hits.iter().filter(|h| h.notice.is_some()).count();
    let rows = shown
        .iter()
        .map(|h| {
            let status = match (h.notice, &h.prev) {
                (Some(Notice::Reminder), Some(p)) => format!("持续 {}", duration_text(now - p.since)),
                _ => "新触发".to_string(),
            };
            let mut row = vec![group_label(input, &h.group)];
            row.extend(ev.value_cells(&h.group));
            row.push(status);
            row
        })
        .collect();
    let title = if input.group_by.is_some() {
        format!("{notices} 项超出条件{}（{rule}）", if notices > shown.len() { format!("，显示最严重的 {}", shown.len()) } else { String::new() })
    } else {
        format!("当前值（{rule}）")
    };
    let header = std::iter::once(blank(&input.group_by).unwrap_or("整体").to_string()).chain(ev.value_headers()).chain(std::iter::once("状态".to_string())).collect();
    ReportContent {
        workbook_name: "告警".into(),
        sheet_name: ev.dataset_name.clone(),
        filters,
        blocks: vec![Block::Table(TableBlock {
            title,
            header,
            rows,
            total: None,
            raw: vec![],
            raw_total: None,
            kinds: vec![],
        })],
        generated_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
    }
}

/// 「已恢复」消息：之前通知过、这次评估不再满足条件的分组
fn build_recovery_content(alert: &StoredAlert, plan: &Plan, now: NaiveDateTime) -> ReportContent {
    let input = &alert.input;
    let ev = &plan.ev;
    let mut filters = Vec::new();
    filters.extend(ev.window_desc.clone());
    filters.extend(ev.filter_descs.iter().cloned());
    let rule = rule_text(input, ev);
    let rows = plan
        .clear
        .iter()
        .take(MAX_LISTED)
        .map(|c| {
            let mut row = vec![group_label(input, &c.group)];
            row.extend(ev.value_cells(&c.group));
            row.push(format!("已恢复（共持续 {}）", duration_text(now - c.since)));
            row
        })
        .collect();
    let n = plan.clear.len();
    let title = if input.group_by.is_some() {
        format!("{n} 项已恢复正常{}（{rule}）", if n > MAX_LISTED { format!("，显示前 {MAX_LISTED} 项") } else { String::new() })
    } else {
        format!("已恢复正常（{rule}）")
    };
    let header = std::iter::once(blank(&input.group_by).unwrap_or("整体").to_string()).chain(ev.value_headers()).chain(std::iter::once("状态".to_string())).collect();
    ReportContent {
        workbook_name: "告警".into(),
        sheet_name: ev.dataset_name.clone(),
        filters,
        blocks: vec![Block::Table(TableBlock { title, header, rows, total: None, raw: vec![], raw_total: None, kinds: vec![] })],
        generated_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
    }
}

// ── 存取与校验 ────────────────────────────────────────

fn load_alerts(conn: &Connection) -> AppResult<Vec<StoredAlert>> {
    let mut stmt = conn.prepare(
        "SELECT id, spec, created_by, created_by_name, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(updated_at, '%Y-%m-%d %H:%M:%S') FROM ledger_alert ORDER BY created_at, id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, spec, by, by_name, created, updated)| match serde_json::from_str::<AlertInput>(&spec) {
            Ok(input) => Some(StoredAlert {
                id,
                input,
                created_by: by.unwrap_or_default(),
                created_by_name: by_name.unwrap_or_default(),
                created_at: created.unwrap_or_default(),
                updated_at: updated.unwrap_or_default(),
            }),
            Err(e) => {
                tracing::warn!("告警规则 {id} 无法解析，已跳过：{e}");
                None
            }
        })
        .collect())
}

fn load_alert(conn: &Connection, id: &str) -> AppResult<StoredAlert> {
    load_alerts(conn)?.into_iter().find(|a| a.id == id).ok_or_else(|| AppError::bad("告警规则不存在"))
}

fn may_manage(user: &CurrentUser, created_by: &str) -> AppResult<()> {
    if !user.enforced || user.admin || (!created_by.is_empty() && created_by == user.id) {
        return Ok(());
    }
    Err(AppError::Forbidden("只有创建人或管理员可以修改这条告警".into()))
}

/// 规则本身（数据集、指标、分组、筛选、窗口、条件）是否可用；试算也走这里
fn validate_rule(conn: &Connection, input: &AlertInput) -> AppResult<()> {
    let def = dataset::dataset(conn, &input.dataset_id)?;
    let compiled = compile(&def, input)?;
    if let Some(g) = blank(&input.group_by) {
        find_col(&def, g)?;
    }
    for f in &input.filters {
        find_col(&def, f.column())?;
    }
    if compiled.freshness.is_some() {
        if input.window.preset != "none" {
            return Err(AppError::bad("数据停更看的是「最新一条数据距今几天」，不能再设时间窗口"));
        }
        if input.guard.is_some() {
            return Err(AppError::bad("数据停更不支持最小样本量"));
        }
        if input.condition.basis.is_change() {
            return Err(AppError::bad("数据停更不能比较上期"));
        }
    }
    if input.condition.basis.is_change() {
        if !COMPARABLE.contains(&input.window.preset.as_str()) {
            return Err(AppError::bad("比较上期需要选择时间窗口：昨天、近 7 天、近 30 天或上月"));
        }
        if blank(&input.window.date_column).is_none() {
            return Err(AppError::bad("比较上期需要设置时间窗口，请选择日期列"));
        }
    }
    if input.window.preset != "none" {
        let col = blank(&input.window.date_column).ok_or_else(|| AppError::bad("设置了时间窗口，请选择日期列"))?;
        find_col(&def, col)?;
        if window_range(&input.window.preset, crate::db::today_local()).is_none() {
            return Err(AppError::bad(format!("未知的时间窗口：{}", input.window.preset)));
        }
    }
    if !input.condition.threshold.is_finite() {
        return Err(AppError::bad("阈值不是有效的数字"));
    }
    if let Some(g) = &input.guard
        && (!g.min.is_finite() || g.min < 0.0)
    {
        return Err(AppError::bad("最小样本量应为不小于 0 的数字"));
    }
    if let Repeat::EveryHours { hours } = input.repeat
        && !(1..=24 * 30).contains(&hours)
    {
        return Err(AppError::bad("重复提醒的间隔应为 1 ~ 720 小时"));
    }
    Ok(())
}

fn validate(conn: &Connection, input: &AlertInput) -> AppResult<()> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::bad("请填写告警名称"));
    }
    if name.chars().count() > 60 {
        return Err(AppError::bad("告警名称最多 60 个字"));
    }
    validate_rule(conn, input)?;
    if input.channel_ids.is_empty() {
        return Err(AppError::bad("请至少选择一个推送渠道"));
    }
    let known: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM ledger_report_channel")?;
        stmt.query_map([], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?
    };
    if input.channel_ids.iter().any(|c| !known.contains(c)) {
        return Err(AppError::bad("所选推送渠道不存在"));
    }
    input.schedule.validate(conn)
}

fn spec_of(input: &AlertInput) -> AppResult<String> {
    let mut clean = input.clone();
    clean.name = clean.name.trim().to_string();
    Ok(serde_json::to_string(&clean)?)
}

pub fn create(conn: &Connection, user: &CurrentUser, input: AlertInput) -> AppResult<String> {
    validate(conn, &input)?;
    let id = new_id("al");
    let now = now_local();
    conn.execute(
        "INSERT INTO ledger_alert (id, name, spec, enabled, created_by, created_by_name, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![id, input.name.trim(), spec_of(&input)?, input.enabled, user.id, user.name, now, now],
    )?;
    Ok(id)
}

pub fn update(conn: &Connection, user: &CurrentUser, id: &str, input: AlertInput) -> AppResult<()> {
    let old = load_alert(conn, id)?;
    may_manage(user, &old.created_by)?;
    validate(conn, &input)?;
    conn.execute(
        "UPDATE ledger_alert SET name = ?, spec = ?, enabled = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
        params![input.name.trim(), spec_of(&input)?, input.enabled, now_local(), id],
    )?;
    // 判断什么变了：定义变了旧的触发状态就不再有意义，清掉让它重新判断
    if definition_key(&old.input) != definition_key(&input) {
        conn.execute("DELETE FROM ledger_alert_state WHERE alert_id = ?", params![id])?;
    }
    Ok(())
}

pub fn delete(conn: &Connection, user: &CurrentUser, id: &str) -> AppResult<()> {
    let old = load_alert(conn, id)?;
    may_manage(user, &old.created_by)?;
    if is_running(id) {
        return Err(AppError::bad("正在评估中，请稍后再删除"));
    }
    conn.execute("DELETE FROM ledger_alert_state WHERE alert_id = ?", params![id])?;
    conn.execute("DELETE FROM ledger_alert_run WHERE alert_id = ?", params![id])?;
    conn.execute("DELETE FROM ledger_alert WHERE id = ?", params![id])?;
    Ok(())
}

// ── 记录与列表 ────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AlertRun {
    pub id: String,
    pub alert_id: String,
    pub trigger: String,
    /// running / success / partial / failed
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub message: Option<String>,
    /// 满足条件的分组数
    pub hits: i64,
    /// 本次通知了几个分组
    pub notified: i64,
    pub results: Vec<ChannelResult>,
}

const RUN_COLS: &str = "id, alert_id, trigger_by, status, strftime(started_at, '%Y-%m-%d %H:%M:%S'), strftime(finished_at, '%Y-%m-%d %H:%M:%S'), message, results, hits, notified";

fn run_from_row(r: &duckdb::Row<'_>) -> duckdb::Result<AlertRun> {
    Ok(AlertRun {
        id: r.get(0)?,
        alert_id: r.get(1)?,
        trigger: r.get(2)?,
        status: r.get(3)?,
        started_at: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        finished_at: r.get(5)?,
        message: r.get(6)?,
        results: r.get::<_, Option<String>>(7)?.and_then(|x| serde_json::from_str(&x).ok()).unwrap_or_default(),
        hits: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
        notified: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
    })
}

pub fn runs(conn: &Connection, alert_id: &str, limit: i64) -> AppResult<Vec<AlertRun>> {
    let mut stmt = conn.prepare(&format!("SELECT {RUN_COLS} FROM ledger_alert_run WHERE alert_id = ? ORDER BY started_at DESC, id DESC LIMIT ?"))?;
    Ok(stmt.query_map(params![alert_id, limit], run_from_row)?.collect::<Result<Vec<_>, _>>()?)
}

fn alert_running() -> &'static Mutex<HashSet<String>> {
    static R: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_running(id: &str) -> bool {
    alert_running().lock().map(|r| r.contains(id)).unwrap_or(false)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertView {
    pub id: String,
    #[serde(flatten)]
    pub input: AlertInput,
    pub dataset_name: Option<String>,
    pub created_by: String,
    pub created_by_name: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_run: Option<AlertRun>,
    pub next_run: Option<String>,
    pub running: bool,
    /// 当前处于触发中的分组数
    pub firing: i64,
}

/// 计划时间的起点：最近一次定时评估，没有则取规则修改时间（新建 / 修改后不补已过的时间点）
fn schedule_base(conn: &Connection, a: &StoredAlert) -> AppResult<NaiveDateTime> {
    let last: Option<String> = conn
        .query_row(
            "SELECT strftime(max(started_at), '%Y-%m-%d %H:%M:%S') FROM ledger_alert_run WHERE alert_id = ? AND trigger_by = 'schedule'",
            params![a.id],
            |row| row.get(0),
        )
        .unwrap_or(None);
    let updated = parse_ts(&a.updated_at).unwrap_or_else(|| chrono::Local::now().naive_local());
    Ok(last.as_deref().and_then(parse_ts).map(|l| l.max(updated)).unwrap_or(updated))
}

pub fn list(conn: &Connection) -> AppResult<Vec<AlertView>> {
    let names: HashMap<String, String> = dataset::datasets(conn).map(|d| d.into_iter().map(|d| (d.id, d.name)).collect()).unwrap_or_default();
    let firing: HashMap<String, i64> = {
        let mut stmt = conn.prepare("SELECT alert_id, count(*) FROM ledger_alert_state GROUP BY 1")?;
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<HashMap<_, _>, _>>()?
    };
    load_alerts(conn)?
        .into_iter()
        .map(|a| {
            let next = if a.input.enabled { a.input.schedule.next_run(schedule_base(conn, &a)?) } else { None };
            Ok(AlertView {
                last_run: runs(conn, &a.id, 1)?.into_iter().next(),
                next_run: next.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                running: is_running(&a.id),
                firing: firing.get(&a.id).copied().unwrap_or(0),
                dataset_name: names.get(&a.input.dataset_id).cloned().or_else(|| dataset::dataset(conn, &a.input.dataset_id).ok().map(|d| d.name)),
                id: a.id,
                input: a.input,
                created_by: a.created_by,
                created_by_name: a.created_by_name,
                created_at: a.created_at,
                updated_at: a.updated_at,
            })
        })
        .collect()
}

// ── 试算 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub key: String,
    pub value: Option<f64>,
    pub value_text: String,
    /// 比较上期时：上期的值和变化量
    pub prev_text: Option<String>,
    pub change_text: Option<String>,
    pub guard: Option<f64>,
    pub verdict: Verdict,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOutcome {
    /// 满足条件的排在前面（越糟越靠前），其次是不满足的，最后是不下结论的
    pub rows: Vec<PreviewRow>,
    pub total_groups: usize,
    pub hits: usize,
    pub clear: usize,
    /// 没有值或样本量不足的分组数
    pub unknown: usize,
    pub window: Option<String>,
    pub metric_label: String,
    pub condition: String,
    pub guard: Option<String>,
    pub elapsed_ms: i64,
}

/// 按当前数据算一遍，看会命中哪些分组；不保存、不发送，也不改触发状态
pub fn preview(conn: &Connection, input: &AlertInput, today: NaiveDate) -> AppResult<PreviewOutcome> {
    validate_rule(conn, input)?;
    let ev = evaluate(conn, input, today)?;
    let volume = is_volume(&input.metric);
    let change = ev.basis.is_change();
    let mut rows: Vec<(f64, PreviewRow)> = ev
        .groups
        .iter()
        .map(|g| {
            let cells = ev.value_cells(g);
            (
                g.judged.unwrap_or(0.0),
                PreviewRow {
                    key: g.key.clone(),
                    value: g.value,
                    value_text: if g.value.is_some() { cells[0].clone() } else { "—".into() },
                    prev_text: change.then(|| cells[1].clone()),
                    change_text: change.then(|| cells[2].clone()),
                    guard: g.guard,
                    verdict: verdict(g, &input.condition, input.guard.as_ref(), volume),
                },
            )
        })
        .collect();
    let rank = |v: Verdict| match v {
        Verdict::Hit => 0,
        Verdict::Clear => 1,
        Verdict::Unknown => 2,
    };
    let worse_first = input.condition.op.bigger_is_worse();
    rows.sort_by(|a, b| {
        rank(a.1.verdict).cmp(&rank(b.1.verdict)).then_with(|| {
            let o = a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal);
            if worse_first { o.reverse() } else { o }
        })
    });
    let count = |v: Verdict| rows.iter().filter(|r| r.1.verdict == v).count();
    let (hits, clear, unknown, total) = (count(Verdict::Hit), count(Verdict::Clear), count(Verdict::Unknown), rows.len());
    rows.truncate(MAX_PREVIEW_ROWS);
    Ok(PreviewOutcome {
        rows: rows.into_iter().map(|r| r.1).collect(),
        total_groups: total,
        hits,
        clear,
        unknown,
        window: ev.window_desc.clone(),
        condition: condition_text(input, &ev.metric_label, ev.fmt),
        metric_label: ev.metric_label,
        guard: input.guard.as_ref().map(guard_text),
        elapsed_ms: ev.elapsed_ms,
    })
}

// ── 评估并通知 ────────────────────────────────────────

/// 评估一次（等待完成）：算指标 → 与触发状态比对 → 通知需要通知的 → 更新状态并记录
pub async fn run(state: &AppState, id: &str, trigger: &str) -> AppResult<AlertRun> {
    {
        let mut r = alert_running().lock().map_err(|_| anyhow::anyhow!("告警状态锁已损坏"))?;
        if !r.insert(id.to_string()) {
            return Err(AppError::bad("正在评估中"));
        }
    }
    let result = run_inner(state, id, trigger).await;
    if let Ok(mut r) = alert_running().lock() {
        r.remove(id);
    }
    result
}

struct Done {
    results: Vec<ChannelResult>,
    hits: i64,
    notified: i64,
    note: Option<String>,
}

async fn run_inner(state: &AppState, id: &str, trigger: &str) -> AppResult<AlertRun> {
    let st = state.clone();
    let (aid, tr) = (id.to_string(), trigger.to_string());
    let (alert, chans, run_id, started) = tokio::task::spawn_blocking(move || -> AppResult<_> {
        let conn = st.connection()?;
        let alert = load_alert(&conn, &aid)?;
        let chans: Vec<StoredChannel> = load_channels(&conn, &st)?.into_iter().filter(|c| alert.input.channel_ids.contains(&c.id)).collect();
        let run_id = format!("ar_{}_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"), &aid[aid.len().saturating_sub(4)..]);
        let started = now_local();
        conn.execute(
            "INSERT INTO ledger_alert_run (id, alert_id, trigger_by, status, started_at) VALUES (?, ?, ?, 'running', CAST(? AS TIMESTAMP))",
            params![run_id, aid, tr, started],
        )?;
        Ok((alert, chans, run_id, started))
    })
    .await??;

    let outcome = tokio::time::timeout(RUN_TIMEOUT, evaluate_and_notify(state, &alert, &chans)).await.unwrap_or_else(|_| Err(AppError::bad("评估超过 5 分钟未完成，已中止")));

    let (status, done) = match outcome {
        Ok(d) => {
            let ok = d.results.iter().filter(|r| r.ok).count();
            let status = if d.results.is_empty() || ok == d.results.len() { "success" } else if ok == 0 { "failed" } else { "partial" };
            (status, d)
        }
        Err(e) => ("failed", Done { results: vec![], hits: 0, notified: 0, note: Some(e.message()) }),
    };
    tracing::info!("告警「{}」{}：{}（命中 {}，通知 {}）", alert.input.name, trigger, status, done.hits, done.notified);
    let record = AlertRun {
        id: run_id,
        alert_id: id.to_string(),
        trigger: trigger.to_string(),
        status: status.into(),
        started_at: started,
        finished_at: Some(now_local()),
        message: done.note,
        hits: done.hits,
        notified: done.notified,
        results: done.results,
    };
    let st = state.clone();
    let rec = record.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = st.connection()?;
        conn.execute(
            "UPDATE ledger_alert_run SET status = ?, finished_at = CAST(? AS TIMESTAMP), message = ?, results = ?, hits = ?, notified = ? WHERE id = ?",
            params![rec.status, rec.finished_at, rec.message, serde_json::to_string(&rec.results)?, rec.hits, rec.notified, rec.id],
        )?;
        conn.execute(
            "DELETE FROM ledger_alert_run WHERE alert_id = ? AND id NOT IN (SELECT id FROM ledger_alert_run WHERE alert_id = ? ORDER BY started_at DESC, id DESC LIMIT ?)",
            params![rec.alert_id, rec.alert_id, KEEP_RUNS],
        )?;
        Ok(())
    })
    .await??;
    Ok(record)
}

async fn evaluate_and_notify(state: &AppState, alert: &StoredAlert, chans: &[StoredChannel]) -> AppResult<Done> {
    // 重计算排队：和透视、SQL 共用并发闸门，不让后台告警把交互查询挤垮
    let permit = state.heavy.acquire().await?;
    let (st, a) = (state.clone(), alert.clone());
    let now = chrono::Local::now().naive_local();
    let plan = tokio::task::spawn_blocking(move || -> AppResult<Plan> {
        let conn = st.connection()?;
        make_plan(&conn, &a, now)
    })
    .await??;
    drop(permit);

    let to_notify = plan.hits.iter().filter(|h| h.notice.is_some()).count();
    let want_recovery = alert.input.notify_recovery && !plan.clear.is_empty();
    if (to_notify > 0 || want_recovery) && chans.is_empty() {
        return Err(AppError::bad("告警没有可用的推送渠道（可能已被删除）"));
    }
    if to_notify > 0 {
        let hits: Vec<serde_json::Value> = plan
            .hits
            .iter()
            .filter_map(|h| {
                let kind = match h.notice.as_ref()? {
                    Notice::New => "new",
                    Notice::Reminder => "reminder",
                };
                Some(serde_json::json!({ "group": h.group.key, "value": h.group.value, "judged": h.group.judged, "note": h.group.note, "notice": kind }))
            })
            .collect();
        let data = serde_json::json!({ "alertId": alert.id, "name": alert.input.name, "datasetId": alert.input.dataset_id, "hits": hits, "totalHits": plan.hits.len() });
        crate::webhook::emit(state, "alert.triggered", None, data);
    }
    let mut results = Vec::new();
    if to_notify > 0 {
        let content = build_content(alert, &plan, now);
        results.extend(send_all(&alert.input.name, chans, &format!("告警：{}", alert.input.name), &content).await);
    }
    // 至少一个渠道发出去了，才算「已通知」：否则下次评估还会把它当作新触发重试
    let delivered = results.iter().any(|r| r.ok);

    // 恢复通知：发出去了才清掉触发状态，否则下次评估再试一次
    let mut recovery_results = Vec::new();
    if want_recovery {
        let content = build_recovery_content(alert, &plan, now);
        recovery_results = send_all(&alert.input.name, chans, &format!("恢复：{}", alert.input.name), &content).await;
    }
    let recovery_delivered = recovery_results.iter().any(|r| r.ok);
    let recovered = if want_recovery && recovery_delivered { plan.clear.len() } else { 0 };
    results.extend(recovery_results);

    let hits = plan.hits.len() as i64;
    let (st, id) = (state.clone(), alert.id.clone());
    let unknown_note = (plan.unknown > 0).then(|| format!("{} 项样本不足 / 无数据，未判断", plan.unknown));
    let mut notes: Vec<String> = match (hits, to_notify) {
        (0, _) => vec!["没有超出条件的项".into()],
        (_, 0) => vec![format!("命中 {hits} 项，都已通知过，本次不重复提醒")],
        _ => vec![],
    };
    notes.extend(unknown_note);
    if recovered > 0 {
        notes.push(format!("{recovered} 项已恢复并通知"));
    }
    let note = (!notes.is_empty()).then(|| notes.join("；"));
    let clear_now = !want_recovery || recovery_delivered;
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = st.connection()?;
        apply_state(&conn, &id, &plan, delivered, clear_now, &now_local())
    })
    .await??;
    Ok(Done { results, hits, notified: if delivered { to_notify as i64 } else { 0 }, note })
}

/// 发送到所有渠道（每个渠道最多等 60 秒），返回各渠道的结果
async fn send_all(name: &str, chans: &[StoredChannel], title: &str, content: &ReportContent) -> Vec<ChannelResult> {
    let msg = Message { title, content, link: None };
    let mut results = Vec::new();
    for ch in chans {
        let r = tokio::time::timeout(Duration::from_secs(60), deliver(ch, &msg, None)).await.unwrap_or_else(|_| Err(AppError::bad("发送超时")));
        if let Err(e) = &r {
            tracing::warn!("告警「{name}」发送到「{}」失败：{}", ch.name, e.message());
        }
        results.push(ChannelResult { channel_id: ch.id.clone(), channel_name: format!("{}（{}）", ch.name, ch.kind.label()), ok: r.is_ok(), message: r.err().map(|e| e.message()) });
    }
    results
}

/// 把这次评估的结果记进触发状态
/// clear_now：已恢复的分组是否现在就清掉状态（要发恢复通知但没发出去时留着，下次重试）
fn apply_state(conn: &Connection, alert_id: &str, plan: &Plan, delivered: bool, clear_now: bool, now: &str) -> AppResult<()> {
    for h in &plan.hits {
        let notified = delivered && h.notice.is_some();
        match &h.prev {
            None => {
                // 新触发但没发出去：不记，下次还算新触发
                if notified {
                    conn.execute(
                        "INSERT INTO ledger_alert_state (alert_id, group_key, since, last_notified, last_value) VALUES (?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP), ?)
                         ON CONFLICT (alert_id, group_key) DO UPDATE SET last_notified = excluded.last_notified, last_value = excluded.last_value",
                        params![alert_id, h.group.key, now, now, h.group.value],
                    )?;
                }
            }
            Some(_) if notified => {
                conn.execute(
                    "UPDATE ledger_alert_state SET last_notified = CAST(? AS TIMESTAMP), last_value = ? WHERE alert_id = ? AND group_key = ?",
                    params![now, h.group.value, alert_id, h.group.key],
                )?;
            }
            Some(_) => {
                conn.execute("UPDATE ledger_alert_state SET last_value = ? WHERE alert_id = ? AND group_key = ?", params![h.group.value, alert_id, h.group.key])?;
            }
        }
    }
    for c in plan.clear.iter().filter(|_| clear_now) {
        conn.execute("DELETE FROM ledger_alert_state WHERE alert_id = ? AND group_key = ?", params![alert_id, c.group.key])?;
    }
    Ok(())
}

// ── 调度 ──────────────────────────────────────────────

fn spawn_run(state: &AppState, id: String, name: String, trigger: &'static str) {
    let st = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run(&st, &id, trigger).await {
            tracing::warn!("告警「{name}」未能执行：{}", e.message());
        }
    });
}

/// 每 30 秒检查一次到期的告警
pub async fn schedule(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let st = state.clone();
        let due = tokio::task::spawn_blocking(move || -> AppResult<Vec<(String, String)>> {
            let conn = st.connection()?;
            let now = chrono::Local::now().naive_local();
            let mut due = Vec::new();
            for a in load_alerts(&conn)?.into_iter().filter(|a| a.input.enabled && !is_running(&a.id)) {
                if a.input.schedule.next_run(schedule_base(&conn, &a)?).is_some_and(|t| t <= now) {
                    due.push((a.id, a.input.name));
                }
            }
            Ok(due)
        })
        .await
        .unwrap_or_else(|e| Err(AppError::Internal(e.into())));
        match due {
            Ok(due) => due.into_iter().for_each(|(id, name)| spawn_run(&state, id, name, "schedule")),
            Err(e) => tracing::warn!("检查告警计划失败：{}", e.message()),
        }
    }
}

/// 数据连接同步成功（有新数据）后：评估设为「同步后」的告警
pub fn on_connector_synced(state: &AppState, connector_id: &str) {
    let due = state.connection().map_err(AppError::from).and_then(|conn| load_alerts(&conn)).map(|list| {
        list.into_iter()
            .filter(|a| a.input.enabled && matches!(&a.input.schedule, Schedule::AfterSync { connector_id: c } if c == connector_id))
            .map(|a| (a.id, a.input.name))
            .collect::<Vec<_>>()
    });
    match due {
        Ok(due) => due.into_iter().for_each(|(id, name)| spawn_run(state, id, name, "after_sync")),
        Err(e) => tracing::warn!("检查同步后告警失败：{}", e.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> NaiveDateTime {
        parse_ts(s).unwrap()
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn g(key: &str, value: Option<f64>, guard: Option<f64>) -> GroupValue {
        GroupValue::new(key.into(), value, guard)
    }

    /// 比较上期：本期 / 上期的值与样本量
    fn gp(key: &str, cur: Option<f64>, prev: Option<f64>, guard: Option<f64>, prev_guard: Option<f64>, basis: Basis) -> GroupValue {
        let mut x = GroupValue::new(key.into(), cur, guard);
        x.prev = prev;
        x.prev_guard = prev_guard;
        x.judge(basis);
        x
    }

    #[test]
    fn rule_json_roundtrip() {
        let json = serde_json::json!({
            "name": "店铺退款率偏高",
            "datasetId": "ds_orders",
            "metric": { "kind": "ratio", "numerator": { "column": "退款额", "agg": "sum" }, "denominator": { "column": "成交额", "agg": "sum" } },
            "metricLabel": "退款率",
            "groupBy": "店铺",
            "filters": [{ "op": "in", "column": "渠道", "values": ["直播"] }],
            "window": { "dateColumn": "下单时间", "preset": "last_7" },
            "condition": { "op": "gt", "threshold": 0.05 },
            "guard": { "agg": "count", "min": 30 },
            "schedule": { "kind": "daily", "at": "09:00" },
            "channelIds": ["rc_1"],
            "repeat": { "kind": "every_hours", "hours": 24 }
        });
        let input: AlertInput = serde_json::from_value(json).unwrap();
        assert!(input.enabled, "enabled 默认为 true");
        assert_eq!(input.condition.op, Op::Gt);
        assert_eq!(input.repeat, Repeat::EveryHours { hours: 24 });
        assert!(matches!(input.metric, Metric::Ratio { .. }));
        // 存进库再读出来是同一条规则
        let back: AlertInput = serde_json::from_str(&spec_of(&input).unwrap()).unwrap();
        assert_eq!(definition_key(&input), definition_key(&back));
        // 省略可选项也能解析：默认不分组、不设窗口、只通知一次
        let min: AlertInput = serde_json::from_value(serde_json::json!({
            "name": "x", "datasetId": "d", "metric": { "kind": "agg", "agg": "count" },
            "condition": { "op": "lt", "threshold": 10 }, "schedule": { "kind": "manual" }, "channelIds": []
        }))
        .unwrap();
        assert_eq!(min.window.preset, "none");
        assert_eq!(min.repeat, Repeat::UntilRecovered);
        assert!(min.group_by.is_none() && min.guard.is_none());
    }

    #[test]
    fn definition_key_ignores_delivery_settings() {
        let base = serde_json::json!({
            "name": "a", "datasetId": "d", "metric": { "kind": "agg", "agg": "count" },
            "condition": { "op": "gt", "threshold": 1 }, "schedule": { "kind": "manual" }, "channelIds": ["x"]
        });
        let a: AlertInput = serde_json::from_value(base.clone()).unwrap();
        // 改名字、渠道、计划、重复策略：不影响触发状态
        let mut b = base.clone();
        b["name"] = "b".into();
        b["channelIds"] = serde_json::json!(["y"]);
        b["schedule"] = serde_json::json!({ "kind": "daily", "at": "08:00" });
        b["repeat"] = serde_json::json!({ "kind": "every_run" });
        assert_eq!(definition_key(&a), definition_key(&serde_json::from_value(b).unwrap()));
        // 改阈值：要清状态
        let mut c = base;
        c["condition"]["threshold"] = 2.into();
        assert_ne!(definition_key(&a), definition_key(&serde_json::from_value(c).unwrap()));
    }

    #[test]
    fn windows_exclude_today() {
        let today = day("2026-10-13");
        assert_eq!(window_range("yesterday", today).unwrap().0, "2026-10-12");
        let (a, b, _) = window_range("last_7", today).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("2026-10-06", "2026-10-12"));
        let (a, b, _) = window_range("last_30", today).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("2026-09-13", "2026-10-12"));
        assert_eq!(window_range("last_month", today).unwrap().0, "2026-09-01");
        assert!(window_range("whenever", today).is_none());
        assert!(window_range("none", today).is_none());
    }

    #[test]
    fn verdicts_respect_condition_and_guard() {
        let over = Condition { op: Op::Gt, threshold: 0.05, basis: Basis::Value };
        let guard = Guard { column: None, agg: "count".into(), min: 30.0 };
        assert_eq!(verdict(&g("a", Some(0.08), Some(100.0)), &over, Some(&guard), false), Verdict::Hit);
        assert_eq!(verdict(&g("a", Some(0.03), Some(100.0)), &over, Some(&guard), false), Verdict::Clear);
        // 恰好等于阈值：> 不算超标，≥ 才算
        assert_eq!(verdict(&g("a", Some(0.05), None), &over, None, false), Verdict::Clear);
        assert_eq!(verdict(&g("a", Some(0.05), None), &Condition { op: Op::Gte, threshold: 0.05, basis: Basis::Value }, None, false), Verdict::Hit);
        // 样本不足：2 单退 1 单（50%）不下结论
        assert_eq!(verdict(&g("small", Some(0.5), Some(2.0)), &over, Some(&guard), false), Verdict::Unknown);
        // 样本量取不到、指标没有值（分母为 0）、值不是有限数：不下结论
        assert_eq!(verdict(&g("a", Some(0.5), None), &over, Some(&guard), false), Verdict::Unknown);
        assert_eq!(verdict(&g("a", None, Some(100.0)), &over, Some(&guard), false), Verdict::Unknown);
        assert_eq!(verdict(&g("a", Some(f64::NAN), None), &over, None, false), Verdict::Unknown);
        // 低于下限
        assert_eq!(verdict(&g("a", Some(5.0), None), &Condition { op: Op::Lt, threshold: 10.0, basis: Basis::Value }, None, false), Verdict::Hit);
    }

    #[test]
    fn notice_state_machine() {
        let now = ts("2026-10-13 09:00:00");
        let prev = |since: &str, last: Option<&str>| Prev { since: ts(since), last_notified: last.map(ts) };
        // 新触发：任何策略都通知
        for r in [Repeat::UntilRecovered, Repeat::EveryRun, Repeat::EveryHours { hours: 24 }] {
            assert_eq!(next_notice(None, &r, now), Some(Notice::New));
        }
        let p = prev("2026-10-10 09:00:00", Some("2026-10-10 09:00:00"));
        // 持续触发：默认不重复，每次都提醒，或隔 N 小时提醒
        assert_eq!(next_notice(Some(&p), &Repeat::UntilRecovered, now), None);
        assert_eq!(next_notice(Some(&p), &Repeat::EveryRun, now), Some(Notice::Reminder));
        assert_eq!(next_notice(Some(&p), &Repeat::EveryHours { hours: 24 }, now), Some(Notice::Reminder));
        // 距上次通知不满 24 小时：不提醒
        let recent = prev("2026-10-10 09:00:00", Some("2026-10-12 12:00:00"));
        assert_eq!(next_notice(Some(&recent), &Repeat::EveryHours { hours: 24 }, now), None);
        // 刚好 24 小时：提醒
        let edge = prev("2026-10-10 09:00:00", Some("2026-10-12 09:00:00"));
        assert_eq!(next_notice(Some(&edge), &Repeat::EveryHours { hours: 24 }, now), Some(Notice::Reminder));
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_value(0.0832, Fmt::Percent), "8.3%");
        assert_eq!(fmt_value(0.05, Fmt::Percent), "5.0%");
        assert_eq!(fmt_value(3313147.54, Fmt::Money), "331.31万");
        assert_eq!(fmt_value(1200.0, Fmt::Number), "1,200");
        assert_eq!(duration_text(chrono::Duration::hours(5)), "5 小时");
        assert_eq!(duration_text(chrono::Duration::hours(72)), "3 天");
    }

    #[test]
    fn compare_windows_are_adjacent_and_equal_length() {
        let today = day("2026-10-13");
        let ((a, b), (c, d)) = compare_windows("last_7", today).unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str(), d.as_str()), ("2026-10-06", "2026-10-12", "2026-09-29", "2026-10-05"));
        let ((a, b), (c, d)) = compare_windows("yesterday", today).unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str(), d.as_str()), ("2026-10-12", "2026-10-12", "2026-10-11", "2026-10-11"));
        let ((a, b), (c, d)) = compare_windows("last_30", today).unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str(), d.as_str()), ("2026-09-13", "2026-10-12", "2026-08-14", "2026-09-12"));
        // 上月 vs 上上月（跨年也对）
        let ((a, b), (c, d)) = compare_windows("last_month", today).unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str(), d.as_str()), ("2026-09-01", "2026-09-30", "2026-08-01", "2026-08-31"));
        let ((a, _), (c, d)) = compare_windows("last_month", day("2026-01-05")).unwrap();
        assert_eq!((a.as_str(), c.as_str(), d.as_str()), ("2025-12-01", "2025-11-01", "2025-11-30"));
        // 本月、不设窗口：没法比较
        assert!(compare_windows("this_month", today).is_none());
        assert!(compare_windows("none", today).is_none());
    }

    #[test]
    fn change_basis_judgement() {
        // 相对变化：销售额 80 万 → 60 万 = -25%
        let drop = Condition { op: Op::Lt, threshold: -0.2, basis: Basis::ChangePct };
        let x = gp("A", Some(600_000.0), Some(800_000.0), None, None, Basis::ChangePct);
        assert_eq!(x.judged, Some(-0.25));
        assert_eq!(verdict(&x, &drop, None, true), Verdict::Hit);
        let mild = gp("B", Some(780_000.0), Some(800_000.0), None, None, Basis::ChangePct);
        assert_eq!(verdict(&mild, &drop, None, true), Verdict::Clear);
        // 上期为 0：相对变化没有意义，不判断
        let from_zero = gp("C", Some(10.0), Some(0.0), None, None, Basis::ChangePct);
        assert_eq!(from_zero.judged, None);
        assert_eq!(verdict(&from_zero, &drop, None, true), Verdict::Unknown);
        // 上期是负数：以绝对值为基准，由 -100 变 -50 是改善 +50%
        assert_eq!(gp("D", Some(-50.0), Some(-100.0), None, None, Basis::ChangePct).judged, Some(0.5));
        // 绝对变化：退款率 4% → 8% = 上升 4 个百分点
        let rise = Condition { op: Op::Gt, threshold: 0.03, basis: Basis::ChangeAbs };
        let r = gp("A", Some(0.08), Some(0.04), None, None, Basis::ChangeAbs);
        assert!((r.judged.unwrap() - 0.04).abs() < 1e-12);
        assert_eq!(verdict(&r, &rise, None, false), Verdict::Hit);
        // 没有上期的值（比率类指标，上期没有这个分组）：不判断
        assert_eq!(verdict(&gp("N", Some(0.08), None, None, None, Basis::ChangeAbs), &rise, None, false), Verdict::Unknown);
    }

    #[test]
    fn change_guard_looks_at_the_right_period() {
        let guard = Guard { column: None, agg: "count".into(), min: 30.0 };
        let drop = Condition { op: Op::Lt, threshold: -0.2, basis: Basis::ChangePct };
        // 「量」类指标：基数（上期）够大就行，本期掉到很少正是要抓的
        let x = gp("A", Some(5.0), Some(100.0), Some(2.0), Some(50.0), Basis::ChangePct);
        assert_eq!(verdict(&x, &drop, Some(&guard), true), Verdict::Hit);
        // 上期本来就很小：不判断
        let small = gp("B", Some(1.0), Some(10.0), Some(1.0), Some(5.0), Basis::ChangePct);
        assert_eq!(verdict(&small, &drop, Some(&guard), true), Verdict::Unknown);
        // 比率类指标：本期和上期的样本量都要够
        let rise = Condition { op: Op::Gt, threshold: 0.03, basis: Basis::ChangeAbs };
        let thin_now = gp("C", Some(0.5), Some(0.04), Some(2.0), Some(100.0), Basis::ChangeAbs);
        assert_eq!(verdict(&thin_now, &rise, Some(&guard), false), Verdict::Unknown);
        let thin_before = gp("D", Some(0.5), Some(0.04), Some(100.0), Some(2.0), Basis::ChangeAbs);
        assert_eq!(verdict(&thin_before, &rise, Some(&guard), false), Verdict::Unknown);
        let both = gp("E", Some(0.5), Some(0.04), Some(100.0), Some(100.0), Basis::ChangeAbs);
        assert_eq!(verdict(&both, &rise, Some(&guard), false), Verdict::Hit);
    }

    #[test]
    fn merge_periods_treats_missing_volume_as_zero() {
        let cur = vec![g("A", Some(50.0), None), g("N", Some(8.0), None)];
        let prev = vec![g("A", Some(100.0), None), g("Z", Some(70.0), None)];
        let m = merge_periods(cur.clone(), prev.clone(), true);
        let by = |k: &str| m.iter().find(|x| x.key == k).unwrap();
        assert_eq!(by("A").prev, Some(100.0));
        // 新出现的分组：上期为 0；上期有、本期没有的分组：本期为 0（卖着卖着没了）
        assert_eq!(by("N").prev, Some(0.0));
        assert_eq!((by("Z").value, by("Z").prev), (Some(0.0), Some(70.0)));
        // 比率类指标：没有可比的一期，保持没有值，也不凭空造出只在上期有的分组
        let m = merge_periods(cur, prev, false);
        assert_eq!(m.len(), 2);
        assert_eq!(m.iter().find(|x| x.key == "N").unwrap().prev, None);
    }

    #[test]
    fn change_and_delta_formatting() {
        assert_eq!(fmt_delta(0.03, Fmt::Percent), "+3.0 个百分点");
        assert_eq!(fmt_delta(-0.015, Fmt::Percent), "-1.5 个百分点");
        assert_eq!(fmt_delta(-250_000.0, Fmt::Money), "-25万");
        assert_eq!(fmt_delta(30.0, Fmt::Number), "+30");
        assert_eq!(fmt_threshold(-0.2, Basis::ChangePct, Fmt::Money), "-20.0%");
        assert_eq!(fmt_threshold(0.12, Basis::ChangePct, Fmt::Money), "+12.0%");
        assert_eq!(fmt_value(5.0, Fmt::Days), "5 天");
    }

    #[test]
    fn new_rule_fields_parse_and_default() {
        let input: AlertInput = serde_json::from_value(serde_json::json!({
            "name": "店铺停更", "datasetId": "d", "metric": { "kind": "freshness", "dateColumn": "下单时间" },
            "groupBy": "店铺", "condition": { "op": "gt", "threshold": 3 }, "schedule": { "kind": "manual" }, "channelIds": [],
            "notifyRecovery": true
        }))
        .unwrap();
        assert!(matches!(&input.metric, Metric::Freshness { date_column } if date_column == "下单时间"));
        assert!(input.notify_recovery);
        assert_eq!(input.condition.basis, Basis::Value, "旧规则没有 basis 字段：按当前值判断");
        let change: AlertInput = serde_json::from_value(serde_json::json!({
            "name": "x", "datasetId": "d", "metric": { "kind": "agg", "agg": "sum", "column": "金额" },
            "window": { "dateColumn": "时间", "preset": "last_7" },
            "condition": { "op": "lt", "threshold": -0.2, "basis": "change_pct" }, "schedule": { "kind": "manual" }, "channelIds": []
        }))
        .unwrap();
        assert_eq!(change.condition.basis, Basis::ChangePct);
        assert!(!change.notify_recovery, "恢复通知默认关闭");
        // 判断依据变了，旧的触发状态要作废
        let mut value_basis = change.clone();
        value_basis.condition.basis = Basis::Value;
        assert_ne!(definition_key(&change), definition_key(&value_basis));
        // 是否通知恢复不影响触发状态
        let mut recov = change.clone();
        recov.notify_recovery = true;
        assert_eq!(definition_key(&change), definition_key(&recov));
    }

    #[test]
    fn worst_first_ordering() {
        let mut v = vec![(0.06, 0), (0.12, 1), (0.08, 2)];
        sort_worst_first(&mut v, Op::Gt);
        assert_eq!(v.iter().map(|x| x.1).collect::<Vec<_>>(), vec![1, 2, 0]);
        sort_worst_first(&mut v, Op::Lt);
        assert_eq!(v.iter().map(|x| x.1).collect::<Vec<_>>(), vec![0, 2, 1]);
    }
}
