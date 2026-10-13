//! 内置账号（LEDGER_AUTH_MODE=local）：用户名 + 密码登录，账号由管理员在界面里管理
//!
//! - 密码：PBKDF2-HMAC-SHA256（60 万次迭代、16 字节随机盐），存成 `pbkdf2_sha256$迭代次数$盐$哈希`（base64）
//! - 会话：登录得到 32 字节随机令牌，库里只存它的 SHA-256；按 LEDGER_SESSION_DAYS 过期；改密码、停用、重置密码时作废
//! - 防暴力尝试：同一用户名 10 分钟内失败 5 次后暂时拒绝；用户名不存在与密码错误返回同样的提示
//! - 首次启动没有任何账号时，按 LEDGER_ADMIN_USER / LEDGER_ADMIN_PASSWORD 建一个管理员（没设密码就随机生成并打印在日志里）
//! - 登录后的「当前用户」与网关模式同构（id 为用户名），审计、行级权限、审批等不需要区分登录方式

use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Extension, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use duckdb::{Connection, OptionalExt, params};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::{CurrentUser, run};
use crate::config::AuthMode;
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::{AppState, parse_json};

const ITERATIONS: u32 = 600_000;
const MIN_PASSWORD: usize = 8;
const MAX_FAILURES: u32 = 5;
const LOCK_WINDOW: Duration = Duration::from_secs(600);

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

/// PBKDF2-HMAC-SHA256，输出 32 字节（一个块）
fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mac = Hmac::<Sha256>::new_from_slice(password).expect("HMAC 接受任意长度的密钥");
    let mut m = mac.clone();
    m.update(salt);
    m.update(&1u32.to_be_bytes());
    let mut u: [u8; 32] = m.finalize().into_bytes().into();
    let mut out = u;
    for _ in 1..iterations {
        let mut m = mac.clone();
        m.update(&u);
        u = m.finalize().into_bytes().into();
        for (o, x) in out.iter_mut().zip(u.iter()) {
            *o ^= x;
        }
    }
    out
}

pub fn hash_password(password: &str) -> String {
    let salt: [u8; 16] = random_bytes();
    let hash = pbkdf2(password.as_bytes(), &salt, ITERATIONS);
    format!("pbkdf2_sha256${ITERATIONS}${}${}", STANDARD.encode(salt), STANDARD.encode(hash))
}

/// 校验密码（常量时间比较）；格式不认识时返回 false
pub fn verify_password(password: &str, stored: &str) -> bool {
    let mut it = stored.split('$');
    let (Some("pbkdf2_sha256"), Some(iter), Some(salt), Some(hash), None) = (it.next(), it.next(), it.next(), it.next(), it.next()) else { return false };
    let (Ok(iter), Ok(salt), Ok(hash)) = (iter.parse::<u32>(), STANDARD.decode(salt), STANDARD.decode(hash)) else { return false };
    if iter == 0 || hash.len() != 32 {
        return false;
    }
    let got = pbkdf2(password.as_bytes(), &salt, iter);
    got.iter().zip(hash.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

pub fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn check_password(p: &str) -> AppResult<()> {
    if p.chars().count() < MIN_PASSWORD {
        return Err(AppError::bad(format!("密码至少 {MIN_PASSWORD} 位")));
    }
    if p.len() > 200 {
        return Err(AppError::bad("密码太长"));
    }
    Ok(())
}

fn check_username(u: &str) -> AppResult<()> {
    let ok = (2..=32).contains(&u.chars().count()) && u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@'));
    if !ok {
        return Err(AppError::bad("用户名为 2 ~ 32 位，只能包含字母、数字和 _ - . @"));
    }
    Ok(())
}

pub(crate) fn expires_after(days: i64) -> String {
    (chrono::Local::now() + chrono::Duration::days(days)).format("%Y-%m-%d %H:%M:%S").to_string()
}

// ── 账号 ──────────────────────────────────────────────

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub username: String,
    pub name: String,
    pub admin: bool,
    pub disabled: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

const COLS: &str = "id, username, name, admin, disabled, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(last_login_at, '%Y-%m-%d %H:%M:%S')";

fn read(r: &duckdb::Row) -> duckdb::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        username: r.get(1)?,
        name: r.get(2)?,
        admin: r.get(3)?,
        disabled: r.get(4)?,
        created_at: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        last_login_at: r.get(6)?,
    })
}

pub fn list(conn: &Connection) -> AppResult<Vec<Account>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM ledger_account ORDER BY created_at, username"))?;
    Ok(stmt.query_map([], read)?.collect::<Result<Vec<_>, _>>()?)
}

fn one(conn: &Connection, id: &str) -> AppResult<Account> {
    conn.query_row(&format!("SELECT {COLS} FROM ledger_account WHERE id = ?"), params![id], read).optional()?.ok_or_else(|| AppError::bad("账号不存在"))
}

fn to_user(a: &Account) -> CurrentUser {
    CurrentUser { id: a.id.clone(), name: a.name.clone(), admin: a.admin, enforced: true }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    pub username: String,
    #[serde(default)]
    pub name: String,
    pub password: String,
    #[serde(default)]
    pub admin: bool,
}

pub fn create(conn: &Connection, input: &NewAccount) -> AppResult<Account> {
    let username = input.username.trim();
    check_username(username)?;
    check_password(&input.password)?;
    let name = Some(input.name.trim()).filter(|n| !n.is_empty()).unwrap_or(username);
    if name.chars().count() > 40 {
        return Err(AppError::bad("显示名不超过 40 个字符"));
    }
    if conn.query_row("SELECT count(*) FROM ledger_account WHERE lower(username) = lower(?)", params![username], |r| r.get::<_, i64>(0))? > 0 {
        return Err(AppError::bad(format!("用户名「{username}」已存在")));
    }
    let now = now_local();
    conn.execute(
        "INSERT INTO ledger_account (id, username, name, password_hash, admin, disabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, false, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![username, username, name, hash_password(&input.password), input.admin, now, now],
    )?;
    let a = one(conn, username)?;
    // 登记到用户目录：建好就能被选为审批人、被 @，不用等对方先登录一次
    crate::approval::touch_user(conn, &to_user(&a))?;
    Ok(a)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountPatch {
    pub name: Option<String>,
    pub admin: Option<bool>,
    pub disabled: Option<bool>,
    /// 管理员重置密码
    pub password: Option<String>,
}

fn enabled_admins(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT count(*) FROM ledger_account WHERE admin AND NOT disabled", [], |r| r.get(0))?)
}

/// 管理员修改账号；停用、重置密码会让该账号已登录的会话失效
pub fn update(conn: &Connection, actor: &CurrentUser, id: &str, patch: &AccountPatch) -> AppResult<Account> {
    let a = one(conn, id)?;
    let losing_admin = a.admin && !a.disabled && (patch.admin == Some(false) || patch.disabled == Some(true));
    if losing_admin && enabled_admins(conn)? <= 1 {
        return Err(AppError::bad("至少要保留一个可用的管理员账号"));
    }
    if a.id == actor.id && patch.disabled == Some(true) {
        return Err(AppError::bad("不能停用自己的账号"));
    }
    let now = now_local();
    if let Some(name) = patch.name.as_deref().map(str::trim) {
        if name.is_empty() || name.chars().count() > 40 {
            return Err(AppError::bad("显示名不能为空，且不超过 40 个字符"));
        }
        conn.execute("UPDATE ledger_account SET name = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![name, now, id])?;
    }
    if let Some(admin) = patch.admin {
        conn.execute("UPDATE ledger_account SET admin = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![admin, now, id])?;
    }
    if let Some(disabled) = patch.disabled {
        conn.execute("UPDATE ledger_account SET disabled = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![disabled, now, id])?;
        if disabled {
            conn.execute("DELETE FROM ledger_session WHERE account_id = ?", params![id])?;
        }
    }
    if let Some(p) = patch.password.as_deref() {
        check_password(p)?;
        conn.execute("UPDATE ledger_account SET password_hash = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![hash_password(p), now, id])?;
        conn.execute("DELETE FROM ledger_session WHERE account_id = ?", params![id])?;
    }
    let a = one(conn, id)?;
    crate::approval::touch_user(conn, &to_user(&a))?;
    Ok(a)
}

/// 首次启动：还没有任何账号时建管理员；没设置密码时随机生成，返回生成的密码（由调用方打印到日志）
pub fn bootstrap(conn: &Connection, username: &str, password: Option<&str>) -> AppResult<Option<String>> {
    if conn.query_row("SELECT count(*) FROM ledger_account", [], |r| r.get::<_, i64>(0))? > 0 {
        return Ok(None);
    }
    let (password, generated) = match password {
        Some(p) => (p.to_string(), false),
        None => (URL_SAFE_NO_PAD.encode(random_bytes::<12>()), true),
    };
    create(conn, &NewAccount { username: username.to_string(), name: "管理员".into(), password: password.clone(), admin: true })?;
    Ok(generated.then_some(password))
}

// ── 登录与会话 ────────────────────────────────────────

/// 登录失败计数存在数据库里（重启服务不会清零，攻击者不能靠让服务重启来重置次数）。
/// 同一用户名连续失败 MAX_FAILURES 次后锁定 LOCK_WINDOW；时间用 Unix 秒
fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn locked(conn: &Connection, key: &str) -> AppResult<bool> {
    let row: Option<(i64, i64)> = conn.query_row("SELECT failures, last_at FROM ledger_login_failure WHERE key = ?", params![key], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    Ok(row.is_some_and(|(n, at)| n >= i64::from(MAX_FAILURES) && unix_now() - at < LOCK_WINDOW.as_secs() as i64))
}

fn note_failure(conn: &Connection, key: &str) -> AppResult<()> {
    let now = unix_now();
    let window = LOCK_WINDOW.as_secs() as i64;
    // 过期的记录顺手清掉，表不会一直增长
    conn.execute("DELETE FROM ledger_login_failure WHERE last_at < ?", params![now - window])?;
    conn.execute(
        "INSERT INTO ledger_login_failure (key, failures, last_at) VALUES (?, 1, ?)
         ON CONFLICT (key) DO UPDATE SET failures = ledger_login_failure.failures + 1, last_at = excluded.last_at",
        params![key, now],
    )?;
    Ok(())
}

fn clear_failures(conn: &Connection, key: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_login_failure WHERE key = ?", params![key])?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResult {
    pub token: String,
    pub days: i64,
    pub user: CurrentUser,
}

pub fn login(conn: &Connection, username: &str, password: &str, days: i64) -> AppResult<LoginResult> {
    let key = username.trim().to_lowercase();
    if key.is_empty() || password.is_empty() {
        return Err(AppError::bad("请输入用户名和密码"));
    }
    if locked(conn, &key)? {
        return Err(AppError::bad("登录失败次数过多，请 10 分钟后再试"));
    }
    let row: Option<(String, String, bool)> = conn
        .query_row("SELECT id, password_hash, disabled FROM ledger_account WHERE lower(username) = ?", params![key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    // 用户名不存在时也做一次同样耗时的校验，避免按响应时间猜出哪些用户名存在
    let stored = row.as_ref().map(|(_, h, _)| h.as_str()).unwrap_or_else(|| DUMMY_HASH.get_or_init(|| hash_password("dummy-password")));
    let valid = verify_password(password, stored);
    let (id, disabled) = match row {
        Some((id, _, disabled)) if valid => (id, disabled),
        _ => {
            note_failure(conn, &key)?;
            return Err(AppError::bad("用户名或密码不正确"));
        }
    };
    if disabled {
        return Err(AppError::bad("账号已停用，请联系管理员"));
    }
    clear_failures(conn, &key)?;
    let token = URL_SAFE_NO_PAD.encode(random_bytes::<32>());
    let now = now_local();
    conn.execute("DELETE FROM ledger_session WHERE expires_at < CAST(? AS TIMESTAMP)", params![now])?;
    conn.execute(
        "INSERT INTO ledger_session (token_hash, account_id, created_at, expires_at) VALUES (?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![token_hash(&token), id, now, expires_after(days)],
    )?;
    conn.execute("UPDATE ledger_account SET last_login_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now, id])?;
    let a = one(conn, &id)?;
    crate::approval::touch_user(conn, &to_user(&a))?;
    Ok(LoginResult { token, days, user: to_user(&a) })
}

static DUMMY_HASH: OnceLock<String> = OnceLock::new();

/// 令牌对应的用户；过期、作废或账号停用时为 None
pub fn session_user(conn: &Connection, token: &str) -> AppResult<Option<CurrentUser>> {
    Ok(conn
        .query_row(
            "SELECT a.id, a.name, a.admin FROM ledger_session s JOIN ledger_account a ON a.id = s.account_id
             WHERE s.token_hash = ? AND s.expires_at > CAST(? AS TIMESTAMP) AND NOT a.disabled",
            params![token_hash(token), now_local()],
            |r| Ok(CurrentUser { id: r.get(0)?, name: r.get(1)?, admin: r.get(2)?, enforced: true }),
        )
        .optional()?)
}

pub fn logout(conn: &Connection, token: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_session WHERE token_hash = ?", params![token_hash(token)])?;
    Ok(())
}

/// 自己改密码：要核对旧密码；其他设备上的登录随之失效（当前这次保留）
pub fn change_password(conn: &Connection, user: &CurrentUser, token: &str, old: &str, new: &str) -> AppResult<()> {
    let hash: Option<String> = conn.query_row("SELECT password_hash FROM ledger_account WHERE id = ?", params![user.id], |r| r.get(0)).optional()?;
    let Some(hash) = hash else { return Err(AppError::bad("当前登录方式不是内置账号")) };
    if !verify_password(old, &hash) {
        return Err(AppError::bad("原密码不正确"));
    }
    check_password(new)?;
    if old == new {
        return Err(AppError::bad("新密码不能与原密码相同"));
    }
    conn.execute("UPDATE ledger_account SET password_hash = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![hash_password(new), now_local(), user.id])?;
    conn.execute("DELETE FROM ledger_session WHERE account_id = ? AND token_hash <> ?", params![user.id, token_hash(token)])?;
    Ok(())
}

/// 命令行重置密码（忘记管理员密码时用）：服务需停止；新密码从标准输入读一行，不会留在 shell 历史里
pub fn cli_reset_password(config: &crate::config::Config, username: &str, make_admin: bool) -> anyhow::Result<()> {
    let conn = crate::db::open_file(&config.db_path)?;
    let id: Option<String> = conn.query_row("SELECT id FROM ledger_account WHERE lower(username) = lower(?)", params![username], |r| r.get(0)).optional()?;
    let id = id.ok_or_else(|| anyhow::anyhow!("没有账号「{username}」"))?;
    eprint!("输入「{username}」的新密码（至少 {MIN_PASSWORD} 位）并回车：");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']);
    check_password(password).map_err(|e| anyhow::anyhow!(e.message()))?;
    conn.execute("UPDATE ledger_account SET password_hash = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![hash_password(password), now_local(), id])?;
    if make_admin {
        conn.execute("UPDATE ledger_account SET admin = true, disabled = false WHERE id = ?", params![id])?;
    }
    conn.execute("DELETE FROM ledger_session WHERE account_id = ?", params![id])?;
    println!("已重置「{username}」的密码{}，原有登录已失效", if make_admin { "并设为管理员" } else { "" });
    Ok(())
}

// ── 接口 ──────────────────────────────────────────────

/// 会话 cookie 的名字：登录时由服务端以 HttpOnly 写入，页面脚本读不到（XSS 偷不走令牌）
pub const SESSION_COOKIE: &str = "gridlake_session";

/// 请求头里的令牌：`x-user-token`，或 `Authorization: Bearer …`（个人访问令牌的标准写法）
fn header_token(headers: &HeaderMap) -> String {
    let direct = headers.get("x-user-token").and_then(|v| v.to_str().ok()).map(str::trim).unwrap_or("");
    if !direct.is_empty() {
        return direct.to_string();
    }
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(|v| v.trim().to_string())
        .unwrap_or_default()
}

fn cookie_token(headers: &HeaderMap) -> String {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default()
}

/// 请求带的登录令牌：优先 `x-user-token` 请求头（脚本调用、前后端分开部署），其次会话 cookie（浏览器）
pub fn token_of(headers: &HeaderMap) -> String {
    let t = header_token(headers);
    if t.is_empty() { cookie_token(headers) } else { t }
}

/// 防跨站请求伪造：令牌只来自 cookie（浏览器会自动携带）的写请求，必须带 `x-requested-with` 请求头。
/// 跨站的页面发不出自定义请求头（要先过 CORS 预检，而本服务不会放行陌生来源），再加上 cookie 是 SameSite=Lax
pub fn csrf_blocked(method: &axum::http::Method, headers: &HeaderMap) -> bool {
    use axum::http::Method;
    let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    !safe && header_token(headers).is_empty() && !cookie_token(headers).is_empty() && !headers.contains_key("x-requested-with")
}

/// 会话 cookie 要不要加 Secure：LEDGER_COOKIE_SECURE=1/0 强制；默认看可信反向代理传来的 X-Forwarded-Proto
fn cookie_secure(db: &AppState, headers: &HeaderMap, peer: std::net::IpAddr) -> bool {
    match std::env::var("LEDGER_COOKIE_SECURE").ok().as_deref().map(str::trim) {
        Some("1" | "true" | "yes") => true,
        Some("0" | "false" | "no") => false,
        _ => db.config.trusted_proxies.contains(&peer) && headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).is_some_and(|v| v.trim().eq_ignore_ascii_case("https")),
    }
}

/// Set-Cookie 的值；max_age_secs 为 0 表示清除
fn session_cookie(token: &str, max_age_secs: i64, secure: bool) -> String {
    format!("{SESSION_COOKIE}={token}; Path=/; Max-Age={max_age_secs}; HttpOnly; SameSite=Lax{}", if secure { "; Secure" } else { "" })
}

fn require_local(db: &AppState) -> AppResult<()> {
    if db.auth.mode != AuthMode::Local {
        return Err(AppError::bad("当前登录方式不是内置账号（LEDGER_AUTH_MODE=local）"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginBody {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

/// POST /api/ledger/auth/login  { username, password } → { token, days, user }，同时用 Set-Cookie 写入 HttpOnly 会话 cookie。
/// 响应里的 token 供脚本 / 前后端分开部署时放进 x-user-token 请求头；浏览器页面不需要保存它
pub async fn login_route(State(db): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> AppResult<Response> {
    require_local(&db)?;
    let started = Instant::now();
    let b: LoginBody = parse_json(&body)?;
    let days = db.config.session_days;
    let username = b.username.trim().to_string();
    let r = run(&db, move |c| login(c, &b.username, &b.password, days)).await;
    crate::auth::audit_login(&db, &headers, peer.ip(), &username, if r.is_ok() { 200 } else { 400 }, started);
    let r = r?;
    let cookie = session_cookie(&r.token, days * 86400, cookie_secure(&db, &headers, peer.ip()));
    Ok(([(axum::http::header::SET_COOKIE, cookie)], ok(r)).into_response())
}

/// POST /api/ledger/auth/logout  作废当前令牌，并清除会话 cookie
pub async fn logout_route(State(db): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> AppResult<Response> {
    require_local(&db)?;
    let token = token_of(&headers);
    run(&db, move |c| logout(c, &token)).await?;
    db.auth.clear_cache();
    let cookie = session_cookie("", 0, cookie_secure(&db, &headers, peer.ip()));
    Ok(([(axum::http::header::SET_COOKIE, cookie)], ok(true)).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordBody {
    #[serde(default)]
    pub old_password: String,
    #[serde(default)]
    pub new_password: String,
}

/// POST /api/ledger/auth/password  { oldPassword, newPassword }
pub async fn password_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Envelope<bool>>> {
    require_local(&db)?;
    let b: PasswordBody = parse_json(&body)?;
    let token = token_of(&headers);
    run(&db, move |c| change_password(c, &user, &token, &b.old_password, &b.new_password)).await?;
    db.auth.clear_cache();
    Ok(ok(true))
}

/// GET /api/ledger/accounts  账号列表（管理员）
pub async fn list_route(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<Account>>>> {
    require_local(&db)?;
    Ok(ok(run(&db, list).await?))
}

/// POST /api/ledger/accounts  { username, name, password, admin }（管理员）
pub async fn create_route(State(db): State<AppState>, body: Bytes) -> AppResult<Json<Envelope<Account>>> {
    require_local(&db)?;
    let b: NewAccount = parse_json(&body)?;
    Ok(ok(run(&db, move |c| create(c, &b)).await?))
}

/// PUT /api/ledger/accounts/{id}  { name?, admin?, disabled?, password? }（管理员）
pub async fn update_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: Bytes) -> AppResult<Json<Envelope<Account>>> {
    require_local(&db)?;
    let b: AccountPatch = parse_json(&body)?;
    let a = run(&db, move |c| update(c, &user, &id, &b)).await?;
    db.auth.clear_cache();
    Ok(ok(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn pbkdf2_matches_known_vectors() {
        // RFC 7914 / 常用的 PBKDF2-HMAC-SHA256 测试向量
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 1)), "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 2)), "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 4096)), "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a");
    }

    #[test]
    fn hash_and_verify() {
        let h = hash_password("correct horse");
        assert!(h.starts_with("pbkdf2_sha256$600000$"));
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("correct horsE", &h));
        assert_ne!(h, hash_password("correct horse"), "每次加不同的盐");
        assert!(!verify_password("x", "not-a-hash"));
        assert!(!verify_password("x", "pbkdf2_sha256$0$AAAA$AAAA"));
    }

    /// 登录失败计数在数据库里：重启（新连接）后仍然有效，成功登录或过期后清零
    #[test]
    fn login_lockout_survives_restart_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l.duckdb");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
            assert!(!locked(&conn, "alice").unwrap());
            for _ in 0..MAX_FAILURES - 1 {
                note_failure(&conn, "alice").unwrap();
            }
            assert!(!locked(&conn, "alice").unwrap(), "还差一次");
            note_failure(&conn, "alice").unwrap();
            assert!(locked(&conn, "alice").unwrap());
            assert!(!locked(&conn, "bob").unwrap(), "按用户名分开计数");
        }
        // 模拟服务重启：重新打开同一个数据文件
        let conn = Connection::open(&path).unwrap();
        assert!(locked(&conn, "alice").unwrap(), "重启后仍然锁定");
        clear_failures(&conn, "alice").unwrap();
        assert!(!locked(&conn, "alice").unwrap());
        // 过了锁定窗口自动解除，并在下一次记录失败时清掉过期记录
        let old = unix_now() - LOCK_WINDOW.as_secs() as i64 - 1;
        conn.execute("INSERT INTO ledger_login_failure VALUES ('carol', 99, ?)", params![old]).unwrap();
        assert!(!locked(&conn, "carol").unwrap());
        note_failure(&conn, "dave").unwrap();
        let n: i64 = conn.query_row("SELECT count(*) FROM ledger_login_failure WHERE key = 'carol'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn session_token_from_header_or_cookie() {
        let mut h = HeaderMap::new();
        assert_eq!(token_of(&h), "");
        h.insert("cookie", "a=1; gridlake_session=abc123; b=2".parse().unwrap());
        assert_eq!(token_of(&h), "abc123");
        h.insert("x-user-token", " hdr ".parse().unwrap());
        assert_eq!(token_of(&h), "hdr", "请求头优先");
        // 名字只是前缀相同的 cookie 不算
        let mut h2 = HeaderMap::new();
        h2.insert("cookie", "xgridlake_session=nope".parse().unwrap());
        assert_eq!(token_of(&h2), "");
    }

    #[test]
    fn csrf_only_guards_cookie_authenticated_writes() {
        use axum::http::Method;
        let mut cookie_only = HeaderMap::new();
        cookie_only.insert("cookie", "gridlake_session=abc".parse().unwrap());
        assert!(csrf_blocked(&Method::POST, &cookie_only));
        assert!(csrf_blocked(&Method::DELETE, &cookie_only));
        assert!(!csrf_blocked(&Method::GET, &cookie_only), "读请求不拦");
        let mut with_xrw = cookie_only.clone();
        with_xrw.insert("x-requested-with", "XMLHttpRequest".parse().unwrap());
        assert!(!csrf_blocked(&Method::POST, &with_xrw));
        // 令牌来自请求头（浏览器不会自动携带）：不需要额外的头；没有任何凭据也不归这里管
        let mut hdr = HeaderMap::new();
        hdr.insert("x-user-token", "abc".parse().unwrap());
        assert!(!csrf_blocked(&Method::POST, &hdr));
        assert!(!csrf_blocked(&Method::POST, &HeaderMap::new()));
    }

    #[test]
    fn cookie_attributes() {
        let c = session_cookie("tok", 604800, false);
        assert_eq!(c, "gridlake_session=tok; Path=/; Max-Age=604800; HttpOnly; SameSite=Lax");
        assert!(session_cookie("tok", 604800, true).ends_with("; Secure"));
        assert!(session_cookie("", 0, false).contains("Max-Age=0"));
    }

    #[test]
    fn username_and_password_rules() {
        assert!(check_username("alice").is_ok() && check_username("a.b-c_d@x").is_ok());
        assert!(check_username("a").is_err() && check_username("中文").is_err() && check_username("a b").is_err());
        assert!(check_password("1234567").is_err() && check_password("12345678").is_ok());
    }
}
