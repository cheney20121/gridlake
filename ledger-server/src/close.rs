//! 月结锁账 + 审批
//!
//! 流程：月结体检（完整性 / 余额勾稽 / 提现到账 / 上月是否已结）→ 提交审批（pending，数据冻结）
//! → 审批通过（locked，禁止导入、删除、改余额、改配对）或驳回（rejected，恢复可编辑）；
//! 已锁账可「反结账」（必须填写原因）。每一步写入 ledger_close_log 留痕

use std::collections::HashSet;

use duckdb::{Connection, OptionalExt, params};
use serde::{Deserialize, Serialize};

use crate::balance;
use crate::bank;
use crate::coverage;
use crate::db::now_local;
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};

/// 冻结数据的状态
const FROZEN: [&str; 2] = ["pending", "locked"];

fn status_label(s: &str) -> &'static str {
    match s {
        "pending" => "审批中",
        "locked" => "已锁账",
        _ => "未结账",
    }
}

fn current_status(conn: &Connection, shop: &str, period: &str) -> AppResult<String> {
    let s: Option<String> = conn
        .query_row("SELECT status FROM ledger_period_close WHERE shop = ? AND period = ?", params![shop, period], |r| r.get(0))
        .optional()?;
    Ok(s.unwrap_or_else(|| "open".into()))
}

/// 已提交审批或已锁账的店铺月份不允许修改数据
pub fn ensure_editable(conn: &Connection, shop: &str, period: &str) -> AppResult<()> {
    let status = current_status(conn, shop, period)?;
    if FROZEN.contains(&status.as_str()) {
        let hint = if status == "locked" { "如需调整请先反结账" } else { "如需调整请先撤回审批" };
        return Err(AppError::bad(format!("「{shop}」{period} {}，数据已冻结，{hint}", status_label(&status))));
    }
    Ok(())
}

pub fn ensure_periods_editable(conn: &Connection, shop: &str, periods: &[String]) -> AppResult<()> {
    for p in periods {
        ensure_editable(conn, shop, p)?;
    }
    Ok(())
}

/// 某月冻结（审批中 / 已锁账）的店铺
pub fn locked_shops(conn: &Connection, period: &str) -> AppResult<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT shop FROM ledger_period_close WHERE period = ? AND status IN ('pending', 'locked')")?;
    let rows = stmt.query_map(params![period], |r| r.get(0))?.collect::<Result<HashSet<String>, _>>()?;
    Ok(rows)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PeriodLock {
    pub shop: String,
    pub period: String,
    pub status: String,
}

/// 全部冻结的店铺月份（导入预览、工具栏标识使用）
pub fn locks(conn: &Connection) -> AppResult<Vec<PeriodLock>> {
    let mut stmt =
        conn.prepare("SELECT shop, period, status FROM ledger_period_close WHERE status IN ('pending', 'locked') ORDER BY period DESC, shop")?;
    let rows = stmt
        .query_map([], |r| Ok(PeriodLock { shop: r.get(0)?, period: r.get(1)?, status: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ── 月结体检 ──────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub key: String,
    pub label: String,
    /// pass 通过 / warn 提示 / fail 不通过
    pub level: String,
    pub message: String,
}

fn check(key: &str, label: &str, level: &str, message: impl Into<String>) -> Check {
    Check { key: key.into(), label: label.into(), level: level.into(), message: message.into() }
}

fn money(v: f64) -> String {
    let neg = v < 0.0;
    let cents = (v.abs() * 100.0).round() as i64;
    let int = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{}{grouped}.{:02}", if neg { "-" } else { "" }, cents % 100)
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CloseFigures {
    pub row_count: i64,
    pub inflow: f64,
    pub outflow: f64,
    pub opening: Option<f64>,
    pub closing: Option<f64>,
    pub withdraw_count: i64,
    pub withdraw_amount: f64,
    pub withdraw_matched: i64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub checks: Vec<Check>,
    pub figures: CloseFigures,
}

fn build_checks(
    conn: &Connection,
    shop: &str,
    period: &str,
    cov: &coverage::Coverage,
    balances: &[balance::BalanceRow],
    withdraws: &bank::WithdrawView,
) -> AppResult<CheckReport> {
    let mut checks = Vec::new();

    // 1. 流水完整性
    match cov.cells.iter().find(|c| c.shop == shop && c.period == period) {
        None => checks.push(check("coverage", "流水完整性", "fail", "该月没有流水数据")),
        Some(c) => checks.push(match c.status.as_str() {
            "complete" => check("coverage", "流水完整性", "pass", format!("{} 条流水，每日均有数据", c.row_count)),
            "missing" => check("coverage", "流水完整性", "fail", "该月未导入流水"),
            "not_expected" => check("coverage", "流水完整性", "pass", "开店前 / 关店后，无需数据"),
            _ if !c.missing_days.is_empty() => {
                check("coverage", "流水完整性", "fail", format!("缺少 {} 天流水，请补导后再结账", c.missing_days.len()))
            }
            _ => check("coverage", "流水完整性", "warn", format!("{} 天笔数异常偏低，可能导出被截断", c.low_days.len())),
        }),
    }

    // 2. 余额勾稽
    let mine: Vec<_> = balances.iter().filter(|b| b.shop == shop).collect();
    let missing = mine.iter().filter(|b| b.status == "missing").count();
    let unbalanced: Vec<_> = mine.iter().filter(|b| b.status == "unbalanced").collect();
    if mine.is_empty() {
        checks.push(check("balance", "余额勾稽", "fail", "没有可勾稽的账户"));
    } else if missing > 0 {
        checks.push(check("balance", "余额勾稽", "fail", format!("{missing} 个账户未录入期初 / 期末余额")));
    } else if !unbalanced.is_empty() {
        let detail = unbalanced.iter().map(|b| format!("{}·{} 差 {}", b.entity, b.account, money(b.diff.unwrap_or(0.0)))).collect::<Vec<_>>().join("；");
        checks.push(check("balance", "余额勾稽", "fail", format!("期初 + 入账 - 出账 ≠ 期末：{detail}，可能漏导或多导流水")));
    } else {
        checks.push(check("balance", "余额勾稽", "pass", format!("{} 个账户全部勾稽相符", mine.len())));
    }
    let broken: Vec<_> = mine.iter().filter(|b| b.continuity_diff.is_some_and(|d| d.abs() >= 0.005)).collect();
    if !broken.is_empty() {
        let detail = broken.iter().map(|b| format!("{}·{} 差 {}", b.entity, b.account, money(b.continuity_diff.unwrap_or(0.0)))).collect::<Vec<_>>().join("；");
        checks.push(check("continuity", "期初衔接", "warn", format!("本月期初与上月期末不一致：{detail}")));
    }

    // 3. 提现到账
    let rows: Vec<_> = withdraws.rows.iter().filter(|r| r.shop == shop).collect();
    let sum = |st: &str| -> (usize, f64) {
        let v: Vec<_> = rows.iter().filter(|r| r.status == st).collect();
        (v.len(), v.iter().map(|r| r.amount).sum())
    };
    let (overdue, overdue_amt) = sum("overdue");
    let (no_bank, no_bank_amt) = sum("no_bank");
    let (transit, transit_amt) = sum("in_transit");
    let (matched, _) = sum("matched");
    if rows.is_empty() {
        checks.push(check("withdraw", "提现到账", "pass", "本月无提现"));
    } else if overdue > 0 {
        checks.push(check("withdraw", "提现到账", "fail", format!("{overdue} 笔提现逾期未到账，共 {}，请追查", money(overdue_amt))));
    } else if no_bank > 0 {
        checks.push(check("withdraw", "提现到账", "warn", format!("{no_bank} 笔提现（{}）无银行流水可核对，请导入银行流水", money(no_bank_amt))));
    } else if transit > 0 {
        checks.push(check("withdraw", "提现到账", "warn", format!("{transit} 笔提现在途（{}），尚在到账期内", money(transit_amt))));
    } else {
        checks.push(check("withdraw", "提现到账", "pass", format!("{matched} 笔提现全部到账")));
    }

    // 4. 上月是否已结（上月有数据才检查）
    let prev = balance::prev_period(period)?;
    let prev_rows: i64 = conn.query_row("SELECT count(*) FROM ledger_detail WHERE shop = ? AND period = ?", params![shop, prev], |r| r.get(0))?;
    if prev_rows > 0 && current_status(conn, shop, &prev)? != "locked" {
        checks.push(check("previous", "上月结账", "warn", format!("{prev} 尚未锁账，建议按月顺序结账")));
    }

    let figures = CloseFigures {
        row_count: cov.cells.iter().find(|c| c.shop == shop && c.period == period).map_or(0, |c| c.row_count),
        // 空集合的 f64 求和结果是 -0.0，加 0.0 归一
        inflow: mine.iter().map(|b| b.inflow).sum::<f64>() + 0.0,
        outflow: mine.iter().map(|b| b.outflow).sum::<f64>() + 0.0,
        opening: mine.iter().map(|b| b.opening).sum(),
        closing: mine.iter().map(|b| b.closing).sum(),
        withdraw_count: rows.len() as i64,
        withdraw_amount: rows.iter().map(|r| r.amount).sum::<f64>() + 0.0,
        withdraw_matched: matched as i64,
    };
    Ok(CheckReport { checks, figures })
}

// ── 总览 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseRecord {
    pub status: String,
    pub submitted_by: Option<String>,
    #[serde(skip)]
    pub submitted_by_id: Option<String>,
    pub submitted_at: Option<String>,
    pub approver: Option<String>,
    #[serde(skip)]
    pub approver_id: Option<String>,
    pub comment: Option<String>,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub decision_comment: Option<String>,
    /// 提交时的体检快照
    pub snapshot: Option<CheckReport>,
}

#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanDo {
    /// 提交审批（未冻结时）
    pub submit: bool,
    /// 通过 / 驳回
    pub decide: bool,
    /// 撤回
    pub cancel: bool,
    /// 反结账
    pub reopen: bool,
    /// 不能通过 / 驳回的原因（给界面提示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decide_blocked: Option<String>,
}

/// 记录里的审批人是不是这个用户：新单据按 id，老单据（没有 id）只能按名字
fn is_approver(user: &CurrentUser, r: &CloseRecord) -> bool {
    match r.approver_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => id == user.id,
        None => r.approver.as_deref() == Some(user.name.as_str()),
    }
}

fn is_submitter(user: &CurrentUser, r: &CloseRecord) -> bool {
    match r.submitted_by_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => id == user.id,
        None => r.submitted_by.as_deref() == Some(user.name.as_str()),
    }
}

/// 谁能做什么：通过 / 驳回 = 审批人或管理员，且不能是提交人本人（`LEDGER_ALLOW_SELF_APPROVE` 可放开）；
/// 撤回 = 提交人或管理员；反结账 = 审批人或管理员。未开启登录校验时都允许
pub fn can_do(user: &CurrentUser, status: &str, record: Option<&CloseRecord>, allow_self: bool) -> CanDo {
    let frozen = FROZEN.contains(&status);
    if !user.enforced {
        return CanDo { submit: !frozen, decide: status == "pending", cancel: status == "pending", reopen: status == "locked", decide_blocked: None };
    }
    let mut c = CanDo { submit: !frozen, ..Default::default() };
    let Some(r) = record else { return c };
    if status == "pending" {
        if is_submitter(user, r) && !allow_self {
            c.decide_blocked = Some("不能审批自己提交的月结".into());
        } else if is_approver(user, r) || user.admin {
            c.decide = true;
        } else {
            c.decide_blocked = Some(format!("需要审批人「{}」处理", r.approver.as_deref().unwrap_or("")));
        }
        c.cancel = is_submitter(user, r) || user.admin;
    }
    if status == "locked" {
        c.reopen = is_approver(user, r) || user.admin;
    }
    c
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseShopRow {
    pub shop: String,
    pub shop_status: String,
    pub status: String,
    pub record: Option<CloseRecord>,
    /// 当前用户能做的操作（由服务端按审批人 / 提交人 / 管理员判断）
    pub can: CanDo,
    /// 实时体检（已冻结时数据不会再变，与快照一致）
    pub report: CheckReport,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseOverview {
    pub period: String,
    pub shops: Vec<CloseShopRow>,
}

fn record(conn: &Connection, shop: &str, period: &str) -> AppResult<Option<CloseRecord>> {
    let r = conn
        .query_row(
            "SELECT status, submitted_by, strftime(submitted_at, '%Y-%m-%d %H:%M:%S'), approver, comment, decided_by,
                    strftime(decided_at, '%Y-%m-%d %H:%M:%S'), decision_comment, checks, submitted_by_id, approver_id
             FROM ledger_period_close WHERE shop = ? AND period = ?",
            params![shop, period],
            |r| {
                let checks: Option<String> = r.get(8)?;
                Ok(CloseRecord {
                    status: r.get(0)?,
                    submitted_by: r.get(1)?,
                    submitted_at: r.get(2)?,
                    approver: r.get(3)?,
                    comment: r.get(4)?,
                    decided_by: r.get(5)?,
                    decided_at: r.get(6)?,
                    decision_comment: r.get(7)?,
                    snapshot: checks.and_then(|s| serde_json::from_str(&s).ok()),
                    submitted_by_id: r.get(9)?,
                    approver_id: r.get(10)?,
                })
            },
        )
        .optional()?;
    Ok(r)
}

/// 某月需要结账的店铺：该月有流水，或档案中运营中的店铺（完整性矩阵里应有数据的）
pub fn overview(conn: &Connection, period: &str, user: &CurrentUser, allow_self: bool) -> AppResult<CloseOverview> {
    balance::prev_period(period)?;
    let cov = coverage::coverage(conn)?;
    let balances = balance::list(conn, period, None)?;
    let withdraws = bank::view(conn, period, None)?;
    let mut stmt = conn.prepare(
        "SELECT s.name, s.status FROM ledger_shop s
         WHERE EXISTS (SELECT 1 FROM ledger_detail d WHERE d.shop = s.name AND d.period = ?)
            OR EXISTS (SELECT 1 FROM ledger_period_close c WHERE c.shop = s.name AND c.period = ?)
         ORDER BY s.name",
    )?;
    let shops = stmt.query_map(params![period, period], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut rows = Vec::new();
    for (shop, shop_status) in shops {
        let report = build_checks(conn, &shop, period, &cov, &balances, &withdraws)?;
        let record = record(conn, &shop, period)?;
        let status = record.as_ref().map_or("open".to_string(), |r| r.status.clone());
        rows.push(CloseShopRow {
            can: can_do(user, &status, record.as_ref(), allow_self),
            status,
            shop,
            shop_status,
            record,
            report,
        });
    }
    Ok(CloseOverview { period: period.to_string(), shops: rows })
}

fn report_for(conn: &Connection, shop: &str, period: &str) -> AppResult<CheckReport> {
    let cov = coverage::coverage(conn)?;
    let balances = balance::list(conn, period, Some(shop))?;
    let withdraws = bank::view(conn, period, Some(shop))?;
    build_checks(conn, shop, period, &cov, &balances, &withdraws)
}

// ── 状态流转 ──────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseAction {
    pub shop: String,
    pub period: String,
    /// submit 提交 / approve 通过 / reject 驳回 / cancel 撤回 / reopen 反结账
    pub action: String,
    #[serde(default)]
    pub operator: String,
    pub approver: Option<String>,
    /// 审批人的用户 id（开启登录校验后必填，从登录过的用户里选）
    #[serde(default)]
    pub approver_id: Option<String>,
    #[serde(default)]
    pub comment: String,
    /// 体检有不通过项时仍提交（必须填写说明）
    #[serde(default)]
    pub accept_risk: bool,
}

fn log(conn: &Connection, shop: &str, period: &str, action: &str, operator: &str, comment: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO ledger_close_log (shop, period, action, operator, comment, logged_at) VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params![shop, period, action, operator, comment, now_local()],
    )?;
    Ok(())
}

/// 月结的通知：只在开启登录校验时发（收件人用用户 id 认）
fn notify(conn: &Connection, user: &CurrentUser, to: &str, kind: &str, a: &CloseAction, from: &str, text: &str) -> AppResult<()> {
    if !user.enforced {
        return Ok(());
    }
    let ref_id = format!("close:{}|{}", a.shop, a.period);
    crate::comment::push(conn, crate::comment::Notice { to, kind, workbook_id: "", workbook_name: "", from, text, ref_id: &ref_id }, &user.id)
}

pub fn act(conn: &Connection, a: &CloseAction, user: &CurrentUser, allow_self: bool) -> AppResult<()> {
    balance::prev_period(&a.period)?;
    let operator = a.operator.trim();
    if operator.is_empty() {
        return Err(AppError::bad("缺少操作人"));
    }
    let comment = a.comment.trim();
    let status = current_status(conn, &a.shop, &a.period)?;
    let existing = record(conn, &a.shop, &a.period)?;
    let can = can_do(user, &status, existing.as_ref(), allow_self);
    let expect = |want: &str, what: &str| -> AppResult<()> {
        if status != want {
            return Err(AppError::bad(format!("「{}」{} 当前{}，不能{what}", a.shop, a.period, status_label(&status))));
        }
        Ok(())
    };

    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> AppResult<()> {
        match a.action.as_str() {
            "submit" => {
                if FROZEN.contains(&status.as_str()) {
                    return Err(AppError::bad(format!("「{}」{} 当前{}，不能重复提交", a.shop, a.period, status_label(&status))));
                }
                let (approver, approver_id) = if user.enforced {
                    let id = a.approver_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| AppError::bad("请选择审批人"))?;
                    let name = crate::approval::user_name(conn, id)?.ok_or_else(|| AppError::bad("审批人需要先登录过本系统，才能被选为审批人"))?;
                    if id == user.id && !allow_self {
                        return Err(AppError::bad("不能把自己设为审批人（需要管理员开启 LEDGER_ALLOW_SELF_APPROVE）"));
                    }
                    (name, id.to_string())
                } else {
                    (a.approver.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| AppError::bad("请选择审批人"))?.to_string(), String::new())
                };
                let approver = approver.as_str();
                let report = report_for(conn, &a.shop, &a.period)?;
                let fails: Vec<_> = report.checks.iter().filter(|c| c.level == "fail").map(|c| c.label.as_str()).collect();
                if !fails.is_empty() {
                    if !a.accept_risk {
                        return Err(AppError::bad(format!("月结体检未通过：{}", fails.join("、"))));
                    }
                    if comment.is_empty() {
                        return Err(AppError::bad("体检未通过时提交，必须填写差异说明"));
                    }
                }
                conn.execute("DELETE FROM ledger_period_close WHERE shop = ? AND period = ?", params![a.shop, a.period])?;
                conn.execute(
                    "INSERT INTO ledger_period_close (shop, period, status, submitted_by, submitted_at, approver, comment, checks, submitted_by_id, approver_id)
                     VALUES (?, ?, 'pending', ?, CAST(? AS TIMESTAMP), ?, ?, ?, ?, ?)",
                    params![a.shop, a.period, operator, now_local(), approver, comment, serde_json::to_string(&report)?, user.id, approver_id],
                )?;
                let note = if fails.is_empty() { comment.to_string() } else { format!("[带差异提交：{}] {comment}", fails.join("、")) };
                log(conn, &a.shop, &a.period, "submit", operator, &format!("审批人：{approver}；{note}"))?;
                notify(conn, user, &approver_id, "close_request", a, operator, &format!("{} {} 月结待审批{}", a.shop, a.period, if note.is_empty() { String::new() } else { format!("：{note}") }))
            }
            "approve" | "reject" => {
                expect("pending", "审批")?;
                if !can.decide {
                    return Err(AppError::Forbidden(can.decide_blocked.clone().unwrap_or_else(|| "没有审批权限".into())));
                }
                let approve = a.action == "approve";
                if !approve && comment.is_empty() {
                    return Err(AppError::bad("驳回时请填写原因"));
                }
                conn.execute(
                    "UPDATE ledger_period_close SET status = ?, decided_by = ?, decided_at = CAST(? AS TIMESTAMP), decision_comment = ?
                     WHERE shop = ? AND period = ?",
                    params![if approve { "locked" } else { "rejected" }, operator, now_local(), comment, a.shop, a.period],
                )?;
                let proxied = user.enforced && existing.as_ref().is_some_and(|r| !is_approver(user, r));
                log(conn, &a.shop, &a.period, &a.action, operator, &if proxied { format!("（管理员代为处理）{comment}") } else { comment.to_string() })?;
                let to = existing.as_ref().and_then(|r| r.submitted_by_id.clone()).unwrap_or_default();
                let text = if approve { format!("{} {} 月结已通过并锁账{}", a.shop, a.period, if comment.is_empty() { String::new() } else { format!("：{comment}") }) } else { format!("{} {} 月结被驳回：{comment}", a.shop, a.period) };
                notify(conn, user, &to, if approve { "close_locked" } else { "close_rejected" }, a, operator, &text)
            }
            "cancel" => {
                expect("pending", "撤回")?;
                if !can.cancel {
                    return Err(AppError::Forbidden("只有提交人（或管理员）可以撤回".into()));
                }
                conn.execute("UPDATE ledger_period_close SET status = 'open' WHERE shop = ? AND period = ?", params![a.shop, a.period])?;
                log(conn, &a.shop, &a.period, "cancel", operator, comment)?;
                let to = existing.as_ref().and_then(|r| r.approver_id.clone()).unwrap_or_default();
                notify(conn, user, &to, "close_cancelled", a, operator, &format!("{} {} 月结提交人撤回了审批", a.shop, a.period))
            }
            "reopen" => {
                expect("locked", "反结账")?;
                if !can.reopen {
                    return Err(AppError::Forbidden("反结账需要审批人（或管理员）".into()));
                }
                if comment.is_empty() {
                    return Err(AppError::bad("反结账必须填写原因"));
                }
                conn.execute("UPDATE ledger_period_close SET status = 'open' WHERE shop = ? AND period = ?", params![a.shop, a.period])?;
                log(conn, &a.shop, &a.period, "reopen", operator, comment)?;
                let to = existing.as_ref().and_then(|r| r.submitted_by_id.clone()).unwrap_or_default();
                notify(conn, user, &to, "close_reopened", a, operator, &format!("{} {} 已反结账：{comment}", a.shop, a.period))
            }
            other => Err(AppError::bad(format!("未知操作：{other}"))),
        }
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseLog {
    pub shop: String,
    pub period: String,
    pub action: String,
    pub operator: Option<String>,
    pub comment: Option<String>,
    pub at: String,
}

pub fn logs(conn: &Connection, period: Option<&str>) -> AppResult<Vec<CloseLog>> {
    let mut stmt = conn.prepare(
        "SELECT shop, period, action, operator, comment, strftime(logged_at, '%Y-%m-%d %H:%M:%S') FROM ledger_close_log
         WHERE CAST($1 AS VARCHAR) IS NULL OR period = $1 ORDER BY logged_at DESC LIMIT 500",
    )?;
    let rows = stmt
        .query_map(params![period], |r| {
            Ok(CloseLog { shop: r.get(0)?, period: r.get(1)?, action: r.get(2)?, operator: r.get(3)?, comment: r.get(4)?, at: r.get(5)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    fn user(id: &str, admin: bool) -> CurrentUser {
        CurrentUser { id: id.into(), name: format!("用户{id}"), admin, enforced: true }
    }

    fn rec(status: &str, submitter: &str, approver: &str) -> CloseRecord {
        CloseRecord {
            status: status.into(),
            submitted_by: Some(format!("用户{submitter}")),
            submitted_by_id: Some(submitter.into()),
            submitted_at: None,
            approver: Some(format!("用户{approver}")),
            approver_id: Some(approver.into()),
            comment: None,
            decided_by: None,
            decided_at: None,
            decision_comment: None,
            snapshot: None,
        }
    }

    #[test]
    fn who_can_do_what() {
        let r = rec("pending", "a", "b");
        // 审批人：能审批，不能撤回
        let c = can_do(&user("b", false), "pending", Some(&r), false);
        assert!(c.decide && !c.cancel && !c.submit);
        // 提交人：不能审批自己的，能撤回
        let c = can_do(&user("a", false), "pending", Some(&r), false);
        assert!(!c.decide && c.cancel);
        assert_eq!(c.decide_blocked.as_deref(), Some("不能审批自己提交的月结"));
        // 放开自审：提交人自己就是审批人时可以审批；不是审批人的提交人仍然不行
        let own_approver = rec("pending", "a", "a");
        assert!(!can_do(&user("a", false), "pending", Some(&own_approver), false).decide);
        assert!(can_do(&user("a", false), "pending", Some(&own_approver), true).decide);
        assert!(!can_do(&user("a", false), "pending", Some(&r), true).decide);
        // 无关的人：都不能；管理员：能审批与撤回
        let c = can_do(&user("c", false), "pending", Some(&r), false);
        assert!(!c.decide && !c.cancel && c.decide_blocked.unwrap().contains("用户b"));
        let c = can_do(&user("root", true), "pending", Some(&r), false);
        assert!(c.decide && c.cancel);
        // 管理员自己提交的也不能自己审批
        let own = rec("pending", "root", "b");
        assert!(!can_do(&user("root", true), "pending", Some(&own), false).decide);
        // 反结账：审批人或管理员
        let locked = rec("locked", "a", "b");
        assert!(can_do(&user("b", false), "locked", Some(&locked), false).reopen);
        assert!(can_do(&user("root", true), "locked", Some(&locked), false).reopen);
        assert!(!can_do(&user("a", false), "locked", Some(&locked), false).reopen);
        // 未冻结：任何人可以提交；已冻结不能再提交
        assert!(can_do(&user("a", false), "open", None, false).submit);
        assert!(!can_do(&user("a", false), "locked", Some(&locked), false).submit);
        // 老单据没有 id：按名字对应
        let mut old = rec("pending", "a", "b");
        old.approver_id = None;
        old.submitted_by_id = None;
        assert!(can_do(&user("b", false), "pending", Some(&old), false).decide);
        assert!(!can_do(&user("a", false), "pending", Some(&old), false).decide);
        // 未开启登录校验：都允许
        let open = CurrentUser::open();
        let c = can_do(&open, "pending", Some(&r), false);
        assert!(c.decide && c.cancel);
        assert!(can_do(&open, "locked", Some(&locked), false).reopen);
    }
}
