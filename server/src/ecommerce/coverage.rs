//! 月结完整性：店铺 × 月份矩阵，按天检测缺口，发现漏导 / 导出被截断
//!
//! 「应有流水的日期」= 当月中、店铺首笔流水之后（新店开店前不算缺）、昨天及以前（当天流水通常尚未导出）、
//! 已关店店铺最后一笔流水之前。区间内某天无流水 → 缺口；某天笔数低于该店日中位数的 30% → 疑似截断

use std::collections::HashMap;

use duckdb::Connection;
use serde::Serialize;

use crate::error::AppResult;
use crate::schema::ident;

const LOW_DAY_RATIO: f64 = 0.3;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageCell {
    pub shop: String,
    pub period: String,
    pub row_count: i64,
    /// complete 完整 / partial 有缺口或疑似截断 / missing 未导入 / not_expected 开店前或关店后
    pub status: String,
    /// 每日流水笔数（下标 0 = 1 日）
    pub days: Vec<i64>,
    /// 应有流水却没有的日期（日，1 起）
    pub missing_days: Vec<u32>,
    /// 笔数异常偏低的日期
    pub low_days: Vec<u32>,
    /// 应有流水的日期区间（日，1 起，含两端）；整月不应有数据时为 None
    pub expected_from: Option<u32>,
    pub expected_to: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageShop {
    pub name: String,
    pub status: String,
    /// 该店日笔数中位数（判断截断的基准）
    pub median_daily: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub today: String,
    pub periods: Vec<String>,
    pub shops: Vec<CoverageShop>,
    pub cells: Vec<CoverageCell>,
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

fn parse_period(p: &str) -> Option<(i32, u32)> {
    let (y, m) = p.split_once('-')?;
    Some((y.parse().ok()?, m.parse().ok()?))
}

fn next_period((y, m): (i32, u32)) -> (i32, u32) {
    if m == 12 { (y + 1, 1) } else { (y, m + 1) }
}

/// 日期字符串 YYYY-MM-DD → (period, day)
fn split_date(d: &str) -> Option<(String, u32)> {
    Some((d.get(0..7)?.to_string(), d.get(8..10)?.parse().ok()?))
}

pub fn coverage(conn: &Connection) -> AppResult<Coverage> {
    // 按本地时区取今天（DuckDB 的 current_date 是 UTC，北京时间 0~8 点会差一天）
    let today_date = crate::db::today_local();
    let (today, yesterday) = (today_date.to_string(), (today_date - chrono::Duration::days(1)).to_string());
    let time = ident("动账时间");

    // 每店每日笔数
    let mut daily: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let mut stmt = conn.prepare(&format!("SELECT shop, substr({time}, 1, 10), count(*) FROM ledger_detail GROUP BY 1, 2"))?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
        let (shop, day, n) = row?;
        daily.entry(shop).or_default().insert(day, n);
    }

    let mut shops = Vec::new();
    let mut stmt = conn.prepare("SELECT name, status FROM ledger_shop ORDER BY status, name")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        shops.push(row?);
    }

    // 月份范围：所有数据的最早月份 ~ 最晚月份
    let all_days: Vec<&String> = daily.values().flat_map(|m| m.keys()).collect();
    let (Some(min_day), Some(max_day)) = (all_days.iter().min(), all_days.iter().max()) else {
        return Ok(Coverage {
            today,
            periods: vec![],
            shops: shops.into_iter().map(|(name, status)| CoverageShop { name, status, median_daily: 0.0 }).collect(),
            cells: vec![],
        });
    };
    let mut periods = Vec::new();
    let (mut cur, end) = (parse_period(&min_day[..7]).unwrap_or((2000, 1)), parse_period(&max_day[..7]).unwrap_or((2000, 1)));
    while cur <= end {
        periods.push(format!("{:04}-{:02}", cur.0, cur.1));
        cur = next_period(cur);
    }

    let mut cells = Vec::new();
    let mut shop_infos = Vec::new();
    for (shop, status) in shops {
        let days_map = daily.get(&shop).cloned().unwrap_or_default();
        let mut counts: Vec<i64> = days_map.values().copied().collect();
        counts.sort_unstable();
        let median = if counts.is_empty() { 0.0 } else { counts[counts.len() / 2] as f64 };
        let first = days_map.keys().min().cloned();
        let last = days_map.keys().max().cloned();

        for period in &periods {
            let (y, m) = parse_period(period).unwrap_or((2000, 1));
            let n_days = days_in_month(y, m);
            let days: Vec<i64> = (1..=n_days).map(|d| *days_map.get(&format!("{period}-{d:02}")).unwrap_or(&0)).collect();
            let row_count: i64 = days.iter().sum();

            // 应有流水的日期区间
            let mut from = 1u32;
            let mut to = n_days;
            let clamp_after = |date: &str, from: &mut u32, to: &mut u32| match split_date(date) {
                Some((p, d)) if p == *period => *from = (*from).max(d),
                Some((p, _)) if p.as_str() > period.as_str() => *to = 0,
                _ => {}
            };
            let clamp_before = |date: &str, to: &mut u32| match split_date(date) {
                Some((p, d)) if p == *period => *to = (*to).min(d),
                Some((p, _)) if p.as_str() < period.as_str() => *to = 0,
                _ => {}
            };
            match &first {
                Some(f) => clamp_after(f, &mut from, &mut to),
                None => to = 0,
            }
            clamp_before(&yesterday, &mut to);
            if status == "closed"
                && let Some(l) = &last
            {
                clamp_before(l, &mut to);
            }

            let expected = to >= from && to > 0;
            let missing_days: Vec<u32> =
                if expected { (from..=to).filter(|d| days[(*d - 1) as usize] == 0).collect() } else { vec![] };
            let low_days: Vec<u32> = if expected && median > 0.0 {
                (from..=to)
                    .filter(|d| {
                        let n = days[(*d - 1) as usize];
                        n > 0 && (n as f64) < median * LOW_DAY_RATIO
                    })
                    .collect()
            } else {
                vec![]
            };
            let cell_status = if !expected {
                if row_count > 0 { "complete" } else { "not_expected" }
            } else if row_count == 0 {
                "missing"
            } else if !missing_days.is_empty() || !low_days.is_empty() {
                "partial"
            } else {
                "complete"
            };
            cells.push(CoverageCell {
                shop: shop.clone(),
                period: period.clone(),
                row_count,
                status: cell_status.into(),
                days,
                missing_days,
                low_days,
                expected_from: expected.then_some(from),
                expected_to: expected.then_some(to),
            });
        }
        shop_infos.push(CoverageShop { name: shop, status, median_daily: median });
    }

    Ok(Coverage { today, periods, shops: shop_infos, cells })
}
