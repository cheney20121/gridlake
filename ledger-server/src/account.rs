//! 内置账号（LEDGER_AUTH_MODE=local）：用户名 + 密码登录，账号由管理员在界面里管理
//!
//! - 密码：PBKDF2-HMAC-SHA256（60 万次迭代、16 字节随机盐），存成 `pbkdf2_sha256$迭代次数$盐$哈希`（base64）
//! - 会话：登录得到 32 字节随机令牌，库里只存它的 SHA-256；按 LEDGER_SESSION_DAYS 过期；改密码、停用、重置密码时作废
//! - 防暴力尝试：同一用户名 10 分钟内失败 5 次后暂时拒绝；用户名不存在与密码错误返回同样的提示
//! - 首次启动没有任何账号时，按 LEDGER_ADMIN_USER / LEDGER_ADMIN_PASSWORD 建一个管理员（没设密码就随机生成并打印在日志里）
//! - 登录后的「当前用户」与网关模式同构（id 为用户名），审计、行级权限、审批等不需要区分登录方式

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Extension, Path, State};
use axum::http::HeaderMap;
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

fn random_bytes<const N: usize>() -> [u8; N] {
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

fn expires_after(days: i64) -> String {
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

fn failures() -> &'static Mutex<HashMap<String, (u32, Instant)>> {
    static F: OnceLock<Mutex<HashMap<String, (u32, Instant)>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

fn locked(key: &str) -> bool {
    failures().lock().ok().and_then(|m| m.get(key).copied()).is_some_and(|(n, at)| n >= MAX_FAILURES && at.elapsed() < LOCK_WINDOW)
}

fn note_failure(key: &str) {
    if let Ok(mut m) = failures().lock() {
        let e = m.entry(key.to_string()).or_insert((0, Instant::now()));
        if e.1.elapsed() >= LOCK_WINDOW {
            *e = (0, Instant::now());
        }
        e.0 += 1;
        e.1 = Instant::now();
    }
}

fn clear_failures(key: &str) {
    if let Ok(mut m) = failures().lock() {
        m.remove(key);
    }
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
    if locked(&key) {
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
            note_failure(&key);
            return Err(AppError::bad("用户名或密码不正确"));
        }
    };
    if disabled {
        return Err(AppError::bad("账号已停用，请联系管理员"));
    }
    clear_failures(&key);
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

fn token_of(headers: &HeaderMap) -> String {
    headers.get("x-user-token").and_then(|v| v.to_str().ok()).map(str::trim).unwrap_or("").to_string()
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

/// POST /api/ledger/auth/login  { username, password } → { token, days, user }
pub async fn login_route(State(db): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Envelope<LoginResult>>> {
    require_local(&db)?;
    let started = Instant::now();
    let b: LoginBody = parse_json(&body)?;
    let days = db.config.session_days;
    let username = b.username.trim().to_string();
    let r = run(&db, move |c| login(c, &b.username, &b.password, days)).await;
    crate::auth::audit_login(&db, &headers, peer.ip(), &username, if r.is_ok() { 200 } else { 400 }, started);
    Ok(ok(r?))
}

/// POST /api/ledger/auth/logout  作废当前令牌
pub async fn logout_route(State(db): State<AppState>, headers: HeaderMap) -> AppResult<Json<Envelope<bool>>> {
    require_local(&db)?;
    let token = token_of(&headers);
    run(&db, move |c| logout(c, &token)).await?;
    db.auth.clear_cache();
    Ok(ok(true))
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

    #[test]
    fn username_and_password_rules() {
        assert!(check_username("alice").is_ok() && check_username("a.b-c_d@x").is_ok());
        assert!(check_username("a").is_err() && check_username("中文").is_err() && check_username("a b").is_err());
        assert!(check_password("1234567").is_err() && check_password("12345678").is_ok());
    }
}
