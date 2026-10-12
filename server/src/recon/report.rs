//! 对账的查询：汇总、未勾对 / 已勾对的行、勾对组

use chrono::NaiveDate;
use duckdb::Connection;
use serde::{Deserialize, Serialize};

use super::Ctx;
use crate::error::{AppError, AppResult};
use crate::schema::literal;

const MAX_PAGE: i64 = 500;

fn yuan(cents: i64) -> f64 {
    cents as f64 / 100.0
}

fn date_lit(s: &str, what: &str) -> AppResult<String> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| AppError::bad(format!("{what}的格式应为 2026-09-30")))?;
    Ok(format!("DATE {}", literal(s.trim())))
}

// ── 汇总 ──────────────────────────────────────────────

#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub rows: i64,
    pub total: f64,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SideSummary {
    pub label: String,
    /// 范围内的全部行
    pub all: Bucket,
    /// 范围内已勾对的行
    pub matched: Bucket,
    /// 截至日的未达账项（未勾对，或勾对的对方日期晚于截至日）：增加 / 减少 / 零
    pub open_in: Bucket,
    pub open_out: Bucket,
    pub open_zero: i64,
    /// 没有日期的行（设了截至日时不在范围内）
    pub undated: i64,
    pub min_date: Option<String>,
    pub max_date: Option<String>,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupStats {
    pub total: i64,
    pub exact: i64,
    pub tolerance: i64,
    pub sum: i64,
    pub manual: i64,
    /// 合计有差额的组（手工确认的）及差额合计（A 方 - B 方）
    pub with_diff: i64,
    pub diff_total: f64,
    /// 失效的组（组里的行被改过或删了）
    pub broken: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub as_of: Option<String>,
    pub left: SideSummary,
    pub right: SideSummary,
    pub groups: GroupStats,
}

pub fn summary(conn: &Connection, ctx: &Ctx, as_of: Option<&str>) -> AppResult<Summary> {
    let d = as_of.map(str::trim).filter(|s| !s.is_empty());
    let (inrange, outstanding) = match d {
        Some(x) => {
            let lit = date_lit(x, "截至日期")?;
            (format!("(dt IS NOT NULL AND dt <= {lit})"), format!("(dt IS NOT NULL AND dt <= {lit} AND (NOT ok OR gdone > {lit}))"))
        }
        None => ("TRUE".to_string(), "(NOT ok)".to_string()),
    };
    let sql = format!(
        "WITH u AS (SELECT *, max(dt) OVER (PARTITION BY gno) AS gdone FROM ({})),
         f AS (SELECT *, {inrange} AS inr, {outstanding} AS opn FROM u)
         SELECT tag,
           count(*) FILTER (WHERE inr), COALESCE(sum(cents) FILTER (WHERE inr), 0),
           count(*) FILTER (WHERE inr AND ok), COALESCE(sum(cents) FILTER (WHERE inr AND ok), 0),
           count(*) FILTER (WHERE opn AND cents > 0), COALESCE(sum(cents) FILTER (WHERE opn AND cents > 0), 0),
           count(*) FILTER (WHERE opn AND cents < 0), COALESCE(sum(cents) FILTER (WHERE opn AND cents < 0), 0),
           count(*) FILTER (WHERE opn AND cents = 0),
           count(*) FILTER (WHERE dt IS NULL),
           strftime(min(dt), '%Y-%m-%d'), strftime(max(dt), '%Y-%m-%d')
         FROM f GROUP BY tag ORDER BY tag",
        ctx.unified()?
    );
    let mut sides = [SideSummary { label: ctx.def.left.label.clone(), ..Default::default() }, SideSummary { label: ctx.def.right.label.clone(), ..Default::default() }];
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                [r.get::<_, i64>(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?],
                r.get::<_, Option<String>>(11)?,
                r.get::<_, Option<String>>(12)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (tag, v, min, max) in rows {
        let s = &mut sides[tag as usize];
        s.all = Bucket { rows: v[0], total: yuan(v[1]) };
        s.matched = Bucket { rows: v[2], total: yuan(v[3]) };
        s.open_in = Bucket { rows: v[4], total: yuan(v[5]) };
        s.open_out = Bucket { rows: v[6], total: yuan(v[7]) };
        s.open_zero = v[8];
        s.undated = v[9];
        s.min_date = min;
        s.max_date = max;
    }
    let [left, right] = sides;

    let mut groups = GroupStats::default();
    let mut stmt = conn.prepare("SELECT method, count(*), count(*) FILTER (WHERE diff_cents <> 0), COALESCE(sum(diff_cents), 0) FROM ledger_recon_group WHERE recon_id = ? GROUP BY method")?;
    let rows = stmt.query_map([&ctx.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut diff = 0;
    for (m, n, nd, d) in rows {
        groups.total += n;
        groups.with_diff += nd;
        diff += d;
        match m.as_str() {
            "exact" => groups.exact = n,
            "tolerance" => groups.tolerance = n,
            "sum" => groups.sum = n,
            _ => groups.manual = n,
        }
    }
    groups.diff_total = yuan(diff);
    groups.broken = conn.query_row(
        &format!(
            "SELECT count(*) FROM ledger_recon_group g
             LEFT JOIN (SELECT gno, count(*) AS c FROM ({}) WHERE gno IS NOT NULL GROUP BY gno) f ON f.gno = g.gno
             WHERE g.recon_id = ? AND COALESCE(f.c, 0) < g.item_count",
            ctx.unified()?
        ),
        [&ctx.id],
        |r| r.get(0),
    )?;
    Ok(Summary { as_of: d.map(String::from), left, right, groups })
}

// ── 行列表 ────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ItemQuery {
    /// left / right
    pub side: String,
    /// open 未勾对 / matched 已勾对 / all
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    /// 金额（元）：只看和它相差不超过容差的（找勾对对象用）
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ItemRow {
    pub key: String,
    pub date: Option<String>,
    pub amount: f64,
    pub corr: Option<String>,
    pub memo: Option<String>,
    pub gno: Option<i64>,
    pub method: Option<String>,
    /// 原勾对的行已变动
    pub stale: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ItemPage {
    pub total: i64,
    pub sum: f64,
    pub rows: Vec<ItemRow>,
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn items(conn: &Connection, ctx: &Ctx, q: &ItemQuery) -> AppResult<ItemPage> {
    let tag = match q.side.as_str() {
        "left" => 0,
        "right" => 1,
        other => return Err(AppError::bad(format!("side 应为 left 或 right：{other}"))),
    };
    let mut cond = vec![format!("tag = {tag}")];
    match q.status.as_deref().unwrap_or("open") {
        "open" => cond.push("NOT ok".into()),
        "matched" => cond.push("ok".into()),
        "all" => {}
        other => return Err(AppError::bad(format!("不认识的状态：{other}"))),
    }
    if let Some(k) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let like = literal(&format!("%{}%", escape_like(&k.to_lowercase())));
        cond.push(format!("lower(COALESCE(ktext, '') || ' ' || COALESCE(memo, '')) LIKE {like} ESCAPE '\\'"));
    }
    if let Some(a) = q.amount {
        let tol = ctx.def.tol_cents();
        let c = (a * 100.0).round() as i64;
        cond.push(format!("cents BETWEEN {} AND {}", c - tol, c + tol));
    }
    if let Some(f) = q.from.as_deref().filter(|s| !s.trim().is_empty()) {
        cond.push(format!("dt >= {}", date_lit(f, "起始日期")?));
    }
    if let Some(t) = q.to.as_deref().filter(|s| !s.trim().is_empty()) {
        cond.push(format!("dt <= {}", date_lit(t, "截止日期")?));
    }
    let where_sql = cond.join(" AND ");
    let u = ctx.unified()?;
    let (total, sum): (i64, i64) = conn.query_row(&format!("SELECT count(*), COALESCE(sum(cents), 0) FROM ({u}) WHERE {where_sql}"), [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let limit = q.limit.unwrap_or(100).clamp(1, MAX_PAGE);
    let offset = q.offset.unwrap_or(0).max(0);
    let mut stmt = conn.prepare(&format!(
        "SELECT rk, strftime(dt, '%Y-%m-%d'), cents, ktext, memo, gno, method, (gno IS NOT NULL AND NOT ok)
         FROM ({u}) WHERE {where_sql} ORDER BY dt NULLS LAST, rid LIMIT {limit} OFFSET {offset}"
    ))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ItemRow {
                key: r.get(0)?,
                date: r.get(1)?,
                amount: yuan(r.get(2)?),
                corr: r.get(3)?,
                memo: r.get(4)?,
                gno: r.get(5)?,
                method: r.get(6)?,
                stale: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ItemPage { total, sum: yuan(sum), rows })
}

// ── 勾对组 ────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GroupQuery {
    /// exact / tolerance / sum / manual
    #[serde(default)]
    pub method: Option<String>,
    /// 只看有差额的
    #[serde(default)]
    pub with_diff: bool,
    /// 只看失效的
    #[serde(default)]
    pub broken: bool,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupItem {
    /// 0 = A 方，1 = B 方
    pub side: i64,
    pub date: Option<String>,
    pub amount: f64,
    pub corr: Option<String>,
    pub memo: Option<String>,
    /// 这一行现在已经找不到了
    pub missing: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupRow {
    pub gno: i64,
    pub method: String,
    pub note: Option<String>,
    pub diff: f64,
    pub by: Option<String>,
    pub at: Option<String>,
    pub broken: bool,
    pub items: Vec<GroupItem>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupPage {
    pub total: i64,
    pub groups: Vec<GroupRow>,
}

pub fn groups(conn: &Connection, ctx: &Ctx, q: &GroupQuery) -> AppResult<GroupPage> {
    let u = ctx.unified()?;
    let mut cond = vec![format!("g.recon_id = {}", literal(&ctx.id))];
    if let Some(m) = q.method.as_deref().filter(|s| !s.is_empty()) {
        if !matches!(m, "exact" | "tolerance" | "sum" | "manual") {
            return Err(AppError::bad(format!("不认识的勾对方式：{m}")));
        }
        cond.push(format!("g.method = {}", literal(m)));
    }
    if q.with_diff {
        cond.push("g.diff_cents <> 0".into());
    }
    if q.broken {
        cond.push("COALESCE(f.c, 0) < g.item_count".into());
    }
    if let Some(k) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let like = literal(&format!("%{}%", escape_like(&k.to_lowercase())));
        cond.push(format!(
            "(lower(COALESCE(g.note, '')) LIKE {like} ESCAPE '\\' OR EXISTS (SELECT 1 FROM ledger_recon_item i2 JOIN found u2 ON u2.tag = i2.side AND u2.rk = i2.row_key
                WHERE i2.recon_id = g.recon_id AND i2.gno = g.gno AND lower(COALESCE(u2.ktext, '') || ' ' || COALESCE(u2.memo, '')) LIKE {like} ESCAPE '\\'))"
        ));
    }
    let where_sql = cond.join(" AND ");
    let with = format!("WITH found AS ({u}), f AS (SELECT gno, count(*) AS c FROM found WHERE gno IS NOT NULL GROUP BY gno)");
    let from = "FROM ledger_recon_group g LEFT JOIN f ON f.gno = g.gno";
    let total: i64 = conn.query_row(&format!("{with} SELECT count(*) {from} WHERE {where_sql}"), [], |r| r.get(0))?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    let mut stmt = conn.prepare(&format!(
        "{with} SELECT g.gno, g.method, g.note, g.diff_cents, g.matched_by, strftime(g.matched_at, '%Y-%m-%d %H:%M'), COALESCE(f.c, 0) < g.item_count
         {from} WHERE {where_sql} ORDER BY g.gno DESC LIMIT {limit} OFFSET {offset}"
    ))?;
    let mut rows = stmt
        .query_map([], |r| {
            Ok(GroupRow { gno: r.get(0)?, method: r.get(1)?, note: r.get(2)?, diff: yuan(r.get(3)?), by: r.get(4)?, at: r.get(5)?, broken: r.get(6)?, items: vec![] })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if !rows.is_empty() {
        let list = rows.iter().map(|g| g.gno.to_string()).collect::<Vec<_>>().join(", ");
        let mut stmt = conn.prepare(&format!(
            "WITH found AS ({u})
             SELECT i.gno, i.side, strftime(u.dt, '%Y-%m-%d'), i.cents, u.ktext, u.memo, u.rk IS NULL
             FROM ledger_recon_item i LEFT JOIN found u ON u.tag = i.side AND u.rk = i.row_key
             WHERE i.recon_id = {} AND i.gno IN ({list}) ORDER BY i.gno, i.side, u.dt NULLS LAST",
            literal(&ctx.id)
        ))?;
        let items = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, GroupItem { side: r.get(1)?, date: r.get(2)?, amount: yuan(r.get(3)?), corr: r.get(4)?, memo: r.get(5)?, missing: r.get(6)? }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (gno, it) in items {
            if let Some(g) = rows.iter_mut().find(|g| g.gno == gno) {
                g.items.push(it);
            }
        }
    }
    Ok(GroupPage { total, groups: rows })
}
