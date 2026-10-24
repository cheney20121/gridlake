//! 异常发现：自动扫描数据集，指出「最近几天」相对「此前一个月」明显偏离的地方
//!
//! 只做可解释的统计，不依赖模型：
//! - **每日走势**（行数与前几个金额 / 数值列的日合计）：最近 7 天的每一天，与此前 28 天的同类水平比较。
//!   先按星期几折算（周末订单少是常态，不算异常），再用中位数 + 绝对中位差（对个别极端日不敏感）算稳健 z 分数；
//!   z ≥ 3.5 **并且**偏离 ≥ 30% 才报，两个条件缺一不可，避免平稳数据上的噪声
//! - **分组变化**（取值不多的文本列，如店铺 / 渠道 / 状态）：最近 7 天与此前 28 天的日均比较，
//!   变化 ≥ 50% 且对总量影响不小的取值，包括「此前有、最近一条都没有」的
//! - 以数据里最新的一天为基准（不是今天），数据停更时仍然有意义；最新一天如果比平时少很多，提示「可能还没取全」
//! - 每条结论带下钻条件（时间范围 / 分组值），前端点开就是对应明细
//! - 读取走 `dataset::datasets()`，受限用户的数据范围自动生效：他们扫到的只是自己范围内的数据

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate};
use duckdb::Connection;
use serde::Serialize;
use serde_json::json;

use crate::dataset::{self, ColumnDef, DatasetDef, Filter, column_sql};
use crate::error::{AppError, AppResult};

/// 对比的最近天数 / 基线天数
pub const RECENT: usize = 7;
pub const BASE: usize = 28;
/// 稳健 z 分数与偏离幅度的门槛
const Z_MIN: f64 = 3.5;
const CHANGE_MIN: f64 = 0.3;
/// 分组变化的门槛
const SEG_CHANGE_MIN: f64 = 0.5;
/// 分组取值的上限（再多就不是「分类」了）
const MAX_CARDINALITY: i64 = 30;
const MAX_METRICS: usize = 3;
const MAX_DIMENSIONS: usize = 3;
const MAX_FINDINGS: usize = 12;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Drill {
    pub title: String,
    pub filters: Vec<Filter>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    /// volume 行数 / metric 金额数值日合计 / segment 分组变化
    pub kind: &'static str,
    /// high / medium / low
    pub severity: &'static str,
    pub title: String,
    pub detail: String,
    pub dataset_id: String,
    pub dataset_name: String,
    pub actual: f64,
    pub expected: f64,
    /// 相对预期的变化（-0.4 表示低了 40%）
    pub change: f64,
    pub drill: Drill,
    #[serde(skip)]
    pub score: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub dataset_id: String,
    pub dataset_name: String,
    /// 用来排时间的列
    pub time_column: Option<String>,
    /// 数据里最新的一天
    pub latest_date: Option<String>,
    pub findings: Vec<Finding>,
    /// 没能扫描 / 只扫描了一部分的原因
    pub note: Option<String>,
}

// ── 统计 ──────────────────────────────────────────────

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[derive(Debug, PartialEq)]
pub struct Spike {
    pub date: NaiveDate,
    pub actual: f64,
    pub expected: f64,
    pub z: f64,
}

/// 把稀疏的「日期 → 值」补成连续的每一天（缺的天记 0）
fn densify(points: &[(NaiveDate, f64)]) -> Vec<(NaiveDate, f64)> {
    let map: BTreeMap<NaiveDate, f64> = points.iter().copied().collect();
    let (Some(first), Some(last)) = (map.keys().next().copied(), map.keys().next_back().copied()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut d = first;
    while d <= last {
        out.push((d, map.get(&d).copied().unwrap_or(0.0)));
        d += Duration::days(1);
    }
    out
}

/// 找最近 `RECENT` 天里偏离此前 `BASE` 天水平的日子。数据不足（基线里有数据的天数 < 14）时返回空
pub fn detect_series(points: &[(NaiveDate, f64)]) -> Vec<Spike> {
    let days = densify(points);
    if days.len() < RECENT + 14 {
        return Vec::new();
    }
    let recent_start = days.len() - RECENT;
    let base: &[(NaiveDate, f64)] = &days[recent_start.saturating_sub(BASE)..recent_start];
    if base.iter().filter(|(_, v)| *v != 0.0).count() < 14 {
        return Vec::new();
    }
    // 星期折算：基线满 4 周才启用
    let mut factor = [1.0_f64; 7];
    if base.len() >= BASE {
        let mean = base.iter().map(|(_, v)| v).sum::<f64>() / base.len() as f64;
        if mean > 0.0 {
            for (wd, f) in factor.iter_mut().enumerate() {
                let xs: Vec<f64> = base.iter().filter(|(d, _)| d.weekday().num_days_from_monday() as usize == wd).map(|(_, v)| *v).collect();
                if !xs.is_empty() {
                    *f = (xs.iter().sum::<f64>() / xs.len() as f64 / mean).clamp(0.3, 3.0);
                }
            }
        }
    }
    let f = |d: NaiveDate| factor[d.weekday().num_days_from_monday() as usize];
    let mut norm: Vec<f64> = base.iter().map(|(d, v)| v / f(*d)).collect();
    let m = median(&mut norm);
    if m <= 0.0 {
        return Vec::new();
    }
    let mut dev: Vec<f64> = norm.iter().map(|v| (v - m).abs()).collect();
    // 1.4826：把绝对中位差折算成标准差；下限取中位数的 5%，避免数据过于平稳时微小波动被放大
    let scale = (1.4826 * median(&mut dev)).max(0.05 * m);
    days[recent_start..]
        .iter()
        .filter_map(|(d, v)| {
            let expected = m * f(*d);
            let z = (v / f(*d) - m) / scale;
            (expected > 0.0 && z.abs() >= Z_MIN && (v - expected).abs() / expected >= CHANGE_MIN).then_some(Spike { date: *d, actual: *v, expected, z })
        })
        .collect()
}

/// 分组取值的变化：日均相比基线变化的幅度；基线日均太小（没有可比性）时返回 None
pub fn segment_change(recent_total: f64, base_total: f64) -> Option<f64> {
    let (r, b) = (recent_total / RECENT as f64, base_total / BASE as f64);
    (b > 0.0).then(|| r / b - 1.0)
}

fn severity(z: f64, change: f64) -> &'static str {
    if z.abs() >= 6.0 && change.abs() >= 0.5 { "high" } else { "medium" }
}

// ── 数据读取 ──────────────────────────────────────────

fn date_expr(col: &ColumnDef) -> String {
    format!("TRY_CAST(substr(CAST({} AS VARCHAR), 1, 10) AS DATE)", column_sql(col))
}

fn fmt_num(x: f64, money: bool) -> String {
    let a = x.abs();
    if a >= 1e8 {
        format!("{:.2} 亿", x / 1e8)
    } else if a >= 1e4 {
        format!("{:.2} 万", x / 1e4)
    } else if money || x.fract() != 0.0 {
        format!("{x:.2}")
    } else {
        format!("{x:.0}")
    }
}

fn pct(x: f64) -> String {
    format!("{}{:.0}%", if x >= 0.0 { "+" } else { "-" }, x.abs() * 100.0)
}

/// 区间按文本比较，结束日要写到当天最后一秒才包含当天的全部记录
fn day_filter(col: &str, from: NaiveDate, to: NaiveDate) -> Filter {
    Filter::Between { column: col.to_string(), min: Some(json!(from.to_string())), max: Some(json!(format!("{to} 23:59:59"))) }
}

/// 扫描一个数据集
pub fn scan(conn: &Connection, def: &DatasetDef) -> AppResult<ScanResult> {
    let mut res = ScanResult { dataset_id: def.id.clone(), dataset_name: def.name.clone(), time_column: None, latest_date: None, findings: Vec::new(), note: None };
    let Some(time) = def.columns.iter().find(|c| c.kind == "datetime" && !c.computed).or_else(|| def.columns.iter().find(|c| c.kind == "datetime")) else {
        res.note = Some("没有日期时间列，无法按时间比较".into());
        return Ok(res);
    };
    res.time_column = Some(time.key.clone());
    let metrics: Vec<&ColumnDef> = def.columns.iter().filter(|c| c.kind == "money").chain(def.columns.iter().filter(|c| matches!(c.kind, "integer" | "number"))).take(MAX_METRICS).collect();
    let d = date_expr(time);

    // 每日聚合：行数 + 各指标列的日合计
    let cols: String = metrics.iter().enumerate().map(|(i, m)| format!(", {} AS m{i}", column_sql(m))).collect();
    let aggs: String = (0..metrics.len()).map(|i| format!(", sum(m{i})")).collect();
    let sql = format!("SELECT CAST(d AS VARCHAR), count(*){aggs} FROM (SELECT {d} AS d{cols} FROM {}) WHERE d IS NOT NULL GROUP BY d ORDER BY d", def.table);
    let mut series: Vec<Vec<(NaiveDate, f64)>> = vec![Vec::new(); metrics.len() + 1];
    {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            let day: String = r.get(0)?;
            let mut vals = Vec::with_capacity(metrics.len() + 1);
            vals.push(r.get::<_, i64>(1)? as f64);
            for i in 0..metrics.len() {
                vals.push(r.get::<_, Option<f64>>(2 + i)?.unwrap_or(0.0));
            }
            Ok((day, vals))
        })?;
        for row in rows {
            let (day, vals) = row?;
            let Ok(date) = NaiveDate::parse_from_str(&day, "%Y-%m-%d") else { continue };
            for (s, v) in series.iter_mut().zip(vals) {
                s.push((date, v));
            }
        }
    }
    let Some(latest) = series[0].last().map(|p| p.0) else {
        res.note = Some("日期列里没有可识别的日期".into());
        return Ok(res);
    };
    res.latest_date = Some(latest.to_string());
    let span = (latest - series[0][0].0).num_days() + 1;
    if (span as usize) < RECENT + 14 {
        res.note = Some(format!("数据只覆盖 {span} 天，至少需要 {} 天才能比较", RECENT + 14));
        return Ok(res);
    }

    // 走势异常
    let push_spikes = |label: &str, kind: &'static str, money: bool, spikes: Vec<Spike>, out: &mut Vec<Finding>| {
        for s in spikes {
            let change = s.actual / s.expected - 1.0;
            let partial = s.date == latest && change < 0.0;
            let mut sev = severity(s.z, change);
            if partial && sev == "high" {
                sev = "medium";
            }
            let verb = if s.actual == 0.0 { "没有数据".to_string() } else if change < 0.0 { format!("偏低 {}", pct(change).trim_start_matches('-')) } else { format!("偏高 {}", pct(change).trim_start_matches('+')) };
            out.push(Finding {
                id: format!("{kind}:{label}:{}", s.date),
                kind,
                severity: sev,
                title: format!("{} {label}{verb}", s.date),
                detail: format!("{label}为 {}，同类日子（按星期折算）通常约 {}；偏离 {:.1} 倍波动{}", fmt_num(s.actual, money), fmt_num(s.expected, money), s.z.abs(), if partial { "。这是数据里最新的一天，可能还没取全" } else { "" }),
                dataset_id: def.id.clone(),
                dataset_name: def.name.clone(),
                actual: s.actual,
                expected: s.expected,
                change,
                drill: Drill { title: format!("{} {}", s.date, label), filters: vec![day_filter(&time.key, s.date, s.date)] },
                score: s.z.abs() * if sev == "high" { 2.0 } else { 1.0 },
            });
        }
    };
    push_spikes("行数", "volume", false, detect_series(&series[0]), &mut res.findings);
    for (i, m) in metrics.iter().enumerate() {
        push_spikes(&format!("{}合计", m.title), "metric", m.kind == "money", detect_series(&series[i + 1]), &mut res.findings);
    }

    // 分组变化
    let recent_from = latest - Duration::days(RECENT as i64 - 1);
    let base_from = recent_from - Duration::days(BASE as i64);
    let dims = candidate_dimensions(conn, def)?;
    for dim in dims {
        let primary = metrics.first();
        let extra = if primary.is_some() { format!(", sum(CASE WHEN d >= DATE '{recent_from}' THEN m END), sum(CASE WHEN d < DATE '{recent_from}' THEN m END)") } else { String::new() };
        let sql = format!(
            "SELECT g, sum(CASE WHEN d >= DATE '{recent_from}' THEN 1 ELSE 0 END), sum(CASE WHEN d < DATE '{recent_from}' THEN 1 ELSE 0 END){extra} \
             FROM (SELECT {d} AS d, CAST({} AS VARCHAR) AS g{} FROM {}) WHERE d >= DATE '{base_from}' AND d <= DATE '{latest}' AND g IS NOT NULL GROUP BY g",
            column_sql(dim),
            primary.map(|m| format!(", {} AS m", column_sql(m))).unwrap_or_default(),
            def.table
        );
        let mut stmt = conn.prepare(&sql)?;
        struct Row {
            g: String,
            rows: (f64, f64),
            metric: Option<(f64, f64)>,
        }
        let rows: Vec<Row> = stmt
            .query_map([], |r| {
                Ok(Row {
                    g: r.get(0)?,
                    rows: (r.get::<_, i64>(1)? as f64, r.get::<_, i64>(2)? as f64),
                    metric: if primary.is_some() { Some((r.get::<_, Option<f64>>(3)?.unwrap_or(0.0), r.get::<_, Option<f64>>(4)?.unwrap_or(0.0))) } else { None },
                })
            })?
            .collect::<Result<_, _>>()?;
        let total_base: f64 = rows.iter().map(|r| r.metric.map_or(r.rows.1, |m| m.1)).sum();
        let mut found: Vec<Finding> = Vec::new();
        for row in &rows {
            // 优先用主指标（金额），没有金额列时用行数
            let (label, money, recent, base) = match (primary, row.metric) {
                (Some(m), Some((r, b))) => (format!("{}合计", m.title), m.kind == "money", r, b),
                _ => ("行数".to_string(), false, row.rows.0, row.rows.1),
            };
            // 基线样本太少（日均不到 1 行）或占比太小的取值不报
            if row.rows.1 < BASE as f64 || total_base <= 0.0 || (base / total_base).abs() < 0.02 {
                continue;
            }
            let Some(change) = segment_change(recent, base) else { continue };
            if change.abs() < SEG_CHANGE_MIN {
                continue;
            }
            let (r_avg, b_avg) = (recent / RECENT as f64, base / BASE as f64);
            let vanished = row.rows.0 == 0.0;
            found.push(Finding {
                id: format!("segment:{}:{}", dim.key, row.g),
                kind: "segment",
                severity: if vanished || change.abs() >= 0.8 { "high" } else { "medium" },
                title: if vanished { format!("{}「{}」最近 {RECENT} 天没有数据", dim.title, row.g) } else { format!("{}「{}」{label}日均{} {}", dim.title, row.g, if change < 0.0 { "下降" } else { "上升" }, pct(change).trim_start_matches(['+', '-'])) },
                detail: format!("最近 {RECENT} 天日均 {}，此前 {BASE} 天日均 {}（{}）", fmt_num(r_avg, money), fmt_num(b_avg, money), pct(change)),
                dataset_id: def.id.clone(),
                dataset_name: def.name.clone(),
                actual: r_avg,
                expected: b_avg,
                change,
                drill: Drill { title: format!("{}「{}」最近 {RECENT} 天", dim.title, row.g), filters: vec![Filter::In { column: dim.key.clone(), values: vec![row.g.clone()] }, day_filter(&time.key, recent_from, latest)] },
                score: change.abs() * 3.0 * (base / total_base).abs().sqrt() * 5.0,
            });
        }
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        res.findings.extend(found.into_iter().take(2));
    }

    res.findings.sort_by(|a, b| b.score.total_cmp(&a.score));
    res.findings.truncate(MAX_FINDINGS);
    Ok(res)
}

/// 适合做分组的文本列：取值数在 2..=30 之间
fn candidate_dimensions<'a>(conn: &Connection, def: &'a DatasetDef) -> AppResult<Vec<&'a ColumnDef>> {
    let texts: Vec<&ColumnDef> = def.columns.iter().filter(|c| c.kind == "text" && !c.computed).take(8).collect();
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!("SELECT {} FROM {}", texts.iter().map(|c| format!("approx_count_distinct({})", column_sql(c))).collect::<Vec<_>>().join(", "), def.table);
    let counts: Vec<i64> = conn.query_row(&sql, [], |r| (0..texts.len()).map(|i| r.get::<_, i64>(i)).collect::<Result<_, _>>())?;
    Ok(texts.into_iter().zip(counts).filter(|(_, n)| (2..=MAX_CARDINALITY).contains(n)).map(|(c, _)| c).take(MAX_DIMENSIONS).collect())
}

/// 按 ID（或名称）找数据集后扫描；数据范围由 `dataset::datasets` 统一处理
pub fn scan_dataset(conn: &Connection, key: &str) -> AppResult<ScanResult> {
    let defs = dataset::datasets(conn)?;
    let def = defs.iter().find(|d| d.id == key).or_else(|| defs.iter().find(|d| d.name == key)).ok_or_else(|| AppError::bad(format!("找不到数据集「{key}」")))?;
    scan(conn, def)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// 从 start 起连续 n 天；值由 f(第几天, 星期几 0=周一) 给出
    fn series(start: &str, n: usize, f: impl Fn(usize, u32) -> f64) -> Vec<(NaiveDate, f64)> {
        (0..n).map(|i| (d(start) + Duration::days(i as i64), 0.0)).map(|(date, _)| (date, f((date - d(start)).num_days() as usize, date.weekday().num_days_from_monday()))).collect()
    }

    #[test]
    fn flat_series_has_no_findings() {
        let s = series("2026-08-01", 60, |i, _| 100.0 + [0.0, 3.0, -2.0, 1.0, -3.0][i % 5]);
        assert!(detect_series(&s).is_empty());
    }

    #[test]
    fn weekly_pattern_is_not_an_anomaly() {
        // 周末只有平日的 40%：这是常态
        let s = series("2026-08-03", 63, |_, wd| if wd >= 5 { 40.0 } else { 100.0 });
        assert!(detect_series(&s).is_empty(), "{:?}", detect_series(&s));
    }

    #[test]
    fn drop_on_a_weekday_is_found() {
        let mut s = series("2026-08-03", 63, |i, wd| (if wd >= 5 { 40.0 } else { 100.0 }) + [0.0, 2.0, -2.0][i % 3]);
        let n = s.len();
        s[n - 3].1 = 30.0; // 最近一周里某个工作日只有 30
        let spikes = detect_series(&s);
        assert_eq!(spikes.len(), 1, "{spikes:?}");
        assert_eq!(spikes[0].date, s[n - 3].0);
        assert!(spikes[0].z < -3.5 && spikes[0].actual / spikes[0].expected < 0.4);
    }

    #[test]
    fn missing_day_counts_as_zero() {
        let mut s = series("2026-08-03", 63, |_, _| 100.0);
        let n = s.len();
        let gone = s.remove(n - 2); // 最近一周里整天没有记录
        let spikes = detect_series(&s);
        assert!(spikes.iter().any(|x| x.date == gone.0 && x.actual == 0.0), "{spikes:?}");
    }

    #[test]
    fn small_wobble_is_ignored_even_when_data_is_very_flat() {
        // 基线几乎没有波动：z 很大但偏离只有 10%，不报
        let mut s = series("2026-08-03", 63, |_, _| 100.0);
        let n = s.len();
        s[n - 2].1 = 110.0;
        assert!(detect_series(&s).is_empty());
    }

    #[test]
    fn too_short_or_sparse_is_skipped() {
        assert!(detect_series(&series("2026-09-01", 15, |_, _| 100.0)).is_empty());
        let sparse: Vec<_> = (0..60).filter(|i| i % 7 == 0).map(|i| (d("2026-08-01") + Duration::days(i), 100.0)).collect();
        assert!(detect_series(&sparse).is_empty());
    }

    #[test]
    fn segment_change_math() {
        assert_eq!(segment_change(70.0, 280.0), Some(0.0));
        assert!((segment_change(35.0, 280.0).unwrap() + 0.5).abs() < 1e-9);
        assert_eq!(segment_change(10.0, 0.0), None);
    }

    #[test]
    fn number_formatting() {
        assert_eq!(fmt_num(123456.0, true), "12.35 万");
        assert_eq!(fmt_num(42.0, false), "42");
        assert_eq!(pct(-0.456), "-46%");
    }
}
