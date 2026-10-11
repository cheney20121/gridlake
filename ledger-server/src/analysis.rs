//! 经营分析：按渠道 / 达人 / 商品 / 下单月份 / 店铺拆解资金流水，跨期矩阵，千川投流 ROI
//!
//! 各指标用与汇总表下钻相同的「规则项」定义（见 detail::RuleTerm）：前端点开任一数字时把同一组规则
//! 传给明细接口，明细贡献合计必然等于这里的数字。所有指标之和 = 资金净额（「其他收支」兜底），保证拆解不漏钱

use std::collections::BTreeSet;

use duckdb::types::Value;
use duckdb::{Connection, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::db::now_local;
use crate::detail::{RuleTerm, contribution_sql};
use crate::error::{AppError, AppResult};
use crate::schema::ident;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MetricDef {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub terms: Vec<RuleTerm>,
}

const SUBSIDY_COLUMNS: &[&str] = &[
    "实际平台补贴_运费",
    "实际平台补贴",
    "其他平台补贴",
    "以旧换新抵扣",
    "政府补贴平台垫资",
    "实际达人补贴",
    "实际抖音支付补贴",
    "实际抖音月付营销补贴",
    "银行补贴",
];
const COMMISSION_COLUMNS: &[&str] = &["佣金", "服务商佣金"];
const OTHER_FEE_COLUMNS: &[&str] = &["渠道分成", "招商服务费", "站外推广费", "其他分成"];

fn cols(columns: &[&str], sign: i8) -> Vec<RuleTerm> {
    columns.iter().map(|c| RuleTerm::Column { column: c.to_string(), op: None, sign }).collect()
}

/// 指标定义（顺序即资金瀑布顺序）：结算实付 + 运费 + 补贴 + 退款 + 平台服务费 + 佣金 + 其他分成 + 其他收支 = 资金净额
pub fn metrics() -> Vec<MetricDef> {
    let parts: Vec<(&'static str, &'static str, &'static str, Vec<&'static str>)> = vec![
        ("gmv", "结算实付", "订单实付应结：用户实付中本期结算给商家的部分", vec!["订单实付应结"]),
        ("freight", "运费", "运费实付", vec!["运费实付"]),
        ("subsidy", "补贴", "平台 / 达人 / 抖音支付 / 银行等各类补贴", SUBSIDY_COLUMNS.to_vec()),
        ("refund", "退款", "订单退款（负数）：结算时扣减与结算后退给用户", vec!["订单退款"]),
        ("platformFee", "平台服务费", "平台技术服务费（退款时退回的部分已抵减）", vec!["平台服务费"]),
        ("commission", "佣金", "达人佣金 + 服务商佣金（退款时退回的部分已抵减）", COMMISSION_COLUMNS.to_vec()),
        ("otherFee", "其他分成", "渠道分成、招商服务费、站外推广费、其他分成", OTHER_FEE_COLUMNS.to_vec()),
    ];
    let mut out: Vec<MetricDef> = parts.iter().map(|(key, label, hint, c)| MetricDef { key, label, hint, terms: cols(c, 1) }).collect();
    // 其他收支 = 资金净额 - 以上各项：运费险、小额打款、提现、千川充值、未开票抵扣等非订单收支，以及平台口径差异
    let mut other = vec![RuleTerm::Flow { sign: 1 }];
    for (_, _, _, c) in &parts {
        other.extend(cols(c, -1));
    }
    out.push(MetricDef { key: "other", label: "其他收支", hint: "资金净额减去以上各项：运费险、小额打款、提现、千川充值等", terms: other });
    out.push(MetricDef { key: "net", label: "资金净额", hint: "入账 - 出账", terms: vec![RuleTerm::Flow { sign: 1 }] });
    out
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0 + 0.0
}

// ── 维度拆解 ──────────────────────────────────────────

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Channel,
    Talent,
    Product,
    OrderMonth,
    Shop,
    Entity,
}

impl Dimension {
    /// (分组键, 显示名)；键为 NULL 时以 "" 返回
    fn exprs(self) -> (String, String) {
        let c = ident;
        match self {
            Self::Channel => (c("计费类型"), format!("COALESCE(any_value({}), '非订单流水')", c("计费类型"))),
            Self::Talent => (c("达人ID"), format!("COALESCE(any_value({}), '（无达人）')", c("达人名称"))),
            Self::Product => (c("商品ID"), format!("COALESCE(any_value({}), '（非订单流水）')", c("商品名称"))),
            Self::OrderMonth => {
                let m = format!("substr({}, 1, 7)", c("下单时间"));
                (m.clone(), format!("COALESCE(any_value({m}), '（非订单流水）')"))
            }
            Self::Shop => ("shop".into(), "any_value(shop)".into()),
            Self::Entity => (c("商户主体名称"), format!("COALESCE(any_value({}), '（空）')", c("商户主体名称"))),
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    /// 下钻用的键（"" 表示该字段为空）
    pub key: String,
    pub name: String,
    pub row_count: i64,
    /// 结算订单数（货款结算入账的去重订单号）
    pub orders: i64,
    /// 退款售后单数（订单退款 < 0 的去重售后编号）
    pub refund_count: i64,
    pub values: Map<String, serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakdown {
    pub period: String,
    pub metrics: Vec<MetricDef>,
    pub rows: Vec<BreakdownRow>,
    pub total: BreakdownRow,
}

fn metric_selects(defs: &[MetricDef]) -> AppResult<Vec<String>> {
    defs.iter().map(|m| Ok(format!("COALESCE(sum({}), 0)::DOUBLE", contribution_sql(&m.terms)?))).collect()
}

fn base_where(period: &str, shop: Option<&str>) -> (String, Vec<Value>) {
    let mut cond = vec!["period = ?".to_string()];
    let mut args = vec![Value::Text(period.to_string())];
    if let Some(s) = shop.filter(|s| !s.is_empty()) {
        cond.push("shop = ?".into());
        args.push(Value::Text(s.to_string()));
    }
    (cond.join(" AND "), args)
}

pub fn breakdown(conn: &Connection, period: &str, shop: Option<&str>, by: Dimension) -> AppResult<Breakdown> {
    let defs = metrics();
    let (key, name) = by.exprs();
    let (where_sql, args) = base_where(period, shop);
    let sql = format!(
        "SELECT COALESCE({key}, '') AS k, {name}, count(*),
                count(DISTINCT {order}) FILTER (WHERE {scene} = '货款结算入账'),
                count(DISTINCT {after}) FILTER (WHERE {refund} < 0),
                {metrics}
         FROM ledger_detail WHERE {where_sql}
         GROUP BY k",
        order = ident("订单号"),
        scene = ident("动账场景"),
        after = ident("售后编号"),
        refund = ident("订单退款"),
        metrics = metric_selects(&defs)?.join(", "),
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(params_from_iter(args.iter()), |r| {
            let mut values = Map::new();
            for (i, m) in defs.iter().enumerate() {
                values.insert(m.key.into(), round2(r.get::<_, f64>(5 + i)?).into());
            }
            Ok(BreakdownRow { key: r.get(0)?, name: r.get(1)?, row_count: r.get(2)?, orders: r.get(3)?, refund_count: r.get(4)?, values })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let gmv = |r: &BreakdownRow| r.values.get("gmv").and_then(|v| v.as_f64()).unwrap_or(0.0);
    rows.sort_by(|a, b| gmv(b).total_cmp(&gmv(a)).then_with(|| a.name.cmp(&b.name)));

    // 合计单独查询：订单可能跨商品 / 达人，各组去重数相加会重复计数
    let total_sql = format!(
        "SELECT count(*), count(DISTINCT {order}) FILTER (WHERE {scene} = '货款结算入账'),
                count(DISTINCT {after}) FILTER (WHERE {refund} < 0), {metrics}
         FROM ledger_detail WHERE {where_sql}",
        order = ident("订单号"),
        scene = ident("动账场景"),
        after = ident("售后编号"),
        refund = ident("订单退款"),
        metrics = metric_selects(&defs)?.join(", "),
    );
    let total = conn.query_row(&total_sql, params_from_iter(args.iter()), |r| {
        let mut values = Map::new();
        for (i, m) in defs.iter().enumerate() {
            values.insert(m.key.into(), round2(r.get::<_, f64>(3 + i)?).into());
        }
        Ok(BreakdownRow { key: String::new(), name: "合计".into(), row_count: r.get(0)?, orders: r.get(1)?, refund_count: r.get(2)?, values })
    })?;
    Ok(Breakdown { period: period.to_string(), metrics: defs, rows, total })
}

// ── 跨期矩阵 ──────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixCell {
    pub order_month: String,
    pub period: String,
    pub gmv: f64,
    pub net: f64,
    pub orders: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossPeriod {
    /// 已导入数据的结算月份（升序）
    pub periods: Vec<String>,
    /// 出现过的下单月份（升序）
    pub order_months: Vec<String>,
    pub cells: Vec<MatrixCell>,
}

/// 下单月份 × 结算月份：只统计带下单时间的订单类流水
pub fn cross_period(conn: &Connection, shop: Option<&str>) -> AppResult<CrossPeriod> {
    let gmv = contribution_sql(&cols(&["订单实付应结"], 1))?;
    let net = contribution_sql(&[RuleTerm::Flow { sign: 1 }])?;
    let om = format!("substr({}, 1, 7)", ident("下单时间"));
    let sql = format!(
        "SELECT {om} AS om, period, COALESCE(sum({gmv}), 0)::DOUBLE, COALESCE(sum({net}), 0)::DOUBLE,
                count(DISTINCT {order}) FILTER (WHERE {scene} = '货款结算入账')
         FROM ledger_detail WHERE {om} IS NOT NULL AND (CAST($1 AS VARCHAR) IS NULL OR shop = $1)
         GROUP BY ALL ORDER BY om, period",
        order = ident("订单号"),
        scene = ident("动账场景"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let cells = stmt
        .query_map(params![shop.filter(|s| !s.is_empty())], |r| {
            Ok(MatrixCell { order_month: r.get(0)?, period: r.get(1)?, gmv: round2(r.get(2)?), net: round2(r.get(3)?), orders: r.get(4)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare("SELECT DISTINCT period FROM ledger_detail WHERE CAST($1 AS VARCHAR) IS NULL OR shop = $1 ORDER BY 1")?;
    let periods = stmt.query_map(params![shop.filter(|s| !s.is_empty())], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    let order_months = cells.iter().map(|c| c.order_month.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    Ok(CrossPeriod { periods, order_months, cells })
}

// ── 千川投流 ──────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QianchuanRow {
    pub shop: String,
    /// 千川账户充值（出账为正）
    pub recharge: f64,
    /// 千川后台实际消耗（手工录入，未录入为 None）
    pub spend: Option<f64>,
    pub spend_remark: Option<String>,
    pub spend_updated_by: Option<String>,
    /// 巨量千川渠道的结算实付
    pub gmv: f64,
    /// 巨量千川渠道的退款（负数）
    pub refund: f64,
    /// 巨量千川渠道的资金净额（结算回款 - 费用 - 退款）
    pub net: f64,
    pub orders: i64,
    /// 店铺全部结算实付（算千川占比）
    pub shop_gmv: f64,
}

pub fn qianchuan(conn: &Connection, period: &str) -> AppResult<Vec<QianchuanRow>> {
    let flow = contribution_sql(&[RuleTerm::Flow { sign: 1 }])?;
    let (billing, scene, gmv, refund) = (ident("计费类型"), ident("动账场景"), ident("订单实付应结"), ident("订单退款"));
    let sql = format!(
        "WITH d AS (SELECT * FROM ledger_detail WHERE period = $1)
         SELECT s.shop,
                COALESCE(-sum({flow}) FILTER (WHERE d.{scene} LIKE '%千川%'), 0)::DOUBLE,
                a.spend::DOUBLE, a.remark, a.updated_by,
                COALESCE(sum(d.{gmv}) FILTER (WHERE d.{billing} = '巨量千川'), 0)::DOUBLE,
                COALESCE(sum(d.{refund}) FILTER (WHERE d.{billing} = '巨量千川'), 0)::DOUBLE,
                COALESCE(sum({flow}) FILTER (WHERE d.{billing} = '巨量千川'), 0)::DOUBLE,
                count(DISTINCT d.{order}) FILTER (WHERE d.{billing} = '巨量千川' AND d.{scene} = '货款结算入账'),
                COALESCE(sum(d.{gmv}), 0)::DOUBLE
         FROM (SELECT DISTINCT shop FROM d UNION SELECT shop FROM ledger_ad_spend WHERE period = $1) s
         LEFT JOIN d ON d.shop = s.shop
         LEFT JOIN ledger_ad_spend a ON a.shop = s.shop AND a.period = $1
         GROUP BY s.shop, a.spend, a.remark, a.updated_by ORDER BY s.shop",
        order = ident("订单号"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![period], |r| {
            Ok(QianchuanRow {
                shop: r.get(0)?,
                recharge: round2(r.get(1)?),
                spend: r.get(2)?,
                spend_remark: r.get(3)?,
                spend_updated_by: r.get(4)?,
                gmv: round2(r.get(5)?),
                refund: round2(r.get(6)?),
                net: round2(r.get(7)?),
                orders: r.get(8)?,
                shop_gmv: round2(r.get(9)?),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdSpendInput {
    pub shop: String,
    pub period: String,
    /// 为空表示删除
    pub spend: Option<f64>,
    pub remark: Option<String>,
    #[serde(default)]
    pub operator: String,
}

/// 千川消耗只影响分析口径、不改动流水，锁账后仍可维护
pub fn save_ad_spend(conn: &Connection, input: &AdSpendInput) -> AppResult<()> {
    crate::balance::prev_period(&input.period)?;
    if input.shop.trim().is_empty() {
        return Err(AppError::bad("店铺不能为空"));
    }
    if let Some(v) = input.spend
        && (!v.is_finite() || !(0.0..1e13).contains(&v))
    {
        return Err(AppError::bad(format!("千川消耗数值不合法：{v}")));
    }
    conn.execute("DELETE FROM ledger_ad_spend WHERE shop = ? AND period = ?", params![input.shop, input.period])?;
    if let Some(v) = input.spend {
        conn.execute(
            "INSERT INTO ledger_ad_spend (shop, period, spend, remark, updated_by, updated_at)
             VALUES (?, ?, CAST(? AS DECIMAL(18,2)), ?, ?, CAST(? AS TIMESTAMP))",
            params![
                input.shop,
                input.period,
                round2(v),
                input.remark.as_deref().map(str::trim).filter(|s| !s.is_empty()),
                input.operator,
                now_local()
            ],
        )?;
    }
    Ok(())
}
