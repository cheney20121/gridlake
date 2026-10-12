//! 实时协作：同一个工作簿的多个用户通过服务端中转 Univer 的变更（mutation），互相看到对方的修改与光标。
//!
//! 传输：下行用 SSE（`GET /collab/{id}/events`，与 AI 助手同一套，带登录头、穿过网关 / 代理都没问题），上行用 POST（`/collab/{id}/send`）。
//! 服务端**不解释**变更的内容，只做三件事：给每批变更编一个全局序号（所有人按同一个顺序收到，包括发送者自己的回显）、
//! 记下「最近一次保存之后」的变更日志（新加入的人先载入已保存版本，再按日志补齐）、转发在线状态（谁在、光标在哪）。
//!
//! 一致性约定（客户端配合）：
//! - 保存时带上 `x-collab-seq`（保存的快照已包含的最后一个序号，保存前先等自己的变更都收到回显）；服务端据此丢掉日志里已经在快照里的部分，
//!   并通知所有人「已保存，版本 N」，各客户端把自己的基准版本更新到 N，所以多人先后保存不会互相冲突
//! - 同一个格子同时被改：以服务端的顺序为准，后到的覆盖先到的（客户端对自己还没收到回显的格子跳过别人的更早的修改）
//! - 没带 `x-collab-seq` 的写入（如恢复历史版本）会让房间里所有人重新载入
//! - 日志太大（超过 64 MB 或 2 万批）时清空日志并要求在线的人保存一次，期间加入的人要等保存完成

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use serde::Deserialize;
use serde_json::{Value as Json2, json};
use tokio::sync::mpsc;

use crate::auth::{CurrentUser, Need, run};
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::AppState;

type Tx = mpsc::UnboundedSender<Json2>;

const COLORS: &[&str] = &["#f97316", "#8b5cf6", "#06b6d4", "#10b981", "#f59e0b", "#ef4444", "#3b82f6", "#ec4899"];
const MAX_CLIENTS: usize = 30;
const MAX_BATCH_BYTES: usize = 1024 * 1024;
const MAX_LOG_BYTES: usize = 64 * 1024 * 1024;
const MAX_LOG_BATCHES: usize = 20_000;

struct Client {
    id: String,
    user_id: String,
    name: String,
    color: String,
    can_edit: bool,
    tx: Tx,
}

struct Entry {
    seq: u64,
    bytes: usize,
    msg: Json2,
}

#[derive(Default)]
struct Room {
    clients: Vec<Client>,
    seq: u64,
    log: VecDeque<Entry>,
    log_bytes: usize,
    /// 日志是相对哪个已保存版本记的
    base_version: i64,
    /// 日志被清空过：序号 ≤ dropped_upto 的变更已经不在日志里，要有人保存了不早于它的快照才完整
    dropped_upto: u64,
    complete: bool,
    colors_given: usize,
}

fn rooms() -> &'static Mutex<HashMap<String, Room>> {
    static R: OnceLock<Mutex<HashMap<String, Room>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn peer_json(c: &Client) -> Json2 {
    json!({ "id": c.id, "name": c.name, "color": c.color, "canEdit": c.can_edit })
}

fn broadcast(room: &mut Room, msg: &Json2, except: Option<&str>) {
    room.clients.retain(|c| except == Some(c.id.as_str()) || c.tx.send(msg.clone()).is_ok());
}

/// 在线的人数（给测试与运维看）
#[allow(dead_code)]
pub fn online(workbook_id: &str) -> usize {
    rooms().lock().map(|r| r.get(workbook_id).map_or(0, |x| x.clients.len())).unwrap_or(0)
}

/// 连接断开：移出房间并通知其他人；最后一个人走了就删除房间（日志不再有用）
fn leave(workbook_id: &str, client_id: &str) {
    let Ok(mut map) = rooms().lock() else { return };
    let Some(room) = map.get_mut(workbook_id) else { return };
    room.clients.retain(|c| c.id != client_id);
    if room.clients.is_empty() {
        map.remove(workbook_id);
        return;
    }
    broadcast(room, &json!({ "type": "leave", "id": client_id }), None);
}

struct Guard {
    workbook_id: String,
    client_id: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        leave(&self.workbook_id, &self.client_id);
    }
}

#[derive(Deserialize)]
pub struct EventsQuery {
    pub client: String,
    /// 客户端载入的工作簿版本（房间是新建的时候作为日志的基准版本）
    #[serde(default)]
    pub version: i64,
    /// 没开登录校验时的显示名
    #[serde(default)]
    pub name: String,
}

fn valid_client_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// GET /api/ledger/collab/{id}/events?client=&version=&name=  加入房间，SSE 推送：hello / join / leave / presence / ops / saved / replaced / request-save / stale
pub async fn events(
    State(db): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(workbook_id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> AppResult<axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>> {
    if !valid_client_id(&q.client) {
        return Err(AppError::bad("client 不合法"));
    }
    let (wb, u) = (workbook_id.clone(), user.clone());
    let can_edit = run(&db, move |c| Ok(crate::workbook::require(c, &wb, &u, Need::Edit, false).is_ok())).await?;
    let (tx, rx) = mpsc::unbounded_channel::<Json2>();
    let name = if user.enforced && !user.name.is_empty() { user.name.clone() } else { q.name.chars().take(30).collect::<String>() };
    let name = if name.trim().is_empty() { "匿名".to_string() } else { name };
    {
        let mut map = rooms().lock().map_err(|_| anyhow::anyhow!("协作状态锁已损坏"))?;
        let room = map.entry(workbook_id.clone()).or_insert_with(|| Room { base_version: q.version, complete: true, ..Default::default() });
        if room.clients.len() >= MAX_CLIENTS {
            return Err(AppError::bad(format!("这个工作簿同时在线的人太多了（最多 {MAX_CLIENTS} 人）")));
        }
        // 同一个客户端重连：先把旧连接移走
        room.clients.retain(|c| c.id != q.client);
        let color = COLORS[room.colors_given % COLORS.len()].to_string();
        room.colors_given += 1;
        let me = Client { id: q.client.clone(), user_id: user.id.clone(), name, color, can_edit, tx: tx.clone() };
        let peers: Vec<Json2> = room.clients.iter().map(peer_json).collect();
        let log: Vec<Json2> = room.log.iter().map(|e| e.msg.clone()).collect();
        let need_save = !room.complete;
        let hello = json!({ "type": "hello", "self": peer_json(&me), "peers": peers, "baseVersion": room.base_version, "seq": room.seq, "log": log, "needSave": need_save });
        let join = json!({ "type": "join", "peer": peer_json(&me) });
        let _ = tx.send(hello);
        broadcast(room, &join, None);
        room.clients.push(me);
        if need_save && let Some(e) = room.clients.iter().find(|c| c.can_edit && c.id != q.client) {
            let _ = e.tx.send(json!({ "type": "request-save" }));
        }
    }
    let guard = Guard { workbook_id, client_id: q.client };
    let stream = futures_util::stream::unfold((rx, guard), |(mut rx, g)| async move {
        rx.recv().await.map(|v| (Ok(axum::response::sse::Event::default().data(v.to_string())), (rx, g)))
    });
    Ok(axum::response::sse::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendBody {
    pub client: String,
    /// ops：一批变更；presence：在线状态（选区）；stale：这次修改太大没有同步，请别人重新载入
    pub kind: String,
    #[serde(default)]
    pub batch_id: String,
    #[serde(default)]
    pub ops: Vec<Json2>,
    #[serde(default)]
    pub presence: Json2,
}

/// POST /api/ledger/collab/{id}/send  { client, kind, batchId?, ops?, presence? }
pub async fn send(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(workbook_id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<Json2>>> {
    if body.len() > MAX_BATCH_BYTES {
        return Err(AppError::bad("这次修改太大，不能实时同步"));
    }
    let req: SendBody = serde_json::from_slice(&body).map_err(|e| AppError::bad(format!("请求格式不对：{e}")))?;
    let _ = db;
    let mut map = rooms().lock().map_err(|_| anyhow::anyhow!("协作状态锁已损坏"))?;
    let room = map.get_mut(&workbook_id).ok_or_else(|| AppError::bad("还没有加入这个工作簿的协作（请先连接）"))?;
    let me = room.clients.iter().find(|c| c.id == req.client).ok_or_else(|| AppError::bad("还没有加入这个工作簿的协作（请先连接）"))?;
    // 不能冒充别人的连接
    if user.enforced && me.user_id != user.id {
        return Err(AppError::Forbidden("这个连接不属于你".into()));
    }
    let (from, can_edit) = (me.id.clone(), me.can_edit);
    match req.kind.as_str() {
        "ops" => {
            if !can_edit {
                return Err(AppError::Forbidden("你对这个工作簿没有编辑权限，不能同步修改".into()));
            }
            if req.ops.is_empty() {
                return Ok(ok(json!({ "seq": room.seq })));
            }
            room.seq += 1;
            let seq = room.seq;
            let msg = json!({ "type": "ops", "from": from, "seq": seq, "batchId": req.batch_id, "ops": req.ops });
            room.log_bytes += body.len();
            room.log.push_back(Entry { seq, bytes: body.len(), msg: msg.clone() });
            broadcast(room, &msg, None);
            if room.log_bytes > MAX_LOG_BYTES || room.log.len() > MAX_LOG_BATCHES {
                room.dropped_upto = seq;
                room.log.clear();
                room.log_bytes = 0;
                room.complete = false;
                if let Some(e) = room.clients.iter().find(|c| c.can_edit) {
                    let _ = e.tx.send(json!({ "type": "request-save" }));
                }
            }
            Ok(ok(json!({ "seq": seq })))
        }
        "presence" => {
            broadcast(room, &json!({ "type": "presence", "from": from, "presence": req.presence }), Some(&from));
            Ok(ok(json!({ "seq": room.seq })))
        }
        "stale" => {
            if !can_edit {
                return Err(AppError::Forbidden("没有编辑权限".into()));
            }
            broadcast(room, &json!({ "type": "stale", "from": from }), Some(&from));
            Ok(ok(json!({ "seq": room.seq })))
        }
        _ => Err(AppError::bad("kind 应为 ops、presence 或 stale")),
    }
}

/// 工作簿保存成功：带了 `x-collab-seq` 的是协作客户端保存的快照（已包含序号 ≤ applied 的变更）；
/// 没带的是别的方式写入（恢复历史版本等），房间里的人需要重新载入
pub fn saved(workbook_id: &str, version: i64, applied: Option<u64>, by: &str) {
    let Ok(mut map) = rooms().lock() else { return };
    let Some(room) = map.get_mut(workbook_id) else { return };
    room.base_version = version;
    match applied {
        Some(a) => {
            while room.log.front().is_some_and(|e| e.seq <= a) {
                if let Some(e) = room.log.pop_front() {
                    room.log_bytes = room.log_bytes.saturating_sub(e.bytes);
                }
            }
            if !room.complete && a >= room.dropped_upto {
                room.complete = true;
            }
            broadcast(room, &json!({ "type": "saved", "version": version, "by": by, "seq": a }), None);
        }
        None => {
            room.log.clear();
            room.log_bytes = 0;
            room.complete = true;
            broadcast(room, &json!({ "type": "replaced", "version": version, "by": by }), None);
        }
    }
}
