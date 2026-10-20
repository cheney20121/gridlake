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
//! - **一键生成填报工作簿**：负责人点「去填报」，服务端为（任务、单位、期次）生成一个工作簿，里面已经放好填报区——
//!   单位列和期次列作为**固定列**写在填报区定义里（`fixed`），不出现在表格中，提交时由前端自动带上，所以负责人只填业务列。
//!   同一个人对同一个（任务、单位、期次）只生成一次，再点直接打开；数据集里已经有的行会预填进去
//!
//! - **审核（退回 / 确认锁定）**：任务可以指定一个**确认人**（管理员始终也能审核）。提交后确认人看过数据，可以**确认**——
//!   这个单位这一期锁定，`before_submit` 拒绝再写（提交和删除都不行）；也可以**退回**并写明原因——负责人收到通知，改完再提交，状态回到「已提交」。
//!   确认过的可以「撤销确认」回到已提交。状态存在 `ledger_collect_review`：没有记录 = 还在等确认；负责人再次提交会删掉「退回」记录
//!
//! - **截止日期与催办**：每一期可以设一个截止日期（可空，当天 24 点前都算按时，过了没提交的算「逾期」；只是提示，不拦截提交）。
//!   催办有两种：管理员 / 确认人在看板上手动催（同一个单位 1 小时内不重复催）；设了截止日期的期次由后台自动催——截止前一天、
//!   截止当天、逾期后每天一次（最多 7 天），每天 9 点以后。催办 = 给负责人发站内通知（顶栏铃铛），任务里配了推送渠道的还会把
//!   「没提交的名单」推到群 / 邮箱。每个（单位，期次）的上次催办时间记在 `ledger_collect_remind`

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;
use std::io::Write;

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use duckdb::{Connection, OptionalExt, params};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::auth::CurrentUser;
use crate::comment::{self, Notice};
use crate::dataset::{self, ColumnDef, DatasetDef, column_sql};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::report::{self, ChannelResult};
use crate::routes::AppState;
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
/// 生成填报工作簿时最多预填多少行已有的数据
const PREFILL_ROWS: usize = 2000;
/// 填报区表头所在的行（0 起）：上面留给标题和说明
const INPUT_HEADER_ROW: usize = 3;
/// 数据区预留的行数（与前端填报区一致，用于设置格式）
const PREPARED_ROWS: usize = 500;

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
    /// 确认人（登录过系统的用户 id）；不指定就只有管理员能审核
    #[serde(default)]
    pub reviewer_id: Option<String>,
    /// 催办时同时推送的渠道（「定时推送」里配置的企业微信 / 钉钉 / 飞书群、邮件）；不选就只发站内通知
    #[serde(default)]
    pub remind_channels: Vec<String>,
    pub units: Vec<UnitInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInput {
    pub name: String,
    /// 不写 / 为空 = 取消确认人
    #[serde(default)]
    pub reviewer_id: Option<String>,
    #[serde(default)]
    pub remind_channels: Vec<String>,
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
    /// 确认人（可空：只有管理员审核）
    reviewer_id: Option<String>,
    reviewer_name: Option<String>,
    /// 催办推送的渠道 id
    remind_channels: Vec<String>,
}

const COLLECT_COLS: &str = "id, name, dataset_id, unit_column, period_column, dimension, created_by_name, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(updated_at, '%Y-%m-%d %H:%M:%S'), reviewer_id, reviewer_name, remind_channels";

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
        reviewer_id: r.get(9)?,
        reviewer_name: r.get(10)?,
        remind_channels: r.get::<_, Option<String>>(11)?.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
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

/// 各期的截止日期（没设的不在里面）
fn load_deadlines(conn: &Connection, id: &str) -> AppResult<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT period, deadline FROM ledger_collect_period WHERE collect_id = ? AND deadline IS NOT NULL AND deadline <> ''")?;
    Ok(stmt.query_map(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<_, _>>()?)
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// 截止日期还剩几天（当天 0，过了是负数）；没设或格式不对返回 None
fn days_left(deadline: Option<&String>, today: NaiveDate) -> Option<i64> {
    let d = NaiveDate::parse_from_str(deadline?, "%Y-%m-%d").ok()?;
    Some((d - today).num_days())
}

fn is_overdue(deadline: Option<&String>, today: NaiveDate) -> bool {
    days_left(deadline, today).is_some_and(|n| n < 0)
}

/// 提示里的截止说明（带前导逗号，没设截止日期就是空）
fn deadline_note(deadline: Option<&String>, today: NaiveDate) -> String {
    let (Some(d), Some(n)) = (deadline, days_left(deadline, today)) else { return String::new() };
    match n {
        0 => "，今天截止".into(),
        1 => "，明天截止".into(),
        n if n > 1 => format!("，{d} 截止（还剩 {n} 天）"),
        n => format!("，已逾期 {} 天（截止 {d}）", -n),
    }
}

/// 截止日期：空 = 不设；否则必须是「年-月-日」
fn check_deadline(deadline: Option<&str>) -> AppResult<Option<String>> {
    let Some(d) = deadline.map(str::trim).filter(|d| !d.is_empty()) else { return Ok(None) };
    NaiveDate::parse_from_str(d, "%Y-%m-%d").map(|d| Some(d.format("%Y-%m-%d").to_string())).map_err(|_| AppError::bad("截止日期格式应为 年-月-日，如 2026-10-20"))
}

/// 催办要推送的渠道：必须都存在
fn check_channels(conn: &Connection, ids: &[String]) -> AppResult<Vec<String>> {
    let mut seen = Vec::new();
    for i in ids {
        if !seen.contains(i) {
            seen.push(i.clone());
        }
    }
    if report::existing_channel_ids(conn, &seen)?.len() != seen.len() {
        return Err(AppError::bad("推送渠道不存在（可能已被删除）"));
    }
    Ok(seen)
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

/// 确认人：必须登录过系统。返回 (用户 id, 姓名)
fn clean_reviewer(conn: &Connection, id: Option<&str>) -> AppResult<Option<(String, String)>> {
    let Some(id) = id.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None) };
    let name = crate::approval::user_name(conn, id)?.ok_or_else(|| AppError::bad("确认人还没有登录过系统，请先让对方登录一次"))?;
    Ok(Some((id.to_string(), name)))
}

/// 确认人要看全部单位的数据才能确认，所以不能同时是同一个维度下某个单位的负责人（负责人只看得到自己的单位）
fn check_reviewer_scope(conn: &Connection, dimension: &str) -> AppResult<()> {
    let clash: Option<(String, String)> = conn
        .query_row(
            "SELECT c.reviewer_name, u.unit FROM ledger_collect c
             JOIN ledger_collect_unit u ON u.user_id = c.reviewer_id
             JOIN ledger_collect c2 ON c2.id = u.collect_id AND c2.dimension = c.dimension
             WHERE c.dimension = ? AND c.reviewer_id IS NOT NULL ORDER BY u.ord LIMIT 1",
            params![dimension],
            |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get(1)?)),
        )
        .optional()?;
    match clash {
        Some((who, unit)) => Err(clash_error(&who, &unit)),
        None => Ok(()),
    }
}

fn clash_error(who: &str, unit: &str) -> AppError {
    AppError::bad(format!("确认人「{who}」同时是填报单位「{unit}」的负责人：负责人只能看到自己的单位，就看不到全部数据来确认。请换一个人，或者取消其中一个身份"))
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
    let reviewer = clean_reviewer(conn, input.reviewer_id.as_deref())?;
    let channels = serde_json::to_string(&check_channels(conn, &input.remind_channels)?)?;
    let dimension = scope::check_dimension(Some(input.dimension.as_deref().map(str::trim).filter(|d| !d.is_empty()).unwrap_or(DEFAULT_DIMENSION)))?;
    let pick = |v: &Option<String>, default: &str| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(default).to_string();
    let (unit_col, period_col) = (pick(&input.unit_column, DEFAULT_UNIT_COLUMN), pick(&input.period_column, DEFAULT_PERIOD_COLUMN));
    if unit_col == period_col {
        return Err(AppError::bad("单位列和期次列不能是同一列"));
    }
    if let Some((rid, rname)) = &reviewer
        && let Some(u) = units.iter().find(|u| u.user_id.as_deref() == Some(rid.as_str()))
    {
        return Err(clash_error(rname, &u.unit));
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
            "INSERT INTO ledger_collect (id, name, dataset_id, unit_column, period_column, dimension, created_by, created_by_name, created_at, updated_at, reviewer_id, reviewer_name, remind_channels)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP), ?, ?, ?)",
            params![id, name, dataset_id, unit_title, period_title, dimension, user.id, user.name, now, now, reviewer.as_ref().map(|r| r.0.clone()), reviewer.as_ref().map(|r| r.1.clone()), channels],
        )?;
        insert_units(conn, &id, &units)?;
        check_reviewer_scope(conn, &dimension)?;
        sync_scope(conn, user, &dimension, &assignees(&units))
    })?;
    Ok(id)
}

pub fn update(conn: &Connection, user: &CurrentUser, id: &str, input: UpdateInput) -> AppResult<()> {
    let c = load_collect(conn, id)?;
    let name = check_name(&input.name)?;
    let units = clean_units(conn, &input.units)?;
    let reviewer = clean_reviewer(conn, input.reviewer_id.as_deref())?;
    let channels = serde_json::to_string(&check_channels(conn, &input.remind_channels)?)?;
    let mut affected = assignees(&load_units(conn, id)?);
    affected.extend(assignees(&units));
    in_tx(conn, || {
        conn.execute(
            "UPDATE ledger_collect SET name = ?, reviewer_id = ?, reviewer_name = ?, remind_channels = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params![name, reviewer.as_ref().map(|r| r.0.clone()), reviewer.as_ref().map(|r| r.1.clone()), channels, now_local(), id],
        )?;
        conn.execute("DELETE FROM ledger_collect_unit WHERE collect_id = ?", params![id])?;
        insert_units(conn, id, &units)?;
        check_reviewer_scope(conn, &c.dimension)?;
        sync_scope(conn, user, &c.dimension, &affected)
    })
}

/// 删除任务和它的单位、期次、提交记录；数据集和里面的数据保留
pub fn delete(conn: &Connection, user: &CurrentUser, id: &str) -> AppResult<()> {
    let c = load_collect(conn, id)?;
    let affected = assignees(&load_units(conn, id)?);
    in_tx(conn, || {
        for t in ["ledger_collect_workbook", "ledger_collect_remind", "ledger_collect_review", "ledger_collect_submit", "ledger_collect_period", "ledger_collect_unit"] {
            conn.execute(&format!("DELETE FROM {t} WHERE collect_id = ?"), params![id])?;
        }
        conn.execute("DELETE FROM ledger_collect WHERE id = ?", params![id])?;
        sync_scope(conn, user, &c.dimension, &affected)
    })
}

pub fn open_period(conn: &Connection, user: &CurrentUser, id: &str, period: &str, deadline: Option<&str>) -> AppResult<()> {
    load_collect(conn, id)?;
    let period = check_period(period)?;
    let deadline = check_deadline(deadline)?;
    let exists: i64 = conn.query_row("SELECT count(*) FROM ledger_collect_period WHERE collect_id = ? AND period = ?", params![id, period], |r| r.get(0))?;
    if exists > 0 {
        return Err(AppError::bad(format!("「{period}」这一期已经开启了")));
    }
    conn.execute("INSERT INTO ledger_collect_period (collect_id, period, opened_at, opened_by, deadline) VALUES (?, ?, CAST(? AS TIMESTAMP), ?, ?)", params![id, period, now_local(), user.name, deadline])?;
    Ok(())
}

/// 设置 / 修改 / 取消（传空）某一期的截止日期
pub fn set_deadline(conn: &Connection, id: &str, period: &str, deadline: Option<&str>) -> AppResult<()> {
    load_collect(conn, id)?;
    let deadline = check_deadline(deadline)?;
    let n = conn.execute("UPDATE ledger_collect_period SET deadline = ? WHERE collect_id = ? AND period = ?", params![deadline, id, period.trim()])?;
    if n == 0 {
        return Err(AppError::bad("没有这一期"));
    }
    // 截止日期变了，之前的催办记录不再适用（比如延期后要能重新催）
    conn.execute("DELETE FROM ledger_collect_remind WHERE collect_id = ? AND period = ?", params![id, period.trim()])?;
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
    // 这一期生成过的工作簿保留（里面可能有人写了东西），只是不再按期次复用
    conn.execute("DELETE FROM ledger_collect_workbook WHERE collect_id = ? AND period = ?", params![id, period])?;
    conn.execute("DELETE FROM ledger_collect_remind WHERE collect_id = ? AND period = ?", params![id, period])?;
    Ok(())
}

// ── 一键生成填报工作簿 ────────────────────────────────

/// 表头样式色（与前端填报区一致）
const HEADER_BG: &str = "#E2EFDA";
const KEY_BG: &str = "#A9D08E";

/// 一列的显示信息：(列名, 类型, 是否业务主键)
type FieldInfo = (String, String, bool);

fn gzip(json: &str) -> AppResult<Vec<u8>> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(json.as_bytes()).map_err(|e| AppError::Internal(e.into()))?;
    enc.finish().map_err(|e| AppError::Internal(e.into()))
}

/// 单元格：数字用数字，其他用文本；空值不写
fn cell(v: &Json, style: Option<&str>) -> Option<Json> {
    let (value, t) = match v {
        Json::Null => return None,
        Json::Number(n) => (Json::Number(n.clone()), 2),
        Json::String(s) if s.is_empty() => return None,
        other => (Json::String(other.as_str().map(String::from).unwrap_or_else(|| other.to_string())), 1),
    };
    let mut c = serde_json::json!({ "v": value, "t": t });
    if let Some(s) = style {
        c["s"] = Json::String(s.into());
    }
    Some(c)
}

/// 列的数据格式对应的样式 id：金额带千分位，文本列按文本（编号、单位名不会被当成数字，如 007 不变成 7）
fn column_style(kind: &str) -> Option<&'static str> {
    match kind {
        "money" => Some("money"),
        "text" | "id" => Some("text"),
        _ => None,
    }
}

/// 生成 Univer 工作簿快照：标题和说明、填报区表头（深绿色是主键）、预填的已有数据、数据区格式，
/// 填报区定义放在工作表的 `custom.inputs` 里（与前端「填报」面板存的位置和格式一致）。
///
/// 数据区的格式要**逐格**写（整列的默认样式在 Univer 里对新输入的值不生效，文本列会把 007 吃成 7），
/// 所以表头下面预留的 PREPARED_ROWS 行，每个金额 / 文本格都带上样式；已有数据的格子也带样式
fn workbook_snapshot(name: &str, title: &str, hint: &str, fields: &[FieldInfo], rows: &[Vec<Json>], spec: &Json) -> Json {
    let mut cells: BTreeMap<usize, BTreeMap<usize, Json>> = BTreeMap::new();
    let put = |cells: &mut BTreeMap<usize, BTreeMap<usize, Json>>, r: usize, c: usize, v: Option<Json>| {
        if let Some(v) = v {
            cells.entry(r).or_default().insert(c, v);
        }
    };
    put(&mut cells, 0, 0, cell(&Json::from(title), Some("title")));
    put(&mut cells, 1, 0, cell(&Json::from(hint), Some("hint")));
    for (i, (field, _, is_key)) in fields.iter().enumerate() {
        put(&mut cells, INPUT_HEADER_ROW, i, cell(&Json::from(field.as_str()), Some(if *is_key { "key" } else { "head" })));
    }
    for (ri, row) in rows.iter().enumerate() {
        for (ci, v) in row.iter().enumerate() {
            put(&mut cells, INPUT_HEADER_ROW + 1 + ri, ci, cell(v, fields.get(ci).and_then(|f| column_style(&f.1))));
        }
    }
    // 数据区预留的行：没有值的格子只写样式
    let prepared_end = INPUT_HEADER_ROW + PREPARED_ROWS.max(rows.len());
    for r in INPUT_HEADER_ROW + 1..=prepared_end {
        for (ci, (_, kind, _)) in fields.iter().enumerate() {
            if let Some(style) = column_style(kind) {
                cells.entry(r).or_default().entry(ci).or_insert_with(|| serde_json::json!({ "s": style }));
            }
        }
    }
    let cell_data: serde_json::Map<String, Json> = cells
        .into_iter()
        .map(|(r, cols)| (r.to_string(), Json::Object(cols.into_iter().map(|(c, v)| (c.to_string(), v)).collect())))
        .collect();
    let column_data: serde_json::Map<String, Json> = (0..fields.len()).map(|i| (i.to_string(), serde_json::json!({ "w": 120 }))).collect();
    serde_json::json!({
        "id": format!("wb-collect-{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f")),
        "name": name,
        "sheetOrder": ["sheet-1"],
        "styles": {
            "title": { "bl": 1, "fs": 14 },
            "hint": { "cl": { "rgb": "#6B7280" } },
            "head": { "bl": 1, "bg": { "rgb": HEADER_BG } },
            "key": { "bl": 1, "bg": { "rgb": KEY_BG } },
            "money": { "n": { "pattern": "#,##0.00" } },
            "text": { "n": { "pattern": "@" } }
        },
        "sheets": {
            "sheet-1": {
                "id": "sheet-1",
                "name": "填报",
                "rowCount": (INPUT_HEADER_ROW + PREPARED_ROWS + 2).max(INPUT_HEADER_ROW + rows.len() + 502),
                "columnCount": (fields.len() + 1).max(8),
                "defaultRowHeight": 24,
                "defaultColumnWidth": 88,
                "cellData": cell_data,
                "columnData": column_data,
                "showGridlines": 1,
                "rightToLeft": 0,
                "custom": { "inputs": [spec] }
            }
        }
    })
}

/// 工作簿名称不超过 80 个字：太长时截短任务名
fn workbook_name(task: &str, unit: &str, period: &str) -> String {
    let tail = format!(" · {unit} · {period}");
    let room = 80usize.saturating_sub(tail.chars().count());
    let head: String = task.chars().take(room.max(1)).collect();
    let name = format!("{head}{tail}");
    name.chars().take(80).collect()
}

/// 数据集里这个单位这一期已有的行（只取要填的列，数字列取数字，其他取文本）
fn existing_rows(conn: &Connection, def: &DatasetDef, fields: &[&ColumnDef], unit_col: &ColumnDef, period_col: &ColumnDef, unit: &str, period: &str) -> AppResult<Vec<Vec<Json>>> {
    let numeric: Vec<bool> = fields.iter().map(|c| matches!(c.kind, "money" | "number")).collect();
    let select: Vec<String> = fields.iter().zip(&numeric).map(|(c, n)| if *n { format!("TRY_CAST(({}) AS DOUBLE)", column_sql(c)) } else { format!("CAST(({}) AS VARCHAR)", column_sql(c)) }).collect();
    let sql = format!(
        "SELECT {} FROM {} WHERE CAST(({}) AS VARCHAR) = ? AND CAST(({}) AS VARCHAR) = ? LIMIT {PREFILL_ROWS}",
        select.join(", "),
        def.table,
        column_sql(unit_col),
        column_sql(period_col)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![unit, period], |r| {
            numeric
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    Ok(if *n { r.get::<_, Option<f64>>(i)?.map(Json::from).unwrap_or(Json::Null) } else { r.get::<_, Option<String>>(i)?.map(Json::from).unwrap_or(Json::Null) })
                })
                .collect::<duckdb::Result<Vec<Json>>>()
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenedWorkbook {
    pub workbook_id: String,
    /// 这次新生成的（false 表示打开的是之前生成的）
    pub created: bool,
}

/// 为（任务、单位、期次）生成填报工作簿，返回工作簿 id。
/// 管理员可以为任何单位生成；负责人只能为自己负责的单位生成；期次必须已开启。同一个人再点直接返回已有的
pub fn open_workbook(conn: &Connection, user: &CurrentUser, id: &str, unit: &str, period: &str) -> AppResult<OpenedWorkbook> {
    let c = load_collect(conn, id)?;
    let unit = unit.trim();
    let period = period.trim();
    let units = load_units(conn, id)?;
    let row = units.iter().find(|u| u.unit == unit).ok_or_else(|| AppError::bad(format!("「{unit}」不在收集任务「{}」的填报单位里", c.name)))?;
    if user.enforced && !user.admin && row.user_id.as_deref() != Some(user.id.as_str()) {
        return Err(AppError::Forbidden(format!("「{unit}」不是你负责的单位")));
    }
    if !load_periods(conn, id)?.iter().any(|p| p.0 == period) {
        return Err(AppError::bad(format!("「{period}」这一期没有开启")));
    }
    // 之前生成过、还没被删除的：直接用
    let known: Option<String> = conn
        .query_row("SELECT workbook_id FROM ledger_collect_workbook WHERE collect_id = ? AND period = ? AND unit = ? AND user_id = ?", params![id, period, unit, user.id], |r| r.get(0))
        .optional()?;
    if let Some(wb) = known.filter(|wb| crate::workbook::is_alive(conn, wb)) {
        return Ok(OpenedWorkbook { workbook_id: wb, created: false });
    }

    let def = dataset::dataset(conn, &c.dataset_id)?;
    let (unit_col, period_col) = match (find_col(&def, &c.unit_column), find_col(&def, &c.period_column)) {
        (Some(u), Some(p)) => (u, p),
        _ => return Err(AppError::bad("数据集里已经没有单位列或期次列，请联系管理员检查收集任务")),
    };
    // 要填的列：数据集里除单位列、期次列之外的实际列（计算列不能填）
    let fields: Vec<&ColumnDef> = def.columns.iter().filter(|col| !col.computed && col.key != unit_col.key && col.key != period_col.key).collect();
    if fields.is_empty() {
        return Err(AppError::bad("数据集里除了单位列和期次列没有别的列可填"));
    }
    let is_key = |col: &ColumnDef| def.primary_key.iter().any(|k| k == &col.title || k == &col.key);
    let info: Vec<FieldInfo> = fields.iter().map(|col| (col.title.clone(), col.kind.to_string(), is_key(col))).collect();
    let prefill = existing_rows(conn, &def, &fields, unit_col, period_col, unit, period)?;
    let spec = serde_json::json!({
        "id": format!("in_{}", chrono::Local::now().format("%y%m%d%H%M%S")),
        "name": "填报",
        "datasetId": def.id,
        "datasetName": def.name,
        "row": INPUT_HEADER_ROW,
        "col": 0,
        "fields": info.iter().map(|f| f.0.clone()).collect::<Vec<_>>(),
        "mode": "upsert",
        "keys": info.iter().filter(|f| f.2).map(|f| f.0.clone()).collect::<Vec<_>>(),
        // 固定列：不显示在表格里，提交时自动带上；服务端靠它们区分是谁、哪一期
        "fixed": { unit_col.title.clone(): unit, period_col.title.clone(): period },
    });
    let title = format!("{} · {unit} · {period}", c.name);
    let hint = format!("{}：{unit}　{}：{period}（提交时自动带上，不用在表里填）。从下面表头的下一行开始逐行填写，填完点工具栏「填报」→「提交到数据集」。", unit_col.title, period_col.title);
    let name = workbook_name(&c.name, unit, period);
    let snapshot = workbook_snapshot(&name, &title, &hint, &info, &prefill, &spec);
    let gz = gzip(&serde_json::to_string(&snapshot)?)?;
    let operator = if user.name.is_empty() { "系统" } else { user.name.as_str() };
    let wb = crate::workbook::create(conn, &name, &gz, operator)?;
    crate::workbook::set_owner(conn, &wb.id, user)?;
    conn.execute(
        "INSERT INTO ledger_collect_workbook (collect_id, period, unit, user_id, workbook_id, created_at) VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))
         ON CONFLICT (collect_id, period, unit, user_id) DO UPDATE SET workbook_id = excluded.workbook_id, created_at = excluded.created_at",
        params![id, period, unit, user.id, wb.id, now_local()],
    )?;
    Ok(OpenedWorkbook { workbook_id: wb.id, created: true })
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

/// 已确认锁定的（期次，单位）
fn locked_pairs(conn: &Connection, id: &str) -> AppResult<BTreeSet<(String, String)>> {
    let mut stmt = conn.prepare("SELECT period, unit FROM ledger_collect_review WHERE collect_id = ? AND state = 'confirmed'")?;
    Ok(stmt.query_map(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<_, _>>()?)
}

/// 写入前校验：数据集属于收集任务时，填报区必须带上单位列和期次列，期次必须已开启，单位必须在任务里，
/// 而且这个单位这一期不能已经被确认锁定（提交和按主键删除都不行）。
/// 删除（按主键）只带主键列：带了单位列和期次列就检查锁定，没带就放行。不属于任何任务的数据集直接通过
pub fn before_submit(conn: &Connection, dataset_id: &str, req: &WritebackRequest) -> AppResult<()> {
    let Some(c) = collect_of_dataset(conn, dataset_id)? else { return Ok(()) };
    let delete = req.mode == "delete";
    let pos = |title: &str| req.columns.iter().position(|t| t == title);
    let (Some(ui), Some(pi)) = (pos(&c.unit_column), pos(&c.period_column)) else {
        if delete {
            return Ok(());
        }
        return Err(AppError::bad(format!("这个数据集属于收集任务「{}」：填报区必须包含「{}」和「{}」两列", c.name, c.unit_column, c.period_column)));
    };
    if !delete {
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
    }
    let locked = locked_pairs(conn, &c.id)?;
    if !locked.is_empty() {
        let hit: BTreeSet<String> = req.rows.iter().map(|row| (cell_text(row.get(pi)), cell_text(row.get(ui)))).filter(|pair| locked.contains(pair)).map(|(p, u)| format!("{u} · {p}")).collect();
        if !hit.is_empty() {
            return Err(AppError::bad(format!("{}已经确认锁定，不能再提交或删除；需要修改请联系确认人「退回」", listed(&hit))));
        }
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
            // 退回后重新提交：回到「已提交」，等确认人再看
            conn.execute("DELETE FROM ledger_collect_review WHERE collect_id = ? AND period = ? AND unit = ? AND state = 'returned'", params![c.id, period, unit])?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        tracing::warn!("记录收集任务的提交状态失败（数据已写入）：{}", e.message());
    }
}

// ── 审核：确认锁定、退回重填 ──────────────────────────

/// 退回原因最多多少个字
const MAX_REASON_CHARS: usize = 200;

/// 管理员，或者这个任务的确认人（没开登录时不区分用户）
fn can_review(user: &CurrentUser, c: &StoredCollect) -> bool {
    !user.enforced || user.admin || c.reviewer_id.as_deref() == Some(user.id.as_str())
}

fn require_review(user: &CurrentUser, c: &StoredCollect) -> AppResult<()> {
    if can_review(user, c) { Ok(()) } else { Err(AppError::Forbidden("只有管理员或这个任务的确认人才能查看看板和审核".into())) }
}

/// 一个（期次，单位）现在的状态：confirmed 已确认（锁定）/ returned 已退回 / submitted 已提交（等确认）/ pending 还没提交
fn phase(submitted: bool, review: Option<&str>) -> &'static str {
    match (review, submitted) {
        (Some("confirmed"), _) => "confirmed",
        (Some("returned"), _) => "returned",
        (_, true) => "submitted",
        _ => "pending",
    }
}

struct Review {
    state: String,
    reason: String,
    at: String,
    by_name: String,
}

/// 审核记录，键是（期次，单位）；period 不为空时只取这一期
fn load_reviews(conn: &Connection, id: &str, period: Option<&str>) -> AppResult<HashMap<(String, String), Review>> {
    let mut stmt = conn.prepare(
        "SELECT period, unit, state, COALESCE(reason, ''), COALESCE(strftime(reviewed_at, '%Y-%m-%d %H:%M:%S'), ''), COALESCE(reviewed_by_name, '')
         FROM ledger_collect_review WHERE collect_id = ? AND (? IS NULL OR period = ?)",
    )?;
    let mut out = HashMap::new();
    for r in stmt.query_map(params![id, period, period], |r| Ok(((r.get::<_, String>(0)?, r.get::<_, String>(1)?), Review { state: r.get(2)?, reason: r.get(3)?, at: r.get(4)?, by_name: r.get(5)? })))? {
        let (k, v) = r?;
        out.insert(k, v);
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewInput {
    pub period: String,
    pub units: Vec<String>,
    /// confirm 确认并锁定 / return 退回重填（要写原因）/ reopen 撤销确认
    pub action: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReviewOutcome {
    pub changed: usize,
}

/// 确认人（或管理员）处理一期里的若干单位。一次要么全部成功，要么都不变：
/// - confirm：只能确认「已提交」的，确认后这个单位这一期锁定，不能再提交或删除
/// - return：「已提交」或「已确认」的都能退回，必须写原因；负责人收到通知，改完再提交就回到「已提交」
/// - reopen：撤销确认，回到「已提交」
pub fn review(conn: &Connection, user: &CurrentUser, id: &str, input: ReviewInput) -> AppResult<ReviewOutcome> {
    let c = load_collect(conn, id)?;
    require_review(user, &c)?;
    let action = input.action.as_str();
    if !matches!(action, "confirm" | "return" | "reopen") {
        return Err(AppError::bad("操作只能是 confirm（确认）、return（退回）、reopen（撤销确认）"));
    }
    let period = input.period.trim();
    if !load_periods(conn, id)?.iter().any(|p| p.0 == period) {
        return Err(AppError::bad(format!("「{period}」这一期没有开启")));
    }
    let reason = input.reason.as_deref().map(str::trim).unwrap_or_default();
    if action == "return" {
        if reason.is_empty() {
            return Err(AppError::bad("退回要写明原因，负责人才知道改什么"));
        }
        if reason.chars().count() > MAX_REASON_CHARS || reason.chars().any(|ch| ch.is_control() && ch != '\n') {
            return Err(AppError::bad(format!("退回原因不超过 {MAX_REASON_CHARS} 个字，不含特殊字符")));
        }
    }
    let mut wanted: Vec<String> = Vec::new();
    for u in &input.units {
        let u = u.trim().to_string();
        if !u.is_empty() && !wanted.contains(&u) {
            wanted.push(u);
        }
    }
    if wanted.is_empty() {
        return Err(AppError::bad("请选择要处理的单位"));
    }
    let units = load_units(conn, id)?;
    let submitted: BTreeSet<String> = {
        let mut stmt = conn.prepare("SELECT unit FROM ledger_collect_submit WHERE collect_id = ? AND period = ?")?;
        stmt.query_map(params![id, period], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?
    };
    let reviews = load_reviews(conn, id, Some(period))?;
    for u in &wanted {
        let Some(row) = units.iter().find(|x| &x.unit == u) else { return Err(AppError::bad(format!("「{u}」不在收集任务「{}」的填报单位里", c.name))) };
        let now = phase(submitted.contains(&row.unit), reviews.get(&(period.to_string(), row.unit.clone())).map(|r| r.state.as_str()));
        // 能处理的状态：确认 ← 已提交；退回 ← 已提交 / 已确认；撤销确认 ← 已确认。其他状态各给一句明确的话（不拼接片段，英文提示才完整）
        let refuse = match (action, now) {
            (_, "pending") => Some(format!("「{u}」这一期还没有提交，没有数据可处理")),
            (_, "returned") => Some(format!("「{u}」这一期已经退回，要等负责人重新提交后才能处理")),
            ("confirm", "confirmed") => Some(format!("「{u}」这一期已经确认锁定了")),
            ("reopen", "submitted") => Some(format!("「{u}」这一期正在等确认，没有可撤销的确认")),
            _ => None,
        };
        if let Some(msg) = refuse {
            return Err(AppError::bad(msg));
        }
    }
    let who = if user.name.is_empty() { "系统" } else { user.name.as_str() };
    in_tx(conn, || {
        for u in &wanted {
            if action == "reopen" {
                conn.execute("DELETE FROM ledger_collect_review WHERE collect_id = ? AND period = ? AND unit = ?", params![id, period, u])?;
                continue;
            }
            let (state, why) = if action == "confirm" { ("confirmed", None) } else { ("returned", Some(reason)) };
            conn.execute(
                "INSERT INTO ledger_collect_review (collect_id, period, unit, state, reason, reviewed_at, reviewed_by, reviewed_by_name) VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?, ?)
                 ON CONFLICT (collect_id, period, unit) DO UPDATE SET state = excluded.state, reason = excluded.reason, reviewed_at = excluded.reviewed_at, reviewed_by = excluded.reviewed_by, reviewed_by_name = excluded.reviewed_by_name",
                params![id, period, u, state, why, now_local(), user.id, who],
            )?;
            // 通知负责人：被退回要去改，被确认说明已经锁定
            let Some(owner) = units.iter().find(|x| &x.unit == u).and_then(|x| x.user_id.as_deref()) else { continue };
            let text = if action == "confirm" { format!("「{u}」{period} 已确认，数据已锁定") } else { format!("「{u}」{period} 被退回：{reason}") };
            let kind = if action == "confirm" { "collect_confirmed" } else { "collect_returned" };
            comment::push(conn, Notice { to: owner, kind, workbook_id: "", workbook_name: &c.name, from: who, text: &text, ref_id: &format!("collect:{id}") }, &user.id)?;
        }
        Ok(())
    })?;
    Ok(ReviewOutcome { changed: wanted.len() })
}

// ── 催办 ──────────────────────────────────────────────

/// 手动催办：同一个单位这么久之内不重复催
const REMIND_GAP_MINUTES: i64 = 60;
/// 自动催办每天从几点开始
const AUTO_FROM_HOUR: u32 = 9;
/// 逾期后最多自动催几天
const AUTO_OVERDUE_DAYS: i64 = 7;
/// 推到群里的名单最多列几个单位
const GROUP_LISTED: usize = 40;
/// `ledger_collect_remind` 里整期推到群里的那一条的 unit
const WHOLE_PERIOD: &str = "*";

/// 某一期里每个单位上次被催的时间和次数
fn load_reminds(conn: &Connection, id: &str, period: &str) -> AppResult<HashMap<String, (String, i64)>> {
    let mut stmt = conn.prepare("SELECT unit, COALESCE(strftime(last_at, '%Y-%m-%d %H:%M:%S'), ''), times FROM ledger_collect_remind WHERE collect_id = ? AND period = ?")?;
    let mut out = HashMap::new();
    for r in stmt.query_map(params![id, period], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, r.get::<_, i64>(2)?))))? {
        let (k, v) = r?;
        out.insert(k, v);
    }
    Ok(out)
}

/// 刚催过吗：手动看间隔，自动看是不是今天已经催过
fn reminded_recently(last: Option<&(String, i64)>, now: NaiveDateTime, auto: bool) -> bool {
    let Some(t) = last.and_then(|l| NaiveDateTime::parse_from_str(&l.0, "%Y-%m-%d %H:%M:%S").ok()) else { return false };
    if auto { t.date() == now.date() } else { (now - t).num_minutes() < REMIND_GAP_MINUTES }
}

fn mark_reminded(conn: &Connection, id: &str, period: &str, unit: &str, now: NaiveDateTime) -> AppResult<()> {
    conn.execute(
        "INSERT INTO ledger_collect_remind (collect_id, period, unit, last_at, times) VALUES (?, ?, ?, CAST(? AS TIMESTAMP), 1)
         ON CONFLICT (collect_id, period, unit) DO UPDATE SET last_at = excluded.last_at, times = ledger_collect_remind.times + 1",
        params![id, period, unit, now.format("%Y-%m-%d %H:%M:%S").to_string()],
    )?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemindInput {
    pub period: String,
    /// 只催这些单位；不写就催这一期所有还没提交的
    #[serde(default)]
    pub units: Option<Vec<String>>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RemindOutcome {
    /// 发了站内通知的负责人数（单位数）
    pub notified: usize,
    /// 刚催过、这次跳过的单位数
    pub skipped: usize,
    /// 没有指派负责人、无处可催的单位数（它们只出现在推送到群的名单里）
    pub unassigned: usize,
    /// 推送到群 / 邮箱的结果（没配渠道、或刚推过则为空）
    pub channels: Vec<ChannelResult>,
}

#[derive(Debug)]
struct RemindPlan {
    outcome: RemindOutcome,
    /// 还没提交的单位：(单位, 负责人姓名, 是不是被退回的)
    open_units: Vec<(String, Option<String>, bool)>,
    push_group: bool,
    note: String,
}

/// 催办的本地部分：挑出要催的单位，给负责人发站内通知并记录。要不要推到群由调用方接着做
fn plan_remind(conn: &Connection, c: &StoredCollect, actor: &CurrentUser, period: &str, wanted: Option<&[String]>, auto: bool, now: NaiveDateTime) -> AppResult<RemindPlan> {
    if !load_periods(conn, &c.id)?.iter().any(|p| p.0 == period) {
        return Err(AppError::bad(format!("「{period}」这一期没有开启")));
    }
    let units = load_units(conn, &c.id)?;
    if let Some(w) = wanted
        && let Some(bad) = w.iter().find(|u| !units.iter().any(|x| &x.unit == *u))
    {
        return Err(AppError::bad(format!("「{bad}」不在收集任务「{}」的填报单位里", c.name)));
    }
    let submitted: BTreeSet<String> = {
        let mut stmt = conn.prepare("SELECT unit FROM ledger_collect_submit WHERE collect_id = ? AND period = ?")?;
        stmt.query_map(params![c.id, period], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?
    };
    let reviews = load_reviews(conn, &c.id, Some(period))?;
    let reminds = load_reminds(conn, &c.id, period)?;
    let deadlines = load_deadlines(conn, &c.id)?;
    let note = deadline_note(deadlines.get(period), now.date());
    let who = if actor.name.is_empty() { "系统" } else { actor.name.as_str() };

    let mut plan = RemindPlan { outcome: RemindOutcome { notified: 0, skipped: 0, unassigned: 0, channels: vec![] }, open_units: vec![], push_group: false, note: note.clone() };
    for u in &units {
        if wanted.is_some_and(|w| !w.contains(&u.unit)) {
            continue;
        }
        let now_phase = phase(submitted.contains(&u.unit), reviews.get(&(period.to_string(), u.unit.clone())).map(|r| r.state.as_str()));
        if !matches!(now_phase, "pending" | "returned") {
            continue;
        }
        let returned = now_phase == "returned";
        plan.open_units.push((u.unit.clone(), u.user_name.clone(), returned));
        let Some(owner) = u.user_id.as_deref() else {
            plan.outcome.unassigned += 1;
            continue;
        };
        if reminded_recently(reminds.get(&u.unit), now, auto) {
            plan.outcome.skipped += 1;
            continue;
        }
        let text = if returned { format!("「{}」{period} 被退回，请修改后重新提交{note}", u.unit) } else { format!("「{}」{period} 还没提交{note}", u.unit) };
        comment::push(conn, Notice { to: owner, kind: "collect_remind", workbook_id: "", workbook_name: &c.name, from: who, text: &text, ref_id: &format!("collect:{}", c.id) }, &actor.id)?;
        mark_reminded(conn, &c.id, period, &u.unit, now)?;
        plan.outcome.notified += 1;
    }
    plan.push_group = !plan.open_units.is_empty() && !c.remind_channels.is_empty() && !reminded_recently(reminds.get(WHOLE_PERIOD), now, auto);
    Ok(plan)
}

/// 推到群里的内容：没提交的名单（一张小表）
fn group_message(c: &StoredCollect, period: &str, plan: &RemindPlan) -> report::SmallTable {
    let n = plan.open_units.len();
    let rows = plan
        .open_units
        .iter()
        .take(GROUP_LISTED)
        .map(|(unit, owner, returned)| vec![unit.clone(), owner.clone().unwrap_or_else(|| "未指派".into()), if *returned { "被退回，待修改" } else { "未提交" }.to_string()])
        .collect();
    let mut notes = vec!["请到 Gridlake「首页 → 我的填报」里填报".to_string()];
    if n > GROUP_LISTED {
        notes.push(format!("名单只列出前 {GROUP_LISTED} 个，共 {n} 个"));
    }
    if let Some(d) = plan.note.strip_prefix('，') {
        notes.insert(0, d.to_string());
    }
    report::SmallTable {
        title: format!("还有 {n} 个单位没有提交"),
        heading: c.name.clone(),
        context: period.to_string(),
        notes,
        header: vec![c.unit_column.clone(), "负责人".into(), "状态".into()],
        rows,
    }
}

/// 催办：给还没提交（含被退回没改好）的单位的负责人发站内通知，任务里配了渠道的还把名单推到群 / 邮箱。
/// 手动（管理员 / 确认人）同一个单位 1 小时内不重复催；自动（auto）一天最多催一次
pub async fn remind(state: &AppState, user: &CurrentUser, id: &str, period: &str, units: Option<Vec<String>>, auto: bool) -> AppResult<RemindOutcome> {
    let (st, user_c, id_c, period_c) = (state.clone(), user.clone(), id.to_string(), period.trim().to_string());
    let (mut plan, channels, collect) = tokio::task::spawn_blocking(move || -> AppResult<_> {
        let conn = st.connection()?;
        let c = load_collect(&conn, &id_c)?;
        if !auto {
            require_review(&user_c, &c)?;
        }
        let now = chrono::Local::now().naive_local();
        let plan = in_tx(&conn, || plan_remind(&conn, &c, &user_c, &period_c, units.as_deref(), auto, now))?;
        Ok((plan, c.remind_channels.clone(), c))
    })
    .await??;
    if plan.open_units.is_empty() && !auto {
        return Err(AppError::bad("这一期没有要催的单位（都已提交）"));
    }
    if plan.push_group {
        plan.outcome.channels = report::push_table(state, &channels, group_message(&collect, period.trim(), &plan)).await?;
        // 不管发没发成功都记一笔：发不出去的渠道不该每次检查都重试
        let (st, id_c, period_c) = (state.clone(), id.to_string(), period.trim().to_string());
        tokio::task::spawn_blocking(move || -> AppResult<()> { mark_reminded(&st.connection()?, &id_c, &period_c, WHOLE_PERIOD, chrono::Local::now().naive_local()) }).await??;
    }
    Ok(plan.outcome)
}

/// 现在该自动催办的（任务，期次）：设了截止日期、9 点以后，截止前一天 / 当天 / 逾期 7 天内
fn due_reminders(conn: &Connection, now: NaiveDateTime) -> AppResult<Vec<(String, String)>> {
    let mut out = Vec::new();
    if now.hour() < AUTO_FROM_HOUR {
        return Ok(out);
    }
    for c in load_collects(conn)? {
        for (period, deadline) in load_deadlines(conn, &c.id)? {
            if days_left(Some(&deadline), now.date()).is_some_and(|n| (-AUTO_OVERDUE_DAYS..=1).contains(&n)) {
                out.push((c.id.clone(), period));
            }
        }
    }
    Ok(out)
}

/// 每 10 分钟看一次有没有该自动催办的期次
pub async fn schedule(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    loop {
        tick.tick().await;
        let st = state.clone();
        let due = tokio::task::spawn_blocking(move || -> AppResult<_> { due_reminders(&st.connection()?, chrono::Local::now().naive_local()) }).await.unwrap_or_else(|e| Err(AppError::Internal(e.into())));
        match due {
            Ok(due) => {
                for (id, period) in due {
                    match remind(&state, &CurrentUser::open(), &id, &period, None, true).await {
                        Ok(o) if o.notified > 0 || !o.channels.is_empty() => tracing::info!("自动催办收集任务 {id} 的「{period}」：通知 {} 人，推送 {} 个渠道", o.notified, o.channels.len()),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("自动催办收集任务 {id} 的「{period}」失败：{}", e.message()),
                    }
                }
            }
            Err(e) => tracing::warn!("检查自动催办失败：{}", e.message()),
        }
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
    /// 最近一期及它的提交进度（已提交含已确认，不含已退回）
    pub latest_period: Option<String>,
    pub latest_submitted: i64,
    pub latest_confirmed: i64,
    /// 最近一期的截止日期，以及到期没提交的单位数
    pub latest_deadline: Option<String>,
    pub latest_overdue: i64,
    /// 催办推送的渠道 id
    pub remind_channels: Vec<String>,
    /// 确认人（没指定就只有管理员审核）
    pub reviewer_id: Option<String>,
    pub reviewer_name: Option<String>,
    pub created_by_name: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 某一期里 (已提交, 已确认) 的单位数（只算任务里现有的单位）；已提交含已确认，不含已退回
fn progress(conn: &Connection, id: &str, period: &str) -> AppResult<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT count(*) FILTER (WHERE COALESCE(r.state, '') <> 'returned'), count(*) FILTER (WHERE r.state = 'confirmed')
         FROM ledger_collect_submit s
         JOIN ledger_collect_unit u ON u.collect_id = s.collect_id AND u.unit = s.unit
         LEFT JOIN ledger_collect_review r ON r.collect_id = s.collect_id AND r.period = s.period AND r.unit = s.unit
         WHERE s.collect_id = ? AND s.period = ?",
        params![id, period],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

fn view(conn: &Connection, c: &StoredCollect, names: &HashMap<String, String>) -> AppResult<CollectView> {
    let (unit_count, assigned_count): (i64, i64) =
        conn.query_row("SELECT count(*), count(user_id) FROM ledger_collect_unit WHERE collect_id = ?", params![c.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let periods = load_periods(conn, &c.id)?;
    let latest = periods.first().map(|p| p.0.clone());
    let (latest_submitted, latest_confirmed) = match &latest {
        Some(p) => progress(conn, &c.id, p)?,
        None => (0, 0),
    };
    let deadline = latest.as_ref().and_then(|p| load_deadlines(conn, &c.id).ok()?.remove(p));
    let latest_overdue = if is_overdue(deadline.as_ref(), today()) { (unit_count - latest_submitted).max(0) } else { 0 };
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
        latest_submitted,
        latest_confirmed,
        latest_deadline: deadline,
        latest_overdue,
        remind_channels: c.remind_channels.clone(),
        latest_period: latest,
        reviewer_id: c.reviewer_id.clone(),
        reviewer_name: c.reviewer_name.clone(),
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
    /// pending 还没提交 / submitted 已提交（等确认）/ returned 已退回 / confirmed 已确认（锁定）
    pub status: &'static str,
    pub submitted_at: Option<String>,
    pub submitted_by_name: Option<String>,
    /// 退回原因（只有已退回的有）
    pub return_reason: Option<String>,
    /// 确认 / 退回的时间和人
    pub reviewed_at: Option<String>,
    pub reviewed_by_name: Option<String>,
    /// 数据集里这个单位这一期现在有几行
    pub rows: i64,
    /// 过了截止日期还没提交（含被退回没改好的）
    pub overdue: bool,
    /// 上次被催办的时间（没催过为空）
    pub reminded_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodSummary {
    pub period: String,
    pub opened_at: String,
    pub total: i64,
    /// 已提交（含已确认，不含已退回）
    pub submitted: i64,
    pub confirmed: i64,
    /// 截止日期（没设为空）和到期没提交的单位数
    pub deadline: Option<String>,
    pub overdue: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub collect: CollectView,
    /// 能不能开启 / 删除期次（只有管理员；确认人只能审核）
    pub can_manage: bool,
    /// 已开启的期次，新的在前
    pub periods: Vec<PeriodSummary>,
    /// 下面 units 是哪一期的状态（没传就是最近一期；还没有任何期次时为空）
    pub period: Option<String>,
    pub units: Vec<UnitStatus>,
}

/// 看板：某一期里每个单位的状态。管理员和这个任务的确认人能看
pub fn detail(conn: &Connection, user: &CurrentUser, id: &str, period: Option<&str>) -> AppResult<Detail> {
    let c = load_collect(conn, id)?;
    require_review(user, &c)?;
    let units = load_units(conn, id)?;
    let opened = load_periods(conn, id)?;
    let chosen = match period.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) if opened.iter().any(|o| o.0 == p) => Some(p.to_string()),
        Some(p) => return Err(AppError::bad(format!("「{p}」这一期没有开启"))),
        None => opened.first().map(|o| o.0.clone()),
    };
    let deadlines = load_deadlines(conn, id)?;
    let today = today();
    let mut periods = Vec::with_capacity(opened.len());
    for (p, at) in &opened {
        let (submitted, confirmed) = progress(conn, id, p)?;
        let total = units.len() as i64;
        let overdue = if is_overdue(deadlines.get(p), today) { (total - submitted).max(0) } else { 0 };
        periods.push(PeriodSummary { period: p.clone(), opened_at: at.clone(), total, submitted, confirmed, deadline: deadlines.get(p).cloned(), overdue });
    }
    let rows = match &chosen {
        Some(p) => {
            let mut sub: HashMap<String, (String, String)> = HashMap::new();
            let mut stmt = conn.prepare("SELECT unit, strftime(submitted_at, '%Y-%m-%d %H:%M:%S'), submitted_by_name FROM ledger_collect_submit WHERE collect_id = ? AND period = ?")?;
            for r in stmt.query_map(params![id, p], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, Option<String>>(2)?.unwrap_or_default())))? {
                let (u, at, by) = r?;
                sub.insert(u, (at, by));
            }
            let reviews = load_reviews(conn, id, Some(p))?;
            let reminds = load_reminds(conn, id, p)?;
            let past = is_overdue(deadlines.get(p), today);
            let counts = row_counts(conn, &c, p);
            units
                .into_iter()
                .map(|u| {
                    let s = sub.get(&u.unit);
                    let rv = reviews.get(&(p.clone(), u.unit.clone()));
                    let status = phase(s.is_some(), rv.map(|r| r.state.as_str()));
                    UnitStatus {
                        rows: counts.get(&u.unit).copied().unwrap_or(0),
                        overdue: past && matches!(status, "pending" | "returned"),
                        reminded_at: reminds.get(&u.unit).map(|r| r.0.clone()),
                        status,
                        submitted_at: s.map(|s| s.0.clone()),
                        submitted_by_name: s.map(|s| s.1.clone()),
                        return_reason: rv.filter(|r| r.state == "returned").map(|r| r.reason.clone()),
                        reviewed_at: rv.map(|r| r.at.clone()),
                        reviewed_by_name: rv.map(|r| r.by_name.clone()),
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
    Ok(Detail { collect: view(conn, &c, &names)?, can_manage: !user.enforced || user.admin, periods, period: chosen, units: rows })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyPeriod {
    pub period: String,
    /// pending / submitted / returned / confirmed，含义同 `UnitStatus`
    pub status: &'static str,
    pub submitted_at: Option<String>,
    /// 退回原因（已退回的才有）
    pub return_reason: Option<String>,
    pub rows: i64,
    /// 这一期的截止日期（没设为空）；过了截止日期还没提交（含被退回没改好的）为 true
    pub deadline: Option<String>,
    pub overdue: bool,
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
    /// 还要我动手的（单位 × 已开启的期次）个数：还没填的，加上被退回的
    pub pending: i64,
    /// 其中已经逾期的
    pub overdue: i64,
    /// 我是不是这个任务的确认人
    pub reviewer: bool,
    /// 确认人姓名（没指定就是管理员）
    pub reviewer_name: Option<String>,
    /// 我是确认人时：最近几期里已提交、等我确认的（单位 × 期次）个数
    pub to_review: i64,
}

/// 我的填报：我负责的单位在最近几期里填了没有（含被退回的），我是确认人的任务还有待确认的数量。没开登录时不区分用户，列出全部
pub fn mine(conn: &Connection, user: &CurrentUser) -> AppResult<Vec<MyTask>> {
    let names = dataset_names(conn);
    let mut out = Vec::new();
    for c in load_collects(conn)? {
        let is_reviewer = !user.enforced || (c.reviewer_id.is_some() && c.reviewer_id.as_deref() == Some(user.id.as_str()));
        let units: Vec<UnitRow> = load_units(conn, &c.id)?.into_iter().filter(|u| !user.enforced || (u.user_id.is_some() && u.user_id.as_deref() == Some(user.id.as_str()))).collect();
        if units.is_empty() && !is_reviewer {
            continue;
        }
        let periods: Vec<String> = load_periods(conn, &c.id)?.into_iter().take(MINE_PERIODS).map(|p| p.0).collect();
        let mut sub: HashMap<(String, String), String> = HashMap::new();
        let mut stmt = conn.prepare("SELECT period, unit, strftime(submitted_at, '%Y-%m-%d %H:%M:%S') FROM ledger_collect_submit WHERE collect_id = ?")?;
        for r in stmt.query_map(params![c.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))? {
            let (p, u, at) = r?;
            sub.insert((p, u), at);
        }
        let reviews = load_reviews(conn, &c.id, None)?;
        let deadlines = load_deadlines(conn, &c.id)?;
        let today = today();
        let counts: HashMap<&String, HashMap<String, i64>> = periods.iter().map(|p| (p, row_counts(conn, &c, p))).collect();
        let (mut pending, mut overdue_n) = (0, 0);
        let my_units = units
            .into_iter()
            .map(|u| MyUnit {
                periods: periods
                    .iter()
                    .map(|p| {
                        let at = sub.get(&(p.clone(), u.unit.clone()));
                        let rv = reviews.get(&(p.clone(), u.unit.clone()));
                        let status = phase(at.is_some(), rv.map(|r| r.state.as_str()));
                        let open = matches!(status, "pending" | "returned");
                        let overdue = open && is_overdue(deadlines.get(p), today);
                        if open {
                            pending += 1;
                        }
                        if overdue {
                            overdue_n += 1;
                        }
                        MyPeriod {
                            deadline: deadlines.get(p).cloned(),
                            overdue,
                            period: p.clone(),
                            status,
                            submitted_at: at.cloned(),
                            return_reason: rv.filter(|r| r.state == "returned").map(|r| r.reason.clone()),
                            rows: counts.get(p).and_then(|m| m.get(&u.unit)).copied().unwrap_or(0),
                        }
                    })
                    .collect(),
                unit: u.unit,
            })
            .collect();
        let mut to_review = 0;
        if is_reviewer {
            for p in &periods {
                let (submitted, confirmed) = progress(conn, &c.id, p)?;
                to_review += submitted - confirmed;
            }
        }
        out.push(MyTask {
            dataset_name: names.get(&c.dataset_id).cloned(),
            id: c.id,
            name: c.name,
            dataset_id: c.dataset_id,
            unit_column: c.unit_column,
            period_column: c.period_column,
            units: my_units,
            pending,
            overdue: overdue_n,
            reviewer: is_reviewer,
            reviewer_name: c.reviewer_name,
            to_review,
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
        CollectInput { name: name.into(), dataset_id: None, new_table: Some(table(&format!("{name}表"))), unit_column: None, period_column: None, dimension: None, reviewer_id: None, remind_channels: vec![], units }
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
        update(&conn, &me, &id, UpdateInput { name: "月度经营".into(), reviewer_id: None, remind_channels: vec![], units: vec![unit("店铺A", Some("u2")), unit("店铺B", Some("u2"))] }).unwrap();
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
        let existing = |id: String| CollectInput { name: "t".into(), dataset_id: Some(id), new_table: None, unit_column: None, period_column: None, dimension: None, reviewer_id: None, remind_channels: vec![], units: vec![unit("A", None)] };
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

        open_period(&conn, &me, &id, "2026-10", None).unwrap();
        assert!(open_period(&conn, &me, &id, "2026-10", None).unwrap_err().message().contains("已经开启"));
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
        let d = detail(&conn, &me, &id, None).unwrap();
        assert_eq!(d.period.as_deref(), Some("2026-10"), "默认看最近一期");
        let by = |u: &str| d.units.iter().find(|x| x.unit == u).unwrap();
        assert_eq!((by("店铺A").status, by("店铺A").submitted_by_name.as_deref(), by("店铺A").rows), ("submitted", Some("Bob"), 1));
        assert_eq!((by("店铺B").status, by("店铺B").rows), ("pending", 0));
        assert_eq!((d.periods[0].total, d.periods[0].submitted), (2, 1));
        assert_eq!((d.collect.latest_period.as_deref(), d.collect.latest_submitted, d.collect.unit_count, d.collect.assigned_count), (Some("2026-10"), 1, 2, 1));
        assert!(detail(&conn, &me, &id, Some("2099-01")).is_err());

        // 我的填报：Bob 只看到自己的单位；没有任务的人什么都没有
        let mine_bob = mine(&conn, &bob).unwrap();
        assert_eq!(mine_bob.len(), 1);
        assert_eq!((mine_bob[0].units.len(), mine_bob[0].pending), (1, 0));
        let nobody = CurrentUser { id: "u9".into(), name: "X".into(), admin: false, enforced: true };
        assert!(mine(&conn, &nobody).unwrap().is_empty());
        assert_eq!(mine(&conn, &CurrentUser::open()).unwrap()[0].units.len(), 2, "没开登录时列出全部");

        // 删期次：有人提交过的不能删；没人提交的可以
        assert!(delete_period(&conn, &id, "2026-10").unwrap_err().message().contains("不能删除"));
        open_period(&conn, &me, &id, "2026-11", None).unwrap();
        delete_period(&conn, &id, "2026-11").unwrap();
        assert!(delete_period(&conn, &id, "2026-11").is_err());
    }

    #[test]
    fn review_confirms_locks_and_returns() {
        let conn = setup();
        add_user(&conn, "u1", "Bob");
        add_user(&conn, "u2", "Carol");
        add_user(&conn, "u3", "Dave");
        let me = admin();
        let as_user = |id: &str, name: &str| CurrentUser { id: id.into(), name: name.into(), admin: false, enforced: true };
        let (bob, carol, dave) = (as_user("u1", "Bob"), as_user("u2", "Carol"), as_user("u3", "Dave"));

        // 确认人不能同时是负责人（看不到全部数据就没法确认）；不存在的用户不行
        let mut clash = new_input("冲突", vec![unit("店铺A", Some("u1")), unit("店铺B", Some("u3"))]);
        clash.reviewer_id = Some("u3".into());
        let e = create(&conn, &me, clash).unwrap_err();
        assert!(e.message().contains("同时是填报单位「店铺B」的负责人"), "{}", e.message());
        let mut ghost = new_input("没人", vec![unit("店铺A", None)]);
        ghost.reviewer_id = Some("nobody".into());
        assert!(create(&conn, &me, ghost).unwrap_err().message().contains("确认人还没有登录过系统"));
        assert_eq!(load_collects(&conn).unwrap().len(), 0, "失败不留下任务");

        let mut input = new_input("月度", vec![unit("店铺A", Some("u1")), unit("店铺B", Some("u2")), unit("店铺C", None)]);
        input.reviewer_id = Some("u3".into());
        let id = create(&conn, &me, input).unwrap();
        let ds = load_collect(&conn, &id).unwrap().dataset_id;
        open_period(&conn, &me, &id, "2026-10", None).unwrap();
        let cols = ["填报单位", "期次", "营收"];
        let row = |u: &str, v: i64| vec![Json::from(u), Json::from("2026-10"), Json::from(v)];
        let submit = |u: &str, v: i64, who: &CurrentUser| {
            let r = req("upsert", &cols, vec![row(u, v)]);
            before_submit(&conn, &ds, &r)?;
            let out = crate::writeback::submit(&conn, &ds, &r)?;
            after_submit(&conn, &ds, &r, &out, who);
            Ok::<_, AppError>(())
        };
        let go = |who: &CurrentUser, action: &str, units: &[&str], reason: Option<&str>| {
            review(&conn, who, &id, ReviewInput { period: "2026-10".into(), units: units.iter().map(|u| u.to_string()).collect(), action: action.into(), reason: reason.map(String::from) })
        };
        let status = |u: &str| detail(&conn, &me, &id, None).unwrap().units.into_iter().find(|x| x.unit == u).unwrap().status;

        // 权限：负责人和外人不能看板、不能审核；确认人和管理员可以
        assert!(detail(&conn, &bob, &id, None).is_err());
        assert!(go(&bob, "confirm", &["店铺A"], None).unwrap_err().message().contains("只有管理员或这个任务的确认人"));
        assert!(detail(&conn, &dave, &id, None).is_ok());
        assert!(!detail(&conn, &dave, &id, None).unwrap().can_manage && detail(&conn, &me, &id, None).unwrap().can_manage);

        // 没提交的不能确认 / 退回
        assert!(go(&dave, "confirm", &["店铺A"], None).unwrap_err().message().contains("还没有提交"));
        assert!(go(&dave, "return", &["店铺A"], Some("x")).unwrap_err().message().contains("还没有提交"));
        submit("店铺A", 100, &bob).unwrap();
        submit("店铺B", 200, &carol).unwrap();
        assert_eq!((status("店铺A"), status("店铺C")), ("submitted", "pending"));

        // 确认人看得到待确认的数量
        let m = mine(&conn, &dave).unwrap();
        assert_eq!((m.len(), m[0].reviewer, m[0].to_review, m[0].units.len()), (1, true, 2, 0));
        assert_eq!(mine(&conn, &bob).unwrap()[0].to_review, 0);

        // 单位不存在 / 期次没开启 / 动作不对 / 没选单位
        assert!(go(&dave, "confirm", &["店铺Z"], None).unwrap_err().message().contains("不在收集任务"));
        assert!(go(&dave, "confirm", &[], None).is_err());
        assert!(go(&dave, "lock", &["店铺A"], None).is_err());
        let wrong_period = review(&conn, &dave, &id, ReviewInput { period: "2099-01".into(), units: vec!["店铺A".into()], action: "confirm".into(), reason: None });
        assert!(wrong_period.unwrap_err().message().contains("没有开启"));

        // 批量确认：有一个不行就整体不变
        assert!(go(&dave, "confirm", &["店铺A", "店铺C"], None).is_err());
        assert_eq!(status("店铺A"), "submitted");
        assert_eq!(go(&dave, "confirm", &["店铺A", "店铺B"], None).unwrap().changed, 2);
        assert_eq!((status("店铺A"), status("店铺B")), ("confirmed", "confirmed"));
        let d = detail(&conn, &me, &id, None).unwrap();
        assert_eq!((d.periods[0].submitted, d.periods[0].confirmed, d.collect.latest_confirmed), (2, 2, 2));
        assert_eq!(mine(&conn, &dave).unwrap()[0].to_review, 0);
        assert!(go(&dave, "confirm", &["店铺A"], None).unwrap_err().message().contains("已经确认锁定"));

        // 锁定后：提交和按主键删除都被拒；别的单位不受影响
        let e = submit("店铺A", 999, &bob).unwrap_err();
        assert!(e.message().contains("店铺A · 2026-10") && e.message().contains("已经确认锁定"), "{}", e.message());
        let e = before_submit(&conn, &ds, &req("delete", &cols, vec![row("店铺A", 100)])).unwrap_err();
        assert!(e.message().contains("已经确认锁定"), "{}", e.message());
        assert!(before_submit(&conn, &ds, &req("delete", &["营收"], vec![vec![Json::from(1)]])).is_ok(), "删除没带单位列和期次列就无从判断，放行");
        submit("店铺C", 300, &me).unwrap();
        assert_eq!(status("店铺C"), "submitted");

        // 退回：要写原因；已确认的也能退回；负责人收到通知；重新提交后回到已提交
        assert!(go(&dave, "return", &["店铺A"], None).unwrap_err().message().contains("原因"));
        assert!(go(&dave, "return", &["店铺A"], Some("  ")).is_err());
        assert!(go(&dave, "return", &["店铺A"], Some(&"错".repeat(201))).is_err());
        go(&dave, "return", &["店铺A"], Some("营收口径不对，按含税填")).unwrap();
        let a = detail(&conn, &me, &id, None).unwrap().units.into_iter().find(|x| x.unit == "店铺A").unwrap();
        assert_eq!((a.status, a.return_reason.as_deref(), a.reviewed_by_name.as_deref()), ("returned", Some("营收口径不对，按含税填"), Some("Dave")));
        assert!(go(&dave, "return", &["店铺A"], Some("再来")).unwrap_err().message().contains("已经退回"));
        let d = detail(&conn, &me, &id, None).unwrap();
        assert_eq!((d.periods[0].submitted, d.periods[0].confirmed), (2, 1), "已退回的不算已提交");
        let m = mine(&conn, &bob).unwrap();
        assert_eq!((m[0].pending, m[0].units[0].periods[0].status, m[0].units[0].periods[0].return_reason.as_deref()), (1, "returned", Some("营收口径不对，按含税填")));
        let note: (String, String, String) = conn
            .query_row("SELECT kind, from_name, text FROM ledger_notification WHERE user_id = 'u1' AND kind = 'collect_returned'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((note.1.as_str(), note.2.contains("店铺A") && note.2.contains("按含税填")), ("Dave", true));
        let confirmed_notes: i64 = conn.query_row("SELECT count(*) FROM ledger_notification WHERE kind = 'collect_confirmed'", [], |r| r.get(0)).unwrap();
        assert_eq!(confirmed_notes, 2, "两个负责人各收到一条确认通知");
        submit("店铺A", 120, &bob).unwrap();
        assert_eq!(status("店铺A"), "submitted");
        assert_eq!(conn.query_row("SELECT count(*) FROM ledger_collect_review WHERE unit = '店铺A'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);

        // 撤销确认：回到已提交，又能改了
        assert!(go(&dave, "reopen", &["店铺A"], None).unwrap_err().message().contains("正在等确认"));
        go(&dave, "reopen", &["店铺B"], None).unwrap();
        assert_eq!(status("店铺B"), "submitted");
        submit("店铺B", 250, &carol).unwrap();

        // 管理员也能审核；没开登录时谁都能；删除任务连审核记录一起删
        go(&me, "confirm", &["店铺C"], None).unwrap();
        assert_eq!(status("店铺C"), "confirmed");
        assert!(go(&CurrentUser::open(), "reopen", &["店铺C"], None).is_ok());
        go(&me, "confirm", &["店铺C"], None).unwrap();
        delete(&conn, &me, &id).unwrap();
        assert_eq!(conn.query_row("SELECT count(*) FROM ledger_collect_review", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }

    #[test]
    fn reviewer_can_be_changed_and_must_not_clash_with_new_assignees() {
        let conn = setup();
        add_user(&conn, "u1", "Bob");
        add_user(&conn, "u3", "Dave");
        let me = admin();
        let mut input = new_input("月度", vec![unit("店铺A", Some("u1"))]);
        input.reviewer_id = Some("u3".into());
        let id = create(&conn, &me, input).unwrap();
        let view_of = || list(&conn).unwrap().remove(0);
        assert_eq!(view_of().reviewer_name.as_deref(), Some("Dave"));
        // 把确认人设成现有负责人：被拒绝，原来的确认人不变
        let bad = UpdateInput { name: "月度".into(), reviewer_id: Some("u1".into()), remind_channels: vec![], units: vec![unit("店铺A", Some("u1"))] };
        assert!(update(&conn, &me, &id, bad).unwrap_err().message().contains("同时是填报单位"));
        assert_eq!(view_of().reviewer_id.as_deref(), Some("u3"));
        // 把确认人指派成负责人：同样被拒绝
        let bad = UpdateInput { name: "月度".into(), reviewer_id: Some("u3".into()), remind_channels: vec![], units: vec![unit("店铺A", Some("u3"))] };
        assert!(update(&conn, &me, &id, bad).is_err());
        // 取消确认人
        update(&conn, &me, &id, UpdateInput { name: "月度".into(), reviewer_id: None, remind_channels: vec![], units: vec![unit("店铺A", Some("u1"))] }).unwrap();
        assert_eq!(view_of().reviewer_id, None);
        // 没有确认人时，只有管理员能审核
        let dave = CurrentUser { id: "u3".into(), name: "Dave".into(), admin: false, enforced: true };
        assert!(detail(&conn, &dave, &id, None).is_err());
        assert!(mine(&conn, &dave).unwrap().is_empty());
    }

    #[test]
    fn deadlines_overdue_and_reminders() {
        let conn = setup();
        add_user(&conn, "u1", "Bob");
        add_user(&conn, "u2", "Carol");
        let me = admin();
        let bob = CurrentUser { id: "u1".into(), name: "Bob".into(), admin: false, enforced: true };
        let id = create(&conn, &me, new_input("月度", vec![unit("店铺A", Some("u1")), unit("店铺B", Some("u2")), unit("店铺C", None)])).unwrap();
        let today = chrono::Local::now().date_naive();
        let fmt = |d: i64| (today + chrono::Duration::days(d)).format("%Y-%m-%d").to_string();

        // 截止日期的格式和渠道要校验
        assert!(open_period(&conn, &me, &id, "P0", Some("2026/10/20")).unwrap_err().message().contains("年-月-日"));
        assert!(load_periods(&conn, &id).unwrap().is_empty(), "格式不对不开启");
        open_period(&conn, &me, &id, "P1", Some(&fmt(-2))).unwrap();
        open_period(&conn, &me, &id, "P2", Some(&fmt(3))).unwrap();
        open_period(&conn, &me, &id, "P3", None).unwrap();
        let mut bad = new_input("坏渠道", vec![unit("X", None)]);
        bad.remind_channels = vec!["nope".into()];
        assert!(create(&conn, &me, bad).unwrap_err().message().contains("推送渠道不存在"));

        // 看板：P1 已过截止日期，3 个单位都逾期；P2 没到；P3 没设
        let d = detail(&conn, &me, &id, Some("P1")).unwrap();
        let by = |p: &str| d.periods.iter().find(|x| x.period == p).unwrap();
        assert_eq!((by("P1").deadline.clone(), by("P1").overdue, by("P2").overdue, by("P3").deadline.clone()), (Some(fmt(-2)), 3, 0, None));
        assert!(d.units.iter().all(|u| u.overdue && u.reminded_at.is_none()));
        assert_eq!((d.collect.latest_deadline.clone(), d.collect.latest_overdue), (None, 0), "最近一期是 P3，没设截止日期");
        // 当天不算逾期
        set_deadline(&conn, &id, "P2", Some(&fmt(0))).unwrap();
        assert_eq!(detail(&conn, &me, &id, Some("P2")).unwrap().periods.iter().find(|p| p.period == "P2").unwrap().overdue, 0);
        set_deadline(&conn, &id, "P2", Some(&fmt(3))).unwrap();
        assert!(set_deadline(&conn, &id, "nope", Some(&fmt(1))).unwrap_err().message().contains("没有这一期"));
        assert!(set_deadline(&conn, &id, "P1", Some("明天")).is_err());

        // 提交了就不逾期
        let ds = load_collect(&conn, &id).unwrap().dataset_id;
        let r = req("upsert", &["填报单位", "期次", "营收"], vec![vec![Json::from("店铺A"), Json::from("P1"), Json::from(1)]]);
        let out = crate::writeback::submit(&conn, &ds, &r).unwrap();
        after_submit(&conn, &ds, &r, &out, &bob);
        let d = detail(&conn, &me, &id, Some("P1")).unwrap();
        assert_eq!((d.periods[2].overdue, d.units.iter().filter(|u| u.overdue).count()), (2, 2));
        let m = mine(&conn, &bob).unwrap();
        assert_eq!(m[0].units[0].periods.iter().map(|p| (p.period.as_str(), p.overdue)).collect::<Vec<_>>(), vec![("P3", false), ("P2", false), ("P1", false)]);
        assert_eq!(m[0].overdue, 0);
        let carol = CurrentUser { id: "u2".into(), name: "Carol".into(), admin: false, enforced: true };
        let m = mine(&conn, &carol).unwrap();
        assert_eq!((m[0].overdue, m[0].pending), (1, 3));
        assert_eq!(m[0].units[0].periods.iter().find(|p| p.period == "P1").map(|p| (p.overdue, p.deadline.clone())), Some((true, Some(fmt(-2)))));

        // 催办：先校验，再通知负责人（A 已提交所以不催）
        let c = load_collect(&conn, &id).unwrap();
        // 固定在今天上午 10 点：下面按「加几小时」推演同一天内的催办，不能让测试跑在深夜时跨过零点
        let now = chrono::Local::now().date_naive().and_hms_opt(10, 0, 0).unwrap();
        let go = |who: &str, period: &str, wanted: Option<&[String]>, auto: bool, at: NaiveDateTime| plan_remind(&conn, &c, &me, period, wanted, auto, at).map(|p| (p, who.to_string()));
        assert!(go("", "P9", None, false, now).unwrap_err().message().contains("没有开启"));
        assert!(go("", "P1", Some(&["店铺Z".to_string()]), false, now).unwrap_err().message().contains("不在收集任务"));
        let (p, _) = go("", "P1", None, false, now).unwrap();
        assert_eq!((p.outcome.notified, p.outcome.skipped, p.outcome.unassigned, p.open_units.len(), p.push_group), (1, 0, 1, 2, false));
        let text: String = conn.query_row("SELECT text FROM ledger_notification WHERE user_id = 'u2' AND kind = 'collect_remind'", [], |r| r.get(0)).unwrap();
        assert!(text.contains("店铺B") && text.contains("还没提交") && text.contains("已逾期 2 天"), "{text}");
        assert_eq!(conn.query_row("SELECT count(*) FROM ledger_notification WHERE user_id = 'u1'", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "已提交的 A 不催");
        // 刚催过：手动 1 小时内不重复；过了才再催；也可以只催指定的单位
        let (p, _) = go("", "P1", None, false, now + chrono::Duration::minutes(30)).unwrap();
        assert_eq!((p.outcome.notified, p.outcome.skipped), (0, 1));
        let (p, _) = go("", "P1", None, false, now + chrono::Duration::hours(2)).unwrap();
        assert_eq!(p.outcome.notified, 1);
        // 自动：同一天只催一次，第二天再催；已提交的 A 之外，被退回的也算要催
        let (p, _) = go("", "P1", None, true, now + chrono::Duration::hours(2)).unwrap();
        assert_eq!((p.outcome.notified, p.outcome.skipped), (0, 1));
        let (p, _) = go("", "P1", None, true, now + chrono::Duration::days(1)).unwrap();
        assert_eq!(p.outcome.notified, 1);
        review(&conn, &me, &id, ReviewInput { period: "P1".into(), units: vec!["店铺A".into()], action: "return".into(), reason: Some("重填".into()) }).unwrap();
        let (p, _) = go("", "P1", Some(&["店铺A".to_string()]), false, now + chrono::Duration::days(2)).unwrap();
        assert_eq!((p.outcome.notified, p.open_units.iter().map(|u| u.2).collect::<Vec<_>>()), (1, vec![true]));
        let text: String = conn.query_row("SELECT text FROM ledger_notification WHERE user_id = 'u1' AND kind = 'collect_remind'", [], |r| r.get(0)).unwrap();
        assert!(text.contains("被退回"), "{text}");
        // 都提交了：没有要催的单位
        let (p, _) = go("", "P3", Some(&[]), false, now).unwrap();
        assert!(p.open_units.is_empty());

        // 推送到群：配了渠道才推；整期的记录限制频率
        conn.execute("UPDATE ledger_collect SET remind_channels = '[\"ch1\"]' WHERE id = ?", params![id]).unwrap();
        let c = load_collect(&conn, &id).unwrap();
        let t0 = now + chrono::Duration::days(5);
        let p = plan_remind(&conn, &c, &me, "P1", None, false, t0).unwrap();
        assert!(p.push_group);
        let g = group_message(&c, "P1", &p);
        assert_eq!((g.heading.as_str(), g.context.as_str(), g.title.as_str()), ("月度", "P1", "还有 3 个单位没有提交"));
        assert_eq!(g.rows, vec![vec!["店铺A", "Bob", "被退回，待修改"], vec!["店铺B", "Carol", "未提交"], vec!["店铺C", "未指派", "未提交"]]);
        assert!(g.notes[0].contains("已逾期 7 天"), "{:?}", g.notes);
        mark_reminded(&conn, &id, "P1", WHOLE_PERIOD, t0).unwrap();
        assert!(!plan_remind(&conn, &c, &me, "P1", None, false, t0 + chrono::Duration::minutes(10)).unwrap().push_group);
        assert!(plan_remind(&conn, &c, &me, "P1", None, false, t0 + chrono::Duration::hours(2)).unwrap().push_group);
        // 延期（改截止日期）后催办记录清空，可以重新催
        set_deadline(&conn, &id, "P1", Some(&fmt(10))).unwrap();
        assert!(load_reminds(&conn, &id, "P1").unwrap().is_empty());

        // 自动催办的时机：9 点以后，截止前一天到逾期 7 天内
        let nine = today.and_hms_opt(10, 0, 0).unwrap();
        let due = |at: NaiveDateTime| {
            let mut v = due_reminders(&conn, at).unwrap();
            v.sort();
            v.into_iter().map(|x| x.1).collect::<Vec<_>>()
        };
        assert!(due(today.and_hms_opt(8, 59, 0).unwrap()).is_empty(), "9 点前不催");
        assert!(due(nine).is_empty(), "P1 延到 10 天后、P2 还有 3 天，都没到时候");
        set_deadline(&conn, &id, "P2", Some(&fmt(1))).unwrap();
        set_deadline(&conn, &id, "P1", Some(&fmt(-7))).unwrap();
        assert_eq!(due(nine), vec!["P1", "P2"], "截止前一天、逾期第 7 天都要催");
        set_deadline(&conn, &id, "P1", Some(&fmt(-8))).unwrap();
        assert_eq!(due(nine), vec!["P2"], "逾期超过 7 天不再自动催");
    }

    fn snapshot_of(conn: &Connection, workbook_id: &str) -> Json {
        use std::io::Read;
        let (_, gz) = crate::workbook::content(conn, workbook_id).unwrap();
        let mut json = String::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut json).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn snapshot_is_a_loadable_entry_area() {
        let fields: Vec<FieldInfo> = vec![("营收".into(), "money".into(), false), ("备注".into(), "text".into(), false), ("商品".into(), "text".into(), true)];
        let rows = vec![vec![Json::from(100.5), Json::Null, Json::from("A")]];
        let spec = serde_json::json!({ "id": "in_x", "fixed": { "填报单位": "店铺A", "期次": "2026-10" } });
        let s = workbook_snapshot("名", "标题", "说明", &fields, &rows, &spec);
        let sheet = &s["sheets"]["sheet-1"];
        assert_eq!(s["sheetOrder"][0], "sheet-1");
        assert_eq!(sheet["cellData"]["0"]["0"]["v"], "标题");
        assert_eq!((sheet["cellData"]["3"]["0"]["v"].as_str(), sheet["cellData"]["3"]["0"]["s"].as_str()), (Some("营收"), Some("head")), "表头在第 4 行");
        assert_eq!(sheet["cellData"]["3"]["2"]["s"], "key", "业务主键列的表头是深绿色");
        assert_eq!((sheet["cellData"]["4"]["0"]["v"].as_f64(), sheet["cellData"]["4"]["2"]["v"].as_str()), (Some(100.5), Some("A")), "已有的数据预填在表头下面");
        assert!(sheet["cellData"]["4"]["1"].get("v").is_none(), "空值不写值，只写样式");
        // 格式逐格写：有值的格子带样式，没值的预留格也带样式（金额带千分位、文本按文本）
        assert_eq!((sheet["cellData"]["4"]["0"]["s"].as_str(), sheet["cellData"]["4"]["1"]["s"].as_str()), (Some("money"), Some("text")));
        assert_eq!((sheet["cellData"]["5"]["0"]["s"].as_str(), sheet["cellData"]["5"]["2"]["s"].as_str()), (Some("money"), Some("text")));
        assert_eq!(sheet["cellData"][(INPUT_HEADER_ROW + PREPARED_ROWS).to_string()]["0"]["s"], "money", "预留到第 PREPARED_ROWS 行");
        assert!(sheet["cellData"].get((INPUT_HEADER_ROW + PREPARED_ROWS + 1).to_string()).is_none());
        assert_eq!(sheet["custom"]["inputs"][0]["fixed"]["期次"], "2026-10", "填报区定义放在工作表的 custom.inputs 里");
        assert!(sheet["rowCount"].as_u64().unwrap() >= 505, "留够数据区的行");
        // 用到的样式 id 都有定义
        for row in sheet["cellData"].as_object().unwrap().values() {
            for c in row.as_object().unwrap().values() {
                if let Some(id) = c["s"].as_str() {
                    assert!(s["styles"].get(id).is_some(), "样式 {id} 没有定义");
                }
            }
        }
        assert_eq!(workbook_name(&"任".repeat(100), "店铺A", "2026-10").chars().count(), 80, "名称不超过 80 个字");
        assert!(workbook_name("月度经营", "店铺A", "2026-10").starts_with("月度经营 · 店铺A"));
    }

    #[test]
    fn open_workbook_generates_once_and_checks_permissions() {
        let conn = setup();
        add_user(&conn, "u1", "Bob");
        add_user(&conn, "u2", "Carol");
        let me = admin();
        let (bob, carol) = (CurrentUser { id: "u1".into(), name: "Bob".into(), admin: false, enforced: true }, CurrentUser { id: "u2".into(), name: "Carol".into(), admin: false, enforced: true });
        let id = create(&conn, &me, new_input("月度", vec![unit("店铺A", Some("u1")), unit("店铺B", Some("u2"))])).unwrap();
        let ds = load_collect(&conn, &id).unwrap().dataset_id;

        assert!(open_workbook(&conn, &bob, &id, "店铺A", "2026-10").unwrap_err().message().contains("没有开启"), "期次没开启不能生成");
        open_period(&conn, &me, &id, "2026-10", None).unwrap();
        assert!(open_workbook(&conn, &bob, &id, "店铺B", "2026-10").unwrap_err().message().contains("不是你负责"), "只能为自己负责的单位生成");
        assert!(open_workbook(&conn, &bob, &id, "店铺Z", "2026-10").unwrap_err().message().contains("不在收集任务"));

        let first = open_workbook(&conn, &bob, &id, "店铺A", "2026-10").unwrap();
        assert!(first.created);
        let again = open_workbook(&conn, &bob, &id, "店铺A", "2026-10").unwrap();
        assert!(!again.created && again.workbook_id == first.workbook_id, "再点直接打开已有的");
        let admin_wb = open_workbook(&conn, &me, &id, "店铺A", "2026-10").unwrap();
        assert!(admin_wb.created && admin_wb.workbook_id != first.workbook_id, "管理员可以为任何单位生成，各有各的一份");

        // 填报区：只有业务列，单位和期次是固定列；主键只有单位 + 期次，所以没有业务主键
        let snap = snapshot_of(&conn, &first.workbook_id);
        let input = &snap["sheets"]["sheet-1"]["custom"]["inputs"][0];
        assert_eq!(input["fields"], serde_json::json!(["营收", "备注"]));
        assert_eq!(input["fixed"], serde_json::json!({ "填报单位": "店铺A", "期次": "2026-10" }));
        assert_eq!((input["mode"].as_str(), input["datasetId"].as_str(), input["keys"].as_array().map(Vec::len)), (Some("upsert"), Some(ds.as_str()), Some(0)));
        assert_eq!((input["row"].as_u64(), input["col"].as_u64()), (Some(INPUT_HEADER_ROW as u64), Some(0)));
        // 所有者是生成的人，默认仅自己可见
        let owner: Option<String> = conn.query_row("SELECT owner_id FROM ledger_workbook WHERE id = ?", params![first.workbook_id], |r| r.get(0)).unwrap();
        assert_eq!(owner.as_deref(), Some("u1"));

        // 数据集里已有的行预填进去
        let cols = ["填报单位", "期次", "营收", "备注"];
        let row = vec![Json::from("店铺B"), Json::from("2026-10"), Json::from(50), Json::from("初稿")];
        crate::writeback::submit(&conn, &ds, &req("upsert", &cols, vec![row])).unwrap();
        let carol_wb = open_workbook(&conn, &carol, &id, "店铺B", "2026-10").unwrap();
        let cells = &snapshot_of(&conn, &carol_wb.workbook_id)["sheets"]["sheet-1"]["cellData"]["4"];
        assert_eq!((cells["0"]["v"].as_f64(), cells["1"]["v"].as_str()), (Some(50.0), Some("初稿")));

        // 工作簿被删除（进回收站）后重新生成一个；删期次时映射一并清掉
        conn.execute("UPDATE ledger_workbook SET deleted_at = now() WHERE id = ?", params![first.workbook_id]).unwrap();
        let fresh = open_workbook(&conn, &bob, &id, "店铺A", "2026-10").unwrap();
        assert!(fresh.created && fresh.workbook_id != first.workbook_id);
        open_period(&conn, &me, &id, "2026-11", None).unwrap();
        open_workbook(&conn, &me, &id, "店铺B", "2026-11").unwrap();
        delete_period(&conn, &id, "2026-11").unwrap();
        let left: i64 = conn.query_row("SELECT count(*) FROM ledger_collect_workbook WHERE period = '2026-11'", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
        delete(&conn, &me, &id).unwrap();
        let all: i64 = conn.query_row("SELECT count(*) FROM ledger_collect_workbook", [], |r| r.get(0)).unwrap();
        assert_eq!(all, 0, "删除任务时映射一并清掉");
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
