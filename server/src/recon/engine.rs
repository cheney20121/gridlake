//! 勾对：自动（三轮）、手工、取消
//!
//! 全部在一个事务里，用临时表 rc_free（还没勾对的行）逐轮消耗；自动勾对的「试算」就是事务最后回滚

use std::collections::HashSet;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use super::Ctx;
use crate::error::{AppError, AppResult};
use crate::schema::literal;

/// 第二轮的候选配对上限：超过说明两侧有大量金额相同、又没有对账键可区分的行，跳过这一轮（并提示）
const MAX_CANDIDATES: i64 = 3_000_000;
/// 手工勾对一次最多多少行
pub const MAX_MANUAL_ROWS: usize = 5000;
const INSERT_CHUNK: usize = 4000;

fn drop_temp(conn: &Connection) {
    let _ = conn.execute_batch("DROP TABLE IF EXISTS rc_free; DROP TABLE IF EXISTS rc_new; DROP TABLE IF EXISTS rc_all; DROP TABLE IF EXISTS rc_gmap; DROP TABLE IF EXISTS rc_broken;");
}

fn keys_not_null(n: usize, alias: &str) -> String {
    if n == 0 { "TRUE".into() } else { (0..n).map(|i| format!("{alias}.k{i} IS NOT NULL")).collect::<Vec<_>>().join(" AND ") }
}

fn keys_equal(n: usize) -> String {
    if n == 0 { "TRUE".into() } else { (0..n).map(|i| format!("l.k{i} = r.k{i}")).collect::<Vec<_>>().join(" AND ") }
}

fn key_cols(n: usize) -> String {
    (0..n).map(|i| format!(", k{i}")).collect()
}

/// 两侧日期都有、且相差不超过窗口天数
fn date_window(days: Option<u32>) -> String {
    match days {
        Some(d) => format!("l.dt IS NOT NULL AND r.dt IS NOT NULL AND abs(date_diff('day', l.dt, r.dt)) <= {d}"),
        None => "TRUE".into(),
    }
}

// ── 失效的勾对 ────────────────────────────────────────

/// 删除失效的勾对组（组里记录的行现在找不到了：被改过或删了），返回删除的组数
pub(super) fn cleanup_broken(conn: &Connection, ctx: &Ctx) -> AppResult<i64> {
    let u = ctx.unified()?;
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE rc_broken AS
         SELECT g.gno FROM ledger_recon_group g
         LEFT JOIN (SELECT gno, count(*) AS c FROM ({u}) WHERE gno IS NOT NULL GROUP BY gno) f ON f.gno = g.gno
         WHERE g.recon_id = {} AND COALESCE(f.c, 0) < g.item_count",
        literal(&ctx.id)
    ))?;
    let n: i64 = conn.query_row("SELECT count(*) FROM rc_broken", [], |r| r.get(0))?;
    if n > 0 {
        conn.execute("DELETE FROM ledger_recon_item WHERE recon_id = ? AND gno IN (SELECT gno FROM rc_broken)", [&ctx.id])?;
        conn.execute("DELETE FROM ledger_recon_group WHERE recon_id = ? AND gno IN (SELECT gno FROM rc_broken)", [&ctx.id])?;
    }
    Ok(n)
}

// ── 自动勾对 ──────────────────────────────────────────

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AutoOutcome {
    /// 只是试算（没有保存）
    pub dry_run: bool,
    /// 金额完全相同的（第一轮，以及第二轮里没有差额的）
    pub exact: i64,
    /// 金额有差额、在容差内的（第二轮）
    pub tolerance: i64,
    /// 第三轮：同一对账键下合计相等
    pub sum: i64,
    pub groups: i64,
    pub matched_rows: i64,
    /// 这次清理掉的失效勾对组
    pub cleared_broken: i64,
    /// 还没勾对的行
    pub left_open: i64,
    pub right_open: i64,
    pub warnings: Vec<String>,
}

/// 把 rc_new（gk, tag, rk, cents）写成勾对组，并从 rc_free 里去掉。返回 (组数, 行数)
fn commit_new(conn: &Connection, ctx: &Ctx, method: &str, operator: &str) -> AppResult<(i64, i64)> {
    let rows: i64 = conn.query_row("SELECT count(*) FROM rc_new", [], |r| r.get(0))?;
    if rows == 0 {
        return Ok((0, 0));
    }
    let base: i64 = conn.query_row("SELECT COALESCE(max(gno), 0) FROM ledger_recon_group WHERE recon_id = ?", [&ctx.id], |r| r.get(0))?;
    conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE rc_gmap AS SELECT gk, dense_rank() OVER (ORDER BY gk) + {base} AS gno FROM (SELECT DISTINCT gk FROM rc_new)"))?;
    conn.execute(
        "INSERT INTO ledger_recon_group (recon_id, gno, method, note, diff_cents, item_count, matched_by, matched_at)
         SELECT ?, m.gno, ?, NULL, sum(CASE n.tag WHEN 0 THEN n.cents ELSE -n.cents END), count(*), ?, CAST(? AS TIMESTAMP)
         FROM rc_new n JOIN rc_gmap m USING (gk) GROUP BY m.gno",
        params![ctx.id, method, operator, crate::db::now_local()],
    )?;
    conn.execute("INSERT INTO ledger_recon_item (recon_id, gno, side, row_key, cents) SELECT ?, m.gno, n.tag, n.rk, n.cents FROM rc_new n JOIN rc_gmap m USING (gk)", [&ctx.id])?;
    conn.execute_batch("DELETE FROM rc_free f WHERE EXISTS (SELECT 1 FROM rc_new n WHERE n.tag = f.tag AND n.rk = f.rk)")?;
    let groups: i64 = conn.query_row("SELECT count(*) FROM rc_gmap", [], |r| r.get(0))?;
    Ok((groups, rows))
}

/// 第一轮：对账键与金额都相同的，同一桶里按日期顺序一一对应
fn pass_exact(conn: &Connection, ctx: &Ctx) -> AppResult<()> {
    let n = ctx.n_keys();
    let part = format!("cents{}", key_cols(n));
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE rc_new AS
         WITH l AS (SELECT *, row_number() OVER (PARTITION BY {part} ORDER BY dt NULLS LAST, rid) AS n FROM rc_free f WHERE tag = 0 AND {kl}),
              r AS (SELECT *, row_number() OVER (PARTITION BY {part} ORDER BY dt NULLS LAST, rid) AS n FROM rc_free f WHERE tag = 1 AND {kr}),
              p AS (SELECT row_number() OVER (ORDER BY l.dt NULLS LAST, l.rid) AS gk, l.rk AS lrk, r.rk AS rrk, l.cents AS lc, r.cents AS rc
                    FROM l JOIN r ON l.cents = r.cents AND l.n = r.n AND {eq} WHERE {win})
         SELECT gk, 0 AS tag, lrk AS rk, lc AS cents FROM p UNION ALL SELECT gk, 1 AS tag, rrk AS rk, rc AS cents FROM p",
        kl = keys_not_null(n, "f"),
        kr = keys_not_null(n, "f"),
        eq = keys_equal(n),
        win = date_window(ctx.def.days),
    ))?;
    Ok(())
}

/// 第二轮：容差 / 日期窗口内，差得最少的优先（先比金额差、再比日期差、再按行的先后），一对一
fn pass_tolerance(conn: &Connection, ctx: &Ctx, warnings: &mut Vec<String>) -> AppResult<()> {
    let n = ctx.n_keys();
    let tol = ctx.def.tol_cents();
    let amount = if tol == 0 { "r.cents = l.cents".to_string() } else { format!("r.cents BETWEEN l.cents - {tol} AND l.cents + {tol}") };
    let from = format!(
        "FROM (SELECT * FROM rc_free f WHERE tag = 0 AND {kl}) l JOIN (SELECT * FROM rc_free f WHERE tag = 1 AND {kr}) r ON {amount} AND {eq} AND {win}",
        kl = keys_not_null(n, "f"),
        kr = keys_not_null(n, "f"),
        eq = keys_equal(n),
        win = date_window(ctx.def.days),
    );
    let count: i64 = conn.query_row(&format!("SELECT count(*) {from}"), [], |r| r.get(0))?;
    if count == 0 {
        conn.execute_batch("CREATE OR REPLACE TEMP TABLE rc_new (gk BIGINT, tag INTEGER, rk VARCHAR, cents BIGINT)")?;
        return Ok(());
    }
    if count > MAX_CANDIDATES {
        warnings.push(format!("容差 / 日期窗口内的候选配对有 {count} 个，太多了，已跳过这一轮：请设置对账键，或缩小容差 / 日期窗口，剩下的手工勾对"));
        conn.execute_batch("CREATE OR REPLACE TEMP TABLE rc_new (gk BIGINT, tag INTEGER, rk VARCHAR, cents BIGINT)")?;
        return Ok(());
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT l.rk, r.rk, l.cents, r.cents {from}
         ORDER BY abs(l.cents - r.cents), COALESCE(abs(date_diff('day', l.dt, r.dt)), 99999), l.rid, r.rid"
    ))?;
    let cands = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let (mut used_l, mut used_r) = (HashSet::new(), HashSet::new());
    let mut picked = Vec::new();
    for (lk, rk, lc, rc) in cands {
        if used_l.contains(&lk) || used_r.contains(&rk) {
            continue;
        }
        used_l.insert(lk.clone());
        used_r.insert(rk.clone());
        picked.push((lk, rk, lc, rc));
    }
    conn.execute_batch("CREATE OR REPLACE TEMP TABLE rc_new (gk BIGINT, tag INTEGER, rk VARCHAR, cents BIGINT)")?;
    for (ci, chunk) in picked.chunks(INSERT_CHUNK / 2).enumerate() {
        let values: Vec<String> = chunk
            .iter()
            .enumerate()
            .flat_map(|(i, (lk, rk, lc, rc))| {
                let gk = ci * (INSERT_CHUNK / 2) + i + 1;
                [format!("({gk}, 0, {}, {lc})", literal(lk)), format!("({gk}, 1, {}, {rc})", literal(rk))]
            })
            .collect();
        conn.execute_batch(&format!("INSERT INTO rc_new VALUES {}", values.join(", ")))?;
    }
    Ok(())
}

/// 第三轮：同一个对账键下，两侧都有、合计相等（在容差内）的所有行汇成一组（多对多；不看日期窗口）
fn pass_sum(conn: &Connection, ctx: &Ctx) -> AppResult<()> {
    let n = ctx.n_keys();
    let ks = (0..n).map(|i| format!("k{i}")).collect::<Vec<_>>().join(", ");
    let tol = ctx.def.tol_cents();
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE rc_new AS
         WITH t AS (SELECT tag, rk, cents, {ks} FROM rc_free f WHERE {kn}),
              g AS (SELECT {ks}, row_number() OVER (ORDER BY {ks}) AS gk FROM t GROUP BY {ks}
                    HAVING count(*) FILTER (WHERE tag = 0) >= 1 AND count(*) FILTER (WHERE tag = 1) >= 1
                       AND abs(sum(CASE WHEN tag = 0 THEN cents ELSE 0 END) - sum(CASE WHEN tag = 1 THEN cents ELSE 0 END)) <= {tol})
         SELECT g.gk, t.tag, t.rk, t.cents FROM t JOIN g USING ({ks})",
        kn = keys_not_null(n, "f"),
    ))?;
    Ok(())
}

fn run_auto(conn: &Connection, ctx: &Ctx, operator: &str) -> AppResult<AutoOutcome> {
    let mut out = AutoOutcome { cleared_broken: cleanup_broken(conn, ctx)?, ..Default::default() };
    let n = ctx.n_keys();
    conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE rc_free AS SELECT tag, rid, rk, cents, dt{} FROM ({}) WHERE NOT ok", key_cols(n), ctx.unified()?))?;

    pass_exact(conn, ctx)?;
    let (g, r) = commit_new(conn, ctx, "exact", operator)?;
    out.exact = g;
    out.groups += g;
    out.matched_rows += r;

    if ctx.def.tolerance > 0.0 || ctx.def.days.is_some() {
        pass_tolerance(conn, ctx, &mut out.warnings)?;
        // 金额完全相同的（只是第一轮的顺序对应被日期窗口挡掉了）仍记为「自动」，有差额的才记为「容差」
        let zero = "gk IN (SELECT gk FROM rc_all GROUP BY gk HAVING sum(CASE tag WHEN 0 THEN cents ELSE -cents END) = 0)";
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE rc_all AS SELECT * FROM rc_new;
             CREATE OR REPLACE TEMP TABLE rc_new AS SELECT * FROM rc_all WHERE {zero};"
        ))?;
        let (g, r) = commit_new(conn, ctx, "exact", operator)?;
        out.exact += g;
        out.groups += g;
        out.matched_rows += r;
        conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE rc_new AS SELECT * FROM rc_all WHERE NOT ({zero});"))?;
        let (g, r) = commit_new(conn, ctx, "tolerance", operator)?;
        out.tolerance = g;
        out.groups += g;
        out.matched_rows += r;
    }

    if ctx.def.group_by_key && n > 0 {
        pass_sum(conn, ctx)?;
        let (g, r) = commit_new(conn, ctx, "sum", operator)?;
        out.sum = g;
        out.groups += g;
        out.matched_rows += r;
    }
    out.left_open = conn.query_row("SELECT count(*) FROM rc_free WHERE tag = 0", [], |r| r.get(0))?;
    out.right_open = conn.query_row("SELECT count(*) FROM rc_free WHERE tag = 1", [], |r| r.get(0))?;
    Ok(out)
}

/// 自动勾对。dry_run 为真时只算结果、不保存。会先清掉失效的勾对（行被改过或删了）
pub fn auto_match(conn: &Connection, ctx: &Ctx, operator: &str, dry_run: bool) -> AppResult<AutoOutcome> {
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = run_auto(conn, ctx, operator);
    let done = match (&r, dry_run) {
        (Ok(_), false) => conn.execute_batch("COMMIT").map_err(AppError::from),
        _ => conn.execute_batch("ROLLBACK").map_err(AppError::from),
    };
    drop_temp(conn);
    done?;
    r.map(|mut o| {
        o.dry_run = dry_run;
        o
    })
}

// ── 手工勾对 ──────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualRequest {
    /// A 方要勾对的行（行标识）
    pub left: Vec<String>,
    pub right: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// 合计差额超过容差时，确认仍然勾对（必须写明原因）
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManualOutcome {
    pub gno: i64,
    pub left_total: f64,
    pub right_total: f64,
    /// A 方合计 - B 方合计
    pub diff: f64,
}

fn rk_list(v: &[String]) -> String {
    v.iter().map(|k| literal(k)).collect::<Vec<_>>().join(", ")
}

pub fn manual_match(conn: &Connection, ctx: &Ctx, req: &ManualRequest, operator: &str) -> AppResult<ManualOutcome> {
    let dedup = |v: &[String]| -> Vec<String> {
        let mut seen = HashSet::new();
        v.iter().filter(|k| seen.insert((*k).clone())).cloned().collect()
    };
    let (left, right) = (dedup(&req.left), dedup(&req.right));
    if left.is_empty() || right.is_empty() {
        return Err(AppError::bad(format!("两边都要选至少一行（{} 和 {}）；只有一边的是未达账项，不需要勾对", ctx.def.left.label, ctx.def.right.label)));
    }
    if left.len() + right.len() > MAX_MANUAL_ROWS {
        return Err(AppError::bad(format!("一次最多勾对 {MAX_MANUAL_ROWS} 行")));
    }
    let note = req.note.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    if note.as_ref().is_some_and(|n| n.chars().count() > 200) {
        return Err(AppError::bad("备注不超过 200 个字"));
    }
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<ManualOutcome> {
        cleanup_broken(conn, ctx)?;
        let sql = format!(
            "SELECT tag, rk, cents FROM ({}) WHERE NOT ok AND ((tag = 0 AND rk IN ({})) OR (tag = 1 AND rk IN ({})))",
            ctx.unified()?,
            rk_list(&left),
            rk_list(&right)
        );
        let mut stmt = conn.prepare(&sql)?;
        let found = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        if found.len() != left.len() + right.len() {
            return Err(AppError::bad(format!(
                "选中的 {} 行里有 {} 行已经被勾对或数据已变动，请刷新后重新选择",
                left.len() + right.len(),
                left.len() + right.len() - found.len()
            )));
        }
        let total = |tag: i64| found.iter().filter(|f| f.0 == tag).map(|f| f.2).sum::<i64>();
        let (lt, rt) = (total(0), total(1));
        let diff = lt - rt;
        if diff.abs() > ctx.def.tol_cents() {
            if !req.force {
                return Err(AppError::bad(format!(
                    "{} 合计 {:.2}，{} 合计 {:.2}，相差 {:.2}，超过了容差 {:.2}；确认要勾对请写明差额原因",
                    ctx.def.left.label,
                    lt as f64 / 100.0,
                    ctx.def.right.label,
                    rt as f64 / 100.0,
                    diff as f64 / 100.0,
                    ctx.def.tolerance
                )));
            }
            if note.is_none() {
                return Err(AppError::bad("合计有差额，必须写明差额的原因（如银行手续费、汇率差）"));
            }
        }
        let gno: i64 = conn.query_row("SELECT COALESCE(max(gno), 0) + 1 FROM ledger_recon_group WHERE recon_id = ?", [&ctx.id], |r| r.get(0))?;
        conn.execute(
            "INSERT INTO ledger_recon_group (recon_id, gno, method, note, diff_cents, item_count, matched_by, matched_at) VALUES (?, ?, 'manual', ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
            params![ctx.id, gno, note, diff, found.len() as i64, operator, crate::db::now_local()],
        )?;
        for (tag, rk, cents) in &found {
            conn.execute("INSERT INTO ledger_recon_item (recon_id, gno, side, row_key, cents) VALUES (?, ?, ?, ?, ?)", params![ctx.id, gno, tag, rk, cents])?;
        }
        Ok(ManualOutcome { gno, left_total: lt as f64 / 100.0, right_total: rt as f64 / 100.0, diff: diff as f64 / 100.0 })
    })();
    match &r {
        Ok(_) => conn.execute_batch("COMMIT")?,
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK");
        }
    }
    drop_temp(conn);
    r
}

// ── 取消勾对 ──────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UnmatchRequest {
    /// 指定的勾对组
    #[serde(default)]
    pub groups: Vec<i64>,
    /// 或者：auto = 全部自动勾对的；all = 全部
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UnmatchOutcome {
    pub groups: i64,
    pub rows: i64,
}

pub fn unmatch(conn: &Connection, ctx: &Ctx, req: &UnmatchRequest) -> AppResult<UnmatchOutcome> {
    let cond = match req.mode.as_deref() {
        Some("all") => "TRUE".to_string(),
        Some("auto") => "method <> 'manual'".to_string(),
        Some(other) => return Err(AppError::bad(format!("不认识的方式：{other}"))),
        None if req.groups.is_empty() => return Err(AppError::bad("请指定要取消的勾对组")),
        None if req.groups.len() > 100_000 => return Err(AppError::bad("一次最多取消 100000 个勾对组")),
        None => format!("gno IN ({})", req.groups.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")),
    };
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<UnmatchOutcome> {
        let groups: i64 = conn.query_row(&format!("SELECT count(*) FROM ledger_recon_group WHERE recon_id = ? AND {cond}"), [&ctx.id], |r| r.get(0))?;
        let rows = conn.execute(
            &format!("DELETE FROM ledger_recon_item WHERE recon_id = ? AND gno IN (SELECT gno FROM ledger_recon_group WHERE recon_id = ? AND {cond})"),
            params![ctx.id, ctx.id],
        )? as i64;
        conn.execute(&format!("DELETE FROM ledger_recon_group WHERE recon_id = ? AND {cond}"), [&ctx.id])?;
        Ok(UnmatchOutcome { groups, rows })
    })();
    match &r {
        Ok(_) => conn.execute_batch("COMMIT")?,
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK");
        }
    }
    r
}
