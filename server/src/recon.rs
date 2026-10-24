//! 逐笔对账：两个数据集（如银行流水与企业账）的逐行勾对，勾对结果作为一个可以直接用于公式 / 透视 / 图表的数据集
//!
//! - **对账方案就是一种数据集**（kind = recon）：定义 = 两侧数据集与列对应（金额必填，日期 / 对账键 / 摘要可选）、金额容差、日期窗口。
//!   它在数据库里是一个**实时视图**：两侧所有行合在一起，带「状态（已勾对 / 未勾对）、勾对组、方式」等列，
//!   所以 DSUMIFS、透视表、图表、SQL 都直接可用，数据导入后不需要刷新（银行余额调节表模板就是对它写公式）
//! - **行标识**：数据集重新导入后 rowid 会变，所以用「整行内容的指纹 + 同内容的序号」；行内容被改过，原来的勾对自动失效
//!   （行回到未勾对，勾对组里少了一行就整组算失效，并提示），不会悄悄算错
//! - **自动勾对**（engine.rs）：① 对账键 + 金额完全相同，按日期顺序一一对应；② 金额容差 / 日期窗口内按「差得最少的优先」配对；
//!   ③ 同一个对账键下两侧合计相等的多笔（多对多）汇总勾对。**手工勾对**任意多对多，合计差额超过容差须说明原因
//! - **未达账项**：截至某日，「未勾对的行」加上「已勾对但对方日期晚于该日的行」（见视图的「勾对完成日」列）
//! - 行级数据范围受限的用户不能做对账（两侧数据集只看得到一部分，勾对会和别人的结果互相踩）；对账结果数据集默认对他们不可见

mod engine;
mod report;

pub use engine::{AutoOutcome, ManualOutcome, ManualRequest, UnmatchOutcome, UnmatchRequest, auto_match, manual_match, unmatch};
pub use report::{GroupPage, GroupQuery, ItemPage, ItemQuery, Summary, groups, items, summary};

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::dataset::{self, ColumnDef, DatasetDef, column_sql};
use crate::error::{AppError, AppResult};
use crate::schema::{ident, literal};

/// 一侧最多几个对账键
pub const MAX_KEYS: usize = 3;
const LABEL_MAX: usize = 12;

fn one() -> i64 {
    1
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    /// 数据集（id 或名称，保存时规范为 id）
    pub dataset: String,
    /// 在结果里显示的「来源」，如「银行」「账面」
    pub label: String,
    /// 金额列（数值列或计算列）
    pub amount: String,
    /// 1 = 原样；-1 = 取反（让两侧同一笔业务的金额符号一致：银行收入为正，企业账的收款也要是正）
    #[serde(default = "one")]
    pub sign: i64,
    #[serde(default)]
    pub date: Option<String>,
    /// 对账键（两侧按顺序一一对应，文本相等才算同一个键；为空的键不参与自动勾对）
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub memo: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReconDef {
    pub left: Side,
    pub right: Side,
    /// 金额容差（元）：自动勾对时两侧金额相差不超过它
    #[serde(default)]
    pub tolerance: f64,
    /// 日期窗口（天）：自动勾对时两侧日期相差不超过它；不设则不看日期
    #[serde(default)]
    pub days: Option<u32>,
    /// 第三轮：同一个对账键下两侧合计相等的多笔也自动勾对（需要设置对账键）
    #[serde(default)]
    pub group_by_key: bool,
}

impl ReconDef {
    pub fn tol_cents(&self) -> i64 {
        (self.tolerance * 100.0).round() as i64
    }
}

// ── 结果数据集的列 ────────────────────────────────────

/// (列名, 类型, 宽度)
pub const COLUMNS: &[(&str, &str, u32)] = &[
    ("来源", "text", 70),
    ("日期", "datetime", 100),
    ("金额", "money", 110),
    ("方向", "text", 60),
    ("状态", "text", 70),
    ("勾对组", "number", 70),
    ("勾对方式", "text", 90),
    ("勾对完成日", "datetime", 100),
    ("勾对差额", "money", 90),
    ("勾对备注", "text", 160),
    ("提示", "text", 180),
    ("对账键", "text", 160),
    ("摘要", "text", 200),
    ("行标识", "id", 120),
];

/// 数据集清单里的定义（`dataset::load` 调用）
pub fn dataset_def(id: String, name: String) -> DatasetDef {
    DatasetDef {
        table: id.clone(),
        id,
        name,
        columns: COLUMNS
            .iter()
            .map(|(k, kind, w)| ColumnDef { key: (*k).into(), title: (*k).into(), kind, width: *w, expr: None, computed: false, formula: None, origin: None })
            .collect(),
        searchable: false,
        supports_rule: false,
        builtin: false,
        primary_key: vec![],
        rid: "__rid".into(),
        kind: "recon".into(),
        scope_sig: String::new(),
        view: None,
        summary: None,
    }
}

// ── 校验 ──────────────────────────────────────────────

fn find_col<'a>(def: &'a DatasetDef, name: &str) -> AppResult<&'a ColumnDef> {
    def.columns.iter().find(|c| c.key == name || c.title == name).ok_or_else(|| AppError::bad(format!("数据集「{}」没有列「{name}」", def.name)))
}

fn check_side(defs: &[DatasetDef], s: &Side, which: &str) -> AppResult<(Side, DatasetDef)> {
    let ds = defs
        .iter()
        .find(|d| (d.id == s.dataset || d.name == s.dataset) && d.kind != "recon")
        .ok_or_else(|| AppError::bad(format!("{which}的数据集「{}」不存在", s.dataset)))?
        .clone();
    let label = s.label.trim().to_string();
    if label.is_empty() || label.chars().count() > LABEL_MAX {
        return Err(AppError::bad(format!("{which}的名称不能为空，且不超过 {LABEL_MAX} 个字")));
    }
    if s.sign != 1 && s.sign != -1 {
        return Err(AppError::bad("金额方向只能是 1（原样）或 -1（取反）"));
    }
    let amount = find_col(&ds, &s.amount)?;
    if !crate::kinds::is_numeric(amount.kind) {
        return Err(AppError::bad(format!("{which}的金额列「{}」不是数值列", amount.title)));
    }
    let date = match s.date.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
        Some(d) => {
            let c = find_col(&ds, d)?;
            if c.kind != "datetime" {
                return Err(AppError::bad(format!("{which}的日期列「{}」不是日期列", c.title)));
            }
            Some(c.key.clone())
        }
        None => None,
    };
    let keys = s.keys.iter().map(|k| find_col(&ds, k).map(|c| c.key.clone())).collect::<AppResult<Vec<_>>>()?;
    let memo = match s.memo.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
        Some(m) => Some(find_col(&ds, m)?.key.clone()),
        None => None,
    };
    Ok((Side { dataset: ds.id.clone(), label, amount: amount.key.clone(), sign: s.sign, date, keys, memo }, ds))
}

/// 校验并规范化（数据集与列都换成 id / key）。`defs` 为调用者能看到的数据集
pub fn validate(defs: &[DatasetDef], d: &ReconDef) -> AppResult<(ReconDef, DatasetDef, DatasetDef)> {
    let (left, ld) = check_side(defs, &d.left, "A 方")?;
    let (right, rd) = check_side(defs, &d.right, "B 方")?;
    if ld.id == rd.id {
        return Err(AppError::bad("两侧不能是同一个数据集"));
    }
    if left.label == right.label {
        return Err(AppError::bad("两侧的名称不能相同（结果里靠它区分来源）"));
    }
    if left.keys.len() != right.keys.len() {
        return Err(AppError::bad("两侧的对账键个数要一样（按顺序一一对应）"));
    }
    if left.keys.len() > MAX_KEYS {
        return Err(AppError::bad(format!("对账键最多 {MAX_KEYS} 个")));
    }
    if !(0.0..=1_000_000.0).contains(&d.tolerance) || !d.tolerance.is_finite() {
        return Err(AppError::bad("金额容差应在 0 ~ 1000000 元之间"));
    }
    if d.days.is_some_and(|x| x > 366) {
        return Err(AppError::bad("日期窗口最多 366 天"));
    }
    if d.days.is_some() && (left.date.is_none() || right.date.is_none()) {
        return Err(AppError::bad("设置了日期窗口，两侧都要选日期列"));
    }
    if d.group_by_key && left.keys.is_empty() {
        return Err(AppError::bad("按对账键汇总勾对需要先设置对账键"));
    }
    Ok((ReconDef { left, right, tolerance: (d.tolerance * 100.0).round() / 100.0, days: d.days, group_by_key: d.group_by_key }, ld, rd))
}

// ── SQL ───────────────────────────────────────────────

/// 一侧的行：rid（全局唯一：原 rid × 2 + 0/1）、fp（整行内容指纹）、cents（金额，已统一符号，分）、dt、ktext、memo、k0..
fn side_sql(def: &DatasetDef, s: &Side, tag: i64) -> AppResult<String> {
    let expr = |name: &str| find_col(def, name).map(|c| format!("({})", column_sql(c)));
    let fp: Vec<String> = def.columns.iter().filter(|c| c.expr.is_none()).map(|c| format!("COALESCE(CAST({} AS VARCHAR), chr(30))", ident(&c.key))).collect();
    if fp.is_empty() {
        return Err(AppError::bad(format!("数据集「{}」没有可用的列", def.name)));
    }
    let mut sel = vec![
        format!("{} * 2 + {tag} AS rid", def.rid),
        format!("md5(concat_ws(chr(31), {})) AS fp", fp.join(", ")),
        format!("CAST(round(COALESCE(CAST({} AS DOUBLE), 0) * {} * 100) AS BIGINT) AS cents", expr(&s.amount)?, s.sign),
    ];
    sel.push(match &s.date {
        Some(d) => format!("TRY_CAST(substr(CAST({} AS VARCHAR), 1, 10) AS DATE) AS dt", expr(d)?),
        None => "CAST(NULL AS DATE) AS dt".into(),
    });
    let mut parts = Vec::new();
    for (i, k) in s.keys.iter().enumerate() {
        let e = format!("NULLIF(trim(CAST({} AS VARCHAR)), '')", expr(k)?);
        sel.push(format!("{e} AS k{i}"));
        parts.push(e);
    }
    sel.push(if parts.is_empty() { "CAST(NULL AS VARCHAR) AS ktext".into() } else { format!("NULLIF(concat_ws(' / ', {}), '') AS ktext", parts.join(", ")) });
    sel.push(match &s.memo {
        Some(m) => format!("CAST({} AS VARCHAR) AS memo", expr(m)?),
        None => "CAST(NULL AS VARCHAR) AS memo".into(),
    });
    Ok(format!("SELECT {} FROM {}", sel.join(", "), def.table))
}

/// 两侧合并后的行，带行标识 rk 与勾对情况：tag（0 = A 方 / 1 = B 方）、…、rk、gno、method、note、diff_cents、item_count、ok
///
/// ok = 在某个勾对组里，且组里记录的行现在都还在（任何一行被改过或删了，整组失效）
pub fn unified_sql(id: &str, l: &DatasetDef, r: &DatasetDef, d: &ReconDef) -> AppResult<String> {
    let a = side_sql(l, &d.left, 0)?;
    let b = side_sql(r, &d.right, 1)?;
    let q = literal(id);
    Ok(format!(
        "SELECT j.*, (j.gno IS NOT NULL AND count(j.gno) OVER (PARTITION BY j.gno) = j.item_count) AS ok
         FROM (
           SELECT x.*, m.gno, g.method, g.note, g.diff_cents, g.item_count
           FROM (
             SELECT 0 AS tag, s.*, s.fp || '#' || row_number() OVER (PARTITION BY s.fp ORDER BY s.rid) AS rk FROM ({a}) s
             UNION ALL
             SELECT 1 AS tag, s.*, s.fp || '#' || row_number() OVER (PARTITION BY s.fp ORDER BY s.rid) AS rk FROM ({b}) s
           ) x
           LEFT JOIN ledger_recon_item m ON m.recon_id = {q} AND m.side = x.tag AND m.row_key = x.rk
           LEFT JOIN ledger_recon_group g ON g.recon_id = {q} AND g.gno = m.gno
         ) j"
    ))
}

/// 结果数据集的视图 SQL
pub fn view_sql(id: &str, l: &DatasetDef, r: &DatasetDef, d: &ReconDef) -> AppResult<String> {
    let u = unified_sql(id, l, r, d)?;
    Ok(format!(
        "SELECT u.rid AS __rid,
           CASE u.tag WHEN 0 THEN {ll} ELSE {rl} END AS \"来源\",
           strftime(u.dt, '%Y-%m-%d') AS \"日期\",
           CAST(u.cents AS DOUBLE) / 100.0 AS \"金额\",
           CASE WHEN u.cents > 0 THEN '收入' WHEN u.cents < 0 THEN '支出' ELSE '零' END AS \"方向\",
           CASE WHEN u.ok THEN '已勾对' ELSE '未勾对' END AS \"状态\",
           CASE WHEN u.ok THEN u.gno END AS \"勾对组\",
           CASE WHEN u.ok THEN CASE u.method WHEN 'exact' THEN '自动' WHEN 'tolerance' THEN '自动（容差）' WHEN 'sum' THEN '自动（汇总）' ELSE '手工' END END AS \"勾对方式\",
           CASE WHEN u.ok THEN strftime(max(u.dt) OVER (PARTITION BY u.gno), '%Y-%m-%d') END AS \"勾对完成日\",
           CASE WHEN u.ok AND row_number() OVER (PARTITION BY u.gno ORDER BY u.tag, u.rid) = 1 THEN CAST(u.diff_cents AS DOUBLE) / 100.0 END AS \"勾对差额\",
           CASE WHEN u.ok THEN u.note END AS \"勾对备注\",
           CASE WHEN u.gno IS NOT NULL AND NOT u.ok THEN '原勾对的行已变动，需要重新勾对' END AS \"提示\",
           u.ktext AS \"对账键\",
           u.memo AS \"摘要\",
           u.rk AS \"行标识\"
         FROM ({u}) u",
        ll = literal(&d.left.label),
        rl = literal(&d.right.label),
    ))
}

// ── 方案的保存 ────────────────────────────────────────

fn stored(conn: &Connection) -> AppResult<Vec<(String, String, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT id, name, definition FROM ledger_dataset WHERE kind = 'recon' AND deleted_at IS NULL ORDER BY created_at")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 对账方案引用的两个数据集 id（回收站 / 删除检查用）
pub fn sources(definition: &str) -> Option<[String; 2]> {
    let d: ReconDef = serde_json::from_str(definition).ok()?;
    Some([d.left.dataset, d.right.dataset])
}

/// 某数据集被哪些对账方案引用（删除前检查）
pub fn dependents(conn: &Connection, dataset_id: &str) -> AppResult<Vec<String>> {
    Ok(stored(conn)?
        .into_iter()
        .filter(|(_, _, d)| d.as_deref().and_then(sources).is_some_and(|s| s.iter().any(|x| x == dataset_id)))
        .map(|(_, n, _)| n)
        .collect())
}

/// 用不受数据范围限制的数据集定义建视图（视图是共享的，不能带着创建者的范围）
fn full_defs(conn: &Connection, d: &ReconDef) -> AppResult<(ReconDef, DatasetDef, DatasetDef)> {
    validate(&dataset::load(conn)?.0, d)
}

/// 按定义重建所有对账视图（计算列、列结构变化后调用；会让对账失效的修改整体取消）
pub fn rebuild_all(conn: &Connection) -> AppResult<()> {
    for (id, name, def) in stored(conn)? {
        let r = (|| -> AppResult<()> {
            let d: ReconDef = serde_json::from_str(def.as_deref().unwrap_or("")).map_err(|e| AppError::bad(format!("定义损坏：{e}")))?;
            let (d, l, r) = full_defs(conn, &d)?;
            conn.execute_batch(&format!("CREATE OR REPLACE VIEW {id} AS {}", view_sql(&id, &l, &r, &d)?))?;
            Ok(())
        })();
        r.map_err(|e| AppError::bad(format!("对账「{name}」会失效（{}），已取消", e.message())))?;
    }
    Ok(())
}

fn check_name(conn: &Connection, name: &str, except: Option<&str>) -> AppResult<String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::bad("名称不能为空，且不超过 40 个字符"));
    }
    if dataset::load(conn)?.0.iter().any(|d| d.name == name && Some(d.id.as_str()) != except) {
        return Err(AppError::bad(format!("已存在名为「{name}」的数据集")));
    }
    Ok(name)
}

pub fn create(conn: &Connection, name: &str, def: &ReconDef) -> AppResult<String> {
    let name = check_name(conn, name, None)?;
    // 调用者要能看到两侧数据集
    validate(&dataset::datasets(conn)?, def)?;
    let (d, l, r) = full_defs(conn, def)?;
    let id = format!("rc_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    let sql = view_sql(&id, &l, &r, &d)?;
    let now = crate::db::now_local();
    conn.execute_batch("BEGIN TRANSACTION")?;
    let res = (|| -> AppResult<()> {
        conn.execute_batch(&format!("CREATE VIEW {id} AS {sql}"))?;
        // 试跑一次，列名 / 表达式有问题当场报错
        conn.query_row(&format!("SELECT count(*) FROM {id}"), [], |r| r.get::<_, i64>(0))?;
        conn.execute(
            "INSERT INTO ledger_dataset (id, name, columns, primary_key, kind, definition, created_at, updated_at)
             VALUES (?, ?, '[]', '[]', 'recon', ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
            params![id, name, serde_json::to_string(&d)?, now, now],
        )?;
        Ok(())
    })();
    match res {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(id)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

fn matches_count(conn: &Connection, id: &str) -> AppResult<i64> {
    Ok(conn.query_row("SELECT count(*) FROM ledger_recon_group WHERE recon_id = ?", [id], |r| r.get(0))?)
}

/// 方案的定义与名称
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub id: String,
    pub name: String,
    pub def: ReconDef,
    /// 已有的勾对组数
    pub groups: i64,
}

pub fn info(conn: &Connection, id: &str) -> AppResult<Info> {
    if !dataset::datasets(conn)?.iter().any(|d| d.id == id && d.kind == "recon") {
        return Err(AppError::bad("对账方案不存在"));
    }
    let (name, def): (String, Option<String>) = conn.query_row("SELECT name, definition FROM ledger_dataset WHERE id = ? AND kind = 'recon' AND deleted_at IS NULL", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let def: ReconDef = serde_json::from_str(def.as_deref().unwrap_or("")).map_err(|e| AppError::bad(format!("对账方案的定义损坏：{e}")))?;
    Ok(Info { id: id.into(), name, def, groups: matches_count(conn, id)? })
}

/// 修改方案。换了数据集、金额列或金额方向时，已有的勾对不再可信：必须确认清空（reset）
pub fn update(conn: &Connection, id: &str, name: &str, def: &ReconDef, reset: bool) -> AppResult<()> {
    let old = info(conn, id)?;
    let name = check_name(conn, name, Some(id))?;
    let (new, _, _) = validate(&dataset::datasets(conn)?, def)?;
    let changed = |a: &Side, b: &Side| a.dataset != b.dataset || a.amount != b.amount || a.sign != b.sign;
    let needs_reset = old.groups > 0 && (changed(&old.def.left, &new.left) || changed(&old.def.right, &new.right));
    if needs_reset && !reset {
        return Err(AppError::bad(format!("换了数据集、金额列或金额方向，已有的 {} 个勾对组不再可信；确认后会清空勾对重新开始", old.groups)));
    }
    let json = serde_json::to_string(&new)?;
    dataset::guarded_change(conn, |c| {
        if needs_reset {
            c.execute("DELETE FROM ledger_recon_item WHERE recon_id = ?", [id])?;
            c.execute("DELETE FROM ledger_recon_group WHERE recon_id = ?", [id])?;
        }
        c.execute("UPDATE ledger_dataset SET name = ?, definition = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![name, json, crate::db::now_local(), id])?;
        Ok(())
    })
}

/// 彻底删除时清掉勾对记录
pub fn purge(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_recon_item WHERE recon_id = ?", [id])?;
    conn.execute("DELETE FROM ledger_recon_group WHERE recon_id = ?", [id])?;
    Ok(())
}

// ── 运行时上下文 ──────────────────────────────────────

/// 一次对账操作用到的方案与两侧数据集（按调用者的权限加载）
pub struct Ctx {
    pub id: String,
    pub def: ReconDef,
    pub left: DatasetDef,
    pub right: DatasetDef,
}

impl Ctx {
    pub fn unified(&self) -> AppResult<String> {
        unified_sql(&self.id, &self.left, &self.right, &self.def)
    }

    pub fn n_keys(&self) -> usize {
        self.def.left.keys.len()
    }
}

pub fn context(conn: &Connection, id: &str) -> AppResult<Ctx> {
    let i = info(conn, id)?;
    let defs = dataset::datasets(conn)?;
    let (def, left, right) = validate(&defs, &i.def)?;
    for d in [&left, &right] {
        if !d.scope_sig.is_empty() {
            return Err(AppError::Forbidden(format!("数据集「{}」设置了行级数据范围，你只能看到其中一部分，不能做对账；请联系管理员处理", d.name)));
        }
    }
    Ok(Ctx { id: id.into(), def, left, right })
}

// ── 新建前的预览 ──────────────────────────────────────

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SideStats {
    pub label: String,
    pub dataset: String,
    pub rows: i64,
    /// 金额合计（已按方向取反）
    pub total: f64,
    pub undated: i64,
    pub min_date: Option<String>,
    pub max_date: Option<String>,
    /// 对账键为空的行数（不参与自动勾对）
    pub blank_keys: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub def: ReconDef,
    pub left: SideStats,
    pub right: SideStats,
    pub warnings: Vec<String>,
}

fn side_stats(conn: &Connection, ds: &DatasetDef, s: &Side, tag: i64) -> AppResult<SideStats> {
    let blank = if s.keys.is_empty() { "false".to_string() } else { (0..s.keys.len()).map(|i| format!("k{i} IS NULL")).collect::<Vec<_>>().join(" OR ") };
    let sql = format!(
        "SELECT count(*), COALESCE(sum(cents), 0), count(*) FILTER (WHERE dt IS NULL), strftime(min(dt), '%Y-%m-%d'), strftime(max(dt), '%Y-%m-%d'), count(*) FILTER (WHERE {blank}) FROM ({})",
        side_sql(ds, s, tag)?
    );
    conn.query_row(&sql, [], |r| {
        Ok(SideStats {
            label: s.label.clone(),
            dataset: ds.name.clone(),
            rows: r.get(0)?,
            total: r.get::<_, i64>(1)? as f64 / 100.0,
            undated: r.get(2)?,
            min_date: r.get(3)?,
            max_date: r.get(4)?,
            blank_keys: r.get(5)?,
        })
    })
    .map_err(Into::into)
}

/// 新建 / 修改前的预览：两侧行数、日期范围、对账键为空的行，以及容易填错的地方
pub fn preview(conn: &Connection, def: &ReconDef) -> AppResult<Preview> {
    let (d, l, r) = validate(&dataset::datasets(conn)?, def)?;
    for x in [&l, &r] {
        if !x.scope_sig.is_empty() {
            return Err(AppError::Forbidden(format!("数据集「{}」设置了行级数据范围，你只能看到其中一部分，不能做对账", x.name)));
        }
    }
    let left = side_stats(conn, &l, &d.left, 0)?;
    let right = side_stats(conn, &r, &d.right, 1)?;
    let mut warnings = Vec::new();
    if left.rows == 0 || right.rows == 0 {
        warnings.push("有一侧没有数据".to_string());
    }
    if left.total != 0.0 && right.total != 0.0 && (left.total > 0.0) != (right.total > 0.0) {
        warnings.push(format!("两侧金额合计的符号相反（{} {}，{} {}）：同一笔业务在两边的正负是否一致？如果相反，把其中一侧的「金额方向」设为取反", left.label, left.total, right.label, right.total));
    }
    if let (Some(a), Some(b), Some(c), Some(e)) = (&left.min_date, &left.max_date, &right.min_date, &right.max_date)
        && (b < c || e < a)
    {
        warnings.push("两侧的日期范围完全不重叠".to_string());
    }
    for s in [&left, &right] {
        if s.rows > 0 && s.blank_keys * 2 > s.rows {
            warnings.push(format!("{}有 {} / {} 行的对账键是空的，这些行不会参与自动勾对", s.label, s.blank_keys, s.rows));
        }
        if s.undated > 0 && d.days.is_some() {
            warnings.push(format!("{}有 {} 行没有日期（或日期格式不对），设置了日期窗口时它们不能自动勾对", s.label, s.undated));
        }
    }
    Ok(Preview { def: d, left, right, warnings })
}

#[cfg(test)]
mod tests;
