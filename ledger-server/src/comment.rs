//! 单元格评论与 @提醒：评论线程（一条根评论 + 若干回复）挂在工作簿某个工作表的某个格子上，
//! @到的人在应用内收到通知（顶栏小铃铛），点通知直接跳到那个格子。
//!
//! - 权限：能查看工作簿就能看评论、发评论（只读共享的工作簿也可以评论）；只能改自己的评论，删除自己的，或由工作簿所有者 / 管理员删除
//! - @提醒：被 @ 的人必须能查看这个工作簿，否则不通知，并在返回里说明（不会替他们开放权限）；不通知自己；回复会通知根评论的作者
//! - 评论按「工作表 + 行列位置」定位：在它上面插入 / 删除行列后，评论不会跟着格子移动（Univer 没有稳定的格子标识）
//! - 没有开启登录校验时没有用户标识：评论署名取请求里的操作人名字，不产生通知

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use duckdb::{Connection, OptionalExt, params};
use serde::{Deserialize, Serialize};

use crate::auth::{CurrentUser, Need, run};
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::AppState;

const MAX_BODY: usize = 2000;
const MAX_MENTIONS: usize = 20;

fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("{prefix}_{}_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"), SEQ.fetch_add(1, Ordering::Relaxed) % 1000)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub workbook_id: String,
    pub sheet_id: String,
    pub row: i64,
    pub col: i64,
    /// 回复时是根评论的 id
    pub parent_id: Option<String>,
    pub body: String,
    pub mentions: Vec<Mention>,
    pub author_id: Option<String>,
    pub author_name: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    /// 只有根评论有：这条线程是否已解决
    pub resolved: bool,
    pub resolved_by: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Mention {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    #[serde(flatten)]
    pub root: Comment,
    pub replies: Vec<Comment>,
}

const COLS: &str = "id, workbook_id, sheet_id, row_idx, col_idx, parent_id, body, mentions, author_id, author_name, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(edited_at, '%Y-%m-%d %H:%M:%S'), COALESCE(resolved, false), resolved_by";

fn from_row(r: &duckdb::Row<'_>) -> duckdb::Result<Comment> {
    Ok(Comment {
        id: r.get(0)?,
        workbook_id: r.get(1)?,
        sheet_id: r.get(2)?,
        row: r.get(3)?,
        col: r.get(4)?,
        parent_id: r.get(5)?,
        body: r.get(6)?,
        mentions: r.get::<_, Option<String>>(7)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        author_id: r.get(8)?,
        author_name: r.get(9)?,
        created_at: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
        edited_at: r.get(11)?,
        resolved: r.get(12)?,
        resolved_by: r.get(13)?,
    })
}

fn get(conn: &Connection, id: &str) -> AppResult<Comment> {
    conn.query_row(&format!("SELECT {COLS} FROM ledger_comment WHERE id = ?"), params![id], from_row).map_err(|_| AppError::bad("评论不存在或已删除"))
}

/// 工作簿的全部评论线程（按位置、先后排序）
pub fn threads(conn: &Connection, workbook_id: &str) -> AppResult<Vec<Thread>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM ledger_comment WHERE workbook_id = ? ORDER BY sheet_id, row_idx, col_idx, created_at, id"))?;
    let all = stmt.query_map(params![workbook_id], from_row)?.collect::<Result<Vec<_>, _>>()?;
    let mut out: Vec<Thread> = all.iter().filter(|c| c.parent_id.is_none()).cloned().map(|root| Thread { root, replies: Vec::new() }).collect();
    for c in all.into_iter().filter(|c| c.parent_id.is_some()) {
        if let Some(t) = out.iter_mut().find(|t| Some(&t.root.id) == c.parent_id.as_ref()) {
            t.replies.push(c);
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewComment {
    #[serde(default)]
    pub sheet_id: String,
    #[serde(default)]
    pub row: i64,
    #[serde(default)]
    pub col: i64,
    /// 回复哪条线程（根评论的 id）；有则位置取自根评论
    #[serde(default)]
    pub parent_id: Option<String>,
    pub body: String,
    /// 被 @ 的用户 id
    #[serde(default)]
    pub mentions: Vec<String>,
    /// 没开登录校验时的署名
    #[serde(default)]
    pub operator: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Added {
    pub comment: Comment,
    /// 没有通知到的人（没有这个工作簿的查看权限）
    pub not_notified: Vec<String>,
}

fn clean_body(body: &str) -> AppResult<String> {
    let b = body.trim();
    if b.is_empty() {
        return Err(AppError::bad("评论内容不能为空"));
    }
    if b.chars().count() > MAX_BODY {
        return Err(AppError::bad(format!("评论最多 {MAX_BODY} 字")));
    }
    Ok(b.to_string())
}

/// 用户目录里的用户，转成 CurrentUser 以便判断他能不能查看工作簿
fn directory_user(conn: &Connection, id: &str) -> AppResult<Option<CurrentUser>> {
    Ok(conn
        .query_row("SELECT id, name, COALESCE(admin, false) FROM ledger_user WHERE id = ?", params![id], |r| Ok(CurrentUser { id: r.get(0)?, name: r.get(1)?, admin: r.get(2)?, enforced: true }))
        .optional()?)
}

fn workbook_name(conn: &Connection, id: &str) -> String {
    conn.query_row("SELECT name FROM ledger_workbook WHERE id = ?", params![id], |r| r.get(0)).unwrap_or_default()
}

fn notify(conn: &Connection, to: &str, kind: &str, c: &Comment, from: &str) -> AppResult<()> {
    let text: String = c.body.chars().take(120).collect();
    conn.execute(
        "INSERT INTO ledger_notification (id, user_id, kind, workbook_id, workbook_name, sheet_id, row_idx, col_idx, comment_id, from_name, text, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params![new_id("nt"), to, kind, c.workbook_id, workbook_name(conn, &c.workbook_id), c.sheet_id, c.row, c.col, c.id, from, text, now_local()],
    )?;
    Ok(())
}

/// 评论之外的通知（审批、月结）：发给谁、类型、关联的工作簿（可空）、关联对象（审批单 id、`close:店铺|月份`）
pub struct Notice<'a> {
    pub to: &'a str,
    pub kind: &'a str,
    pub workbook_id: &'a str,
    pub workbook_name: &'a str,
    pub from: &'a str,
    pub text: &'a str,
    pub ref_id: &'a str,
}

/// 发一条通知（收件人为空、或就是操作人自己时不发）；与业务操作在同一个事务里写入
pub fn push(conn: &Connection, n: Notice, actor_id: &str) -> AppResult<()> {
    if n.to.is_empty() || n.to == actor_id {
        return Ok(());
    }
    let text: String = n.text.chars().take(120).collect();
    conn.execute(
        "INSERT INTO ledger_notification (id, user_id, kind, workbook_id, workbook_name, sheet_id, row_idx, col_idx, comment_id, from_name, text, created_at, ref_id)
         VALUES (?, ?, ?, ?, ?, '', 0, 0, '', ?, ?, CAST(? AS TIMESTAMP), ?)",
        params![new_id("nt"), n.to, n.kind, n.workbook_id, n.workbook_name, n.from, text, now_local(), n.ref_id],
    )?;
    Ok(())
}

/// 给被 @ 的人发通知；返回没有通知到的人（不能查看工作簿 / 不在用户目录里）
fn notify_mentions(conn: &Connection, c: &Comment, ids: &[String], author: &CurrentUser, already: &[String]) -> AppResult<Vec<String>> {
    let mut missed = Vec::new();
    for m in &c.mentions {
        if !ids.contains(&m.id) || m.id == author.id || already.contains(&m.id) {
            continue;
        }
        match directory_user(conn, &m.id)? {
            Some(u) if crate::workbook::require(conn, &c.workbook_id, &u, Need::View, false).is_ok() => notify(conn, &u.id, "mention", c, &author.name)?,
            _ => missed.push(m.name.clone()),
        }
    }
    Ok(missed)
}

/// 把请求里的用户 id 解析成 @ 的人（必须是登录过的用户）
fn resolve_mentions(conn: &Connection, ids: &[String]) -> AppResult<Vec<Mention>> {
    if ids.len() > MAX_MENTIONS {
        return Err(AppError::bad(format!("一条评论最多 @ {MAX_MENTIONS} 个人")));
    }
    let mut out: Vec<Mention> = Vec::new();
    for id in ids {
        if out.iter().any(|m| &m.id == id) {
            continue;
        }
        let name = crate::approval::user_name(conn, id)?.ok_or_else(|| AppError::bad("要 @ 的用户不存在（需要登录过本系统）"))?;
        out.push(Mention { id: id.clone(), name });
    }
    Ok(out)
}

pub fn add(conn: &Connection, workbook_id: &str, user: &CurrentUser, input: &NewComment) -> AppResult<Added> {
    let body = clean_body(&input.body)?;
    let (sheet_id, row, col, parent_id, root) = match input.parent_id.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => {
            let root = get(conn, p)?;
            if root.workbook_id != workbook_id || root.parent_id.is_some() {
                return Err(AppError::bad("只能回复本工作簿里的评论线程"));
            }
            (root.sheet_id.clone(), root.row, root.col, Some(root.id.clone()), Some(root))
        }
        None => {
            if input.sheet_id.trim().is_empty() || input.row < 0 || input.col < 0 {
                return Err(AppError::bad("请先选中要评论的单元格"));
            }
            (input.sheet_id.clone(), input.row, input.col, None, None)
        }
    };
    let mentions = resolve_mentions(conn, &input.mentions)?;
    let author_name = if user.enforced { user.name.clone() } else { input.operator.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "匿名".into()) };
    let id = new_id("cm");
    let now = now_local();
    conn.execute(
        "INSERT INTO ledger_comment (id, workbook_id, sheet_id, row_idx, col_idx, parent_id, body, mentions, author_id, author_name, created_at, resolved)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), false)",
        params![id, workbook_id, sheet_id, row, col, parent_id, body, serde_json::to_string(&mentions)?, Some(user.id.clone()).filter(|u| user.enforced && !u.is_empty()), author_name, now],
    )?;
    let comment = get(conn, &id)?;
    let mut not_notified = Vec::new();
    if user.enforced {
        let ids: Vec<String> = mentions.iter().map(|m| m.id.clone()).collect();
        not_notified = notify_mentions(conn, &comment, &ids, user, &[])?;
        // 回复：通知线程的作者（已经因为 @ 通知过的不重复，自己回复自己不通知）
        if let Some(r) = root.as_ref().and_then(|r| r.author_id.clone()).filter(|a| a != &user.id && !ids.contains(a)) {
            notify(conn, &r, "reply", &comment, &user.name)?;
        }
    }
    Ok(Added { comment, not_notified })
}

/// 能不能改 / 删这条评论：作者本人；删除另外允许工作簿所有者与管理员
fn require_comment(conn: &Connection, id: &str, user: &CurrentUser, need: Need, author_only: bool) -> AppResult<Comment> {
    let c = get(conn, id)?;
    crate::workbook::require(conn, &c.workbook_id, user, Need::View, false)?;
    let is_author = !user.enforced || c.author_id.as_deref() == Some(user.id.as_str());
    if is_author {
        return Ok(c);
    }
    if author_only {
        return Err(AppError::Forbidden("只能修改自己的评论".into()));
    }
    crate::workbook::require(conn, &c.workbook_id, user, need, false).map_err(|_| AppError::Forbidden("只有评论的作者、工作簿所有者或管理员可以这样操作".into()))?;
    Ok(c)
}

pub fn edit(conn: &Connection, id: &str, user: &CurrentUser, body: &str, mention_ids: &[String]) -> AppResult<Added> {
    let old = require_comment(conn, id, user, Need::Manage, true)?;
    let body = clean_body(body)?;
    let mentions = resolve_mentions(conn, mention_ids)?;
    conn.execute("UPDATE ledger_comment SET body = ?, mentions = ?, edited_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![body, serde_json::to_string(&mentions)?, now_local(), id])?;
    let comment = get(conn, id)?;
    // 只通知这次新增的 @（原来就 @ 过的不重复打扰）
    let before: Vec<String> = old.mentions.iter().map(|m| m.id.clone()).collect();
    let ids: Vec<String> = mentions.iter().map(|m| m.id.clone()).collect();
    let not_notified = if user.enforced { notify_mentions(conn, &comment, &ids, user, &before)? } else { Vec::new() };
    Ok(Added { comment, not_notified })
}

/// 删除一条评论；删除根评论时整条线程（含回复与相关通知）一起删
pub fn delete(conn: &Connection, id: &str, user: &CurrentUser) -> AppResult<usize> {
    let c = require_comment(conn, id, user, Need::Manage, false)?;
    let mut ids = vec![c.id.clone()];
    if c.parent_id.is_none() {
        let mut stmt = conn.prepare("SELECT id FROM ledger_comment WHERE parent_id = ?")?;
        ids.extend(stmt.query_map(params![id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?);
    }
    for i in &ids {
        conn.execute("DELETE FROM ledger_notification WHERE comment_id = ?", params![i])?;
        conn.execute("DELETE FROM ledger_comment WHERE id = ?", params![i])?;
    }
    Ok(ids.len())
}

/// 标记 / 取消标记线程已解决（能查看工作簿的人都可以，与评论的权限一致）
pub fn resolve(conn: &Connection, id: &str, user: &CurrentUser, resolved: bool) -> AppResult<Comment> {
    let c = get(conn, id)?;
    crate::workbook::require(conn, &c.workbook_id, user, Need::View, false)?;
    if c.parent_id.is_some() {
        return Err(AppError::bad("只能标记整条线程为已解决，请对线程的第一条评论操作"));
    }
    let who = if user.enforced { user.name.clone() } else { String::new() };
    conn.execute(
        "UPDATE ledger_comment SET resolved = ?, resolved_by = ?, resolved_at = CASE WHEN ? THEN CAST(? AS TIMESTAMP) ELSE NULL END WHERE id = ?",
        params![resolved, if resolved { Some(who) } else { None }, resolved, now_local(), id],
    )?;
    get(conn, id)
}

/// 工作簿被彻底删除时清掉它的评论与通知
pub fn purge_workbook(conn: &Connection, workbook_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_notification WHERE workbook_id = ?", params![workbook_id])?;
    conn.execute("DELETE FROM ledger_comment WHERE workbook_id = ?", params![workbook_id])?;
    Ok(())
}

// ── 通知 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    /// mention：被 @；reply：你的评论有了回复；
    /// approval_request / approval_approved / approval_rejected / approval_cancelled：文件审批；
    /// close_request / close_locked / close_rejected / close_cancelled / close_reopened：月结
    pub kind: String,
    pub workbook_id: String,
    pub workbook_name: String,
    pub sheet_id: String,
    pub row: i64,
    pub col: i64,
    pub comment_id: String,
    pub from_name: String,
    pub text: String,
    pub created_at: String,
    pub read: bool,
    /// 审批单 id，或月结的 `close:店铺|月份`（评论通知为空）
    pub ref_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbox {
    pub items: Vec<Notification>,
    pub unread: i64,
}

pub fn inbox(conn: &Connection, user: &CurrentUser, limit: i64) -> AppResult<Inbox> {
    if !user.enforced || user.id.is_empty() {
        return Ok(Inbox { items: Vec::new(), unread: 0 });
    }
    let mut stmt = conn.prepare(
        "SELECT id, kind, workbook_id, COALESCE(workbook_name, ''), sheet_id, row_idx, col_idx, comment_id, COALESCE(from_name, ''), COALESCE(text, ''), strftime(created_at, '%Y-%m-%d %H:%M:%S'), read_at IS NOT NULL, COALESCE(ref_id, '')
         FROM ledger_notification WHERE user_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
    )?;
    let items = stmt
        .query_map(params![user.id, limit.clamp(1, 200)], |r| {
            Ok(Notification {
                id: r.get(0)?,
                kind: r.get(1)?,
                workbook_id: r.get(2)?,
                workbook_name: r.get(3)?,
                sheet_id: r.get(4)?,
                row: r.get(5)?,
                col: r.get(6)?,
                comment_id: r.get(7)?,
                from_name: r.get(8)?,
                text: r.get(9)?,
                created_at: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                read: r.get(11)?,
                ref_id: r.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let unread: i64 = conn.query_row("SELECT count(*) FROM ledger_notification WHERE user_id = ? AND read_at IS NULL", params![user.id], |r| r.get(0))?;
    Ok(Inbox { items, unread })
}

/// 标记已读：ids 为空表示全部
pub fn mark_read(conn: &Connection, user: &CurrentUser, ids: &[String]) -> AppResult<()> {
    if !user.enforced || user.id.is_empty() {
        return Ok(());
    }
    let now = now_local();
    if ids.is_empty() {
        conn.execute("UPDATE ledger_notification SET read_at = CAST(? AS TIMESTAMP) WHERE user_id = ? AND read_at IS NULL", params![now, user.id])?;
    } else {
        for id in ids {
            conn.execute("UPDATE ledger_notification SET read_at = CAST(? AS TIMESTAMP) WHERE id = ? AND user_id = ? AND read_at IS NULL", params![now, id, user.id])?;
        }
    }
    Ok(())
}

// ── 路由 ──────────────────────────────────────────────

/// GET /api/ledger/workbooks/{id}/comments
pub async fn list_comments(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Vec<Thread>>>> {
    Ok(ok(run(&db, move |c| threads(c, &id)).await?))
}

/// POST /api/ledger/workbooks/{id}/comments  { sheetId, row, col, body, mentions? } 或 { parentId, body, mentions? }（回复）
pub async fn add_comment(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, Json(input): Json<NewComment>) -> AppResult<Json<Envelope<Added>>> {
    let _guard = db.import_lock.lock().await;
    let r = run(&db, move |c| add(c, &id, &user, &input)).await?;
    db.checkpoint();
    Ok(ok(r))
}

#[derive(Deserialize)]
pub struct EditBody {
    pub body: String,
    #[serde(default)]
    pub mentions: Vec<String>,
}

/// PUT /api/ledger/comments/{id}  { body, mentions? }
pub async fn edit_comment(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, Json(input): Json<EditBody>) -> AppResult<Json<Envelope<Added>>> {
    let _guard = db.import_lock.lock().await;
    let r = run(&db, move |c| edit(c, &id, &user, &input.body, &input.mentions)).await?;
    db.checkpoint();
    Ok(ok(r))
}

/// DELETE /api/ledger/comments/{id}
pub async fn delete_comment(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> AppResult<Json<Envelope<usize>>> {
    let _guard = db.import_lock.lock().await;
    let n = run(&db, move |c| delete(c, &id, &user)).await?;
    db.checkpoint();
    Ok(ok(n))
}

#[derive(Deserialize)]
pub struct ResolveBody {
    pub resolved: bool,
}

/// POST /api/ledger/comments/{id}/resolve  { resolved }
pub async fn resolve_comment(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, Json(input): Json<ResolveBody>) -> AppResult<Json<Envelope<Comment>>> {
    let _guard = db.import_lock.lock().await;
    let c = run(&db, move |c| resolve(c, &id, &user, input.resolved)).await?;
    db.checkpoint();
    Ok(ok(c))
}

#[derive(Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

/// GET /api/ledger/notifications  我的通知（新的在前）与未读数
pub async fn list_notifications(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Query(q): Query<InboxQuery>) -> AppResult<Json<Envelope<Inbox>>> {
    Ok(ok(run(&db, move |c| inbox(c, &user, q.limit.unwrap_or(50))).await?))
}

#[derive(Deserialize)]
pub struct ReadBody {
    #[serde(default)]
    pub ids: Vec<String>,
}

/// POST /api/ledger/notifications/read  { ids? }  标记已读；不带 ids 表示全部
pub async fn read_notifications(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Json(input): Json<ReadBody>) -> AppResult<Json<Envelope<bool>>> {
    let _guard = db.import_lock.lock().await;
    run(&db, move |c| mark_read(c, &user, &input.ids)).await?;
    Ok(ok(true))
}
