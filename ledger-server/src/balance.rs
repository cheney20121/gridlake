//! 余额勾稽：证明「流水是全的」
//!
//! 每个 店铺 × 主体 × 动账账户 × 月份 录入平台资金账户的期初、期末余额，校验
//! `期初 + 入账 - 出账 = 期末`。不平说明有漏导或多导（汇总表内部自洽无法发现这类问题）；
//! 同时检查本月期初是否等于上月期末（衔接）

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::close;
use crate::error::{AppError, AppResult};
use crate::schema::ident;

/// 小于 1 分视为相等
const TOLERANCE: f64 = 0.005;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceRow {
    pub shop: String,
    pub entity: String,
    pub account: String,
    pub period: String,
    pub row_count: i64,
    pub inflow: f64,
    pub outflow: f64,
    /// 入账 - 出账
    pub net: f64,
    pub opening: Option<f64>,
    pub closing: Option<f64>,
    /// 期初 + 净额（期初未录入时为 None）
    pub expected_closing: Option<f64>,
    /// 期末 - 应有期末
    pub diff: Option<f64>,
    /// 上月录入的期末余额
    pub prev_closing: Option<f64>,
    /// 本月期初 - 上月期末
    pub continuity_diff: Option<f64>,
    /// missing 未录入 / balanced 已平 / unbalanced 不平
    pub status: String,
    pub remark: Option<String>,
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceInput {
    pub shop: String,
    pub entity: String,
    pub account: String,
    pub period: String,
    pub opening: Option<f64>,
    pub closing: Option<f64>,
    pub remark: Option<String>,
}

pub fn prev_period(period: &str) -> AppResult<String> {
    let (y, m) = period
        .split_once('-')
        .and_then(|(y, m)| Some((y.parse::<i32>().ok()?, m.parse::<u32>().ok()?)))
        .filter(|(_, m)| (1..=12).contains(m))
        .ok_or_else(|| AppError::bad(format!("月份格式应为 YYYY-MM：{period}")))?;
    Ok(if m == 1 { format!("{}-12", y - 1) } else { format!("{y}-{:02}", m - 1) })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// 某月全部 店铺 × 主体 × 账户 的勾稽结果（有流水或录过余额的组合都会列出）；shop 为空表示全部店铺
pub fn list(conn: &Connection, period: &str, shop: Option<&str>) -> AppResult<Vec<BalanceRow>> {
    let prev = prev_period(period)?;
    let (amt, dir, account, entity) = (ident("动账金额"), ident("动账方向"), ident("动账账户"), ident("商户主体名称"));
    let sql = format!(
        "WITH flow AS (
           SELECT shop, {entity} AS entity, COALESCE(NULLIF({account}, ''), '聚合账户') AS account, count(*) AS row_count,
                  COALESCE(sum({amt}) FILTER (WHERE {dir} = '入账'), 0) AS inflow,
                  COALESCE(sum({amt}) FILTER (WHERE {dir} = '出账'), 0) AS outflow
           FROM ledger_detail WHERE period = $1 GROUP BY ALL
         ),
         bal AS (SELECT * FROM ledger_balance WHERE period = $1),
         keys AS (SELECT shop, entity, account FROM flow UNION SELECT shop, entity, account FROM bal)
         SELECT k.shop, k.entity, k.account, COALESCE(f.row_count, 0),
                COALESCE(f.inflow, 0)::DOUBLE, COALESCE(f.outflow, 0)::DOUBLE,
                b.opening::DOUBLE, b.closing::DOUBLE,
                (b.opening + COALESCE(f.inflow, 0) - COALESCE(f.outflow, 0))::DOUBLE,
                (b.closing - (b.opening + COALESCE(f.inflow, 0) - COALESCE(f.outflow, 0)))::DOUBLE,
                p.closing::DOUBLE, (b.opening - p.closing)::DOUBLE,
                b.remark, b.updated_by, strftime(b.updated_at, '%Y-%m-%d %H:%M:%S')
         FROM keys k
         LEFT JOIN flow f USING (shop, entity, account)
         LEFT JOIN bal b USING (shop, entity, account)
         LEFT JOIN ledger_balance p ON p.shop = k.shop AND p.entity = k.entity AND p.account = k.account AND p.period = $2
         WHERE CAST($3 AS VARCHAR) IS NULL OR k.shop = $3
         ORDER BY k.shop, k.entity, k.account"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![period, prev, shop], |r| {
            let inflow: f64 = r.get(4)?;
            let outflow: f64 = r.get(5)?;
            let opening: Option<f64> = r.get(6)?;
            let closing: Option<f64> = r.get(7)?;
            let diff: Option<f64> = r.get::<_, Option<f64>>(9)?.map(round2);
            let status = match (opening, closing, diff) {
                (Some(_), Some(_), Some(d)) if d.abs() < TOLERANCE => "balanced",
                (Some(_), Some(_), _) => "unbalanced",
                _ => "missing",
            };
            Ok(BalanceRow {
                shop: r.get(0)?,
                entity: r.get(1)?,
                account: r.get(2)?,
                period: period.to_string(),
                row_count: r.get(3)?,
                inflow: round2(inflow),
                outflow: round2(outflow),
                net: round2(inflow - outflow),
                opening,
                closing,
                expected_closing: r.get::<_, Option<f64>>(8)?.map(round2),
                diff,
                prev_closing: r.get(10)?,
                continuity_diff: r.get::<_, Option<f64>>(11)?.map(round2),
                status: status.into(),
                remark: r.get(12)?,
                updated_by: r.get(13)?,
                updated_at: r.get(14)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 批量保存余额（覆盖同一 店铺 × 主体 × 账户 × 月份 的记录）；已提交审批或已锁账的月份不能修改
pub fn save(conn: &Connection, items: &[BalanceInput], operator: &str) -> AppResult<()> {
    for it in items {
        if it.shop.trim().is_empty() || it.entity.trim().is_empty() || it.account.trim().is_empty() {
            return Err(AppError::bad("店铺、主体、账户不能为空"));
        }
        prev_period(&it.period)?;
        close::ensure_editable(conn, &it.shop, &it.period)?;
        for v in [it.opening, it.closing].into_iter().flatten() {
            if !v.is_finite() || v.abs() >= 1e15 {
                return Err(AppError::bad(format!("余额数值不合法：{v}")));
            }
        }
    }
    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> anyhow::Result<()> {
        for it in items {
            conn.execute(
                "DELETE FROM ledger_balance WHERE shop = ? AND entity = ? AND account = ? AND period = ?",
                params![it.shop, it.entity, it.account, it.period],
            )?;
            let remark = it.remark.as_deref().map(str::trim).filter(|s| !s.is_empty());
            if it.opening.is_none() && it.closing.is_none() && remark.is_none() {
                continue;
            }
            conn.execute(
                "INSERT INTO ledger_balance (shop, entity, account, period, opening, closing, remark, updated_by, updated_at)
                 VALUES (?, ?, ?, ?, CAST(? AS DECIMAL(18,2)), CAST(? AS DECIMAL(18,2)), ?, ?, CAST(? AS TIMESTAMP))",
                params![it.shop, it.entity, it.account, it.period, it.opening.map(round2), it.closing.map(round2), remark, operator, crate::db::now_local()],
            )?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e.into());
        }
    }
    Ok(())
}
