//! AI 分析助手：接 OpenAI 兼容的模型（可为客户内网的 DeepSeek / Qwen），把平台能力做成工具供模型调用
//!
//! - 会话按工作簿保存（ledger_ai_thread / ledger_ai_message），继续提问时带上之前的对话与工具结果
//! - 模型只能通过工具取数（服务端汇总，不把明细发给模型），回答里的数字都有对应的查询
//! - 透视表、图表、公式、计算列只生成「建议」（ledger_ai_action），用户在界面上确认后才创建，并记录谁在何时应用
//! - 对话过程以 SSE 推送：文本增量、工具调用进度、建议
//! - 回答在服务端后台运行：页面关闭、断网后继续，回来时重新连上（先补发已产生的事件，再接着推送）；「停止」需显式调用
//! - 长对话自动整理摘要：较早的轮次由模型压缩成要点（结论、关键数字、口径、未完成事项），之后带摘要 + 最近几轮

pub mod llm;
pub mod tools;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use tokio::sync::mpsc;

use crate::auth::CurrentUser;
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::routes::AppState;
use llm::Message;

/// 一次提问最多调用工具的轮数
const DEFAULT_MAX_STEPS: usize = 10;
/// 带入模型的历史：最近几轮提问（更早的整理成摘要）
const HISTORY_TURNS: usize = 6;
/// 摘要输入里每条工具结果最多保留的字数
const SUMMARY_TOOL_CHARS: usize = 400;
/// 手动编辑的摘要最长字数（自动整理的限制在 600 字内）
const MAX_SUMMARY_CHARS: usize = 4000;

pub fn config() -> Option<llm::Config> {
    llm::Config::from_env()
}

fn max_steps() -> usize {
    std::env::var("LEDGER_AI_MAX_STEPS").ok().and_then(|v| v.parse().ok()).filter(|n| (1..=30).contains(n)).unwrap_or(DEFAULT_MAX_STEPS)
}

fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("{prefix}_{}_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"), SEQ.fetch_add(1, Ordering::Relaxed) % 1000)
}

// ── 会话 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub workbook_id: String,
    pub title: String,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// 正在后台回答
    pub running: bool,
}

fn thread_from_row(r: &duckdb::Row<'_>) -> duckdb::Result<Thread> {
    let id: String = r.get(0)?;
    Ok(Thread {
        running: is_running(&id),
        id,
        workbook_id: r.get(1)?,
        title: r.get(2)?,
        created_by: r.get(3)?,
        created_at: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        updated_at: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
    })
}

const THREAD_COLS: &str = "id, workbook_id, title, created_by, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(updated_at, '%Y-%m-%d %H:%M:%S')";

/// 能看到的会话：对话里有按创建人数据范围查出的结果，所以只有创建人和管理员能看；没开认证时不限。
/// 升级前创建的会话没有记创建人的 ID，按创建人的名字认
pub fn threads(conn: &Connection, workbook_id: &str, viewer: &CurrentUser) -> AppResult<Vec<Thread>> {
    let sql = |filter: &str| format!("SELECT {THREAD_COLS} FROM ledger_ai_thread WHERE workbook_id = ?{filter} ORDER BY updated_at DESC, created_at DESC, id DESC LIMIT 100");
    if !viewer.enforced || viewer.admin {
        let mut stmt = conn.prepare(&sql(""))?;
        return Ok(stmt.query_map(params![workbook_id], thread_from_row)?.collect::<Result<Vec<_>, _>>()?);
    }
    let mut stmt = conn.prepare(&sql(" AND (owner_id = ? OR (owner_id IS NULL AND created_by = ?))"))?;
    Ok(stmt.query_map(params![workbook_id, viewer.id, viewer.name], thread_from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// 这个用户能不能访问会话：创建人和管理员（没开认证时不限）
fn can_access(user: &CurrentUser, owner_id: Option<&str>, created_by: Option<&str>) -> bool {
    if !user.enforced || user.admin {
        return true;
    }
    match owner_id.filter(|o| !o.is_empty()) {
        Some(o) => o == user.id,
        None => created_by.is_some_and(|n| !n.is_empty() && n == user.name),
    }
}

/// 访问某个会话（读、提问、停止、删除、处理其中的建议）的权限：先要能查看它所在的工作簿，再要是创建人或管理员。
/// 会话不存在时放行，由具体接口报「会话不存在」
pub fn require_thread(conn: &Connection, id: &str, user: &CurrentUser) -> AppResult<()> {
    let row: Option<(String, Option<String>, Option<String>)> =
        duckdb::OptionalExt::optional(conn.query_row("SELECT workbook_id, owner_id, created_by FROM ledger_ai_thread WHERE id = ?", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))))?;
    let Some((wb, owner_id, created_by)) = row else { return Ok(()) };
    if !wb.is_empty() {
        crate::workbook::require(conn, &wb, user, crate::auth::Need::View, false)?;
    }
    if can_access(user, owner_id.as_deref(), created_by.as_deref()) {
        return Ok(());
    }
    let who = created_by.filter(|n| !n.is_empty()).map_or("其他人".to_string(), |n| format!("「{n}」"));
    Err(AppError::Forbidden(format!("这个对话是{who}的，只有本人和管理员能查看")))
}

/// 建议所在会话的访问权限（处理 AI 建议时用）
pub fn require_action(conn: &Connection, action_id: &str, user: &CurrentUser) -> AppResult<()> {
    let thread_id: Option<String> = duckdb::OptionalExt::optional(conn.query_row("SELECT thread_id FROM ledger_ai_action WHERE id = ?", [action_id], |r| r.get(0)))?;
    match thread_id {
        Some(t) => require_thread(conn, &t, user),
        None => Ok(()),
    }
}

fn thread(conn: &Connection, id: &str) -> AppResult<Thread> {
    conn.query_row(&format!("SELECT {THREAD_COLS} FROM ledger_ai_thread WHERE id = ?"), params![id], thread_from_row).map_err(|_| AppError::bad("会话不存在"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewThread {
    pub workbook_id: String,
    #[serde(default)]
    pub operator: Option<String>,
    /// 创建人的用户 ID（服务端按登录用户填，请求里的值不采用）
    #[serde(skip)]
    pub owner_id: Option<String>,
}

pub fn create_thread(conn: &Connection, input: NewThread) -> AppResult<Thread> {
    if input.workbook_id.trim().is_empty() {
        return Err(AppError::bad("工作簿还没有保存到服务端"));
    }
    let id = new_id("th");
    let now = now_local();
    conn.execute(
        "INSERT INTO ledger_ai_thread (id, workbook_id, title, created_by, owner_id, created_at, updated_at) VALUES (?, ?, '新对话', ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![id, input.workbook_id, input.operator, input.owner_id.filter(|o| !o.is_empty()), now, now],
    )?;
    thread(conn, &id)
}

pub fn delete_thread(conn: &Connection, id: &str) -> AppResult<()> {
    if is_running(id) {
        return Err(AppError::bad("正在回答中，请先停止"));
    }
    conn.execute("DELETE FROM ledger_ai_action WHERE thread_id = ?", params![id])?;
    conn.execute("DELETE FROM ledger_ai_message WHERE thread_id = ?", params![id])?;
    conn.execute("DELETE FROM ledger_ai_thread WHERE id = ?", params![id])?;
    Ok(())
}

// ── 消息与建议 ────────────────────────────────────────

struct StoredMessage {
    id: String,
    seq: i64,
    role: String,
    content: Option<String>,
    tool_calls: Vec<llm::ToolCall>,
    tool_call_id: Option<String>,
    display: Option<String>,
    meta: Option<Json>,
    created_at: String,
}

fn load_messages(conn: &Connection, thread_id: &str) -> AppResult<Vec<StoredMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, tool_calls, tool_call_id, display, meta, strftime(created_at, '%Y-%m-%d %H:%M:%S'), seq FROM ledger_ai_message WHERE thread_id = ? ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![thread_id], |r| {
        Ok(StoredMessage {
            id: r.get(0)?,
            role: r.get(1)?,
            content: r.get(2)?,
            tool_calls: r.get::<_, Option<String>>(3)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
            tool_call_id: r.get(4)?,
            display: r.get(5)?,
            meta: r.get::<_, Option<String>>(6)?.and_then(|s| serde_json::from_str(&s).ok()),
            created_at: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            seq: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn save_message(conn: &Connection, thread_id: &str, m: &Message, display: Option<&str>, meta: Option<&Json>) -> AppResult<String> {
    let id = new_id("msg");
    let seq: i64 = conn.query_row("SELECT coalesce(max(seq), 0) + 1 FROM ledger_ai_message WHERE thread_id = ?", params![thread_id], |r| r.get(0))?;
    conn.execute(
        "INSERT INTO ledger_ai_message (id, thread_id, seq, role, content, tool_calls, tool_call_id, display, meta, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params![
            id,
            thread_id,
            seq,
            m.role,
            m.content,
            if m.tool_calls.is_empty() { None } else { Some(serde_json::to_string(&m.tool_calls)?) },
            m.tool_call_id,
            display,
            meta.map(|x| x.to_string()),
            now_local()
        ],
    )?;
    conn.execute("UPDATE ledger_ai_thread SET updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now_local(), thread_id])?;
    Ok(id)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    pub id: String,
    pub thread_id: String,
    pub kind: String,
    pub title: String,
    pub payload: Json,
    /// proposed / applied / dismissed / undone
    pub status: String,
    pub result: Option<String>,
    /// 应用时记下的撤销信息（新建的透视表页签 / 图表 / 被覆盖的单元格原值 / 新建的计算列），界面据此撤销
    pub undo: Option<Json>,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
}

fn actions(conn: &Connection, thread_id: &str) -> AppResult<Vec<ActionView>> {
    let mut stmt = conn.prepare(
        "SELECT id, thread_id, kind, title, payload, status, result, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(decided_at, '%Y-%m-%d %H:%M:%S'), decided_by, undo
         FROM ledger_ai_action WHERE thread_id = ? ORDER BY created_at",
    )?;
    let rows = stmt.query_map(params![thread_id], |r| {
        Ok(ActionView {
            id: r.get(0)?,
            thread_id: r.get(1)?,
            kind: r.get(2)?,
            title: r.get(3)?,
            payload: r.get::<_, String>(4).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Json::Null),
            status: r.get(5)?,
            result: r.get(6)?,
            created_at: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            decided_at: r.get(8)?,
            decided_by: r.get(9)?,
            undo: r.get::<_, Option<String>>(10)?.and_then(|s| serde_json::from_str(&s).ok()),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn save_action(conn: &Connection, thread_id: &str, payload: &Json) -> AppResult<ActionView> {
    let id = new_id("act");
    let kind = payload.get("kind").and_then(Json::as_str).unwrap_or("").to_string();
    let title = match kind.as_str() {
        "formula" => format!("{} 写入公式", payload.get("cell").and_then(Json::as_str).unwrap_or("")),
        "calc" => format!("计算列「{}」", payload.get("name").and_then(Json::as_str).unwrap_or("")),
        "plan" => format!("方案「{}」（{} 步）", payload.get("title").and_then(Json::as_str).unwrap_or(""), payload.get("steps").and_then(Json::as_array).map_or(0, Vec::len)),
        _ => payload.get("title").and_then(Json::as_str).unwrap_or("").to_string(),
    };
    let now = now_local();
    conn.execute(
        "INSERT INTO ledger_ai_action (id, thread_id, kind, title, payload, status, created_at) VALUES (?, ?, ?, ?, ?, 'proposed', CAST(? AS TIMESTAMP))",
        params![id, thread_id, kind, title, payload.to_string(), now],
    )?;
    Ok(ActionView { id, thread_id: thread_id.into(), kind, title, payload: payload.clone(), status: "proposed".into(), result: None, undo: None, created_at: now, decided_at: None, decided_by: None })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecideInput {
    /// applied / dismissed / undone
    pub status: String,
    #[serde(default)]
    pub result: Option<String>,
    /// 应用时的撤销信息（status = applied 时保存）
    #[serde(default)]
    pub undo: Option<Json>,
    #[serde(default)]
    pub operator: Option<String>,
}

/// 记录建议的处理结果（已应用 / 忽略 / 已撤销）。状态只能 proposed → applied | dismissed、applied → undone
pub fn decide(conn: &Connection, id: &str, input: DecideInput) -> AppResult<()> {
    let current: Option<String> = duckdb::OptionalExt::optional(conn.query_row("SELECT status FROM ledger_ai_action WHERE id = ?", params![id], |r| r.get(0)))?;
    let Some(current) = current else { return Err(AppError::bad("建议不存在")) };
    match (current.as_str(), input.status.as_str()) {
        ("proposed", "applied" | "dismissed") | ("applied", "undone") => {}
        (_, "applied" | "dismissed" | "undone") => return Err(AppError::bad(format!("这条建议已经是「{}」状态，不能再改为「{}」", status_label(&current), status_label(&input.status)))),
        _ => return Err(AppError::bad("状态应为 applied、dismissed 或 undone")),
    }
    let undo = if input.status == "applied" { input.undo.as_ref().map(Json::to_string) } else { None };
    conn.execute(
        "UPDATE ledger_ai_action SET status = ?, result = ?, undo = COALESCE(?, undo), decided_at = CAST(? AS TIMESTAMP), decided_by = ? WHERE id = ?",
        params![input.status, input.result, undo, now_local(), input.operator, id],
    )?;
    Ok(())
}

fn status_label(s: &str) -> &'static str {
    match s {
        "proposed" => "待处理",
        "applied" => "已应用",
        "dismissed" => "已忽略",
        _ => "已撤销",
    }
}

/// 界面上显示的一条
#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "role")]
pub enum Item {
    #[serde(rename = "user")]
    User { id: String, text: String, created_at: String },
    #[serde(rename = "assistant")]
    Assistant { id: String, text: String, created_at: String },
    #[serde(rename = "tool")]
    #[serde(rename_all = "camelCase")]
    Tool { id: String, name: String, summary: String, ok: bool, action_id: Option<String>, created_at: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    pub thread: Thread,
    pub items: Vec<Item>,
    pub actions: Vec<ActionView>,
    pub running: bool,
    /// 较早轮次整理成的摘要，及摘要覆盖到的最后一条消息（界面在它后面显示分隔线）
    pub summary: Option<String>,
    pub summarized_until: Option<String>,
    /// 上一次回答没有结束就中断了（服务重启或崩溃）：界面提供「继续回答」
    pub interrupted: bool,
}

/// 最后一条消息说明回答没有走完：提问还没回答、工具结果还没交给模型，或模型要调用工具但还没有结果。
/// 正常结束、出错、停止时最后一条都是助手的文字
fn is_interrupted(stored: &[StoredMessage]) -> bool {
    stored.last().is_some_and(|m| m.role == "user" || m.role == "tool" || (m.role == "assistant" && !m.tool_calls.is_empty()))
}

/// 最后一轮模型要调用的工具里，没有结果的补上一条「未执行」的结果（模型接口要求每个调用都有对应的结果）；返回补了几条
fn fill_dangling_tool_calls(conn: &Connection, thread_id: &str, stored: &[StoredMessage]) -> AppResult<usize> {
    let Some(pos) = stored.iter().rposition(|m| m.role == "assistant" && !m.tool_calls.is_empty()) else { return Ok(0) };
    let done: std::collections::HashSet<&str> = stored[pos + 1..].iter().filter(|m| m.role == "tool").filter_map(|m| m.tool_call_id.as_deref()).collect();
    let mut n = 0;
    for call in stored[pos].tool_calls.iter().filter(|c| !done.contains(c.id.as_str())) {
        let meta = json!({ "name": call.function.name, "summary": "服务重启，未执行", "ok": false });
        save_message(conn, thread_id, &Message::tool(&call.id, "（服务重启时这次工具调用没有执行，需要的话请重新调用）".to_string()), None, Some(&meta))?;
        n += 1;
    }
    Ok(n)
}

/// 手动编辑会话摘要。留空表示清除：之后提问时较早的轮次会重新整理
pub fn set_summary(conn: &Connection, id: &str, text: Option<&str>) -> AppResult<()> {
    thread(conn, id)?;
    if is_running(id) {
        return Err(AppError::bad("正在回答中，请先停止"));
    }
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    match text {
        Some(t) => {
            if t.chars().count() > MAX_SUMMARY_CHARS {
                return Err(AppError::bad(format!("摘要最多 {MAX_SUMMARY_CHARS} 字（现在 {} 字）", t.chars().count())));
            }
            conn.execute("UPDATE ledger_ai_thread SET summary = ? WHERE id = ?", params![t, id])?;
        }
        None => {
            conn.execute("UPDATE ledger_ai_thread SET summary = NULL, summary_seq = 0 WHERE id = ?", params![id])?;
        }
    }
    Ok(())
}

/// 会话的摘要与摘要覆盖到的消息序号
fn summary_of(conn: &Connection, thread_id: &str) -> AppResult<(Option<String>, i64)> {
    Ok(conn.query_row("SELECT summary, coalesce(summary_seq, 0) FROM ledger_ai_thread WHERE id = ?", params![thread_id], |r| Ok((r.get(0)?, r.get(1)?)))?)
}

pub fn detail(conn: &Connection, id: &str) -> AppResult<ThreadDetail> {
    let t = thread(conn, id)?;
    let (summary, summary_seq) = summary_of(conn, id)?;
    let stored = load_messages(conn, id)?;
    let interrupted = !is_running(id) && is_interrupted(&stored);
    // 分隔线放在摘要覆盖的最后一条「界面上看得到」的消息后面
    let summarized_until = if summary.is_some() {
        stored.iter().rfind(|m| m.seq <= summary_seq && (m.role == "user" || m.role == "tool" || m.content.as_deref().is_some_and(|c| !c.trim().is_empty()))).map(|m| m.id.clone())
    } else {
        None
    };
    let mut items = Vec::new();
    for m in stored {
        match m.role.as_str() {
            "user" => items.push(Item::User { id: m.id, text: m.display.or(m.content).unwrap_or_default(), created_at: m.created_at }),
            "assistant" => {
                let text = m.content.unwrap_or_default();
                if !text.trim().is_empty() {
                    items.push(Item::Assistant { id: m.id, text, created_at: m.created_at });
                }
            }
            "tool" => {
                let meta = m.meta.unwrap_or(Json::Null);
                items.push(Item::Tool {
                    id: m.id,
                    name: meta.get("name").and_then(Json::as_str).unwrap_or("").into(),
                    summary: meta.get("summary").and_then(Json::as_str).unwrap_or("").into(),
                    ok: meta.get("ok").and_then(Json::as_bool).unwrap_or(true),
                    action_id: meta.get("actionId").and_then(Json::as_str).map(String::from),
                    created_at: m.created_at,
                });
            }
            _ => {}
        }
    }
    Ok(ThreadDetail { running: is_running(id), actions: actions(conn, id)?, thread: t, items, summary, summarized_until, interrupted })
}

// ── 对话 ──────────────────────────────────────────────

/// 一次正在进行的回答：已产生的事件（重连时补发）、订阅者、停止标记
struct Run {
    events: Vec<Json>,
    subs: Vec<Events>,
    cancel: Arc<AtomicBool>,
}

fn runs() -> &'static Mutex<HashMap<String, Run>> {
    static R: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn is_running(id: &str) -> bool {
    runs().lock().map(|r| r.contains_key(id)).unwrap_or(false)
}

/// 订阅正在进行的回答：先补发已产生的事件，再接收后续事件；没有在回答时返回 None
pub fn subscribe(thread_id: &str) -> Option<mpsc::UnboundedReceiver<Json>> {
    let mut r = runs().lock().ok()?;
    let run = r.get_mut(thread_id)?;
    let (tx, rx) = mpsc::unbounded_channel();
    for e in &run.events {
        let _ = tx.send(e.clone());
    }
    run.subs.push(tx);
    Some(rx)
}

/// 停止回答（正在调用模型时立即中断，已产生的文字保留）
pub fn stop(thread_id: &str) -> bool {
    runs().lock().ok().and_then(|r| r.get(thread_id).map(|run| run.cancel.store(true, Ordering::Relaxed))).is_some()
}

fn cancelled(thread_id: &str) -> bool {
    runs().lock().ok().and_then(|r| r.get(thread_id).map(|run| run.cancel.load(Ordering::Relaxed))).unwrap_or(true)
}

fn cancel_flag(thread_id: &str) -> Arc<AtomicBool> {
    runs().lock().ok().and_then(|r| r.get(thread_id).map(|run| run.cancel.clone())).unwrap_or_else(|| Arc::new(AtomicBool::new(true)))
}

/// 等到被停止
async fn wait_cancel(flag: Arc<AtomicBool>) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// 前端随提问附带的界面上下文
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Context {
    /// 当前工作表名称
    pub sheet_name: Option<String>,
    /// 当前选区地址（如 B3:D10）与其中的值（最多 20 行 × 10 列）
    pub selection: Option<String>,
    pub selection_values: Option<Vec<Vec<Json>>>,
    /// 当前工作表是透视表时的定义
    pub pivot: Option<Json>,
    /// 工作簿里的数据表（set_table_view 的对象）
    pub tables: Vec<TableCtx>,
}

/// 工作簿里一张数据表当前的视图
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableCtx {
    pub name: String,
    #[serde(default)]
    pub dataset_name: String,
    /// float（浮动表格）/ cells（铺到单元格）
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub filters: Vec<crate::dataset::Filter>,
    #[serde(default)]
    pub sort: Vec<crate::dataset::SortSpec>,
    #[serde(default)]
    pub keyword: Option<String>,
    /// 列样式（列名 → { numFmt, rules }）：set_column_format 的规则是整体替换，助手要知道现有的
    #[serde(default)]
    pub column_styles: std::collections::BTreeMap<String, Json>,
}

/// 上下文里最多列出的数据表数
const MAX_CTX_TABLES: usize = 12;

fn table_ctx_text(t: &TableCtx) -> String {
    let mut s = format!("「{}」（数据集「{}」{}", t.name, t.dataset_name, if t.mode == "cells" { "，单元格模式" } else { "" });
    s.push_str(&if t.fields.is_empty() { "，显示全部列".to_string() } else { format!("，显示列：{}", t.fields.join("、")) });
    if !t.filters.is_empty() {
        s.push_str(&format!("，筛选：{}", tools::filters_desc(&t.filters)));
    }
    if !t.sort.is_empty() {
        s.push_str(&format!("，排序：{}", t.sort.iter().map(|x| format!("{}{}", x.column, if x.desc { "降序" } else { "升序" })).collect::<Vec<_>>().join("、")));
    }
    if let Some(k) = t.keyword.as_deref().filter(|k| !k.is_empty()) {
        s.push_str(&format!("，搜索「{k}」"));
    }
    for (col, st) in &t.column_styles {
        let mut bits = Vec::new();
        if let Some(f) = st.get("numFmt").and_then(Json::as_str).filter(|f| *f != "default") {
            bits.push(format!("数字格式 {f}"));
        }
        for r in st.get("rules").and_then(Json::as_array).into_iter().flatten() {
            bits.push(tools::rule_label(col, r));
        }
        if !bits.is_empty() {
            s.push_str(&format!("，「{col}」列样式：{}", bits.join("；")));
        }
    }
    s.push('）');
    s
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatInput {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub context: Context,
    /// 接着回答被中断的上一个问题（不新增提问）
    #[serde(default)]
    pub resume: bool,
}

fn context_text(c: &Context) -> String {
    let mut parts = Vec::new();
    if let Some(s) = &c.sheet_name {
        parts.push(format!("当前工作表「{s}」"));
    }
    if let Some(p) = &c.pivot {
        parts.push(format!("该工作表是数据透视表，定义：{}", p.to_string().chars().take(1500).collect::<String>()));
    }
    if let Some(sel) = &c.selection {
        let mut s = format!("选中区域 {sel}");
        if let Some(v) = c.selection_values.as_ref().filter(|v| !v.is_empty()) {
            let rows: Vec<String> = v.iter().take(20).map(|r| r.iter().take(10).map(|x| x.as_str().map(String::from).unwrap_or_else(|| if x.is_null() { String::new() } else { x.to_string() })).collect::<Vec<_>>().join("\t")).collect();
            s.push_str(&format!("，内容：\n{}", rows.join("\n")));
        }
        parts.push(s);
    }
    if !c.tables.is_empty() {
        let list: Vec<String> = c.tables.iter().take(MAX_CTX_TABLES).map(table_ctx_text).collect();
        parts.push(format!("工作簿里的数据表：{}", list.join("；")));
    }
    if parts.is_empty() { String::new() } else { format!("【界面上下文】{}\n【问题】", parts.join("；")) }
}

fn system_prompt(conn: &Connection) -> AppResult<String> {
    let mut catalog = String::new();
    for d in crate::dataset::list(conn)? {
        let cols: Vec<String> = d.def.columns.iter().take(60).map(|c| format!("{}({})", c.title, crate::kinds::label(c.kind))).collect();
        catalog.push_str(&format!("- 「{}」{} 行：{}{}\n", d.def.name, d.row_count, cols.join("、"), if d.def.columns.len() > 60 { "…" } else { "" }));
    }
    Ok(format!(
        "你是 Gridlake 的数据分析助手，服务电商企业的财务与经营人员。今天是 {today}。

可用数据集：
{catalog}
工作原则：
1. 回答中的每个数字都必须来自工具结果（query_data、compare_periods 等），不能编造、估算或凭常识补充。需要占比、同比、环比时优先用 values 的 show 选项让服务端计算；自己根据查询结果计算时写明算式。
2. 不确定列名或筛选值的写法时，先用 describe_dataset / column_values 确认，筛选值必须与数据中的写法完全一致。
3. 「本月」「上月」「今年」等按今天推算出具体日期范围。同比 = 与上年同期比，环比 = 与上一期比。注意数据的实际起止日期：本期数据不完整（如月中）时要提醒，不要直接与完整月份比较下结论。
4. 回答用简洁的中文：先给结论（含关键数字），再列依据（数据集、筛选条件、口径）。金额保留两位小数，可写成「x.xx 万」。
5. 用户需要一张表、想保留或继续分析结果时，用 create_pivot 生成透视表建议；需要看趋势、对比时用 create_chart 生成图表建议；用户要求写公式、加列时用 write_formula / add_calc_column。跨数据集关联、排名、累计等 query_data 做不到的计算用 run_sql（只读，先汇总再取数）。建议需要用户确认后才会创建，一次回答中最多提 3 个建议，并在回答里说明建议的内容。需求要同时产出多个东西（如「做月度销售透视表并配趋势图」）时，用 propose_plan 合成一张方案卡（2~8 步，用户预览后一次应用、一次撤销），不要拆成多张零散的建议；方案里的步骤不能引用前面步骤才会新建的计算列，但可以调整本方案前面新建的数据表（如「把退款订单放成一张表，金额用万元显示」= create_data_table + set_column_format，表名要一致）。save_query_as_dataset 会在服务端创建数据，不能放进方案。
6. 解释「为什么下降 / 上升」时用 compare_periods，选 1~3 个最相关的维度归因，说明贡献最大的几项。
7. 用户问「有什么异常 / 哪里不对劲 / 最近有没有问题」时先用 find_anomalies 扫描，再对其中最重要的 1~3 项用 query_data / compare_periods 核实原因；扫描结果里的数字可以直接引用，并说明比较的是最近 7 天与此前 28 天。
8. 数据不足以回答时直接说明缺少什么数据，不要猜测。不要在回答里输出工具调用的 JSON。
9. 用户想「只看…」「按…排序」「隐藏某些列」「搜索某个词」，并且界面上下文里有对应的数据表时，用 set_table_view 调整那张数据表的视图（数据表里千万行也只取可视区域），不要把明细行贴进回答。filters / sort / fields / keyword 给了就整体替换该项：要在现有筛选上追加，把界面上下文里列出的现有条件和新条件一起写进 filters；筛选值先用 column_values 确认写法。
10. 用户想让某一列「用万元显示」「显示成百分比」「超过某个值标红」「含某个词加粗」时，用 set_column_format（只改显示、不改数据）。rules 给了就整体替换该列的全部规则：要在现有规则上追加，把界面上下文里列出的现有规则一并写进去。阈值必须来自用户或查询结果；不确定时先用 query_data 看一下分布再定。条件格式只对浮动表格有效，界面上下文里标了「单元格模式」的数据表只能设数字格式（万元也不行）。
11. 用户想「把…放成一张表」「在表格里看…的明细」，而界面上下文里没有合适的数据表时，用 create_data_table 新建（可直接带上筛选、排序、显示列）；已经有对应的数据表就用 set_table_view 调整它，不要重复新建。默认用浮动表格（mode=float）：命中行数很多也只取可视区域；只有用户明确要把数据铺进单元格（如要用 A:A 区域引用或做公式）且命中不超过 1 万行时才用 cells。要汇总用 create_pivot，不要用数据表。
12. 用户想把一条查询的结果「存成数据集」「保存下来以后反复用」时：先用 run_sql 确认结果对、列名各不相同，再用 save_query_as_dataset（用户确认后才会创建）。名称不能与已有数据集重名。只是看一眼汇总、不需要长期保留时不要存，用 create_pivot。
13. 用户问「数据有没有问题 / 数据质量怎么样 / 哪些列缺数据」时先用 profile_dataset 体检，结论引用其中的数字。用户要清洗数据（删重复、去空白、统一写法、补空值、删掉某些行）时：先用 profile_dataset / column_values 找出问题和准确写法，再用 propose_clean 给出建议（会直接修改数据集，用户看过改前改后的预览、确认后才执行，可撤销最近一次）；一次只提一种清洗，多个问题分几次提，并说明先后顺序。清洗和 create_window_dataset 都不能放进方案。
14. 用户想要「每组前 N 名」「累计」「排名」「和上一行比」「移动平均」「占组内比例」这类逐行的结果并保留下来时，用 create_window_dataset 生成新数据集（源数据更新后自动重算）；只是回答一个数字用 query_data / run_sql。",
        today = {
            use chrono::Datelike;
            let now = chrono::Local::now();
            let w = ["一", "二", "三", "四", "五", "六", "日"][now.weekday().num_days_from_monday() as usize];
            format!("{}（星期{w}）", now.format("%Y-%m-%d"))
        }
    ))
}

/// 最近 HISTORY_TURNS 轮提问的起点（保证 tool 消息跟在对应的 assistant 后面）
fn recent_start(stored: &[StoredMessage]) -> usize {
    let user_idx: Vec<usize> = stored.iter().enumerate().filter(|(_, m)| m.role == "user").map(|(i, _)| i).collect();
    user_idx.len().checked_sub(HISTORY_TURNS).map(|k| user_idx[k]).unwrap_or(0)
}

/// 从数据库消息重建模型历史：最近 HISTORY_TURNS 轮
fn history(stored: &[StoredMessage]) -> Vec<Message> {
    stored[recent_start(stored)..]
        .iter()
        .map(|m| Message { role: m.role.clone(), content: m.content.clone(), tool_calls: m.tool_calls.clone(), tool_call_id: m.tool_call_id.clone() })
        .collect()
}

/// 摘要的输入：一条消息写成一行（工具结果截短，只留说明与开头部分）
fn summary_line(m: &StoredMessage) -> Option<String> {
    match m.role.as_str() {
        "user" => Some(format!("用户：{}", m.display.as_deref().or(m.content.as_deref()).unwrap_or(""))),
        "assistant" => m.content.as_deref().filter(|c| !c.trim().is_empty()).map(|c| format!("助手：{c}")),
        "tool" => {
            let meta = m.meta.clone().unwrap_or(Json::Null);
            let name = meta.get("name").and_then(Json::as_str).unwrap_or("");
            let summary = meta.get("summary").and_then(Json::as_str).unwrap_or("");
            let body: String = m.content.as_deref().unwrap_or("").chars().take(SUMMARY_TOOL_CHARS).collect();
            Some(format!("工具 {name}（{summary}）：{body}"))
        }
        _ => None,
    }
}

const SUMMARY_PROMPT: &str = "你负责把一段数据分析对话整理成要点，供之后继续回答时参考。
保留：用户问过的问题；得出的结论与关键数字（带单位、时间范围和口径）；用到的数据集、筛选条件和列名写法；用户提出的偏好与约定（如金额单位、店铺范围）；已生成的透视表 / 图表等建议；还没解决的问题。
去掉寒暄、重复内容和工具调用的细节。数字必须与原文一致，不要推算新数字。
用中文分条列出，不超过 600 字，直接输出要点，不要加开头语。";

/// 把已有摘要和新增的较早对话合并成新摘要
async fn summarize(cfg: &llm::Config, previous: Option<&str>, lines: &[String]) -> AppResult<String> {
    let mut text = String::new();
    if let Some(p) = previous.filter(|p| !p.trim().is_empty()) {
        text.push_str(&format!("【已有摘要】\n{p}\n\n"));
    }
    text.push_str("【需要并入摘要的对话】\n");
    text.push_str(&lines.join("\n"));
    let msgs = vec![Message::system(SUMMARY_PROMPT), Message::user(text)];
    let turn = llm::chat(cfg, &msgs, &json!([]), &mut |_: &str| {}).await?;
    let out = turn.content.trim().to_string();
    if out.is_empty() {
        return Err(AppError::bad("模型没有返回摘要"));
    }
    Ok(out)
}

/// 推送给前端的事件
pub type Events = mpsc::UnboundedSender<Json>;

/// 记下事件并推送给所有订阅者（断开的订阅者自动移除）
fn emit(thread_id: &str, v: Json) {
    if let Ok(mut r) = runs().lock()
        && let Some(run) = r.get_mut(thread_id)
    {
        run.subs.retain(|tx| tx.send(v.clone()).is_ok());
        run.events.push(v);
    }
}

/// 结束：推送最后一个事件后移除（订阅者的通道随之关闭）
fn finish(thread_id: &str, last: Json) {
    emit(thread_id, last);
    if let Ok(mut r) = runs().lock() {
        r.remove(thread_id);
    }
}

/// 处理一次提问（在后台任务中执行，与前端连接无关；事件经 emit 推送给当前的订阅者）
pub async fn chat(state: AppState, thread_id: String, input: ChatInput) {
    let result = chat_inner(&state, &thread_id, input).await;
    match result {
        Ok(()) => finish(&thread_id, json!({ "type": "done" })),
        Err(e) => {
            let msg = e.message();
            tracing::warn!("AI 助手出错：{msg}");
            // 错误也记进会话，便于回看
            let st = state.clone();
            let tid = thread_id.clone();
            let m = msg.clone();
            let _ = tokio::task::spawn_blocking(move || st.connection().map_err(AppError::from).and_then(|conn| save_message(&conn, &tid, &Message { role: "assistant".into(), content: Some(format!("（出错：{m}）")), ..Default::default() }, None, None))).await;
            finish(&thread_id, json!({ "type": "error", "message": msg }));
        }
    }
}

/// 开始前检查：模型已配置、会话存在、提问有效（接着回答时确有被中断的回答）、不在回答中
pub fn begin(conn: &Connection, thread_id: &str, input: &ChatInput) -> AppResult<()> {
    if config().is_none() {
        return Err(AppError::bad("服务端尚未配置 AI 模型（LEDGER_AI_BASE_URL、LEDGER_AI_MODEL），请联系管理员"));
    }
    thread(conn, thread_id)?;
    // 提问有误时直接拒绝，不要等进了后台再报错（那样错误会被记进对话里）
    if input.resume {
        if !is_interrupted(&load_messages(conn, thread_id)?) {
            return Err(AppError::bad("这个对话没有被中断的回答"));
        }
    }
    else if input.message.trim().is_empty() {
        return Err(AppError::bad("请输入问题"));
    }
    let mut r = runs().lock().map_err(|_| anyhow::anyhow!("AI 状态锁已损坏"))?;
    if r.contains_key(thread_id) {
        return Err(AppError::bad("这个对话正在回答中"));
    }
    r.insert(thread_id.to_string(), Run { events: Vec::new(), subs: Vec::new(), cancel: Arc::new(AtomicBool::new(false)) });
    Ok(())
}

async fn blocking<T: Send + 'static>(state: &AppState, f: impl FnOnce(&Connection) -> AppResult<T> + Send + 'static) -> AppResult<T> {
    let st = state.clone();
    let scope = crate::scope::current_task_scope();
    tokio::task::spawn_blocking(move || {
        let conn = st.connection()?;
        crate::scope::run_with(scope, || f(&conn))
    })
    .await?
}

async fn chat_inner(state: &AppState, thread_id: &str, input: ChatInput) -> AppResult<()> {
    let cfg = config().ok_or_else(|| AppError::bad("服务端尚未配置 AI 模型"))?;
    let resume = input.resume;
    let text = input.message.trim().to_string();
    if text.is_empty() && !resume {
        return Err(AppError::bad("请输入问题"));
    }
    let content = format!("{}{text}", context_text(&input.context));
    // 先保存提问（界面立刻能看到），再整理历史；接着回答时不新增提问，从已有的对话继续
    let tid = thread_id.to_string();
    let (stored, summary, summary_seq, user_id, system) = blocking(state, {
        let (text, content) = (text.clone(), content.clone());
        move |conn| {
            let mut stored = load_messages(conn, &tid)?;
            let (summary, summary_seq) = summary_of(conn, &tid)?;
            if resume {
                if !is_interrupted(&stored) {
                    return Err(AppError::bad("这个对话没有被中断的回答"));
                }
                if fill_dangling_tool_calls(conn, &tid, &stored)? > 0 {
                    stored = load_messages(conn, &tid)?;
                }
                let last_user = stored.iter().rev().find(|m| m.role == "user").map(|m| m.id.clone()).unwrap_or_default();
                return Ok((stored, summary, summary_seq, last_user, system_prompt(conn)?));
            }
            let first = !stored.iter().any(|m| m.role == "user");
            let id = save_message(conn, &tid, &Message::user(content), Some(&text), None)?;
            if first {
                let title: String = text.chars().take(30).collect();
                conn.execute("UPDATE ledger_ai_thread SET title = ? WHERE id = ?", params![title, tid])?;
            }
            Ok((stored, summary, summary_seq, id, system_prompt(conn)?))
        }
    })
    .await?;
    emit(thread_id, json!({ "type": "start", "userMessageId": user_id }));

    // 最近几轮之前、还没并入摘要的对话：整理进摘要（失败时退回只带最近几轮）
    let start = recent_start(&stored);
    let pending: Vec<String> = stored[..start].iter().filter(|m| m.seq > summary_seq).filter_map(summary_line).collect();
    let mut summary = summary;
    if !pending.is_empty() {
        emit(thread_id, json!({ "type": "status", "text": "正在整理较早的对话…" }));
        let upto = stored[start - 1].seq;
        match summarize(&cfg, summary.as_deref(), &pending).await {
            Ok(next) => {
                let (tid, n) = (thread_id.to_string(), next.clone());
                blocking(state, move |conn| {
                    conn.execute("UPDATE ledger_ai_thread SET summary = ?, summary_seq = ? WHERE id = ?", params![n, upto, tid])?;
                    Ok(())
                })
                .await?;
                summary = Some(next);
                emit(thread_id, json!({ "type": "summary", "summary": summary }));
            }
            Err(e) => tracing::warn!("AI 对话摘要失败，只带最近 {HISTORY_TURNS} 轮：{}", e.message()),
        }
    }
    let system = match &summary {
        Some(sm) => format!("{system}\n\n【本对话较早部分的摘要】（之前的问答已整理如下，回答时可引用其中的结论；需要最新数字时重新查询）\n{sm}"),
        None => system,
    };
    let mut messages = vec![Message::system(system)];
    messages.extend(history(&stored));
    if !resume {
        messages.push(Message::user(content));
    }

    let tool_defs = tools::definitions();
    let steps = max_steps();
    let cancel = cancel_flag(thread_id);
    for step in 0..steps {
        if cancelled(thread_id) {
            // 也要留下一条结尾，否则这个对话会被当成「被中断」
            emit(thread_id, json!({ "type": "text", "delta": "（已停止）" }));
            let tid = thread_id.to_string();
            blocking(state, move |conn| save_message(conn, &tid, &Message { role: "assistant".into(), content: Some("（已停止）".into()), ..Default::default() }, None, None)).await?;
            return Ok(());
        }
        // 流式文字同时记下来：中途停止时保存已产生的部分
        let streamed = Arc::new(Mutex::new(String::new()));
        let (tid, buf) = (thread_id.to_string(), streamed.clone());
        let mut on_text = move |t: &str| {
            if let Ok(mut b) = buf.lock() {
                b.push_str(t);
            }
            emit(&tid, json!({ "type": "text", "delta": t }));
        };
        let turn = tokio::select! {
            r = llm::chat(&cfg, &messages, &tool_defs, &mut on_text) => r?,
            _ = wait_cancel(cancel.clone()) => {
                let partial = streamed.lock().map(|b| b.clone()).unwrap_or_default();
                let note = if partial.trim().is_empty() { "（已停止）".to_string() } else { format!("{partial}\n\n（已停止）") };
                emit(thread_id, json!({ "type": "text", "delta": if partial.trim().is_empty() { "（已停止）" } else { "\n\n（已停止）" } }));
                let tid = thread_id.to_string();
                blocking(state, move |conn| save_message(conn, &tid, &Message { role: "assistant".into(), content: Some(note), ..Default::default() }, None, None)).await?;
                return Ok(());
            }
        };
        let assistant = Message { role: "assistant".into(), content: Some(turn.content.clone()), tool_calls: turn.tool_calls.clone(), ..Default::default() };
        let tid = thread_id.to_string();
        let a = assistant.clone();
        blocking(state, move |conn| save_message(conn, &tid, &a, None, None)).await?;
        messages.push(assistant);
        if turn.tool_calls.is_empty() {
            return Ok(());
        }
        if !turn.content.trim().is_empty() {
            // 工具调用前的说明文字与后续回答分段显示
            emit(thread_id, json!({ "type": "break" }));
        }
        for call in &turn.tool_calls {
            emit(thread_id, json!({ "type": "tool", "id": call.id, "name": call.function.name, "status": "running" }));
            let (name, args, tid) = (call.function.name.clone(), call.function.arguments.clone(), thread_id.to_string());
            let call_id = call.id.clone();
            let (tool_msg, outcome_summary, ok, action) = blocking(state, move |conn| {
                let o = tools::execute(conn, &name, &args);
                let action = match &o.action {
                    Some(p) => Some(save_action(conn, &tid, p)?),
                    None => None,
                };
                let meta = json!({ "name": name, "summary": o.summary, "ok": o.ok, "actionId": action.as_ref().map(|a| a.id.clone()), "args": serde_json::from_str::<Json>(&args).unwrap_or(Json::Null) });
                let msg = Message::tool(&call_id, o.content.clone());
                save_message(conn, &tid, &msg, None, Some(&meta))?;
                Ok((msg, o.summary, o.ok, action))
            })
            .await?;
            emit(thread_id, json!({ "type": "tool", "id": call.id, "name": call.function.name, "status": if ok { "done" } else { "error" }, "summary": outcome_summary, "actionId": action.as_ref().map(|a| a.id.clone()) }));
            if let Some(a) = action {
                emit(thread_id, json!({ "type": "action", "action": a }));
            }
            messages.push(tool_msg);
        }
        if step + 1 == steps {
            let note = format!("（已调用 {steps} 轮工具仍未得出结论，先停在这里。可以把问题拆小一些，或告诉我从哪个方向继续。）");
            emit(thread_id, json!({ "type": "text", "delta": note }));
            let tid = thread_id.to_string();
            blocking(state, move |conn| save_message(conn, &tid, &Message { role: "assistant".into(), content: Some(note), ..Default::default() }, None, None)).await?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    /// 模型 @ 主机（不含密钥）
    pub model: Option<String>,
}

pub fn status() -> Status {
    let c = config();
    Status { configured: c.is_some(), model: c.map(|c| c.describe()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: &str) -> StoredMessage {
        StoredMessage { id: String::new(), seq: 0, role: role.into(), content: Some(role.into()), tool_calls: vec![], tool_call_id: None, display: None, meta: None, created_at: String::new() }
    }

    #[test]
    fn history_starts_at_user_turn() {
        let mut stored = Vec::new();
        for _ in 0..8 {
            stored.extend([m("user"), m("assistant"), m("tool"), m("assistant")]);
        }
        let h = history(&stored);
        assert_eq!(h.first().unwrap().role, "user");
        assert_eq!(h.iter().filter(|x| x.role == "user").count(), HISTORY_TURNS);
        let ctx = Context { sheet_name: Some("透视表1".into()), selection: Some("B2:C3".into()), selection_values: Some(vec![vec![json!("店铺"), json!(12.5)]]), pivot: None, tables: vec![] };
        assert!(context_text(&ctx).contains("店铺\t12.5"));
        let ctx = Context {
            tables: vec![TableCtx {
                name: "订单表".into(),
                dataset_name: "订单".into(),
                mode: "float".into(),
                fields: vec!["店铺".into(), "金额".into()],
                filters: vec![crate::dataset::Filter::In { column: "店铺".into(), values: vec!["A".into(), "B".into()] }],
                sort: vec![crate::dataset::SortSpec { column: "金额".into(), desc: true }],
                keyword: Some("退款".into()),
                column_styles: Default::default(),
            }],
            ..Default::default()
        };
        let t = context_text(&ctx);
        assert!(t.contains("工作簿里的数据表：「订单表」（数据集「订单」，显示列：店铺、金额，筛选：店铺=A/B，排序：金额降序，搜索「退款」）"), "{t}");
        assert!(context_text(&Context { tables: vec![TableCtx { name: "x".into(), dataset_name: "y".into(), mode: "cells".into(), fields: vec![], filters: vec![], sort: vec![], keyword: None, column_styles: Default::default() }], ..Default::default() }).contains("单元格模式，显示全部列"));
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        // 所有人可编辑的工作簿：能不能看会话只取决于会话本身
        conn.execute("INSERT INTO ledger_workbook (id, name, version, size, stored_size, data, owner_id, owner_name, visibility) VALUES ('wb1', '测试', 1, 0, 0, ''::BLOB, 'u0', 'Root', 'edit')", []).unwrap();
        conn
    }

    fn user(id: &str, name: &str, admin: bool) -> CurrentUser {
        CurrentUser { id: id.into(), name: name.into(), admin, enforced: true }
    }

    fn new_thread(conn: &Connection, wb: &str, owner: &User, name: &str) -> String {
        create_thread(conn, NewThread { workbook_id: wb.into(), operator: Some(name.into()), owner_id: Some(owner.0.into()) }).unwrap().id
    }

    struct User(&'static str);

    #[test]
    fn thread_visibility_owner_admin_and_legacy() {
        let conn = db();
        let (alice, bob, root) = (user("u1", "Alice", false), user("u2", "Bob", false), user("u0", "Root", true));
        let t1 = new_thread(&conn, "wb1", &User("u1"), "Alice");
        let t2 = new_thread(&conn, "wb1", &User("u2"), "Bob");
        // 升级前的会话：没有创建人 ID，只有名字
        conn.execute("INSERT INTO ledger_ai_thread (id, workbook_id, title, created_by) VALUES ('old', 'wb1', '旧', 'Alice')", []).unwrap();
        conn.execute("INSERT INTO ledger_ai_thread (id, workbook_id, title) VALUES ('anon', 'wb1', '无主')", []).unwrap();
        let ids = |u: &CurrentUser| {
            let mut v: Vec<String> = threads(&conn, "wb1", u).unwrap().into_iter().map(|t| t.id).collect();
            v.sort();
            v
        };
        assert_eq!(ids(&alice), { let mut v = vec![t1.clone(), "old".to_string()]; v.sort(); v });
        assert_eq!(ids(&bob), vec![t2.clone()]);
        assert_eq!(ids(&root).len(), 4, "管理员看到全部");
        assert_eq!(ids(&CurrentUser::open()).len(), 4, "没开认证时不限");
        // 访问：Bob 看不到 Alice 的，错误里点明是谁的；管理员可以
        assert!(require_thread(&conn, &t1, &alice).is_ok());
        let err = require_thread(&conn, &t1, &bob).unwrap_err();
        assert!(matches!(&err, AppError::Forbidden(m) if m.contains("Alice")), "{err:?}");
        assert!(require_thread(&conn, &t1, &root).is_ok());
        assert!(require_thread(&conn, "不存在", &bob).is_ok(), "不存在的会话交给接口报错");
        assert!(require_thread(&conn, "anon", &bob).is_err(), "没有创建人的旧会话只有管理员能看");
        assert!(require_thread(&conn, "anon", &root).is_ok());
    }

    #[test]
    fn action_access_follows_thread() {
        let conn = db();
        let t1 = new_thread(&conn, "wb1", &User("u1"), "Alice");
        let a = save_action(&conn, &t1, &json!({ "kind": "pivot", "title": "x" })).unwrap();
        assert!(require_action(&conn, &a.id, &user("u1", "Alice", false)).is_ok());
        assert!(require_action(&conn, &a.id, &user("u2", "Bob", false)).is_err());
        assert!(require_action(&conn, "没有这条", &user("u2", "Bob", false)).is_ok());
    }

    fn put(conn: &Connection, thread: &str, role: &str, content: &str, calls: Vec<llm::ToolCall>, call_id: Option<&str>) {
        let mut msg = Message { role: role.into(), content: Some(content.into()), tool_calls: calls, ..Default::default() };
        msg.tool_call_id = call_id.map(String::from);
        save_message(conn, thread, &msg, None, None).unwrap();
    }

    fn call(id: &str) -> llm::ToolCall {
        serde_json::from_value(json!({ "id": id, "type": "function", "function": { "name": "query_data", "arguments": "{}" } })).unwrap()
    }

    #[test]
    fn interrupted_detection_and_dangling_calls() {
        let conn = db();
        let t = new_thread(&conn, "wb1", &User("u1"), "Alice");
        let state = |conn: &Connection| is_interrupted(&load_messages(conn, &t).unwrap());
        assert!(!state(&conn), "空对话不算");
        put(&conn, &t, "user", "问题", vec![], None);
        assert!(state(&conn), "提问后还没有回答");
        put(&conn, &t, "assistant", "", vec![call("c1"), call("c2")], None);
        assert!(state(&conn), "模型要调用工具但还没有结果");
        put(&conn, &t, "tool", "结果1", vec![], Some("c1"));
        assert!(state(&conn), "只有一个工具有结果");
        // 补齐没有结果的 c2，且不重复补
        let stored = load_messages(&conn, &t).unwrap();
        assert_eq!(fill_dangling_tool_calls(&conn, &t, &stored).unwrap(), 1);
        let stored = load_messages(&conn, &t).unwrap();
        assert_eq!(fill_dangling_tool_calls(&conn, &t, &stored).unwrap(), 0);
        let ids: Vec<_> = stored.iter().filter(|m| m.role == "tool").map(|m| m.tool_call_id.clone().unwrap()).collect();
        assert_eq!(ids, vec!["c1", "c2"]);
        assert!(state(&conn), "工具结果还没交给模型");
        put(&conn, &t, "assistant", "结论", vec![], None);
        assert!(!state(&conn), "回答结束");
        // 出错、停止时留下的助手文字也算结束
        put(&conn, &t, "user", "再问", vec![], None);
        put(&conn, &t, "assistant", "（出错：x）", vec![], None);
        assert!(!state(&conn));
    }

    #[test]
    fn summary_edit_and_clear() {
        let conn = db();
        let t = new_thread(&conn, "wb1", &User("u1"), "Alice");
        conn.execute("UPDATE ledger_ai_thread SET summary = '自动整理', summary_seq = 7 WHERE id = ?", [&t]).unwrap();
        set_summary(&conn, &t, Some("  手改后的摘要  ")).unwrap();
        assert_eq!(summary_of(&conn, &t).unwrap(), (Some("手改后的摘要".to_string()), 7), "只改文字，覆盖到的位置不变");
        assert!(set_summary(&conn, &t, Some(&"字".repeat(MAX_SUMMARY_CHARS + 1))).is_err());
        assert_eq!(summary_of(&conn, &t).unwrap().0.as_deref(), Some("手改后的摘要"), "超长时不改");
        set_summary(&conn, &t, Some("   ")).unwrap();
        assert_eq!(summary_of(&conn, &t).unwrap(), (None, 0), "留空 = 清除，较早的轮次下次重新整理");
        assert!(set_summary(&conn, "没有", None).is_err());
    }
}
