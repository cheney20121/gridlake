//! 数据概况：一眼看清一个数据集（按当前筛选）每一列的情况——空值有多少、有多少种不同的值、范围、分布
//!
//! - 概况（[`overview`]）：所有列的统计在**一次扫描**里算完：非空 / 空值个数、近似去重数（HyperLogLog），
//!   文本列的空白个数与长度，数值列的合计、平均、标准差、近似四分位数、零和负数的个数，日期列的范围与认不出的写法，是否列的「是」个数
//! - 单列明细（[`column`]）：数值列等宽分箱的直方图（整数取值少时每个值一档），日期列按日 / 月 / 年计数（按跨度自动选），
//!   以及最常见的取值和其余的个数。前端点一档就能按它筛选（区间 / 前缀 / 取值）
//! - 和数据表看到的行一致：同样的筛选、关键字、汇总项规则，数据范围受限的用户只统计自己范围内的行（数据来源已换成带过滤的子查询）
//! - 去重数：先用 HyperLogLog 估一遍，估出来不超过 10 万的列再精确数一遍（取值少的列估得反而不准，精确数也便宜）；
//!   更多的（订单号这类）保留近似值并标明。分位数是近似值（1 亿行上也是一次扫描几秒钟）

use duckdb::types::Value;
use duckdb::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::dataset::{self, ColumnDef, QuerySpec};
use crate::error::{AppError, AppResult};
use crate::kinds;
use crate::tabular::to_json;

/// 直方图的档数
const BINS: i64 = 20;
/// 整数列的取值个数不超过这么多时，每个值一档
const SMALL_INTEGER_RANGE: i64 = 30;
/// 最常见的取值列出多少个
const TOP: i64 = 20;
/// 一次最多统计多少列（列再多就分批请求）
const MAX_COLUMNS: usize = 300;
/// 估计的不同值个数不超过这么多时精确再数一遍
const EXACT_DISTINCT_UNDER: i64 = 100_000;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRequest {
    #[serde(flatten)]
    pub spec: QuerySpec,
    /// 只统计这些列（列名或标题）；不填统计全部列
    #[serde(default)]
    pub columns: Option<Vec<String>>,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ColumnProfile {
    pub key: String,
    pub title: String,
    pub kind: &'static str,
    pub computed: bool,
    pub non_null: i64,
    pub nulls: i64,
    /// 不同值个数（不含空值）；distinct_exact 为 false 时是 HyperLogLog 的估计值
    pub distinct: i64,
    pub distinct_exact: bool,
    pub min: Json,
    pub max: Json,
    /// 文本 / 编号：只有空白的个数，长度的最小 / 平均 / 最大
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blanks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_len: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_len: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_len: Option<i64>,
    /// 数值：合计、平均、标准差、近似四分位数，零和负数的个数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub std: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q1: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q3: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zeros: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negatives: Option<i64>,
    /// 日期时间：不是「2026-09-15」「2026-09-15 10:00:00」写法的个数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid: Option<i64>,
    /// 是否：「是」的个数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trues: Option<i64>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// 按当前筛选的行数
    pub rows: i64,
    pub columns: Vec<ColumnProfile>,
    pub elapsed_ms: u64,
}

/// 一列要算的统计：(SQL 表达式, 写回哪个字段)
type Stat = (String, fn(&mut ColumnProfile, &duckdb::Row<'_>, usize) -> duckdb::Result<()>);

fn int(r: &duckdb::Row<'_>, i: usize) -> duckdb::Result<i64> {
    Ok(r.get::<_, Option<i64>>(i)?.unwrap_or(0))
}

fn float(r: &duckdb::Row<'_>, i: usize) -> duckdb::Result<Option<f64>> {
    Ok(r.get::<_, Option<f64>>(i)?.filter(|v| v.is_finite()))
}

/// 日期时间列的「认得出」写法（与导入时转换后的写法一致）
const DATE_RE: &str = r"\d{4}-\d{2}-\d{2}( \d{2}:\d{2}(:\d{2})?)?";

fn stats_for(col: &ColumnDef) -> Vec<Stat> {
    let c = format!("({})", dataset::column_sql(col));
    let valid = if col.kind == "datetime" { format!(" FILTER (WHERE regexp_full_match(CAST({c} AS VARCHAR), '{DATE_RE}'))") } else { String::new() };
    let mut out: Vec<Stat> = vec![
        (format!("count({c})"), |p, r, i| {
            p.non_null = int(r, i)?;
            Ok(())
        }),
        (format!("approx_count_distinct({c})"), |p, r, i| {
            p.distinct = int(r, i)?;
            Ok(())
        }),
        // 日期时间列的范围只看认得出的写法（「下周一」这类文本不该排在最后）
        (format!("min({c}){valid}"), |p, r, i| {
            p.min = to_json(r.get_ref(i)?);
            Ok(())
        }),
        (format!("max({c}){valid}"), |p, r, i| {
            p.max = to_json(r.get_ref(i)?);
            Ok(())
        }),
    ];
    let num = format!("TRY_CAST({c} AS DOUBLE)");
    match col.kind {
        k if kinds::is_numeric(k) => {
            out.push((format!("sum({num})"), |p, r, i| {
                p.sum = float(r, i)?;
                Ok(())
            }));
            out.push((format!("avg({num})"), |p, r, i| {
                p.mean = float(r, i)?;
                Ok(())
            }));
            out.push((format!("stddev_samp({num})"), |p, r, i| {
                p.std = float(r, i)?;
                Ok(())
            }));
            out.push((format!("approx_quantile({num}, 0.25)"), |p, r, i| {
                p.q1 = float(r, i)?;
                Ok(())
            }));
            out.push((format!("approx_quantile({num}, 0.5)"), |p, r, i| {
                p.median = float(r, i)?;
                Ok(())
            }));
            out.push((format!("approx_quantile({num}, 0.75)"), |p, r, i| {
                p.q3 = float(r, i)?;
                Ok(())
            }));
            out.push((format!("count(*) FILTER (WHERE {num} = 0)"), |p, r, i| {
                p.zeros = Some(int(r, i)?);
                Ok(())
            }));
            out.push((format!("count(*) FILTER (WHERE {num} < 0)"), |p, r, i| {
                p.negatives = Some(int(r, i)?);
                Ok(())
            }));
        }
        "datetime" => {
            out.push((format!("count(*) FILTER (WHERE {c} IS NOT NULL AND NOT regexp_full_match(CAST({c} AS VARCHAR), '{DATE_RE}'))"), |p, r, i| {
                p.invalid = Some(int(r, i)?);
                Ok(())
            }));
        }
        "boolean" => {
            out.push((format!("count(*) FILTER (WHERE {c})"), |p, r, i| {
                p.trues = Some(int(r, i)?);
                Ok(())
            }));
        }
        _ => {
            let t = format!("CAST({c} AS VARCHAR)");
            out.push((format!("count(*) FILTER (WHERE trim({t}) = '')"), |p, r, i| {
                p.blanks = Some(int(r, i)?);
                Ok(())
            }));
            out.push((format!("min(length({t}))"), |p, r, i| {
                p.min_len = r.get(i)?;
                Ok(())
            }));
            out.push((format!("avg(length({t}))"), |p, r, i| {
                p.avg_len = float(r, i)?;
                Ok(())
            }));
            out.push((format!("max(length({t}))"), |p, r, i| {
                p.max_len = r.get(i)?;
                Ok(())
            }));
        }
    }
    out
}

fn pick<'a>(def: &'a dataset::DatasetDef, only: Option<&[String]>) -> AppResult<Vec<&'a ColumnDef>> {
    let cols: Vec<&ColumnDef> = match only {
        Some(list) if !list.is_empty() => list.iter().map(|k| dataset::find_column(def, k)).collect::<AppResult<_>>()?,
        _ => def.columns.iter().collect(),
    };
    if cols.len() > MAX_COLUMNS {
        return Err(AppError::bad(format!("一次最多统计 {MAX_COLUMNS} 列，请分批（columns）")));
    }
    Ok(cols)
}

/// 所有（或指定的）列的概况，一次扫描
pub fn overview(conn: &Connection, dataset_id: &str, req: &ProfileRequest) -> AppResult<Profile> {
    let started = std::time::Instant::now();
    let def = dataset::dataset(conn, dataset_id)?;
    let cols = pick(&def, req.columns.as_deref())?;
    let (where_sql, args) = dataset::rows_sql(&def, &req.spec)?;
    let plans: Vec<Vec<Stat>> = cols.iter().map(|c| stats_for(c)).collect();
    let mut selects = vec!["count(*)".to_string()];
    selects.extend(plans.iter().flatten().map(|(sql, _)| sql.clone()));
    let sql = format!("SELECT {} FROM {} WHERE {where_sql}", selects.join(", "), def.table);
    let (rows, columns) = conn.query_row(&sql, params_from_iter(args.iter()), |r| {
        let rows: i64 = r.get(0)?;
        let mut i = 1;
        let mut out = Vec::with_capacity(cols.len());
        for (col, plan) in cols.iter().zip(&plans) {
            let mut p = ColumnProfile { key: col.key.clone(), title: col.title.clone(), kind: col.kind, computed: col.computed, ..Default::default() };
            for (_, write) in plan {
                write(&mut p, r, i)?;
                i += 1;
            }
            p.nulls = rows - p.non_null;
            // 近似去重数可能比非空个数略大
            p.distinct = p.distinct.min(p.non_null);
            out.push(p);
        }
        Ok((rows, out))
    })?;
    let mut columns = columns;
    // 第二遍：取值不多的列精确数一遍不同值
    let small: Vec<usize> = columns.iter().enumerate().filter(|(_, p)| p.distinct <= EXACT_DISTINCT_UNDER).map(|(i, _)| i).collect();
    if !small.is_empty() {
        let exprs: Vec<String> = small.iter().map(|&i| format!("count(DISTINCT ({}))", dataset::column_sql(cols[i]))).collect();
        let sql = format!("SELECT {} FROM {} WHERE {where_sql}", exprs.join(", "), def.table);
        let exact: Vec<i64> = conn.query_row(&sql, params_from_iter(args.iter()), |r| (0..small.len()).map(|k| int(r, k)).collect())?;
        for (&i, n) in small.iter().zip(exact) {
            columns[i].distinct = n;
            columns[i].distinct_exact = true;
        }
    }
    Ok(Profile { rows, columns, elapsed_ms: started.elapsed().as_millis() as u64 })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnRequest {
    #[serde(flatten)]
    pub spec: QuerySpec,
    pub column: String,
}

/// 直方图的一档。按这一档筛选：数值用 [from, to] 区间，日期用 prefix 前缀
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    pub count: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TopValue {
    pub value: String,
    pub count: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ColumnDetail {
    pub key: String,
    pub title: String,
    pub kind: &'static str,
    pub rows: i64,
    pub nulls: i64,
    /// bin（等宽分箱）/ value（整数每个值一档）/ day / month / year；文本、是否列没有直方图
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granularity: Option<&'static str>,
    pub histogram: Vec<Bucket>,
    /// 最常见的取值（不含空值）与其余非空值的个数
    pub top: Vec<TopValue>,
    pub others: i64,
    pub elapsed_ms: u64,
}

/// 数值的显示：整数不带小数，其他保留到有意义的位数
fn num_label(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// 数值列的分档：(下界, 档宽, 档数, 是否每个整数一档)
fn bins(min: f64, max: f64, integer: bool) -> (f64, f64, i64, bool) {
    if integer && max - min < SMALL_INTEGER_RANGE as f64 {
        return (min, 1.0, (max - min) as i64 + 1, true);
    }
    if max <= min {
        return (min, 1.0, 1, false);
    }
    (min, (max - min) / BINS as f64, BINS, false)
}

/// 日期按跨度选粒度：两个月以内按日，十年以内按月，再长按年
fn time_grain(min: &str, max: &str) -> (&'static str, usize) {
    let day = |s: &str| chrono::NaiveDate::parse_from_str(s.get(..10).unwrap_or(""), "%Y-%m-%d").ok();
    match (day(min), day(max)) {
        (Some(a), Some(b)) if (b - a).num_days() <= 62 => ("day", 10),
        (Some(a), Some(b)) if (b - a).num_days() <= 3660 => ("month", 7),
        _ => ("year", 4),
    }
}

/// 一列的分布：直方图 + 最常见的取值
pub fn column(conn: &Connection, dataset_id: &str, req: &ColumnRequest) -> AppResult<ColumnDetail> {
    let started = std::time::Instant::now();
    let def = dataset::dataset(conn, dataset_id)?;
    let col = dataset::find_column(&def, &req.column)?.clone();
    let (where_sql, args) = dataset::rows_sql(&def, &req.spec)?;
    let src = &def.table;
    let c = format!("({})", dataset::column_sql(&col));
    let p = params_from_iter(args.iter());
    let valid = if col.kind == "datetime" { format!(" FILTER (WHERE regexp_full_match(CAST({c} AS VARCHAR), '{DATE_RE}'))") } else { String::new() };
    let (rows, non_null, min, max): (i64, i64, Json, Json) =
        conn.query_row(&format!("SELECT count(*), count({c}), min({c}){valid}, max({c}){valid} FROM {src} WHERE {where_sql}"), p, |r| Ok((r.get(0)?, r.get(1)?, to_json(r.get_ref(2)?), to_json(r.get_ref(3)?))))?;

    let mut histogram = Vec::new();
    let mut granularity = None;
    if non_null > 0 && kinds::is_numeric(col.kind) {
        let (lo, hi) = (json_f64(&min), json_f64(&max));
        if let (Some(lo), Some(hi)) = (lo, hi) {
            let (base, width, n, per_value) = bins(lo, hi, col.kind == "integer");
            granularity = Some(if per_value { "value" } else { "bin" });
            let mut stmt = conn.prepare(&format!(
                "SELECT least(greatest(floor((TRY_CAST({c} AS DOUBLE) - ?) / ?)::BIGINT, 0), ?) AS b, count(*) FROM {src}
                 WHERE {where_sql} AND {c} IS NOT NULL GROUP BY b ORDER BY b"
            ))?;
            // 参数顺序：SQL 里分箱表达式在前、WHERE 在后
            let ordered: Vec<Value> = [Value::Double(base), Value::Double(width), Value::BigInt(n - 1)].into_iter().chain(args.iter().cloned()).collect();
            let counts: std::collections::HashMap<i64, i64> = stmt.query_map(params_from_iter(ordered.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
            for b in 0..n {
                let from = base + width * b as f64;
                let to = if b == n - 1 { hi } else if per_value { from } else { base + width * (b + 1) as f64 };
                let label = if per_value || n == 1 { num_label(from) } else { format!("{} ~ {}", num_label(from), num_label(to)) };
                histogram.push(Bucket { label, from: Some(from), to: Some(to), prefix: None, count: counts.get(&b).copied().unwrap_or(0) });
            }
        }
    } else if non_null > 0 && col.kind == "datetime" {
        let (grain, len) = time_grain(min.as_str().unwrap_or(""), max.as_str().unwrap_or(""));
        granularity = Some(grain);
        let mut stmt = conn.prepare(&format!(
            "SELECT left(CAST({c} AS VARCHAR), {len}) AS k, count(*) FROM {src} WHERE {where_sql} AND {c} IS NOT NULL
             AND regexp_full_match(CAST({c} AS VARCHAR), '{DATE_RE}') GROUP BY k ORDER BY k LIMIT 1000"
        ))?;
        histogram = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                let k: String = r.get(0)?;
                Ok(Bucket { label: k.clone(), from: None, to: None, prefix: Some(k), count: r.get(1)? })
            })?
            .collect::<Result<_, _>>()?;
    }

    let mut stmt = conn.prepare(&format!(
        "SELECT CAST({c} AS VARCHAR) AS v, count(*) AS n FROM {src} WHERE {where_sql} AND {c} IS NOT NULL GROUP BY v ORDER BY n DESC, v LIMIT {TOP}"
    ))?;
    let top: Vec<TopValue> = stmt.query_map(params_from_iter(args.iter()), |r| Ok(TopValue { value: r.get(0)?, count: r.get(1)? }))?.collect::<Result<_, _>>()?;
    let others = non_null - top.iter().map(|t| t.count).sum::<i64>();
    Ok(ColumnDetail {
        key: col.key,
        title: col.title,
        kind: col.kind,
        rows,
        nulls: rows - non_null,
        granularity,
        histogram,
        top,
        others,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn json_f64(v: &Json) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binning() {
        assert_eq!(bins(1.0, 5.0, true), (1.0, 1.0, 5, true), "取值少的整数：每个值一档");
        assert_eq!(bins(0.0, 100.0, false), (0.0, 5.0, BINS, false));
        assert_eq!(bins(0.0, 1000.0, true).2, BINS, "取值多的整数：等宽分箱");
        assert_eq!(bins(7.0, 7.0, false), (7.0, 1.0, 1, false), "只有一个值");
        assert_eq!(num_label(12.0), "12");
        assert_eq!(num_label(0.125), "0.125");
        assert_eq!(num_label(2.50), "2.5");
        assert_eq!(time_grain("2026-09-01 08:00:00", "2026-10-15"), ("day", 10));
        assert_eq!(time_grain("2024-01-01", "2026-10-15"), ("month", 7));
        assert_eq!(time_grain("1990-01-01", "2026-10-15"), ("year", 4));
    }
}
