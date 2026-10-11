//! 文件审批：把工作簿的某个版本交给指定审批人，通过 / 驳回 / 撤回，全程留痕
//!
//! - 审批人从「登录过的用户」里选（ledger_user，id 与登录校验得到的 id 同源）；只有审批人（或管理员代为处理，会留痕）能审批，
//!   发起人不能审批自己的单据（`LEDGER_ALLOW_SELF_APPROVE=true` 可放开）
//! - 审批的是发起时工作簿的**已保存版本**：该版本改为永久保留，之后工作簿再被修改也能看到当时审批的内容；
//!   审批期间审批人对这个工作簿自动有查看权限（即使它是私有的）
//! - 没有开启登录校验（`LEDGER_AUTH_MODE=off`）时不区分用户：审批人填名字即可，任何人都能处理，与旧版演示行为接近

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use duckdb::{Connection, OptionalExt, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{CurrentUser, run};
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::AppState;

// ── 用户目录 ──────────────────────────────────────────

/// 记录 / 更新登录过的用户（登录校验通过后异步调用）
pub fn touch_user(conn: &Connection, u: &CurrentUser) -> AppResult<()> {
    if !u.enforced || u.id.is_empty() {
        return Ok(());
    }
    let now = now_local();
    conn.execute(
        "INSERT INTO ledger_user (id, name, admin, first_seen, last_seen) VALUES (?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))
         ON CONFLICT (id) DO UPDATE SET name = excluded.name, admin = excluded.admin, last_seen = excluded.last_seen",
        params![u.id, u.name, u.admin, now, now],
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserItem {
    pub id: String,
    pub name: String,
}

/// 按名字搜索登录过的用户（选审批人用）
pub fn search_users(conn: &Connection, keyword: &str, limit: i64) -> AppResult<Vec<UserItem>> {
    let kw = format!("%{}%", keyword.trim().replace('%', "\\%"));
    let mut stmt = conn.prepare("SELECT id, name FROM ledger_user WHERE name ILIKE ? OR id = ? ORDER BY last_seen DESC, name LIMIT ?")?;
    let rows = stmt
        .query_map(params![kw, keyword.trim(), limit], |r| Ok(UserItem { id: r.get(0)?, name: r.get(1)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn user_name(conn: &Connection, id: &str) -> AppResult<Option<String>> {
    Ok(conn.query_row("SELECT name FROM ledger_user WHERE id = ?", params![id], |r| r.get(0)).optional()?)
}

// ── 审批单 ────────────────────────────────────────────

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub at: String,
    /// submit 发起 / approve 通过 / reject 驳回 / cancel 撤回
    pub action: String,
    pub user_name: String,
    pub comment: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    pub workbook_id: String,
    pub workbook_name: String,
    /// 审批的版本
    pub workbook_version: i64,
    /// 工作簿现在的版本（大于审批版本说明之后又修改过；工作簿已删除时为空）
    pub current_version: Option<i64>,
    pub initiator_id: String,
    pub initiator_name: String,
    pub approver_id: String,
    pub approver_name: String,
    pub comment: String,
    /// 发起时由页面统计的概况（行数、金额等），仅供展示
    pub summary: Value,
    /// pending 审批中 / approved 已通过 / rejected 已驳回 / cancelled 已撤回
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    pub decision_comment: Option<String>,
    /// 当前用户能否通过 / 驳回、撤回
    pub can_decide: bool,
    pub can_cancel: bool,
    /// 详情里带历史
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
}

const COLS: &str = "a.id, a.workbook_id, COALESCE(a.workbook_name, ''), a.workbook_version, w.version, COALESCE(a.initiator_id, ''), COALESCE(a.initiator_name, ''),
    COALESCE(a.approver_id, ''), COALESCE(a.approver_name, ''), COALESCE(a.comment, ''), a.summary, a.status,
    strftime(a.created_at, '%Y-%m-%d %H:%M:%S'), strftime(a.decided_at, '%Y-%m-%d %H:%M:%S'), a.decided_by, a.decision_comment";
const FROM: &str = "FROM ledger_approval a LEFT JOIN ledger_workbook w ON w.id = a.workbook_id AND w.deleted_at IS NULL";

fn read(r: &duckdb::Row, user: &CurrentUser, allow_self: bool) -> duckdb::Result<Approval> {
    let summary: Option<String> = r.get(10)?;
    let mut a = Approval {
        id: r.get(0)?,
        workbook_id: r.get(1)?,
        workbook_name: r.get(2)?,
        workbook_version: r.get(3)?,
        current_version: r.get(4)?,
        initiator_id: r.get(5)?,
        initiator_name: r.get(6)?,
        approver_id: r.get(7)?,
        approver_name: r.get(8)?,
        comment: r.get(9)?,
        summary: summary.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null),
        status: r.get(11)?,
        created_at: r.get(12)?,
        decided_at: r.get(13)?,
        decided_by: r.get(14)?,
        decision_comment: r.get(15)?,
        can_decide: false,
        can_cancel: false,
        events: Vec::new(),
    };
    a.can_decide = a.status == "pending" && may_decide(user, &a.approver_id, &a.initiator_id, allow_self).is_ok();
    a.can_cancel = a.status == "pending" && may_cancel(user, &a.initiator_id).is_ok();
    Ok(a)
}

/// 谁能通过 / 驳回：审批人本人，或管理员代为处理；发起人不能审批自己的单据（除非放开）
pub fn may_decide(user: &CurrentUser, approver_id: &str, initiator_id: &str, allow_self: bool) -> Result<(), String> {
    if !user.enforced {
        return Ok(());
    }
    if !allow_self && !initiator_id.is_empty() && initiator_id == user.id {
        return Err("不能审批自己发起的单据".into());
    }
    if (!approver_id.is_empty() && approver_id == user.id) || user.admin {
        return Ok(());
    }
    Err("只有指定的审批人（或管理员）可以处理".into())
}

pub fn may_cancel(user: &CurrentUser, initiator_id: &str) -> Result<(), String> {
    if !user.enforced || user.admin || (!initiator_id.is_empty() && initiator_id == user.id) {
        Ok(())
    } else {
        Err("只有发起人（或管理员）可以撤回".into())
    }
}

fn involved(user: &CurrentUser, a: &Approval) -> bool {
    !user.enforced || user.admin || a.initiator_id == user.id || a.approver_id == user.id
}

/// 审批人是否因为某张单据对这个工作簿有查看权限（审批中或已处理的都算：事后复核也要能看到当时的内容）
pub fn grants_view(conn: &Connection, workbook_id: &str, user_id: &str) -> bool {
    !user_id.is_empty()
        && conn
            .query_row("SELECT count(*) FROM ledger_approval WHERE workbook_id = ? AND approver_id = ?", params![workbook_id, user_id], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
            > 0
}

fn add_event(conn: &Connection, id: &str, action: &str, user: &CurrentUser, operator: &str, comment: &str) -> AppResult<()> {
    let name = if user.enforced { user.name.as_str() } else { operator };
    conn.execute(
        "INSERT INTO ledger_approval_event (approval_id, happened_at, action, user_id, user_name, comment) VALUES (?, CAST(? AS TIMESTAMP), ?, ?, ?, ?)",
        params![id, now_local(), action, user.id, name, comment],
    )?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApproval {
    pub workbook_id: String,
    /// 审批人（用户目录里的 id）；未开启登录校验时可只填名字
    #[serde(default)]
    pub approver_id: String,
    #[serde(default)]
    pub approver_name: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub summary: Value,
    #[serde(default)]
    pub operator: String,
}

fn new_id() -> String {
    format!("ap_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"))
}

pub fn create(conn: &Connection, user: &CurrentUser, input: &NewApproval, allow_self: bool) -> AppResult<Approval> {
    let operator = if user.enforced { user.name.clone() } else { input.operator.trim().to_string() };
    if operator.is_empty() {
        return Err(AppError::bad("缺少发起人"));
    }
    let row: Option<(String, i64)> = conn
        .query_row("SELECT name, version FROM ledger_workbook WHERE id = ? AND deleted_at IS NULL", params![input.workbook_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((wb_name, version)) = row else { return Err(AppError::bad("工作簿不存在或已删除")) };

    let (approver_id, approver_name) = if user.enforced {
        let id = input.approver_id.trim();
        if id.is_empty() {
            return Err(AppError::bad("请选择审批人"));
        }
        let Some(name) = user_name(conn, id)? else {
            return Err(AppError::bad("审批人需要先登录过本系统，才能被选为审批人"));
        };
        if !allow_self && id == user.id {
            return Err(AppError::bad("不能把自己设为审批人（需要管理员开启 LEDGER_ALLOW_SELF_APPROVE）"));
        }
        (id.to_string(), name)
    } else {
        let name = input.approver_name.trim();
        if name.is_empty() {
            return Err(AppError::bad("请填写审批人"));
        }
        (String::new(), name.to_string())
    };

    if let Some((who, id)) = conn
        .query_row("SELECT approver_name, id FROM ledger_approval WHERE workbook_id = ? AND status = 'pending' LIMIT 1", params![input.workbook_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .optional()?
    {
        return Err(AppError::bad(format!("「{wb_name}」已有一张审批中的单据（审批人：{who}，{id}），请等待结果或先撤回")));
    }

    let id = new_id();
    let comment = Some(input.comment.trim()).filter(|c| !c.is_empty()).unwrap_or("请审批确认").to_string();
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        // 审批的版本永久保留（自动保存的版本默认只留最近 30 个）
        conn.execute(
            "UPDATE ledger_workbook_version SET is_auto = false, description = CASE WHEN COALESCE(description, '') = '' THEN ? ELSE description END WHERE workbook_id = ? AND version = ?",
            params![format!("审批 {id}"), input.workbook_id, version],
        )?;
        conn.execute(
            "INSERT INTO ledger_approval (id, workbook_id, workbook_name, workbook_version, initiator_id, initiator_name, approver_id, approver_name, comment, summary, status, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', CAST(? AS TIMESTAMP))",
            params![id, input.workbook_id, wb_name, version, user.id, operator, approver_id, approver_name, comment, input.summary.to_string(), now_local()],
        )?;
        add_event(conn, &id, "submit", user, &operator, &comment)
    })();
    match r {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    get(conn, user, &id, allow_self)
}

pub fn get(conn: &Connection, user: &CurrentUser, id: &str, allow_self: bool) -> AppResult<Approval> {
    let mut a = conn
        .query_row(&format!("SELECT {COLS} {FROM} WHERE a.id = ?"), params![id], |r| read(r, user, allow_self))
        .optional()?
        .ok_or_else(|| AppError::bad("审批单不存在"))?;
    if !involved(user, &a) {
        return Err(AppError::Forbidden("只有发起人、审批人和管理员可以查看这张审批单".into()));
    }
    let mut stmt = conn.prepare(
        "SELECT strftime(happened_at, '%Y-%m-%d %H:%M:%S'), action, COALESCE(user_name, ''), COALESCE(comment, '') FROM ledger_approval_event WHERE approval_id = ? ORDER BY happened_at, rowid",
    )?;
    a.events = stmt
        .query_map(params![id], |r| Ok(Event { at: r.get(0)?, action: r.get(1)?, user_name: r.get(2)?, comment: r.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(a)
}

/// todo 待我审批 / started 我发起的 / done 我处理或发起的已结束单据 / all 全部（仅管理员）
pub fn list(conn: &Connection, user: &CurrentUser, box_: &str, allow_self: bool) -> AppResult<Vec<Approval>> {
    let (cond, args): (&str, Vec<String>) = match (box_, user.enforced) {
        ("all", true) if !user.admin => return Err(AppError::Forbidden("只有管理员可以查看全部审批单".into())),
        ("todo", true) => ("a.status = 'pending' AND a.approver_id = ?", vec![user.id.clone()]),
        ("started", true) => ("a.initiator_id = ?", vec![user.id.clone()]),
        ("done", true) => ("a.status <> 'pending' AND (a.initiator_id = ? OR a.approver_id = ?)", vec![user.id.clone(), user.id.clone()]),
        ("all", _) | (_, false) => (if box_ == "todo" { "a.status = 'pending'" } else if box_ == "done" { "a.status <> 'pending'" } else { "1 = 1" }, vec![]),
        (other, _) => return Err(AppError::bad(format!("未知的列表：{other}"))),
    };
    let mut stmt = conn.prepare(&format!("SELECT {COLS} {FROM} WHERE {cond} ORDER BY a.created_at DESC LIMIT 200"))?;
    let rows = stmt.query_map(duckdb::params_from_iter(args.iter()), |r| read(r, user, allow_self))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 待我审批的数量（审批中心角标）
pub fn todo_count(conn: &Connection, user: &CurrentUser) -> AppResult<i64> {
    Ok(if user.enforced {
        conn.query_row("SELECT count(*) FROM ledger_approval WHERE status = 'pending' AND approver_id = ?", params![user.id], |r| r.get(0))?
    } else {
        conn.query_row("SELECT count(*) FROM ledger_approval WHERE status = 'pending'", [], |r| r.get(0))?
    })
}

fn pending(conn: &Connection, id: &str) -> AppResult<(String, String)> {
    let row: Option<(String, String, String)> = conn
        .query_row("SELECT status, COALESCE(approver_id, ''), COALESCE(initiator_id, '') FROM ledger_approval WHERE id = ?", params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((status, approver, initiator)) = row else { return Err(AppError::bad("审批单不存在")) };
    if status != "pending" {
        let label = match status.as_str() {
            "approved" => "已通过",
            "rejected" => "已驳回",
            _ => "已撤回",
        };
        return Err(AppError::bad(format!("这张审批单{label}，不能再处理")));
    }
    Ok((approver, initiator))
}

pub fn decide(conn: &Connection, user: &CurrentUser, id: &str, approve: bool, comment: &str, operator: &str, allow_self: bool) -> AppResult<Approval> {
    let comment = comment.trim();
    let (approver, initiator) = pending(conn, id)?;
    may_decide(user, &approver, &initiator, allow_self).map_err(AppError::Forbidden)?;
    if !approve && comment.is_empty() {
        return Err(AppError::bad("驳回时请填写原因"));
    }
    let who = if user.enforced { user.name.clone() } else { operator.trim().to_string() };
    let proxy = user.enforced && approver != user.id;
    let text = if proxy { format!("（管理员代为处理）{comment}") } else { comment.to_string() };
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        conn.execute(
            "UPDATE ledger_approval SET status = ?, decided_at = CAST(? AS TIMESTAMP), decided_by = ?, decision_comment = ? WHERE id = ? AND status = 'pending'",
            params![if approve { "approved" } else { "rejected" }, now_local(), who, text, id],
        )?;
        add_event(conn, id, if approve { "approve" } else { "reject" }, user, operator, &text)
    })();
    match r {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    get(conn, user, id, allow_self)
}

pub fn cancel(conn: &Connection, user: &CurrentUser, id: &str, comment: &str, operator: &str, allow_self: bool) -> AppResult<Approval> {
    let (_, initiator) = pending(conn, id)?;
    may_cancel(user, &initiator).map_err(AppError::Forbidden)?;
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        conn.execute("UPDATE ledger_approval SET status = 'cancelled', decided_at = CAST(? AS TIMESTAMP), decided_by = ? WHERE id = ? AND status = 'pending'", params![now_local(), if user.enforced { user.name.as_str() } else { operator }, id])?;
        add_event(conn, id, "cancel", user, operator, comment.trim())
    })();
    match r {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    get(conn, user, id, allow_self)
}

// ── 接口 ──────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UsersQuery {
    #[serde(default)]
    pub keyword: String,
}

/// GET /api/ledger/users?keyword=  登录过的用户（选审批人）
pub async fn users(State(db): State<AppState>, Query(q): Query<UsersQuery>) -> AppResult<Json<Envelope<Vec<UserItem>>>> {
    Ok(ok(run(&db, move |c| search_users(c, &q.keyword, 30)).await?))
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub r#box: String,
}

/// GET /api/ledger/approvals?box=todo|started|done|all
pub async fn list_approvals(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Query(q): Query<ListQuery>) -> AppResult<Json<Envelope<Vec<Approval>>>> {
    let allow = db.config.allow_self_approve;
    Ok(ok(run(&db, move |c| list(c, &user, if q.r#box.is_empty() { "todo" } else { &q.r#box }, allow)).await?))
}

/// GET /api/ledger/approvals/count  { todo }
pub async fn count(State(db): State<AppState>, Extension(user): Extension<CurrentUser>) -> AppResult<Json<Envelope<Value>>> {
    let n = run(&db, move |c| todo_count(c, &user)).await?;
    Ok(ok(serde_json::json!({ "todo": n })))
}

/// POST /api/ledger/approvals  { workbookId, approverId, comment, summary }  发起审批（审批的是工作簿当前已保存的版本）
pub async fn create_approval(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Json(input): Json<NewApproval>) -> AppResult<Json<Envelope<Approval>>> {
    let allow = db.config.allow_self_approve;
    let _guard = db.import_lock.lock().await;
    let a = run(&db, move |c| create(c, &user, &input, allow)).await?;
    db.checkpoint();
    Ok(ok(a))
}

/// GET /api/ledger/approvals/{id}  详情（含历史）
pub async fn get_approval(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> AppResult<Json<Envelope<Approval>>> {
    let allow = db.config.allow_self_approve;
    Ok(ok(run(&db, move |c| get(c, &user, &id, allow)).await?))
}

#[derive(Deserialize)]
pub struct DecideBody {
    pub approve: bool,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/approvals/{id}/decide  { approve, comment }
pub async fn decide_approval(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, Json(b): Json<DecideBody>) -> AppResult<Json<Envelope<Approval>>> {
    let allow = db.config.allow_self_approve;
    let _guard = db.import_lock.lock().await;
    let a = run(&db, move |c| decide(c, &user, &id, b.approve, &b.comment, &b.operator, allow)).await?;
    db.checkpoint();
    Ok(ok(a))
}

#[derive(Deserialize)]
pub struct CancelBody {
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/approvals/{id}/cancel  { comment }
pub async fn cancel_approval(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, Json(b): Json<CancelBody>) -> AppResult<Json<Envelope<Approval>>> {
    let allow = db.config.allow_self_approve;
    let _guard = db.import_lock.lock().await;
    let a = run(&db, move |c| cancel(c, &user, &id, &b.comment, &b.operator, allow)).await?;
    db.checkpoint();
    Ok(ok(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, admin: bool) -> CurrentUser {
        CurrentUser { id: id.into(), name: format!("用户{id}"), admin, enforced: true }
    }

    fn setup() -> (Connection, String) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        let gz = crate::workbook::gzip(br#"{"sheets":{}}"#);
        let wb = crate::workbook::create(&conn, "利润表", &gz, "x").unwrap();
        for u in [user("a", false), user("b", false), user("c", false), user("root", true)] {
            touch_user(&conn, &u).unwrap();
        }
        (conn, wb.id)
    }

    fn list4(conn: &Connection, u: &CurrentUser, b: &str) -> AppResult<Vec<Approval>> {
        list(conn, u, b, false)
    }

    fn new(wb: &str, approver: &str) -> NewApproval {
        NewApproval { workbook_id: wb.into(), approver_id: approver.into(), approver_name: String::new(), comment: String::new(), summary: serde_json::json!({ "rows": 3 }), operator: String::new() }
    }

    #[test]
    fn lifecycle_and_rules() {
        let (conn, wb) = setup();
        let (a, b, c, root) = (user("a", false), user("b", false), user("c", false), user("root", true));

        // 发起：审批人必须是登录过的用户，不能是自己
        assert!(create(&conn, &a, &new(&wb, ""), false).unwrap_err().message().contains("选择审批人"));
        assert!(create(&conn, &a, &new(&wb, "ghost"), false).unwrap_err().message().contains("先登录过"));
        assert!(create(&conn, &a, &new(&wb, "a"), false).unwrap_err().message().contains("自己"));
        let ap = create(&conn, &a, &new(&wb, "b"), false).unwrap();
        assert_eq!((ap.status.as_str(), ap.approver_name.as_str(), ap.initiator_name.as_str(), ap.workbook_version), ("pending", "用户b", "用户a", 1));
        assert_eq!(ap.comment, "请审批确认");
        assert_eq!(ap.events.len(), 1);
        assert!(!ap.can_decide && ap.can_cancel);
        // 同一个工作簿同时只能有一张审批中的单据
        assert!(create(&conn, &a, &new(&wb, "c"), false).unwrap_err().message().contains("已有一张审批中"));

        // 审批版本永久保留
        let auto: bool = conn.query_row("SELECT is_auto FROM ledger_workbook_version WHERE workbook_id = ? AND version = 1", params![wb], |r| r.get(0)).unwrap();
        assert!(!auto);

        // 审批人有查看权限，其他人没有
        assert!(grants_view(&conn, &wb, "b"));
        assert!(!grants_view(&conn, &wb, "c"));

        // 列表与可见性
        assert_eq!(list4(&conn, &b, "todo").unwrap().len(), 1);
        assert!(list4(&conn, &b, "todo").unwrap()[0].can_decide);
        assert!(list4(&conn, &a, "todo").unwrap().is_empty());
        assert_eq!(list4(&conn, &a, "started").unwrap().len(), 1);
        assert!(list4(&conn, &c, "started").unwrap().is_empty());
        assert!(list4(&conn, &c, "all").is_err());
        assert_eq!(list4(&conn, &root, "all").unwrap().len(), 1);
        assert_eq!(todo_count(&conn, &b).unwrap(), 1);
        assert_eq!(todo_count(&conn, &c).unwrap(), 0);
        assert!(get(&conn, &c, &ap.id, false).is_err());
        assert!(get(&conn, &b, &ap.id, false).is_ok());

        // 处理：无关的人、发起人都不行；驳回要写原因
        assert!(decide(&conn, &c, &ap.id, true, "", "", false).unwrap_err().message().contains("审批人"));
        assert!(decide(&conn, &a, &ap.id, true, "", "", false).unwrap_err().message().contains("自己发起"));
        assert!(decide(&conn, &b, &ap.id, false, "  ", "", false).unwrap_err().message().contains("原因"));
        let done = decide(&conn, &b, &ap.id, false, "金额对不上", "", false).unwrap();
        assert_eq!((done.status.as_str(), done.decided_by.as_deref(), done.decision_comment.as_deref()), ("rejected", Some("用户b"), Some("金额对不上")));
        assert_eq!(done.events.len(), 2);
        assert!(!done.can_decide && !done.can_cancel);
        // 已处理的不能再处理 / 撤回
        assert!(decide(&conn, &b, &ap.id, true, "", "", false).unwrap_err().message().contains("已驳回"));
        assert!(cancel(&conn, &a, &ap.id, "", "", false).is_err());
        assert_eq!(todo_count(&conn, &b).unwrap(), 0);
        assert_eq!(list4(&conn, &a, "done").unwrap().len(), 1);
        // 处理完仍保留审批人的查看权限（事后复核）
        assert!(grants_view(&conn, &wb, "b"));

        // 再发一张：撤回只有发起人（或管理员），管理员可代为审批并留痕
        let ap2 = create(&conn, &a, &new(&wb, "b"), false).unwrap();
        assert!(cancel(&conn, &c, &ap2.id, "", "", false).is_err());
        let cancelled = cancel(&conn, &a, &ap2.id, "发错了", "", false).unwrap();
        assert_eq!(cancelled.status, "cancelled");
        let ap3 = create(&conn, &a, &new(&wb, "b"), false).unwrap();
        assert!(get(&conn, &root, &ap3.id, false).unwrap().can_decide);
        let by_admin = decide(&conn, &root, &ap3.id, true, "同意", "", false).unwrap();
        assert_eq!(by_admin.status, "approved");
        assert!(by_admin.decision_comment.unwrap().starts_with("（管理员代为处理）"));
        assert!(by_admin.events.iter().any(|e| e.action == "approve" && e.user_name == "用户root"));
    }

    #[test]
    fn self_approval_is_opt_in() {
        let (conn, wb) = setup();
        let a = user("a", false);
        let ap = create(&conn, &a, &new(&wb, "a"), true).unwrap();
        assert!(decide(&conn, &a, &ap.id, true, "", "", false).is_err());
        assert_eq!(decide(&conn, &a, &ap.id, true, "", "", true).unwrap().status, "approved");
        // 管理员也不能审批自己发起的单据
        let root = user("root", true);
        let ap2 = create(&conn, &root, &new(&wb, "a"), false).unwrap();
        assert!(decide(&conn, &root, &ap2.id, true, "", "", false).unwrap_err().message().contains("自己发起"));
    }

    #[test]
    fn without_auth_anyone_can_handle() {
        let (conn, wb) = setup();
        let open = CurrentUser::open();
        let mut input = new(&wb, "");
        input.operator = "小王".into();
        assert!(create(&conn, &open, &input, false).unwrap_err().message().contains("填写审批人"));
        input.approver_name = "张经理".into();
        let ap = create(&conn, &open, &input, false).unwrap();
        assert_eq!((ap.initiator_name.as_str(), ap.approver_name.as_str()), ("小王", "张经理"));
        assert!(ap.can_decide);
        assert_eq!(list4(&conn, &open, "todo").unwrap().len(), 1);
        assert_eq!(decide(&conn, &open, &ap.id, true, "", "张经理", false).unwrap().decided_by.as_deref(), Some("张经理"));
    }

    #[test]
    fn user_directory() {
        let (conn, _) = setup();
        assert_eq!(search_users(&conn, "用户", 10).unwrap().len(), 4);
        assert_eq!(search_users(&conn, "root", 10).unwrap()[0].id, "root");
        assert!(search_users(&conn, "不存在", 10).unwrap().is_empty());
        // 改名后更新；未开启认证时不记
        let mut renamed = user("a", false);
        renamed.name = "新名字".into();
        touch_user(&conn, &renamed).unwrap();
        assert_eq!(search_users(&conn, "新名", 10).unwrap().len(), 1);
        touch_user(&conn, &CurrentUser::open()).unwrap();
        assert_eq!(search_users(&conn, "", 50).unwrap().len(), 4);
    }
}
