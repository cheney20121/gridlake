//! 事件 Webhook：数据集导入、数据连接同步、填报提交、审批、告警等事件发生时，把一份 JSON POST 到管理员登记的地址，
//! 让外部系统（数据管道、工单、机器人、自建脚本）跟着动起来
//!
//! - 事件先写进 `ledger_webhook_delivery`（发送队列，进程重启不丢），后台任务 [`run`] 发送；
//!   失败（非 2xx、超时、连不上）按 1 分钟、5 分钟、30 分钟、2 小时、6 小时重试，共 6 次
//! - 签名：`X-Gridlake-Signature: t=<秒级时间戳>,v1=<hex(HMAC-SHA256(密钥, "<t>.<请求体>"))>`。接收方用登记时拿到的密钥核对，
//!   并拒绝时间戳太旧的请求（防重放）；密钥只在创建 / 重置时显示一次，库里加密存放
//! - 只发元信息（id、名称、行数、操作人），不发数据行；地址按网址导入同一套规则检查（`netguard`），连接时、每次重定向再检查
//! - 不保证顺序（按 `occurredAt` 排）；同一事件可能投递不止一次（对方处理了但响应超时），按 `X-Gridlake-Delivery` 去重

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use duckdb::{Connection, params};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::auth::CurrentUser;
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::{AppState, parse_json};

/// 可以订阅的事件（名称，说明）。名称是对外的约定：只增不改
pub const EVENTS: &[(&str, &str)] = &[
    ("dataset.created", "新建数据集：导入新建、新建填报数据集、外部 Parquet 数据集"),
    ("dataset.imported", "向已有数据集导入：追加、整体替换、按主键更新"),
    ("dataset.deleted", "数据集移入回收站"),
    ("dataset.refreshed", "外部数据集的文件有变化，重新读取了（手动刷新、自动检查、修改读取方式）"),
    ("sql.refreshed", "SQL 查询重算并写入数据集（手动、定时、同步后）"),
    ("connector.synced", "数据连接同步结束（成功、没有新数据、失败）"),
    ("writeback.submitted", "填报提交"),
    ("collect.reviewed", "收集任务审核：确认、退回、撤销确认"),
    ("approval.created", "发起文件审批"),
    ("approval.decided", "文件审批通过、驳回或撤回"),
    ("alert.triggered", "告警触发（新触发，以及按重复策略再次提醒）"),
];

/// 「测试」按钮发的事件，不能订阅
const PING: &str = "ping";
/// 第 n 次失败后多久重试；用完还失败就放弃
const RETRY_SECS: [u64; 5] = [60, 300, 1800, 7200, 21600];
const TIMEOUT: Duration = Duration::from_secs(10);
/// 发送记录：每个 Webhook 只留最近这么多条，且不超过 7 天
const KEEP_DELIVERIES: i64 = 500;
const KEEP_DAYS: i64 = 7;
const MAX_HOOKS: i64 = 50;

/// 发送队列的唤醒信号，和「哪些事件有人订阅」的缓存（没人订阅时事件直接丢掉，不碰数据库）
#[derive(Default)]
pub struct Hub {
    wake: tokio::sync::Notify,
    /// 启用中的 Webhook 各自订阅的事件；None 表示还没读过库（增删改后清空）
    subs: Mutex<Option<Vec<Vec<String>>>>,
}

impl Hub {
    fn invalidate(&self) {
        if let Ok(mut s) = self.subs.lock() {
            *s = None;
        }
    }

    /// 有没有启用中的 Webhook 订阅了这个事件；还不知道时返回 None
    fn wants(&self, event: &str) -> Option<bool> {
        self.subs.lock().ok()?.as_ref().map(|subs| subs.iter().any(|ev| matches(ev, event)))
    }
}

fn matches(subscribed: &[String], event: &str) -> bool {
    subscribed.iter().any(|e| e == "*" || e == event)
}

/// 第 attempts 次（从 1 起）失败后多久重试；None 表示不再重试
fn retry_after(attempts: i64) -> Option<Duration> {
    let i = usize::try_from(attempts).ok()?.checked_sub(1)?;
    RETRY_SECS.get(i).map(|s| Duration::from_secs(*s))
}

fn sign(secret: &str, ts: i64, body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC 接受任意长度的密钥");
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(body.as_bytes());
    hex(&mac.finalize().into_bytes())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn new_secret() -> String {
    format!("whsec_{}", hex(&crate::account::random_bytes::<24>()))
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", hex(&crate::account::random_bytes::<10>()))
}

// ── 发出事件 ──────────────────────────────────────────

/// 在已有数据库连接的地方（阻塞线程里）发出事件：给每个订阅了它的 Webhook 排一条投递，然后唤醒发送任务。
/// 失败只记日志，不影响业务操作本身
pub fn emit_with(conn: &Connection, hub: &Hub, event: &str, actor: Option<&CurrentUser>, data: Value) {
    match enqueue(conn, hub, event, actor, &data) {
        Ok(0) => {}
        Ok(_) => hub.wake.notify_one(),
        Err(e) => tracing::warn!("Webhook 事件 {event} 排队失败：{}", e.message()),
    }
}

/// 在异步处理函数里发出事件（没人订阅时什么都不做）
pub fn emit(state: &AppState, event: &'static str, actor: Option<&CurrentUser>, data: Value) {
    if state.webhooks.wants(event) == Some(false) {
        return;
    }
    let st = state.clone();
    let actor = actor.cloned();
    tokio::task::spawn_blocking(move || match st.connection() {
        Ok(conn) => emit_with(&conn, &st.webhooks, event, actor.as_ref(), data),
        Err(e) => tracing::warn!("Webhook 事件 {event} 排队失败：{e:#}"),
    });
}

/// 启用中的 Webhook（id，订阅的事件）；顺便刷新缓存
fn enabled_hooks(conn: &Connection, hub: &Hub) -> AppResult<Vec<(String, Vec<String>)>> {
    let mut stmt = conn.prepare("SELECT id, events FROM ledger_webhook WHERE enabled ORDER BY created_at")?;
    let rows: Vec<(String, Vec<String>)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(id, ev)| (id, serde_json::from_str(&ev).unwrap_or_default()))
        .collect();
    if let Ok(mut s) = hub.subs.lock() {
        *s = Some(rows.iter().map(|(_, ev)| ev.clone()).collect());
    }
    Ok(rows)
}

fn actor_json(actor: Option<&CurrentUser>) -> Value {
    match actor {
        Some(u) if !u.id.is_empty() || !u.name.is_empty() => json!({ "id": u.id, "name": u.name }),
        _ => Value::Null,
    }
}

fn enqueue(conn: &Connection, hub: &Hub, event: &str, actor: Option<&CurrentUser>, data: &Value) -> AppResult<usize> {
    if hub.wants(event) == Some(false) {
        return Ok(0);
    }
    let targets: Vec<String> = enabled_hooks(conn, hub)?.into_iter().filter(|(_, ev)| matches(ev, event)).map(|(id, _)| id).collect();
    let now = now_local();
    for hook in &targets {
        let id = new_id("wd");
        let body = json!({ "id": id, "event": event, "occurredAt": now, "actor": actor_json(actor), "data": data });
        conn.execute(
            "INSERT INTO ledger_webhook_delivery (id, webhook_id, event, payload, status, attempts, created_at, next_at)
             VALUES (?, ?, ?, ?, 'pending', 0, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
            params![id, hook, event, body.to_string(), now, now],
        )?;
    }
    Ok(targets.len())
}

// ── 发送 ──────────────────────────────────────────────

struct Due {
    id: String,
    hook_id: String,
    hook_name: String,
    url: Result<String, String>,
    secret: Result<String, String>,
    event: String,
    payload: String,
    attempts: i64,
}

struct Attempt {
    ok: bool,
    http_status: Option<i32>,
    error: Option<String>,
    response: Option<String>,
}

fn client() -> AppResult<reqwest::Client> {
    crate::netguard::client_builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("Gridlake-Webhook/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AppError::bad(format!("创建 HTTP 客户端失败：{e}")))
}

async fn send(client: &reqwest::Client, url: &str, secret: &str, event: &str, delivery: &str, payload: &str) -> Attempt {
    let ts = chrono::Utc::now().timestamp();
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("X-Gridlake-Event", event)
        .header("X-Gridlake-Delivery", delivery)
        .header("X-Gridlake-Signature", format!("t={ts},v1={}", sign(secret, ts, payload)))
        .body(payload.to_string())
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let text: String = r.text().await.unwrap_or_default().chars().take(500).collect();
            let response = (!text.trim().is_empty()).then_some(text);
            if status.is_success() {
                Attempt { ok: true, http_status: Some(i32::from(status.as_u16())), error: None, response }
            } else {
                Attempt { ok: false, http_status: Some(i32::from(status.as_u16())), error: Some(format!("对方返回 HTTP {}", status.as_u16())), response }
            }
        }
        Err(e) => {
            let mut chain = vec![e.to_string()];
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                chain.push(s.to_string());
                src = s.source();
            }
            let why = if e.is_timeout() { format!("{} 秒内没有响应", TIMEOUT.as_secs()) } else { chain.last().cloned().unwrap_or_default() };
            Attempt { ok: false, http_status: None, error: Some(format!("发送失败：{why}")), response: None }
        }
    }
}

fn load_due(conn: &Connection, state: &AppState, limit: i64) -> AppResult<Vec<Due>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.webhook_id, h.name, h.url_enc, h.secret_enc, d.event, d.payload, d.attempts
         FROM ledger_webhook_delivery d JOIN ledger_webhook h ON h.id = d.webhook_id
         WHERE d.status = 'pending' AND h.enabled AND d.next_at <= CAST(? AS TIMESTAMP)
         ORDER BY d.next_at, d.created_at LIMIT ?",
    )?;
    let rows = stmt
        .query_map(params![now_local(), limit], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, i64>(7)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let secret = &state.connectors.secret;
    let undecryptable = |what: &str| format!("Webhook 的{what}无法解密（密钥文件可能已更换），请重新填写");
    Ok(rows
        .into_iter()
        .map(|(id, hook_id, hook_name, url, sec, event, payload, attempts)| Due {
            url: secret.decrypt(&url).map_err(|_| undecryptable("地址")),
            secret: secret.decrypt(&sec).map_err(|_| undecryptable("签名密钥")),
            id,
            hook_id,
            hook_name,
            event,
            payload,
            attempts,
        })
        .collect())
}

/// 记下一次发送的结果：成功 / 还能重试（排到下次）/ 用完重试次数（失败）；同时更新 Webhook 的最近状态
fn record(conn: &Connection, id: &str, hook_id: &str, attempts: i64, a: &Attempt, retry: bool) -> AppResult<String> {
    let now = chrono::Local::now().naive_local();
    let fmt = |t: chrono::NaiveDateTime| t.format("%Y-%m-%d %H:%M:%S").to_string();
    let next = if a.ok || !retry { None } else { retry_after(attempts).and_then(|d| chrono::Duration::from_std(d).ok()).map(|d| fmt(now + d)) };
    let status = if a.ok {
        "success"
    } else if next.is_some() {
        "pending"
    } else {
        "failed"
    };
    conn.execute(
        "UPDATE ledger_webhook_delivery SET status = ?, attempts = ?, http_status = ?, error = ?, response = ?, last_attempt_at = CAST(? AS TIMESTAMP),
           next_at = CAST(? AS TIMESTAMP) WHERE id = ?",
        params![status, attempts, a.http_status, a.error, a.response, fmt(now), next, id],
    )?;
    conn.execute(
        "UPDATE ledger_webhook SET last_status = ?, last_at = CAST(? AS TIMESTAMP), last_error = ? WHERE id = ?",
        params![if a.ok { "success" } else { "failed" }, fmt(now), a.error, hook_id],
    )?;
    Ok(status.to_string())
}

async fn deliver(client: &reqwest::Client, d: &Due) -> Attempt {
    match (&d.url, &d.secret) {
        (Ok(url), Ok(secret)) => match reqwest::Url::parse(url).map_err(|_| AppError::bad("地址格式不正确")).and_then(|u| crate::netguard::check_url(&u)) {
            Ok(()) => send(client, url, secret, &d.event, &d.id, &d.payload).await,
            Err(e) => Attempt { ok: false, http_status: None, error: Some(e.message()), response: None },
        },
        (Err(e), _) | (_, Err(e)) => Attempt { ok: false, http_status: None, error: Some(e.clone()), response: None },
    }
}

async fn blocking<T: Send + 'static>(state: &AppState, f: impl FnOnce(&Connection) -> AppResult<T> + Send + 'static) -> AppResult<T> {
    let st = state.clone();
    tokio::task::spawn_blocking(move || {
        let conn = st.connection()?;
        f(&conn)
    })
    .await?
}

/// 后台发送任务：有新事件时被唤醒，另外每 15 秒看一次有没有到点该重试的；每小时清理一次旧记录
pub async fn run(state: AppState) {
    let client = match client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Webhook 发送任务没有启动：{}", e.message());
            return;
        }
    };
    let mut purge = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tokio::select! {
            () = state.webhooks.wake.notified() => {}
            () = tokio::time::sleep(Duration::from_secs(15)) => {}
            _ = purge.tick() => {
                if let Err(e) = blocking(&state, purge_old).await {
                    tracing::warn!("清理 Webhook 发送记录失败：{}", e.message());
                }
            }
        }
        loop {
            let st = state.clone();
            let due = match blocking(&state, move |c| load_due(c, &st, 16)).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("读取 Webhook 发送队列失败：{}", e.message());
                    break;
                }
            };
            if due.is_empty() {
                break;
            }
            let attempts = futures_util::future::join_all(due.iter().map(|d| deliver(&client, d))).await;
            let saved = blocking(&state, move |c| {
                for (d, a) in due.iter().zip(&attempts) {
                    let status = record(c, &d.id, &d.hook_id, d.attempts + 1, a, true)?;
                    if status == "failed" {
                        tracing::warn!("Webhook「{}」的 {} 事件发送 {} 次都失败，已放弃：{}", d.hook_name, d.event, d.attempts + 1, a.error.as_deref().unwrap_or(""));
                    }
                }
                Ok(())
            })
            .await;
            if let Err(e) = saved {
                tracing::warn!("记录 Webhook 发送结果失败：{}", e.message());
                break;
            }
        }
    }
}

fn purge_old(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "DELETE FROM ledger_webhook_delivery WHERE status <> 'pending' AND created_at < CAST(? AS TIMESTAMP) - to_days(CAST(? AS INTEGER))",
        params![now_local(), KEEP_DAYS],
    )?;
    conn.execute(
        "DELETE FROM ledger_webhook_delivery WHERE id IN (
           SELECT id FROM (SELECT id, row_number() OVER (PARTITION BY webhook_id ORDER BY created_at DESC, rowid DESC) AS n FROM ledger_webhook_delivery) WHERE n > ?)",
        params![KEEP_DELIVERIES],
    )?;
    Ok(())
}

// ── 管理（只有管理员） ────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInput {
    pub name: String,
    /// 接收地址（http / https）；修改时不填表示不变
    #[serde(default)]
    pub url: Option<String>,
    /// 订阅的事件名（见 `GET /ledger/webhooks` 的 events）；`*` 表示全部
    pub events: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventInfo {
    name: &'static str,
    description: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookView {
    id: String,
    name: String,
    /// 地址（查询参数部分隐藏：里面常带令牌）
    url: String,
    events: Vec<String>,
    enabled: bool,
    created_by: String,
    created_at: String,
    last_status: Option<String>,
    last_at: Option<String>,
    last_error: Option<String>,
    /// 排队等待发送 / 重试的个数
    pending: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookList {
    hooks: Vec<HookView>,
    events: Vec<EventInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedHook {
    id: String,
    /// 签名密钥：只在这里显示一次
    secret: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryView {
    id: String,
    event: String,
    /// pending 等待发送 / 重试，success 成功，failed 重试用完仍失败，cancelled Webhook 被停用时取消
    status: String,
    attempts: i64,
    http_status: Option<i32>,
    error: Option<String>,
    /// 对方响应的前 500 个字
    response: Option<String>,
    created_at: String,
    last_attempt_at: Option<String>,
    next_at: Option<String>,
    payload: Value,
}

fn mask_url(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?…"),
        None => url.to_string(),
    }
}

fn validate(conn: &Connection, input: &HookInput, id: Option<&str>) -> AppResult<(String, Vec<String>)> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::bad("请填写 Webhook 名称"));
    }
    if name.chars().count() > 50 {
        return Err(AppError::bad("Webhook 名称不能超过 50 个字"));
    }
    let dup: i64 = conn.query_row("SELECT count(*) FROM ledger_webhook WHERE name = ? AND id <> ?", params![name, id.unwrap_or("")], |r| r.get(0))?;
    if dup > 0 {
        return Err(AppError::bad(format!("已有名为「{name}」的 Webhook")));
    }
    let mut events: Vec<String> = Vec::new();
    for e in &input.events {
        let e = e.trim();
        if e != "*" && !EVENTS.iter().any(|(n, _)| *n == e) {
            return Err(AppError::bad(format!("不认识的事件：{e}")));
        }
        if !events.iter().any(|x| x == e) {
            events.push(e.to_string());
        }
    }
    if events.is_empty() {
        return Err(AppError::bad("请至少选择一个事件"));
    }
    if events.iter().any(|e| e == "*") {
        events = vec!["*".into()];
    }
    Ok((name, events))
}

fn check_target(url: &str) -> AppResult<String> {
    let url = url.trim();
    let parsed = reqwest::Url::parse(url).map_err(|_| AppError::bad("接收地址格式不正确，应以 http:// 或 https:// 开头"))?;
    crate::netguard::check_url(&parsed)?;
    Ok(url.to_string())
}

fn list(conn: &Connection, state: &AppState) -> AppResult<HookList> {
    let mut stmt = conn.prepare(
        "SELECT h.id, h.name, h.url_enc, h.events, h.enabled, COALESCE(h.created_by, ''), strftime(h.created_at, '%Y-%m-%d %H:%M:%S'),
                h.last_status, strftime(h.last_at, '%Y-%m-%d %H:%M:%S'), h.last_error,
                (SELECT count(*) FROM ledger_webhook_delivery d WHERE d.webhook_id = h.id AND d.status = 'pending')
         FROM ledger_webhook h ORDER BY h.created_at",
    )?;
    let secret = &state.connectors.secret;
    let hooks = stmt
        .query_map([], |r| {
            Ok(HookView {
                id: r.get(0)?,
                name: r.get(1)?,
                url: r.get(2)?,
                events: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                enabled: r.get(4)?,
                created_by: r.get(5)?,
                created_at: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                last_status: r.get(7)?,
                last_at: r.get(8)?,
                last_error: r.get(9)?,
                pending: r.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|mut h| {
            h.url = secret.decrypt(&h.url).map_or_else(|_| "（无法解密，请重新填写）".into(), |u| mask_url(&u));
            h
        })
        .collect();
    Ok(HookList { hooks, events: EVENTS.iter().map(|(name, description)| EventInfo { name, description }).collect() })
}

fn create(conn: &Connection, state: &AppState, user: &CurrentUser, input: &HookInput) -> AppResult<CreatedHook> {
    let (name, events) = validate(conn, input, None)?;
    let url = check_target(input.url.as_deref().unwrap_or(""))?;
    let n: i64 = conn.query_row("SELECT count(*) FROM ledger_webhook", [], |r| r.get(0))?;
    if n >= MAX_HOOKS {
        return Err(AppError::bad(format!("Webhook 最多 {MAX_HOOKS} 个")));
    }
    let id = new_id("wh");
    let secret = new_secret();
    let s = &state.connectors.secret;
    conn.execute(
        "INSERT INTO ledger_webhook (id, name, url_enc, secret_enc, events, enabled, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params![id, name, s.encrypt(&url)?, s.encrypt(&secret)?, serde_json::to_string(&events)?, input.enabled, user.name, now_local()],
    )?;
    state.webhooks.invalidate();
    Ok(CreatedHook { id, secret })
}

fn exists(conn: &Connection, id: &str) -> AppResult<()> {
    let n: i64 = conn.query_row("SELECT count(*) FROM ledger_webhook WHERE id = ?", params![id], |r| r.get(0))?;
    if n == 0 {
        return Err(AppError::bad("Webhook 不存在"));
    }
    Ok(())
}

fn update(conn: &Connection, state: &AppState, id: &str, input: &HookInput) -> AppResult<()> {
    exists(conn, id)?;
    let (name, events) = validate(conn, input, Some(id))?;
    if let Some(url) = input.url.as_deref().filter(|u| !u.trim().is_empty()) {
        let url = check_target(url)?;
        conn.execute("UPDATE ledger_webhook SET url_enc = ? WHERE id = ?", params![state.connectors.secret.encrypt(&url)?, id])?;
    }
    conn.execute("UPDATE ledger_webhook SET name = ?, events = ?, enabled = ? WHERE id = ?", params![name, serde_json::to_string(&events)?, input.enabled, id])?;
    if !input.enabled {
        // 停用：排队中的不再发送（重新启用后也不补发，免得一下子涌过去很多旧事件）
        conn.execute("UPDATE ledger_webhook_delivery SET status = 'cancelled', next_at = NULL WHERE webhook_id = ? AND status = 'pending'", params![id])?;
    }
    state.webhooks.invalidate();
    Ok(())
}

fn delete(conn: &Connection, state: &AppState, id: &str) -> AppResult<()> {
    exists(conn, id)?;
    conn.execute("DELETE FROM ledger_webhook_delivery WHERE webhook_id = ?", params![id])?;
    conn.execute("DELETE FROM ledger_webhook WHERE id = ?", params![id])?;
    state.webhooks.invalidate();
    Ok(())
}

fn rotate_secret(conn: &Connection, state: &AppState, id: &str) -> AppResult<String> {
    exists(conn, id)?;
    let secret = new_secret();
    conn.execute("UPDATE ledger_webhook SET secret_enc = ? WHERE id = ?", params![state.connectors.secret.encrypt(&secret)?, id])?;
    Ok(secret)
}

fn deliveries(conn: &Connection, id: &str, limit: i64) -> AppResult<Vec<DeliveryView>> {
    exists(conn, id)?;
    let mut stmt = conn.prepare(
        "SELECT id, event, status, attempts, http_status, error, response, strftime(created_at, '%Y-%m-%d %H:%M:%S'),
                strftime(last_attempt_at, '%Y-%m-%d %H:%M:%S'), strftime(next_at, '%Y-%m-%d %H:%M:%S'), payload
         FROM ledger_webhook_delivery WHERE webhook_id = ? ORDER BY created_at DESC, rowid DESC LIMIT ?",
    )?;
    let rows = stmt
        .query_map(params![id, limit], |r| {
            Ok(DeliveryView {
                id: r.get(0)?,
                event: r.get(1)?,
                status: r.get(2)?,
                attempts: r.get(3)?,
                http_status: r.get(4)?,
                error: r.get(5)?,
                response: r.get(6)?,
                created_at: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                last_attempt_at: r.get(8)?,
                next_at: r.get(9)?,
                payload: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or(Value::Null),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 重新发送：放回队列立即发（重试次数从头算）
fn redeliver(conn: &Connection, id: &str) -> AppResult<()> {
    let n = conn.execute(
        "UPDATE ledger_webhook_delivery SET status = 'pending', attempts = 0, next_at = CAST(? AS TIMESTAMP) WHERE id = ?
           AND webhook_id IN (SELECT id FROM ledger_webhook WHERE enabled)",
        params![now_local(), id],
    )?;
    if n == 0 {
        let known: i64 = conn.query_row("SELECT count(*) FROM ledger_webhook_delivery WHERE id = ?", params![id], |r| r.get(0))?;
        return Err(if known == 0 { AppError::bad("发送记录不存在（可能已过期清理）") } else { AppError::bad("Webhook 已停用，请先启用") });
    }
    Ok(())
}

// ── 接口 ──────────────────────────────────────────────

/// GET /api/ledger/webhooks  Webhook 列表与可订阅的事件
pub async fn list_hooks(State(db): State<AppState>) -> AppResult<Json<Envelope<HookList>>> {
    let st = db.clone();
    Ok(ok(blocking(&db, move |c| list(c, &st)).await?))
}

/// POST /api/ledger/webhooks  { name, url, events, enabled? }  新建 Webhook，返回 id 和签名密钥（只显示这一次）
pub async fn create_hook(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<CreatedHook>>> {
    let input: HookInput = parse_json(&body)?;
    let st = db.clone();
    let r = blocking(&db, move |c| create(c, &st, &user, &input)).await?;
    db.checkpoint();
    Ok(ok(r))
}

/// PUT /api/ledger/webhooks/{id}  { name, url?, events, enabled }  修改（地址不填表示不变；停用时取消排队中的发送）
pub async fn update_hook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: HookInput = parse_json(&body)?;
    let st = db.clone();
    blocking(&db, move |c| update(c, &st, &id, &input)).await?;
    db.checkpoint();
    Ok(ok(true))
}

/// DELETE /api/ledger/webhooks/{id}  删除（连同发送记录）
pub async fn delete_hook(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    let st = db.clone();
    blocking(&db, move |c| delete(c, &st, &id)).await?;
    db.checkpoint();
    Ok(ok(true))
}

/// POST /api/ledger/webhooks/{id}/secret  重置签名密钥，返回新密钥（只显示这一次；旧密钥立即失效）
pub async fn rotate_hook_secret(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<String>>> {
    let st = db.clone();
    let s = blocking(&db, move |c| rotate_secret(c, &st, &id)).await?;
    db.checkpoint();
    Ok(ok(s))
}

/// POST /api/ledger/webhooks/{id}/test  立即发一个 ping 事件（不重试），返回这次发送的结果；停用的也能测试
pub async fn test_hook(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> AppResult<Json<Envelope<DeliveryView>>> {
    let st = db.clone();
    let hook = id.clone();
    let d = blocking(&db, move |c| {
        exists(c, &hook)?;
        let did = new_id("wd");
        let now = now_local();
        let body = json!({ "id": did, "event": PING, "occurredAt": now, "actor": actor_json(Some(&user)), "data": { "message": "Gridlake Webhook 测试" } });
        c.execute(
            "INSERT INTO ledger_webhook_delivery (id, webhook_id, event, payload, status, attempts, created_at) VALUES (?, ?, ?, ?, 'pending', 0, CAST(? AS TIMESTAMP))",
            params![did, hook, PING, body.to_string(), now],
        )?;
        let (url, sec, name): (String, String, String) = c.query_row("SELECT url_enc, secret_enc, name FROM ledger_webhook WHERE id = ?", params![hook], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let secret = &st.connectors.secret;
        Ok(Due {
            url: secret.decrypt(&url).map_err(|_| "Webhook 的地址无法解密（密钥文件可能已更换），请重新填写".to_string()),
            secret: secret.decrypt(&sec).map_err(|_| "Webhook 的签名密钥无法解密，请重置密钥".to_string()),
            id: did,
            hook_id: hook,
            hook_name: name,
            event: PING.into(),
            payload: body.to_string(),
            attempts: 0,
        })
    })
    .await?;
    let started = Instant::now();
    let a = deliver(&client()?, &d).await;
    tracing::info!(hook = %d.hook_name, ok = a.ok, ms = started.elapsed().as_millis() as u64, "webhook test");
    let (did, hook) = (d.id.clone(), d.hook_id.clone());
    let view = blocking(&db, move |c| {
        record(c, &did, &hook, 1, &a, false)?;
        deliveries(c, &hook, KEEP_DELIVERIES)?.into_iter().find(|v| v.id == did).ok_or_else(|| AppError::bad("发送记录丢失"))
    })
    .await?;
    db.checkpoint();
    Ok(ok(view))
}

#[derive(Deserialize)]
pub struct DeliveriesQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

/// GET /api/ledger/webhooks/{id}/deliveries?limit=  最近的发送记录（新的在前，含请求体与对方响应）
pub async fn hook_deliveries(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<DeliveriesQuery>) -> AppResult<Json<Envelope<Vec<DeliveryView>>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, KEEP_DELIVERIES);
    Ok(ok(blocking(&db, move |c| deliveries(c, &id, limit)).await?))
}

/// POST /api/ledger/webhooks/deliveries/{id}/redeliver  重新发送一条记录（同一个请求体与 X-Gridlake-Delivery，签名时间戳是新的）
pub async fn redeliver_hook(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |c| redeliver(c, &id)).await?;
    db.webhooks.wake.notify_one();
    Ok(ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_schedule() {
        assert_eq!(retry_after(0), None);
        assert_eq!(retry_after(1), Some(Duration::from_secs(60)));
        assert_eq!(retry_after(5), Some(Duration::from_secs(21600)));
        assert_eq!(retry_after(6), None, "共 6 次，第 6 次失败后放弃");
    }

    #[test]
    fn signature_matches_reference() {
        // python3 -c "import hmac,hashlib;print(hmac.new(b'whsec_x',b'1700000000.{\"a\":1}',hashlib.sha256).hexdigest())"
        assert_eq!(sign("whsec_x", 1_700_000_000, r#"{"a":1}"#), "b3438763ab46d9634b9844c8722818eaf523f803db1327484c6067462a5fd6d1");
    }

    #[test]
    fn subscriptions() {
        let hub = Hub::default();
        assert_eq!(hub.wants("dataset.created"), None);
        *hub.subs.lock().unwrap() = Some(vec![vec!["dataset.created".into()], vec![]]);
        assert_eq!(hub.wants("dataset.created"), Some(true));
        assert_eq!(hub.wants("alert.triggered"), Some(false));
        *hub.subs.lock().unwrap() = Some(vec![vec!["*".into()]]);
        assert_eq!(hub.wants("alert.triggered"), Some(true));
        hub.invalidate();
        assert_eq!(hub.wants("alert.triggered"), None);
        assert_eq!(mask_url("https://x.example/hook?token=abc"), "https://x.example/hook?…");
        assert_eq!(mask_url("https://x.example/hook"), "https://x.example/hook");
    }
}
