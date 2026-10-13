//! 多人填报：收集任务
//!
//! 一个收集任务绑定一个填报数据集，数据集里有两列把「谁填的、哪一期」分开：**填报单位**列（如店铺、部门）和**期次**列（如 2026-10）。
//! 每个单位有一个负责人。所有人往同一个数据集里填，各填各的行——汇总不用另做，透视表、仪表盘直接引用这个数据集。
//!
//! - **范围**：任务里的单位和负责人会同步成数据权限：数据集绑定到「填报单位」列，负责人在这个维度上只能看到、只能填自己负责的单位
//!   （见 `scope.rs`，写入时服务端按范围列校验）。同一个维度下的所有任务合并计算，所以一个人可以在几个任务里各负责几个单位
//! - **期次**：管理员手动开启一期（如 2026-10）。提交时填报区必须带上单位列和期次列，期次必须是已开启的，单位必须在任务里
//! - **状态**：某一期某个单位提交成功就记一条（`ledger_collect_submit`），没有记录就是还没填；看板按单位 × 期次展示
//! - **和核心的接口只有两个**：`before_submit`（写入前校验）、`after_submit`（写入后记录状态），由 `routes/writeback.rs` 调用；
//!   数据集不属于任何任务时它们什么都不做
//!
//! 第 1 阶段只有「谁填了、谁没填」；退回重填、确认后锁定、截止时间与催办在后面的阶段。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use duckdb::{Connection, OptionalExt, params};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::auth::CurrentUser;
use crate::dataset::{self, ColumnDef, DatasetDef, column_sql};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::scope::{self, BindingInput, RuleInput};
use crate::writeback::{NewColumn, NewInputDataset, WritebackOutcome, WritebackRequest};

const DEFAULT_DIMENSION: &str = "填报单位";
const DEFAULT_UNIT_COLUMN: &str = "填报单位";
const DEFAULT_PERIOD_COLUMN: &str = "期次";
const MAX_UNITS: usize = 2000;
const MAX_NAME_CHARS: usize = 60;
const MAX_UNIT_CHARS: usize = 60;
const MAX_PERIOD_CHARS: usize = 20;
/// 「我的填报」里每个任务最多显示最近几期
const MINE_PERIODS: usize = 6;
/// 提示里最多列出几个有问题的值
const MAX_LISTED: usize = 5;

// ── 输入 ──────────────────────────────────────────────

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UnitInput {
    pub unit: String,
    /// 负责人（登录过系统的用户 id）；不指派就只能由管理员代填
    #[serde(default)]
    pub user_id: Option<String>,
}

/// 新建一张填报表：业务列由用户定义，单位列和期次列自动加在最前面，主键 = 单位 + 期次 + 业务主键
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTable {
    pub name: String,
    pub columns: Vec<NewColumn>,
    /// 业务主键列（同一单位同一期里区分不同行的列）；不写表示每个单位每期只填一行
    #[serde(default)]
    pub key_columns: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectInput {
    pub name: String,
    /// 用已有的数据集（主键必须包含单位列和期次列）…
    #[serde(default)]
    pub dataset_id: Option<String>,
    /// …或者新建一张
    #[serde(default)]
    pub new_table: Option<NewTable>,
    #[serde(default)]
    pub unit_column: Option<String>,
    #[serde(default)]
    pub period_column: Option<String>,
    /// 数据权限的维度名，默认「填报单位」；同一个维度下的任务共用，不要在数据权限里手动改这个维度
    #[serde(default)]
    pub dimension: Option<String>,
    pub units: Vec<UnitInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInput {
    pub name: String,
    pub units: Vec<UnitInput>,
}

// ── 存取 ──────────────────────────────────────────────

struct StoredCollect {
    id: String,
    name: String,
    dataset_id: String,
    unit_column: String,
    period_column: String,
    dimension: String,
    created_by_name: String,
    created_at: String,
    updated_at: String,
}

const COLLECT_COLS: &str = "id, name, dataset_id, unit_column, period_column, dimension, created_by_name, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(updated_at, '%Y-%m-%d %H:%M:%S')";

fn collect_from_row(r: &duckdb::Row<'_>) -> duckdb::Result<StoredCollect> {
    Ok(StoredCollect {
        id: r.get(0)?,
        name: r.get(1)?,
        dataset_id: r.get(2)?,
        unit_column: r.get(3)?,
        period_column: r.get(4)?,
        dimension: r.get(5)?,
        created_by_name: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
        created_at: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
        updated_at: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
    })
}

fn load_collects(conn: &Connection) -> AppResult<Vec<StoredCollect>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLLECT_COLS} FROM ledger_collect ORDER BY created_at, id"))?;
    Ok(stmt.query_map([], collect_from_row)?.collect::<Result<Vec<_>, _>>()?)
}

fn load_collect(conn: &Connection, id: &str) -> AppResult<StoredCollect> {
    conn.query_row(&format!("SELECT {COLLECT_COLS} FROM ledger_collect WHERE id = ?"), params![id], collect_from_row).optional()?.ok_or_else(|| AppError::bad("收集任务不存在"))
}

fn collect_of_dataset(conn: &Connection, dataset_id: &str) -> AppResult<Option<StoredCollect>> {
    Ok(conn.query_row(&format!("SELECT {COLLECT_COLS} FROM ledger_collect WHERE dataset_id = ?"), params![dataset_id], collect_from_row).optional()?)
}

struct UnitRow {
    unit: String,
    user_id: Option<String>,
    user_name: Option<String>,
}

fn load_units(conn: &Connection, id: &str) -> AppResult<Vec<UnitRow>> {
    let mut stmt = conn.prepare("SELECT unit, user_id, user_name FROM ledger_collect_unit WHERE collect_id = ? ORDER BY ord")?;
    Ok(stmt
        .query_map(params![id], |r| Ok(UnitRow { unit: r.get(0)?, user_id: r.get(1)?, user_name: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// 已开启的期次：(期次, 开启时间)，新的在前
fn load_periods(conn: &Connection, id: &str) -> AppResult<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT period, strftime(opened_at, '%Y-%m-%d %H:%M:%S') FROM ledger_collect_period WHERE collect_id = ? ORDER BY opened_at DESC, period DESC")?;
    Ok(stmt.query_map(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?.collect::<Result<Vec<_>, _>>()?)
}

fn dataset_names(conn: &Connection) -> HashMap<String, String> {
    let Ok(mut stmt) = conn.prepare("SELECT id, name FROM ledger_dataset WHERE deleted_at IS NULL") else { return HashMap::new() };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    rows.map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

fn in_tx<T>(conn: &Connection, f: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
    conn.execute_batch("BEGIN TRANSACTION")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

fn new_id() -> String {
    format!("co_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"))
}

fn find_col<'a>(def: &'a DatasetDef, name: &str) -> Option<&'a ColumnDef> {
    def.columns.iter().find(|c| c.key == name || c.title == name)
}

// ── 校验 ──────────────────────────────────────────────

fn check_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::bad("请填写任务名称"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::bad(format!("任务名称最多 {MAX_NAME_CHARS} 个字")));
    }
    Ok(name.to_string())
}

fn clean_units(conn: &Connection, input: &[UnitInput]) -> AppResult<Vec<UnitRow>> {
    if input.is_empty() {
        return Err(AppError::bad("请至少添加一个填报单位"));
    }
    if input.len() > MAX_UNITS {
        return Err(AppError::bad(format!("填报单位最多 {MAX_UNITS} 个")));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(input.len());
    for u in input {
        let unit = u.unit.trim();
        if unit.is_empty() {
            return Err(AppError::bad("填报单位的名称不能为空"));
        }
        if unit.chars().count() > MAX_UNIT_CHARS || unit.chars().any(char::is_control) {
            return Err(AppError::bad(format!("填报单位「{unit}」的名称过长或含控制字符（不超过 {MAX_UNIT_CHARS} 个字）")));
        }
        if !seen.insert(unit.to_string()) {
            return Err(AppError::bad(format!("填报单位「{unit}」重复")));
        }
        let user_id = u.user_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let user_name = match user_id {
            Some(id) => Some(crate::approval::user_name(conn, id)?.ok_or_else(|| AppError::bad(format!("负责人「{unit}」还没有登录过系统，请先让对方登录一次")))?),
            None => None,
        };
        out.push(UnitRow { unit: unit.to_string(), user_id: user_id.map(String::from), user_name });
    }
    Ok(out)
}

fn check_period(period: &str) -> AppResult<String> {
    let p = period.trim();
    if p.is_empty() {
        return Err(AppError::bad("请填写期次，如 2026-10"));
    }
    if p.chars().count() > MAX_PERIOD_CHARS || p.chars().any(|c| c.is_control() || c == '/') {
        return Err(AppError::bad(format!("期次不超过 {MAX_PERIOD_CHARS} 个字，不含斜杠和控制字符")));
    }
    Ok(p.to_string())
}

/// 已有数据集能不能拿来收集：普通表、单位列和期次列是文本、主键包含这两列（否则不同单位 / 不同期的数据会互相覆盖）
fn check_existing(conn: &Connection, id: &str, unit: &str, period: &str) -> AppResult<(String, String, String)> {
    let def = dataset::dataset(conn, id)?;
    if def.kind != "table" {
        return Err(AppError::bad("只有普通数据集（导入的表、填报表）能用来收集，关联视图和汇总数据集不行"));
    }
    if collect_of_dataset(conn, &def.id)?.is_some() {
        return Err(AppError::bad("这个数据集已经属于另一个收集任务"));
    }
    let col = |name: &str| find_col(&def, name).ok_or_else(|| AppError::bad(format!("数据集「{}」没有「{name}」列", def.name)));
    let (u, p) = (col(unit)?, col(period)?);
    for c in [u, p] {
        if !matches!(c.kind, "text" | "id") {
            return Err(AppError::bad(format!("「{}」必须是文本列", c.title)));
        }
    }
    let in_pk = |c: &ColumnDef| def.primary_key.iter().any(|k| k == &c.title || k == &c.key);
    if !(in_pk(u) && in_pk(p)) {
        return Err(AppError::bad(format!("数据集的主键必须包含「{}」和「{}」，否则不同单位 / 不同期的数据会互相覆盖", u.title, p.title)));
    }
    Ok((def.id.clone(), u.title.clone(), p.title.clone()))
}

/// 新建填报表的定义：单位列、期次列自动加在最前面；主键 = 单位 + 期次 + 业务主键
fn plan_new_table(t: &NewTable, unit: &str, period: &str) -> AppResult<NewInputDataset> {
    let mut columns: Vec<NewColumn> = Vec::new();
    let mut titles: Vec<String> = Vec::new();
    for c in &t.columns {
        let title = c.title.trim().to_string();
        if (title == unit || title == period) && c.kind != "text" {
            return Err(AppError::bad(format!("「{title}」必须是文本列")));
        }
        titles.push(title.clone());
        columns.push(NewColumn { title, kind: c.kind.clone() });
    }
    // 单位列、期次列在最前面（用户已经列出的就不重复加）
    let mut front = Vec::new();
    for name in [unit, period] {
        if !titles.iter().any(|t| t == name) {
            front.push(NewColumn { title: name.to_string(), kind: "text".into() });
        }
    }
    front.extend(columns);
    let columns = front;
    let mut primary_key = vec![unit.to_string(), period.to_string()];
    for k in &t.key_columns {
        let k = k.trim();
        if k == unit || k == period {
            continue;
        }
        if !columns.iter().any(|c| c.title == k) {
            return Err(AppError::bad(format!("业务主键列「{k}」不在列里")));
        }
        primary_key.push(k.to_string());
    }
    if columns.len() < 3 {
        return Err(AppError::bad("请至少添加一个要填写的列"));
    }
    Ok(NewInputDataset { name: t.name.clone(), columns, primary_key })
}

// ── 数据范围同步 ──────────────────────────────────────

/// 把这个维度下所有任务的「单位 → 负责人」同步成数据权限：数据集绑定到单位列，负责人在这个维度上只能看 / 填自己负责的单位。
/// affected 是这次变动涉及的用户（包括被换掉的负责人）：他们名下没有单位了就删掉这个维度的规则。
/// 没开登录（没有用户）时什么都不做
fn sync_scope(conn: &Connection, user: &CurrentUser, dimension: &str, affected: &BTreeSet<String>) -> AppResult<()> {
    if !user.enforced {
        return Ok(());
    }
    for c in load_collects(conn)?.iter().filter(|c| c.dimension == dimension) {
        let Ok(def) = dataset::dataset(conn, &c.dataset_id) else { continue };
        let Some(col) = find_col(&def, &c.unit_column) else { continue };
        scope::save_binding(conn, &c.dataset_id, &BindingInput { scope_column: Some(col.key.clone()), scope_dimension: Some(dimension.to_string()), open: false })?;
    }
    let mut by_user: BTreeMap<String, Vec<String>> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT u.user_id, u.unit FROM ledger_collect_unit u JOIN ledger_collect c ON c.id = u.collect_id WHERE c.dimension = ? AND u.user_id IS NOT NULL ORDER BY u.ord",
        )?;
        for row in stmt.query_map(params![dimension], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (uid, unit) = row?;
            by_user.entry(uid).or_default().push(unit);
        }
    }
    let everyone: BTreeSet<&String> = affected.iter().chain(by_user.keys()).collect();
    for uid in everyone {
        match by_user.get(uid) {
            Some(units) => scope::save_rule(
                conn,
                &RuleInput { subject_id: uid.clone(), subject_name: String::new(), dimension: Some(dimension.to_string()), mode: "values".into(), values: units.clone(), operator: user.name.clone() },
            )?,
            None => scope::delete_rule(conn, uid, Some(dimension))?,
        }
    }
    Ok(())
}

fn assignees(units: &[UnitRow]) -> BTreeSet<String> {
    units.iter().filter_map(|u| u.user_id.clone()).collect()
}

fn insert_units(conn: &Connection, id: &str, units: &[UnitRow]) -> AppResult<()> {
    for (i, u) in units.iter().enumerate() {
        conn.execute("INSERT INTO ledger_collect_unit (collect_id, unit, user_id, user_name, ord) VALUES (?, ?, ?, ?, ?)", params![id, u.unit, u.user_id, u.user_name, i as i64])?;
    }
    Ok(())
}

// ── 任务的增删改 ──────────────────────────────────────

pub fn create(conn: &Connection, user: &CurrentUser, input: CollectInput) -> AppResult<String> {
    let name = check_name(&input.name)?;
    let units = clean_units(conn, &input.units)?;
    let dimension = scope::check_dimension(Some(input.dimension.as_deref().map(str::trim).filter(|d| !d.is_empty()).unwrap_or(DEFAULT_DIMENSION)))?;
    let pick = |v: &Option<String>, default: &str| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(default).to_string();
    let (unit_col, period_col) = (pick(&input.unit_column, DEFAULT_UNIT_COLUMN), pick(&input.period_column, DEFAULT_PERIOD_COLUMN));
    if unit_col == period_col {
        return Err(AppError::bad("单位列和期次列不能是同一列"));
    }
    // 先把能检查的都检查完，再建数据集（建数据集自带事务，之后失败不好回滚）
    let (dataset_id, new_def) = match (input.dataset_id.as_deref().map(str::trim).filter(|s| !s.is_empty()), &input.new_table) {
        (Some(id), None) => (Some(check_existing(conn, id, &unit_col, &period_col)?), None),
        (None, Some(t)) => {
            let def = plan_new_table(t, &unit_col, &period_col)?;
            if def.name.trim().is_empty() {
                return Err(AppError::bad("请填写填报表的名称"));
            }
            (None, Some(def))
        }
        _ => return Err(AppError::bad("请选择一个已有的数据集，或者新建一张填报表（二选一）")),
    };
    let (dataset_id, unit_title, period_title) = match (dataset_id, new_def) {
        (Some(found), _) => found,
        (None, Some(def)) => {
            let id = crate::writeback::create_dataset(conn, &def)?;
            dataset::set_owner(conn, &id, user)?;
            (id, unit_col.clone(), period_col.clone())
        }
        (None, None) => unreachable!("上面已经排除"),
    };
    let id = new_id();
    let now = now_local();
    in_tx(conn, || {
        conn.execute(
            "INSERT INTO ledger_collect (id, name, dataset_id, unit_column, period_column, dimension, created_by, created_by_name, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
            params![id, name, dataset_id, unit_title, period_title, dimension, user.id, user.name, now, now],
        )?;
        insert_units(conn, &id, &units)?;
        sync_scope(conn, user, &dimension, &assignees(&units))
    })?;
    Ok(id)
}

pub fn update(conn: &Connection, user: &CurrentUser, id: &str, input: UpdateInput) -> AppResult<()> {
    let c = load_collect(conn, id)?;
    let name = check_name(&input.name)?;
    let units = clean_units(conn, &input.units)?;
    let mut affected = assignees(&load_units(conn, id)?);
    affected.extend(assignees(&units));
    in_tx(conn, || {
        conn.execute("UPDATE ledger_collect SET name = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![name, now_local(), id])?;
        conn.execute("DELETE FROM ledger_collect_unit WHERE collect_id = ?", params![id])?;
        insert_units(conn, id, &units)?;
        sync_scope(conn, user, &c.dimension, &affected)
    })
}

/// 删除任务和它的单位、期次、提交记录；数据集和里面的数据保留
pub fn delete(conn: &Connection, user: &CurrentUser, id: &str) -> AppResult<()> {
    let c = load_collect(conn, id)?;
    let affected = assignees(&load_units(conn, id)?);
    in_tx(conn, || {
        for t in ["ledger_collect_submit", "ledger_collect_period", "ledger_collect_unit"] {
            conn.execute(&format!("DELETE FROM {t} WHERE collect_id = ?"), params![id])?;
        }
        conn.execute("DELETE FROM ledger_collect WHERE id = ?", params![id])?;
        sync_scope(conn, user, &c.dimension, &affected)
    })
}

pub fn open_period(conn: &Connection, user: &CurrentUser, id: &str, period: &str) -> AppResult<()> {
    load_collect(conn, id)?;
    let period = check_period(period)?;
    let exists: i64 = conn.query_row("SELECT count(*) FROM ledger_collect_period WHERE collect_id = ? AND period = ?", params![id, period], |r| r.get(0))?;
    if exists > 0 {
        return Err(AppError::bad(format!("「{period}」这一期已经开启了")));
    }
    conn.execute("INSERT INTO ledger_collect_period (collect_id, period, opened_at, opened_by) VALUES (?, ?, CAST(? AS TIMESTAMP), ?)", params![id, period, now_local(), user.name])?;
    Ok(())
}

/// 删除一期：只能删还没有人提交过的（开错了、写错了）
pub fn delete_period(conn: &Connection, id: &str, period: &str) -> AppResult<()> {
    load_collect(conn, id)?;
    let submitted: i64 = conn.query_row("SELECT count(*) FROM ledger_collect_submit WHERE collect_id = ? AND period = ?", params![id, period], |r| r.get(0))?;
    if submitted > 0 {
        return Err(AppError::bad(format!("「{period}」已经有人提交了数据，不能删除这一期")));
    }
    let n = conn.execute("DELETE FROM ledger_collect_period WHERE collect_id = ? AND period = ?", params![id, period])?;
    if n == 0 {
        return Err(AppError::bad("没有这一期"));
    }
    Ok(())
}

// ── 填报时的两个检查点（routes/writeback.rs 调用）──────

/// 单元格 → 文本（单位、期次都按文本比较；数字去掉多余的小数点，如 2026 不变成 2026.0）
fn cell_text(v: Option<&Json>) -> String {
    match v {
        Some(Json::String(s)) => s.trim().to_string(),
        Some(Json::Number(n)) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        Some(Json::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn listed(set: &BTreeSet<String>) -> String {
    let shown: Vec<String> = set.iter().take(MAX_LISTED).map(|v| if v.is_empty() { "（空）".to_string() } else { format!("「{v}」") }).collect();
    if set.len() > MAX_LISTED { format!("{} 等 {} 个", shown.join("、"), set.len()) } else { shown.join("、") }
}

/// 写入前校验：数据集属于收集任务时，填报区必须带上单位列和期次列，期次必须已开启，单位必须在任务里。
/// 删除（按主键）只带主键列，不在这里检查。不属于任何任务的数据集直接通过
pub fn before_submit(conn: &Connection, dataset_id: &str, req: &WritebackRequest) -> AppResult<()> {
    let Some(c) = collect_of_dataset(conn, dataset_id)? else { return Ok(()) };
    if req.mode == "delete" {
        return Ok(());
    }
    let pos = |title: &str| req.columns.iter().position(|t| t == title);
    let (Some(ui), Some(pi)) = (pos(&c.unit_column), pos(&c.period_column)) else {
        return Err(AppError::bad(format!("这个数据集属于收集任务「{}」：填报区必须包含「{}」和「{}」两列", c.name, c.unit_column, c.period_column)));
    };
    let open: BTreeSet<String> = load_periods(conn, &c.id)?.into_iter().map(|p| p.0).collect();
    if open.is_empty() {
        return Err(AppError::bad(format!("收集任务「{}」还没有开启任何一期，请联系管理员先开启", c.name)));
    }
    let units: BTreeSet<String> = load_units(conn, &c.id)?.into_iter().map(|u| u.unit).collect();
    let (mut bad_periods, mut bad_units) = (BTreeSet::new(), BTreeSet::new());
    for row in &req.rows {
        let (u, p) = (cell_text(row.get(ui)), cell_text(row.get(pi)));
        if !open.contains(&p) {
            bad_periods.insert(p);
        }
        if !units.contains(&u) {
            bad_units.insert(u);
        }
    }
    if !bad_periods.is_empty() {
        let can: Vec<String> = open.iter().take(MAX_LISTED).cloned().collect();
        return Err(AppError::bad(format!("「{}」列里的期次{}没有开启（现在可以填：{}）", c.period_column, listed(&bad_periods), can.join("、"))));
    }
    if !bad_units.is_empty() {
        return Err(AppError::bad(format!("「{}」列里的{}不在收集任务「{}」的填报单位里", c.unit_column, listed(&bad_units), c.name)));
    }
    Ok(())
}

/// 写入成功后：提交里出现的每个（单位，期次）记为已提交。记录失败不影响已经写入的数据，只写日志
pub fn after_submit(conn: &Connection, dataset_id: &str, req: &WritebackRequest, outcome: &WritebackOutcome, user: &CurrentUser) {
    if outcome.batch_id.is_none() || req.mode == "delete" {
        return;
    }
    let result = (|| -> AppResult<()> {
        let Some(c) = collect_of_dataset(conn, dataset_id)? else { return Ok(()) };
        let pos = |title: &str| req.columns.iter().position(|t| t == title);
        let (Some(ui), Some(pi)) = (pos(&c.unit_column), pos(&c.period_column)) else { return Ok(()) };
        let pairs: BTreeSet<(String, String)> = req.rows.iter().map(|row| (cell_text(row.get(ui)), cell_text(row.get(pi)))).collect();
        let who = if user.name.is_empty() { req.operator.clone() } else { user.name.clone() };
        let now = now_local();
        for (unit, period) in pairs {
            conn.execute(
                "INSERT INTO ledger_collect_submit (collect_id, period, unit, submitted_at, submitted_by, submitted_by_name, batch_id) VALUES (?, ?, ?, CAST(? AS TIMESTAMP), ?, ?, ?)
                 ON CONFLICT (collect_id, period, unit) DO UPDATE SET submitted_at = excluded.submitted_at, submitted_by = excluded.submitted_by, submitted_by_name = excluded.submitted_by_name, batch_id = excluded.batch_id",
                params![c.id, period, unit, now, user.id, who, outcome.batch_id],
            )?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        tracing::warn!("记录收集任务的提交状态失败（数据已写入）：{}", e.message());
    }
}

// ── 查看：任务列表、看板、我的填报 ────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectView {
    pub id: String,
    pub name: String,
    pub dataset_id: String,
    /// 数据集已被删除时为空
    pub dataset_name: Option<String>,
    pub unit_column: String,
    pub period_column: String,
    pub dimension: String,
    pub unit_count: i64,
    /// 指派了负责人的单位数
    pub assigned_count: i64,
    pub period_count: i64,
    /// 最近一期及它的提交进度
    pub latest_period: Option<String>,
    pub latest_submitted: i64,
    pub created_by_name: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 某一期里已提交的单位数（只算任务里现有的单位）
fn submitted_count(conn: &Connection, id: &str, period: &str) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM ledger_collect_submit s JOIN ledger_collect_unit u ON u.collect_id = s.collect_id AND u.unit = s.unit WHERE s.collect_id = ? AND s.period = ?",
        params![id, period],
        |r| r.get(0),
    )?)
}

fn view(conn: &Connection, c: &StoredCollect, names: &HashMap<String, String>) -> AppResult<CollectView> {
    let (unit_count, assigned_count): (i64, i64) =
        conn.query_row("SELECT count(*), count(user_id) FROM ledger_collect_unit WHERE collect_id = ?", params![c.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let periods = load_periods(conn, &c.id)?;
    let latest = periods.first().map(|p| p.0.clone());
    Ok(CollectView {
        id: c.id.clone(),
        name: c.name.clone(),
        dataset_id: c.dataset_id.clone(),
        dataset_name: names.get(&c.dataset_id).cloned(),
        unit_column: c.unit_column.clone(),
        period_column: c.period_column.clone(),
        dimension: c.dimension.clone(),
        unit_count,
        assigned_count,
        period_count: periods.len() as i64,
        latest_submitted: match &latest {
            Some(p) => submitted_count(conn, &c.id, p)?,
            None => 0,
        },
        latest_period: latest,
        created_by_name: c.created_by_name.clone(),
        created_at: c.created_at.clone(),
        updated_at: c.updated_at.clone(),
    })
}

pub fn list(conn: &Connection) -> AppResult<Vec<CollectView>> {
    let names = dataset_names(conn);
    load_collects(conn)?.iter().map(|c| view(conn, c, &names)).collect()
}

/// 某一期里每个单位已经填了几行（数据集已删除、列不在了就是空）。受限用户调用时数据集已按他的范围过滤，只数得到自己的行
fn row_counts(conn: &Connection, c: &StoredCollect, period: &str) -> HashMap<String, i64> {
    let Ok(def) = dataset::dataset(conn, &c.dataset_id) else { return HashMap::new() };
    let (Some(u), Some(p)) = (find_col(&def, &c.unit_column), find_col(&def, &c.period_column)) else { return HashMap::new() };
    let sql = format!("SELECT CAST(({}) AS VARCHAR), count(*) FROM {} WHERE CAST(({}) AS VARCHAR) = ? GROUP BY 1", column_sql(u), def.table, column_sql(p));
    let Ok(mut stmt) = conn.prepare(&sql) else { return HashMap::new() };
    let rows = stmt.query_map(params![period], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, i64>(1)?)));
    rows.map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitStatus {
    pub unit: String,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    /// submitted / pending
    pub status: &'static str,
    pub submitted_at: Option<String>,
    pub submitted_by_name: Option<String>,
    /// 数据集里这个单位这一期现在有几行
    pub rows: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodSummary {
    pub period: String,
    pub opened_at: String,
    pub total: i64,
    pub submitted: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub collect: CollectView,
    /// 已开启的期次，新的在前
    pub periods: Vec<PeriodSummary>,
    /// 下面 units 是哪一期的状态（没传就是最近一期；还没有任何期次时为空）
    pub period: Option<String>,
    pub units: Vec<UnitStatus>,
}

/// 看板：某一期里每个单位的提交状态
pub fn detail(conn: &Connection, id: &str, period: Option<&str>) -> AppResult<Detail> {
    let c = load_collect(conn, id)?;
    let units = load_units(conn, id)?;
    let opened = load_periods(conn, id)?;
    let chosen = match period.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) if opened.iter().any(|o| o.0 == p) => Some(p.to_string()),
        Some(p) => return Err(AppError::bad(format!("「{p}」这一期没有开启"))),
        None => opened.first().map(|o| o.0.clone()),
    };
    let mut periods = Vec::with_capacity(opened.len());
    for (p, at) in &opened {
        periods.push(PeriodSummary { period: p.clone(), opened_at: at.clone(), total: units.len() as i64, submitted: submitted_count(conn, id, p)? });
    }
    let rows = match &chosen {
        Some(p) => {
            let mut sub: HashMap<String, (String, String)> = HashMap::new();
            let mut stmt = conn.prepare("SELECT unit, strftime(submitted_at, '%Y-%m-%d %H:%M:%S'), submitted_by_name FROM ledger_collect_submit WHERE collect_id = ? AND period = ?")?;
            for r in stmt.query_map(params![id, p], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, Option<String>>(2)?.unwrap_or_default())))? {
                let (u, at, by) = r?;
                sub.insert(u, (at, by));
            }
            let counts = row_counts(conn, &c, p);
            units
                .into_iter()
                .map(|u| {
                    let s = sub.get(&u.unit);
                    UnitStatus {
                        rows: counts.get(&u.unit).copied().unwrap_or(0),
                        status: if s.is_some() { "submitted" } else { "pending" },
                        submitted_at: s.map(|s| s.0.clone()),
                        submitted_by_name: s.map(|s| s.1.clone()),
                        unit: u.unit,
                        user_id: u.user_id,
                        user_name: u.user_name,
                    }
                })
                .collect()
        }
        None => vec![],
    };
    let names = dataset_names(conn);
    Ok(Detail { collect: view(conn, &c, &names)?, periods, period: chosen, units: rows })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyPeriod {
    pub period: String,
    /// submitted / pending
    pub status: &'static str,
    pub submitted_at: Option<String>,
    pub rows: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyUnit {
    pub unit: String,
    pub periods: Vec<MyPeriod>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyTask {
    pub id: String,
    pub name: String,
    pub dataset_id: String,
    pub dataset_name: Option<String>,
    pub unit_column: String,
    pub period_column: String,
    pub units: Vec<MyUnit>,
    /// 还没填的（单位 × 已开启的期次）个数
    pub pending: i64,
}

/// 我的填报：我负责的单位在最近几期里填了没有。没开登录时不区分用户，列出全部
pub fn mine(conn: &Connection, user: &CurrentUser) -> AppResult<Vec<MyTask>> {
    let names = dataset_names(conn);
    let mut out = Vec::new();
    for c in load_collects(conn)? {
        let units: Vec<UnitRow> = load_units(conn, &c.id)?.into_iter().filter(|u| !user.enforced || (u.user_id.is_some() && u.user_id.as_deref() == Some(user.id.as_str()))).collect();
        if units.is_empty() {
            continue;
        }
        let periods: Vec<String> = load_periods(conn, &c.id)?.into_iter().take(MINE_PERIODS).map(|p| p.0).collect();
        let mut sub: HashMap<(String, String), String> = HashMap::new();
        let mut stmt = conn.prepare("SELECT period, unit, strftime(submitted_at, '%Y-%m-%d %H:%M:%S') FROM ledger_collect_submit WHERE collect_id = ?")?;
        for r in stmt.query_map(params![c.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))? {
            let (p, u, at) = r?;
            sub.insert((p, u), at);
        }
        let counts: HashMap<&String, HashMap<String, i64>> = periods.iter().map(|p| (p, row_counts(conn, &c, p))).collect();
        let mut pending = 0;
        let my_units = units
            .into_iter()
            .map(|u| MyUnit {
                periods: periods
                    .iter()
                    .map(|p| {
                        let at = sub.get(&(p.clone(), u.unit.clone()));
                        if at.is_none() {
                            pending += 1;
                        }
                        MyPeriod { period: p.clone(), status: if at.is_some() { "submitted" } else { "pending" }, submitted_at: at.cloned(), rows: counts.get(p).and_then(|m| m.get(&u.unit)).copied().unwrap_or(0) }
                    })
                    .collect(),
                unit: u.unit,
            })
            .collect();
        out.push(MyTask {
            dataset_name: names.get(&c.dataset_id).cloned(),
            id: c.id,
            name: c.name,
            dataset_id: c.dataset_id,
            unit_column: c.unit_column,
            period_column: c.period_column,
            units: my_units,
            pending,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        conn
    }

    fn admin() -> CurrentUser {
        CurrentUser { id: "u0".into(), name: "Root".into(), admin: true, enforced: true }
    }

    fn add_user(conn: &Connection, id: &str, name: &str) {
        conn.execute("INSERT INTO ledger_user (id, name, admin, first_seen, last_seen) VALUES (?, ?, false, now(), now())", params![id, name]).unwrap();
    }

    fn table(name: &str) -> NewTable {
        NewTable {
            name: name.into(),
            columns: vec![NewColumn { title: "营收".into(), kind: "money".into() }, NewColumn { title: "备注".into(), kind: "text".into() }],
            key_columns: vec![],
        }
    }

    fn new_input(name: &str, units: Vec<UnitInput>) -> CollectInput {
        CollectInput { name: name.into(), dataset_id: None, new_table: Some(table(&format!("{name}表"))), unit_column: None, period_column: None, dimension: None, units }
    }

    fn unit(u: &str, who: Option<&str>) -> UnitInput {
        UnitInput { unit: u.into(), user_id: who.map(String::from) }
    }

    fn req(mode: &str, cols: &[&str], rows: Vec<Vec<Json>>) -> WritebackRequest {
        WritebackRequest { mode: mode.into(), columns: cols.iter().map(|s| s.to_string()).collect(), rows, keys: vec![], operator: "测试".into(), source: None }
    }

    fn rule(conn: &Connection, uid: &str) -> Option<Vec<String>> {
        conn.query_row("SELECT vals FROM ledger_scope_rule WHERE subject_id = ? AND dimension = '填报单位'", params![uid], |r| r.get::<_, String>(0))
            .optional()
            .unwrap()
            .map(|v| serde_json::from_str(&v).unwrap())
    }

    #[test]
    fn cells_are_compared_as_text() {
        assert_eq!(cell_text(Some(&Json::from(" 店铺A "))), "店铺A");
        assert_eq!(cell_text(Some(&Json::from(2026))), "2026");
        assert_eq!(cell_text(Some(&Json::from(2026.0))), "2026");
        assert_eq!(cell_text(Some(&Json::from(1.5))), "1.5");
        assert_eq!(cell_text(Some(&Json::Null)), "");
        assert_eq!(cell_text(None), "");
    }

    #[test]
    fn periods_and_names_are_validated() {
        assert_eq!(check_period(" 2026-10 ").unwrap(), "2026-10");
        assert!(check_period("").is_err());
        assert!(check_period("2026/10").is_err());
        assert!(check_period(&"x".repeat(21)).is_err());
        assert!(check_name("  ").is_err());
        assert!(check_name(&"名".repeat(61)).is_err());
    }

    #[test]
    fn new_table_gets_unit_and_period_columns_and_composite_key() {
        let mut t = table("t");
        t.key_columns = vec!["备注".into(), "填报单位".into()];
        let d = plan_new_table(&t, "填报单位", "期次").unwrap();
        let titles: Vec<&str> = d.columns.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["填报单位", "期次", "营收", "备注"], "单位、期次在最前面");
        assert_eq!(d.primary_key, vec!["填报单位", "期次", "备注"], "业务主键里重复写了单位列也只算一次");
        // 用户自己已经列出单位列：不重复加；但必须是文本列
        let mut own = table("t");
        own.columns.insert(0, NewColumn { title: "填报单位".into(), kind: "text".into() });
        assert_eq!(plan_new_table(&own, "填报单位", "期次").unwrap().columns.len(), 4);
        own.columns[0].kind = "money".into();
        assert!(plan_new_table(&own, "填报单位", "期次").is_err());
        let mut bad_key = table("t");
        bad_key.key_columns = vec!["不存在".into()];
        assert!(plan_new_table(&bad_key, "填报单位", "期次").is_err());
        let mut empty = table("t");
        empty.columns.clear();
        assert!(plan_new_table(&empty, "填报单位", "期次").is_err(), "没有业务列没有意义");
    }

    #[test]
    fn create_syncs_scope_and_rejects_bad_input() {
        let conn = setup();
        for (id, name) in [("u1", "Bob"), ("u2", "Carol")] {
            add_user(&conn, id, name);
        }
        let me = admin();
        let id = create(&conn, &me, new_input("月度经营", vec![unit("店铺A", Some("u1")), unit("店铺B", Some("u2")), unit("店铺C", None), unit("店铺D", Some("u1"))])).unwrap();
        let c = load_collect(&conn, &id).unwrap();
        assert_eq!((c.unit_column.as_str(), c.period_column.as_str(), c.dimension.as_str()), ("填报单位", "期次", "填报单位"));
        assert_eq!(rule(&conn, "u1"), Some(vec!["店铺A".into(), "店铺D".into()]), "一个人负责多个单位");
        assert_eq!(rule(&conn, "u2"), Some(vec!["店铺B".into()]));
        let (col, dim): (Option<String>, Option<String>) = conn.query_row("SELECT scope_column, scope_dimension FROM ledger_dataset WHERE id = ?", params![c.dataset_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((col.as_deref(), dim.as_deref()), (Some("填报单位"), Some("填报单位")), "数据集按单位列绑定");

        // 换负责人：旧负责人名下没有单位了，规则删掉；新负责人拿到规则
        update(&conn, &me, &id, UpdateInput { name: "月度经营".into(), units: vec![unit("店铺A", Some("u2")), unit("店铺B", Some("u2"))] }).unwrap();
        assert_eq!(rule(&conn, "u1"), None);
        assert_eq!(rule(&conn, "u2"), Some(vec!["店铺A".into(), "店铺B".into()]));

        // 输入检查：重复单位、空单位、没登录过的负责人、没有单位、二选一
        let dup = create(&conn, &me, new_input("x1", vec![unit("A", None), unit(" A ", None)]));
        assert!(dup.unwrap_err().message().contains("重复"));
        assert!(create(&conn, &me, new_input("x2", vec![unit("  ", None)])).is_err());
        assert!(create(&conn, &me, new_input("x3", vec![unit("A", Some("nobody"))])).unwrap_err().message().contains("还没有登录过"));
        assert!(create(&conn, &me, new_input("x4", vec![])).is_err());
        let mut both = new_input("x5", vec![unit("A", None)]);
        both.dataset_id = Some("ds_x".into());
        assert!(create(&conn, &me, both).unwrap_err().message().contains("二选一"));
        assert_eq!(list(&conn).unwrap().len(), 1, "失败的创建不留下任务");

        // 删除任务：规则清掉，数据集保留
        delete(&conn, &me, &id).unwrap();
        assert_eq!(rule(&conn, "u2"), None);
        assert!(dataset::dataset(&conn, &c.dataset_id).is_ok());
        assert!(load_collect(&conn, &id).is_err());
    }

    #[test]
    fn open_dataset_must_have_unit_and_period_in_primary_key() {
        let conn = setup();
        let me = CurrentUser::open();
        let ds = |name: &str, pk: &[&str]| {
            let input = NewInputDataset {
                name: name.into(),
                columns: ["填报单位", "期次", "营收"].iter().map(|t| NewColumn { title: (*t).into(), kind: if *t == "营收" { "money" } else { "text" }.into() }).collect(),
                primary_key: pk.iter().map(|s| s.to_string()).collect(),
            };
            crate::writeback::create_dataset(&conn, &input).unwrap()
        };
        let existing = |id: String| CollectInput { name: "t".into(), dataset_id: Some(id), new_table: None, unit_column: None, period_column: None, dimension: None, units: vec![unit("A", None)] };
        let weak = ds("只按单位", &["填报单位"]);
        assert!(create(&conn, &me, existing(weak)).unwrap_err().message().contains("主键必须包含"));
        let good = ds("完整主键", &["填报单位", "期次"]);
        let id = create(&conn, &me, existing(good.clone())).unwrap();
        assert_eq!(load_collect(&conn, &id).unwrap().dataset_id, good);
        assert!(create(&conn, &me, existing(good)).unwrap_err().message().contains("已经属于另一个收集任务"), "一个数据集只属于一个任务");
        let mut wrong = existing(ds("列名不对", &["填报单位", "期次"]));
        wrong.unit_column = Some("店铺".into());
        assert!(create(&conn, &me, wrong).unwrap_err().message().contains("没有「店铺」列"));
    }

    #[test]
    fn submit_checks_and_status_board() {
        let conn = setup();
        add_user(&conn, "u1", "Bob");
        let me = admin();
        let id = create(&conn, &me, new_input("月度", vec![unit("店铺A", Some("u1")), unit("店铺B", None)])).unwrap();
        let ds = load_collect(&conn, &id).unwrap().dataset_id;
        let cols = ["填报单位", "期次", "营收"];
        let row = |u: &str, p: &str, v: i64| vec![Json::from(u), Json::from(p), Json::from(v)];

        // 不属于任何任务的数据集：不检查
        assert!(before_submit(&conn, "ds_other", &req("upsert", &cols, vec![])).is_ok());
        // 还没开启任何一期
        let e = before_submit(&conn, &ds, &req("upsert", &cols, vec![row("店铺A", "2026-10", 1)])).unwrap_err();
        assert!(e.message().contains("还没有开启任何一期"), "{}", e.message());

        open_period(&conn, &me, &id, "2026-10").unwrap();
        assert!(open_period(&conn, &me, &id, "2026-10").unwrap_err().message().contains("已经开启"));
        // 必须带单位列和期次列
        let e = before_submit(&conn, &ds, &req("upsert", &["填报单位", "营收"], vec![])).unwrap_err();
        assert!(e.message().contains("必须包含"), "{}", e.message());
        // 期次没开启、单位不在任务里、单位 / 期次为空
        let e = before_submit(&conn, &ds, &req("upsert", &cols, vec![row("店铺A", "2026-11", 1)])).unwrap_err();
        assert!(e.message().contains("2026-11") && e.message().contains("现在可以填：2026-10"), "{}", e.message());
        let e = before_submit(&conn, &ds, &req("upsert", &cols, vec![row("店铺Z", "2026-10", 1)])).unwrap_err();
        assert!(e.message().contains("店铺Z") && e.message().contains("不在收集任务"), "{}", e.message());
        let blank = vec![Json::Null, Json::from("2026-10"), Json::from(1)];
        assert!(before_submit(&conn, &ds, &req("upsert", &cols, vec![blank])).unwrap_err().message().contains("（空）"));
        // 删除只带主键列，不检查
        assert!(before_submit(&conn, &ds, &req("delete", &["填报单位"], vec![vec![Json::from("店铺Z")]])).is_ok());
        let ok = req("upsert", &cols, vec![row("店铺A", "2026-10", 100)]);
        assert!(before_submit(&conn, &ds, &ok).is_ok());

        // 提交成功 → A 记为已提交；没成功（有错误）或删除都不记
        let out = crate::writeback::submit(&conn, &ds, &ok).unwrap();
        assert!(out.batch_id.is_some());
        let bob = CurrentUser { id: "u1".into(), name: "Bob".into(), admin: false, enforced: true };
        after_submit(&conn, &ds, &req("upsert", &cols, vec![row("店铺B", "2026-10", 1)]), &WritebackOutcome { batch_id: None, ..out_like(&out) }, &bob);
        after_submit(&conn, &ds, &ok, &out, &bob);
        let d = detail(&conn, &id, None).unwrap();
        assert_eq!(d.period.as_deref(), Some("2026-10"), "默认看最近一期");
        let by = |u: &str| d.units.iter().find(|x| x.unit == u).unwrap();
        assert_eq!((by("店铺A").status, by("店铺A").submitted_by_name.as_deref(), by("店铺A").rows), ("submitted", Some("Bob"), 1));
        assert_eq!((by("店铺B").status, by("店铺B").rows), ("pending", 0));
        assert_eq!((d.periods[0].total, d.periods[0].submitted), (2, 1));
        assert_eq!((d.collect.latest_period.as_deref(), d.collect.latest_submitted, d.collect.unit_count, d.collect.assigned_count), (Some("2026-10"), 1, 2, 1));
        assert!(detail(&conn, &id, Some("2099-01")).is_err());

        // 我的填报：Bob 只看到自己的单位；没有任务的人什么都没有
        let mine_bob = mine(&conn, &bob).unwrap();
        assert_eq!(mine_bob.len(), 1);
        assert_eq!((mine_bob[0].units.len(), mine_bob[0].pending), (1, 0));
        let nobody = CurrentUser { id: "u9".into(), name: "X".into(), admin: false, enforced: true };
        assert!(mine(&conn, &nobody).unwrap().is_empty());
        assert_eq!(mine(&conn, &CurrentUser::open()).unwrap()[0].units.len(), 2, "没开登录时列出全部");

        // 删期次：有人提交过的不能删；没人提交的可以
        assert!(delete_period(&conn, &id, "2026-10").unwrap_err().message().contains("不能删除"));
        open_period(&conn, &me, &id, "2026-11").unwrap();
        delete_period(&conn, &id, "2026-11").unwrap();
        assert!(delete_period(&conn, &id, "2026-11").is_err());
    }

    /// 复制一份 outcome（WritebackOutcome 没有 Clone）
    fn out_like(o: &WritebackOutcome) -> WritebackOutcome {
        WritebackOutcome {
            batch_id: o.batch_id.clone(),
            dataset_name: o.dataset_name.clone(),
            inserted: o.inserted,
            updated: o.updated,
            unchanged: o.unchanged,
            deleted: o.deleted,
            not_found: o.not_found,
            errors: vec![],
            error_count: 0,
            elapsed_ms: 0,
        }
    }
}
