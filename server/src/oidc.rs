//! 单点登录（OIDC，授权码 + PKCE）：在内置账号（LEDGER_AUTH_MODE=local）上叠加「用公司账号登录」
//!
//! 流程：登录页的按钮整页跳到 `GET /api/ledger/auth/oidc/login` → 本服务生成 state / nonce / PKCE 校验码，
//! 把用户重定向到身份提供方 → 登录后回到 `GET /api/ledger/auth/oidc/callback?code&state` → 本服务用授权码向令牌接口换
//! id_token，校验后找到（或建出）对应的内置账号，开一个与密码登录完全一样的会话（同一个 cookie），再跳回应用。
//! 之后的权限、行级数据范围、审批、审计都和内置账号一样，不区分登录方式。
//!
//! 安全要点：
//! - state 同时放在服务端（一次性、10 分钟有效）和浏览器的 HttpOnly cookie 里，回调时两边必须对得上（防登录 CSRF / 会话固定）；
//! - PKCE（S256）防授权码被截获；nonce 写进 id_token 防重放；
//! - id_token 是服务端直接向令牌接口（HTTPS）换来的，按 OIDC 规范 3.1.3.7 此时可不验签，但必须校验 iss / aud / exp / nonce；
//!   所以签发者地址必须是 https（本机回环地址除外，或显式 LEDGER_OIDC_ALLOW_HTTP=1）；
//! - 默认不会把身份提供方的用户绑到同名的已有本地账号上（否则身份提供方里能注册同名用户的人就能顶替本地管理员），
//!   需要时打开 LEDGER_OIDC_LINK_BY_USERNAME；
//! - 失败时只给浏览器一个简短的错误代码（`/login?sso_error=...`），细节记在服务日志里，不把身份提供方的报错原样回显；
//! - 管理员权限可以通过组声明同步（LEDGER_OIDC_ADMIN_GROUP）：只同步经 SSO 绑定的账号，本地建的管理员（应急用）不受影响。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use duckdb::{Connection, OptionalExt, params};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::account::{LoginResult, random_bytes};
use crate::config::OidcConfig;
use crate::db::now_local;
use crate::error::AppError;
use crate::routes::AppState;

/// 开始登录的接口路径（相对 API 根）；前端据此拼出整页跳转的地址
pub const LOGIN_PATH: &str = "/ledger/auth/oidc/login";
const STATE_COOKIE: &str = "gridlake_oidc";
const PENDING_TTL: Duration = Duration::from_secs(600);
const PENDING_MAX: usize = 5000;
const DISCOVERY_TTL: Duration = Duration::from_secs(3600);

struct Pending {
    nonce: String,
    verifier: String,
    /// 登录完成后回到的站内路径
    ret: String,
    at: Instant,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static DISCOVERY: LazyLock<Mutex<Option<(Instant, Provider)>>> = LazyLock::new(|| Mutex::new(None));

#[derive(Clone, Deserialize)]
struct Provider {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    userinfo_endpoint: Option<String>,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Option<Vec<String>>,
}

/// 登录失败：code 给浏览器（前端据此显示提示），detail 只进日志
#[derive(Debug)]
struct Fail {
    code: &'static str,
    detail: String,
}

fn fail(code: &'static str, detail: impl Into<String>) -> Fail {
    Fail { code, detail: detail.into() }
}

impl From<AppError> for Fail {
    fn from(e: AppError) -> Self {
        fail("internal", e.message())
    }
}

impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Self {
        fail("internal", e.to_string())
    }
}

impl From<duckdb::Error> for Fail {
    fn from(e: duckdb::Error) -> Self {
        fail("internal", e.to_string())
    }
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// 只允许回到本站内的路径（不能是 //evil.com 或带协议的地址）
fn safe_return(v: Option<&str>) -> String {
    match v.map(str::trim) {
        Some(p) if p.starts_with('/') && !p.starts_with("//") && !p.contains('\\') && !p.contains('\n') && !p.contains('\r') && p.len() <= 500 => p.to_string(),
        _ => "/".to_string(),
    }
}

fn to_login(code: &str) -> Response {
    redirect(&format!("/login?sso_error={code}"), None, None)
}

fn redirect(to: &str, set_cookie: Option<String>, clear_state: Option<String>) -> Response {
    let mut r = (StatusCode::FOUND, [(header::LOCATION, to.to_string()), (header::CACHE_CONTROL, "no-store".to_string())]).into_response();
    for c in set_cookie.into_iter().chain(clear_state) {
        if let Ok(v) = HeaderValue::from_str(&c) {
            r.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    r
}

fn state_cookie(value: &str, max_age: i64, secure: bool) -> String {
    format!("{STATE_COOKIE}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{}", if secure { "; Secure" } else { "" })
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

// ── 发现文档 ─────────────────────────────────────────

async fn discover(db: &AppState, cfg: &OidcConfig) -> Result<Provider, Fail> {
    let cached = DISCOVERY.lock().ok().and_then(|g| g.clone());
    if let Some((at, p)) = &cached
        && at.elapsed() < DISCOVERY_TTL
    {
        return Ok(p.clone());
    }
    let url = format!("{}/.well-known/openid-configuration", cfg.issuer);
    let fetched = async {
        let resp = db.auth.http().get(&url).send().await.map_err(|e| fail("unreachable", format!("读取 {url} 失败：{e}")))?;
        if !resp.status().is_success() {
            return Err(fail("unreachable", format!("读取 {url} 返回 {}", resp.status())));
        }
        let p: Provider = resp.json().await.map_err(|e| fail("unreachable", format!("{url} 不是有效的 OIDC 发现文档：{e}")))?;
        if p.issuer.trim_end_matches('/') != cfg.issuer {
            return Err(fail("config", format!("发现文档里的 issuer（{}）与 LEDGER_OIDC_ISSUER（{}）不一致", p.issuer, cfg.issuer)));
        }
        Ok(p)
    }
    .await;
    match fetched {
        Ok(p) => {
            if let Ok(mut g) = DISCOVERY.lock() {
                *g = Some((Instant::now(), p.clone()));
            }
            Ok(p)
        }
        // 身份提供方临时连不上时，用上次的缓存继续让人登录
        Err(e) => match cached {
            Some((_, p)) => {
                tracing::warn!("OIDC 发现文档刷新失败，沿用缓存：{}", e.detail);
                Ok(p)
            }
            None => Err(e),
        },
    }
}

// ── 第一步：跳到身份提供方 ───────────────────────────

#[derive(Deserialize)]
pub struct LoginQuery {
    #[serde(rename = "return")]
    ret: Option<String>,
}

/// GET /api/ledger/auth/oidc/login?return=/路径  → 302 到身份提供方
pub async fn login_route(State(db): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Query(q): Query<LoginQuery>) -> Response {
    let Some(cfg) = db.config.oidc.as_ref() else { return to_login("config") };
    match start(&db, cfg, &q).await {
        Ok((url, state)) => redirect(&url, Some(state_cookie(&state, PENDING_TTL.as_secs() as i64, crate::account::cookie_secure(&db, &headers, peer.ip()))), None),
        Err(e) => {
            tracing::warn!("OIDC 开始登录失败：{}", e.detail);
            to_login(e.code)
        }
    }
}

async fn start(db: &AppState, cfg: &OidcConfig, q: &LoginQuery) -> Result<(String, String), Fail> {
    let provider = discover(db, cfg).await?;
    let state = b64(&random_bytes::<24>());
    let nonce = b64(&random_bytes::<24>());
    let verifier = b64(&random_bytes::<48>());
    let challenge = b64(&Sha256::digest(verifier.as_bytes()));
    {
        let mut pending = PENDING.lock().map_err(|_| fail("internal", "状态表被污染"))?;
        pending.retain(|_, p| p.at.elapsed() < PENDING_TTL);
        if pending.len() >= PENDING_MAX {
            return Err(fail("busy", "等待回调的登录请求太多"));
        }
        pending.insert(state.clone(), Pending { nonce: nonce.clone(), verifier, ret: safe_return(q.ret.as_deref()), at: Instant::now() });
    }
    let mut url = reqwest::Url::parse(&provider.authorization_endpoint).map_err(|e| fail("config", format!("authorization_endpoint 无效：{e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &cfg.redirect_url)
        .append_pair("scope", &cfg.scopes)
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok((url.to_string(), state))
}

// ── 第二步：回调 ─────────────────────────────────────

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// GET /api/ledger/auth/oidc/callback  身份提供方登录后回到这里；成功 → 写会话 cookie 并 302 回应用，失败 → 302 到登录页带错误代码
pub async fn callback_route(State(db): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Query(q): Query<CallbackQuery>) -> Response {
    let started = Instant::now();
    let Some(cfg) = db.config.oidc.as_ref() else { return to_login("config") };
    let secure = crate::account::cookie_secure(&db, &headers, peer.ip());
    let clear = state_cookie("", 0, secure);
    let outcome = finish(&db, cfg, &headers, &q).await;
    match outcome {
        Ok((r, ret)) => {
            crate::auth::audit_login(&db, &headers, peer.ip(), &r.user.id, 200, started);
            db.auth.clear_cache();
            let session = crate::account::session_cookie(&r.token, r.days * 86400, secure);
            redirect(&ret, Some(session), Some(clear))
        }
        Err(e) => {
            tracing::warn!("OIDC 登录失败（{}）：{}", e.code, e.detail);
            crate::auth::audit_login(&db, &headers, peer.ip(), "sso", 400, started);
            let mut r = to_login(e.code);
            if let Ok(v) = HeaderValue::from_str(&clear) {
                r.headers_mut().append(header::SET_COOKIE, v);
            }
            r
        }
    }
}

async fn finish(db: &AppState, cfg: &OidcConfig, headers: &HeaderMap, q: &CallbackQuery) -> Result<(LoginResult, String), Fail> {
    if let Some(err) = &q.error {
        let code = if err == "access_denied" { "denied" } else { "idp_error" };
        return Err(fail(code, format!("身份提供方返回错误 {err}：{}", q.error_description.as_deref().unwrap_or(""))));
    }
    let (Some(code), Some(state)) = (q.code.as_deref(), q.state.as_deref()) else { return Err(fail("expired", "回调缺少 code 或 state")) };
    // 浏览器里的 state cookie 必须与回调带回来的一致：别人构造的回调链接发给受害者时，受害者的浏览器里没有对应的 cookie
    if cookie_value(headers, STATE_COOKIE).as_deref() != Some(state) {
        return Err(fail("expired", "state cookie 与回调不一致（链接过期、换了浏览器，或疑似伪造的回调）"));
    }
    let pending = PENDING.lock().map_err(|_| fail("internal", "状态表被污染"))?.remove(state);
    let Some(pending) = pending.filter(|p| p.at.elapsed() < PENDING_TTL) else { return Err(fail("expired", "登录请求已过期或已使用")) };

    let provider = discover(db, cfg).await?;
    let tokens = exchange(db, cfg, &provider, code, &pending.verifier).await?;
    let mut claims = id_token_claims(cfg, &tokens.id_token, &pending.nonce)?;
    complete_claims(db, cfg, &provider, &tokens.access_token, &mut claims).await;
    let identity = Identity::from_claims(cfg, &claims)?;
    let days = db.config.session_days;
    let (db2, cfg2) = (db.clone(), cfg.clone());
    let result = tokio::task::spawn_blocking(move || login_identity(&db2.connection()?, &cfg2, &cfg2.issuer, &identity, days)).await.map_err(|e| fail("internal", e.to_string()))??;
    Ok((result, pending.ret))
}

struct Tokens {
    id_token: String,
    access_token: Option<String>,
}

async fn exchange(db: &AppState, cfg: &OidcConfig, provider: &Provider, code: &str, verifier: &str) -> Result<Tokens, Fail> {
    let mut form: Vec<(&str, &str)> = vec![("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &cfg.redirect_url), ("code_verifier", verifier)];
    let mut req = db.auth.http().post(&provider.token_endpoint);
    let basic = cfg.client_secret.is_some()
        && provider.token_endpoint_auth_methods_supported.as_ref().is_some_and(|m| m.iter().any(|x| x == "client_secret_basic") && !m.iter().any(|x| x == "client_secret_post"));
    match (&cfg.client_secret, basic) {
        (Some(secret), true) => req = req.basic_auth(&cfg.client_id, Some(secret)),
        (Some(secret), false) => {
            form.push(("client_id", &cfg.client_id));
            form.push(("client_secret", secret));
        }
        (None, _) => form.push(("client_id", &cfg.client_id)),
    }
    let resp = req.header(header::ACCEPT, "application/json").form(&form).send().await.map_err(|e| fail("token", format!("请求令牌接口失败：{e}")))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| fail("token", format!("读取令牌接口响应失败：{e}")))?;
    if !status.is_success() {
        return Err(fail("token", format!("令牌接口返回 {status}：{}", body.chars().take(300).collect::<String>())));
    }
    let v: Value = serde_json::from_str(&body).map_err(|e| fail("token", format!("令牌接口返回的不是 JSON：{e}")))?;
    let id_token = v.get("id_token").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| fail("token", "令牌接口的响应里没有 id_token（scope 里要有 openid）"))?;
    Ok(Tokens { id_token: id_token.to_string(), access_token: v.get("access_token").and_then(Value::as_str).map(str::to_string) })
}

/// 解出 id_token 的声明并校验 iss / aud / exp / nonce（不验签，见模块说明）
fn id_token_claims(cfg: &OidcConfig, token: &str, nonce: &str) -> Result<Value, Fail> {
    let payload = token.split('.').nth(1).ok_or_else(|| fail("token", "id_token 格式不对"))?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).map_err(|e| fail("token", format!("id_token 解码失败：{e}")))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| fail("token", format!("id_token 不是 JSON：{e}")))?;
    check_claims(cfg, &v, nonce, chrono::Utc::now().timestamp())?;
    Ok(v)
}

fn check_claims(cfg: &OidcConfig, v: &Value, nonce: &str, now: i64) -> Result<(), Fail> {
    let iss = v.get("iss").and_then(Value::as_str).unwrap_or("");
    if iss.trim_end_matches('/') != cfg.issuer {
        return Err(fail("token", format!("id_token 的 iss（{iss}）不是 {}", cfg.issuer)));
    }
    let auds: Vec<&str> = match v.get("aud") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    if !auds.contains(&cfg.client_id.as_str()) {
        return Err(fail("token", format!("id_token 的 aud（{auds:?}）不含 {}", cfg.client_id)));
    }
    if auds.len() > 1 && v.get("azp").and_then(Value::as_str) != Some(cfg.client_id.as_str()) {
        return Err(fail("token", "id_token 有多个 aud 但 azp 不是本应用"));
    }
    // 允许 60 秒的时钟误差
    match v.get("exp").and_then(Value::as_i64) {
        Some(exp) if exp + 60 > now => {}
        _ => return Err(fail("token", "id_token 已过期或没有 exp")),
    }
    if v.get("nonce").and_then(Value::as_str) != Some(nonce) {
        return Err(fail("token", "id_token 的 nonce 对不上"));
    }
    Ok(())
}

/// id_token 里缺登录名 / 姓名 / 组时，用访问令牌到 userinfo 接口补（补不到不算失败；sub 必须一致）
async fn complete_claims(db: &AppState, cfg: &OidcConfig, provider: &Provider, access_token: &Option<String>, claims: &mut Value) {
    let missing = |k: &str| claims.get(k).is_none();
    let need = missing(&cfg.username_claim) || missing(&cfg.name_claim) || (cfg.admin_group.is_some() && missing(&cfg.groups_claim));
    let (true, Some(url), Some(at)) = (need, &provider.userinfo_endpoint, access_token) else { return };
    let got = async {
        let resp = db.auth.http().get(url).bearer_auth(at).send().await.ok()?;
        resp.status().is_success().then_some(())?;
        resp.json::<Value>().await.ok()
    }
    .await;
    let Some(Value::Object(info)) = got else {
        tracing::warn!("OIDC userinfo 读取失败，只用 id_token 里的信息");
        return;
    };
    if info.get("sub") != claims.get("sub") {
        tracing::warn!("OIDC userinfo 的 sub 与 id_token 不一致，已忽略");
        return;
    }
    if let Some(obj) = claims.as_object_mut() {
        for (k, v) in info {
            obj.entry(k).or_insert(v);
        }
    }
}

// ── 声明 → 内置账号 ──────────────────────────────────

struct Identity {
    subject: String,
    username: String,
    name: String,
    /// 配置了管理员组时：是否在组里
    admin: Option<bool>,
}

fn claim_text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn claim_list(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).collect(),
        Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).collect(),
        _ => vec![],
    }
}

/// 把任意文本变成合法的内置用户名（2 ~ 32 位，字母数字和 _ - . @）
fn sanitize_username(raw: &str) -> String {
    let mut s: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@') { c } else { '_' }).take(32).collect();
    if s.chars().count() < 2 {
        s = format!("u_{s}");
    }
    s
}

impl Identity {
    fn from_claims(cfg: &OidcConfig, v: &Value) -> Result<Self, Fail> {
        let subject = claim_text(v, "sub").ok_or_else(|| fail("claims", "id_token 里没有 sub"))?;
        let email = claim_text(v, "email");
        let raw_user = claim_text(v, &cfg.username_claim)
            .or_else(|| email.as_deref().and_then(|e| e.split('@').next()).filter(|s| !s.is_empty()).map(str::to_string))
            .unwrap_or_else(|| subject.clone());
        let username = sanitize_username(&raw_user);
        let name = claim_text(v, &cfg.name_claim).unwrap_or_else(|| username.clone()).chars().take(40).collect();
        let admin = cfg.admin_group.as_ref().map(|g| claim_list(v, &cfg.groups_claim).iter().any(|x| x == g));
        Ok(Self { subject, username, name, admin })
    }
}

/// 找到（或建出）这个身份对应的内置账号并开会话
fn login_identity(conn: &Connection, cfg: &OidcConfig, issuer: &str, who: &Identity, days: i64) -> Result<LoginResult, Fail> {
    let bound: Option<String> = conn.query_row("SELECT account_id FROM ledger_oidc_identity WHERE issuer = ? AND subject = ?", params![issuer, who.subject], |r| r.get(0)).optional()?;
    let existing = match &bound {
        Some(id) => conn.query_row("SELECT id, disabled, admin FROM ledger_account WHERE id = ?", params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, bool>(2)?))).optional()?,
        None => None,
    };
    let now = now_local();
    let account_id = if let Some((id, disabled, admin)) = existing {
        if disabled {
            return Err(fail("disabled", format!("账号 {id} 已停用")));
        }
        // 同步姓名与管理员身份（只同步经 SSO 绑定的账号；不能把最后一个管理员降级）
        conn.execute("UPDATE ledger_account SET name = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![who.name, now, id])?;
        if let Some(want) = who.admin
            && want != admin
            && (want || crate::account::enabled_admins(conn)? > 1)
        {
            conn.execute("UPDATE ledger_account SET admin = ? WHERE id = ?", params![want, id])?;
        }
        id
    } else {
        if bound.is_some() {
            // 绑定的账号已经不存在：清掉旧绑定，当作第一次登录
            conn.execute("DELETE FROM ledger_oidc_identity WHERE issuer = ? AND subject = ?", params![issuer, who.subject])?;
        }
        let linked = if cfg.link_by_username {
            // 只绑「还没绑过任何身份」的同名账号
            conn.query_row(
                "SELECT id, disabled FROM ledger_account a WHERE lower(username) = lower(?)
                 AND NOT EXISTS (SELECT 1 FROM ledger_oidc_identity i WHERE i.account_id = a.id)",
                params![who.username],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
            )
            .optional()?
        } else {
            None
        };
        let id = match linked {
            Some((id, disabled)) => {
                if disabled {
                    return Err(fail("disabled", format!("账号 {id} 已停用")));
                }
                id
            }
            None => {
                if !cfg.auto_create {
                    return Err(fail("not_provisioned", format!("{}（sub={}）还没有对应的账号，且未开启自动建账号", who.username, who.subject)));
                }
                let id = unique_username(conn, &who.username)?;
                // 不可用于密码登录的占位哈希（不是合法的 pbkdf2 串，verify_password 永远失败）
                conn.execute(
                    "INSERT INTO ledger_account (id, username, name, password_hash, admin, disabled, created_at, updated_at) VALUES (?, ?, ?, '!sso', ?, false, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
                    params![id, id, who.name, who.admin.unwrap_or(false), now, now],
                )?;
                id
            }
        };
        conn.execute("INSERT INTO ledger_oidc_identity (issuer, subject, account_id, created_at) VALUES (?, ?, ?, CAST(? AS TIMESTAMP))", params![issuer, who.subject, id, now])?;
        id
    };
    conn.execute("UPDATE ledger_oidc_identity SET last_login_at = CAST(? AS TIMESTAMP) WHERE issuer = ? AND subject = ?", params![now, issuer, who.subject])?;
    Ok(crate::account::start_session(conn, &account_id, days)?)
}

/// 重名时加 -2、-3……（用户名不区分大小写）；总长不超过 32
fn unique_username(conn: &Connection, base: &str) -> Result<String, Fail> {
    for n in 1..1000 {
        let cand = if n == 1 {
            base.to_string()
        } else {
            let suffix = format!("-{n}");
            format!("{}{suffix}", base.chars().take(32 - suffix.len()).collect::<String>())
        };
        let taken: i64 = conn.query_row("SELECT count(*) FROM ledger_account WHERE lower(username) = lower(?) OR lower(id) = lower(?)", params![cand, cand], |r| r.get(0))?;
        if taken == 0 {
            return Ok(cand);
        }
    }
    Err(fail("internal", "找不到可用的用户名"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> OidcConfig {
        OidcConfig::parse(|k| {
            match k {
                "LEDGER_OIDC_ISSUER" => Some("https://idp.example.com/realm/"),
                "LEDGER_OIDC_CLIENT_ID" => Some("gridlake"),
                "LEDGER_OIDC_REDIRECT_URL" => Some("https://app.example.com/api/ledger/auth/oidc/callback"),
                "LEDGER_OIDC_ADMIN_GROUP" => Some("gridlake-admins"),
                _ => None,
            }
            .map(str::to_string)
        })
        .unwrap()
        .unwrap()
    }

    #[test]
    fn return_path_must_stay_on_this_site() {
        assert_eq!(safe_return(Some("/workbook/abc?x=1")), "/workbook/abc?x=1");
        for bad in ["//evil.com", "https://evil.com", "/\\evil.com", "evil", "", "/a\nb"] {
            assert_eq!(safe_return(Some(bad)), "/", "{bad}");
        }
        assert_eq!(safe_return(None), "/");
    }

    #[test]
    fn id_token_claims_are_checked() {
        let c = cfg();
        let now = 1_700_000_000;
        let good = json!({"iss": "https://idp.example.com/realm", "aud": "gridlake", "exp": now + 300, "nonce": "n1", "sub": "u1"});
        assert!(check_claims(&c, &good, "n1", now).is_ok());
        // 签发者带结尾斜杠也认
        let slash = json!({"iss": "https://idp.example.com/realm/", "aud": ["gridlake"], "exp": now + 300, "nonce": "n1"});
        assert!(check_claims(&c, &slash, "n1", now).is_ok());
        let cases = [
            ("iss 不对", json!({"iss": "https://evil.example.com", "aud": "gridlake", "exp": now + 300, "nonce": "n1"})),
            ("aud 不含本应用", json!({"iss": "https://idp.example.com/realm", "aud": "other", "exp": now + 300, "nonce": "n1"})),
            ("多个 aud 没有 azp", json!({"iss": "https://idp.example.com/realm", "aud": ["gridlake", "other"], "exp": now + 300, "nonce": "n1"})),
            ("已过期", json!({"iss": "https://idp.example.com/realm", "aud": "gridlake", "exp": now - 120, "nonce": "n1"})),
            ("没有 exp", json!({"iss": "https://idp.example.com/realm", "aud": "gridlake", "nonce": "n1"})),
            ("nonce 不对", json!({"iss": "https://idp.example.com/realm", "aud": "gridlake", "exp": now + 300, "nonce": "other"})),
            ("没有 nonce", json!({"iss": "https://idp.example.com/realm", "aud": "gridlake", "exp": now + 300})),
        ];
        for (why, v) in cases {
            assert!(check_claims(&c, &v, "n1", now).is_err(), "{why}");
        }
        // 多个 aud 但 azp 是本应用：通过；60 秒内的时钟误差：通过
        let azp = json!({"iss": "https://idp.example.com/realm", "aud": ["gridlake", "other"], "azp": "gridlake", "exp": now - 30, "nonce": "n1"});
        assert!(check_claims(&c, &azp, "n1", now).is_ok());
    }

    #[test]
    fn identity_from_claims() {
        let c = cfg();
        let who = Identity::from_claims(&c, &json!({"sub": "s1", "preferred_username": "zhang.san", "name": "张三", "groups": ["dev", "gridlake-admins"]})).unwrap();
        assert_eq!((who.username.as_str(), who.name.as_str(), who.admin), ("zhang.san", "张三", Some(true)));
        // 缺登录名：回退到邮箱 @ 之前，再回退到 sub；中文等非法字符换成下划线
        let who = Identity::from_claims(&c, &json!({"sub": "s2", "email": "li.si@corp.com"})).unwrap();
        assert_eq!((who.username.as_str(), who.name.as_str(), who.admin), ("li.si", "li.si", Some(false)));
        assert_eq!(Identity::from_claims(&c, &json!({"sub": "abc-123"})).unwrap().username, "abc-123");
        assert_eq!(sanitize_username("王五 wu"), "___wu");
        assert_eq!(sanitize_username("x"), "u_x");
        assert_eq!(sanitize_username(&"a".repeat(50)).len(), 32);
        assert!(Identity::from_claims(&c, &json!({"preferred_username": "x"})).is_err(), "没有 sub 不能登录");
        // 组声明是逗号分隔的字符串也认；没配管理员组时不同步管理权限
        let who = Identity::from_claims(&c, &json!({"sub": "s", "groups": "a, gridlake-admins"})).unwrap();
        assert_eq!(who.admin, Some(true));
        let mut plain = c.clone();
        plain.admin_group = None;
        assert_eq!(Identity::from_claims(&plain, &json!({"sub": "s", "groups": ["gridlake-admins"]})).unwrap().admin, None);
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn
    }

    fn who(sub: &str, username: &str, admin: Option<bool>) -> Identity {
        Identity { subject: sub.into(), username: username.into(), name: format!("{username} 的名字"), admin }
    }

    #[test]
    fn first_login_creates_account_and_second_reuses_it() {
        let conn = db();
        let c = cfg();
        let a = login_identity(&conn, &c, &c.issuer, &who("s1", "zhang", Some(false)), 7).unwrap();
        assert_eq!((a.user.id.as_str(), a.user.admin), ("zhang", false));
        let b = login_identity(&conn, &c, &c.issuer, &who("s1", "zhang", Some(false)), 7).unwrap();
        assert_eq!(b.user.id, "zhang");
        assert_ne!(a.token, b.token);
        // 另一个人用了同名：不会顶替，而是建新账号
        let other = login_identity(&conn, &c, &c.issuer, &who("s2", "ZHANG", Some(false)), 7).unwrap();
        assert_eq!(other.user.id, "ZHANG-2");
        // 密码登录用不了 SSO 账号
        assert!(crate::account::login(&conn, "zhang", "", 7).is_err());
        assert!(crate::account::login(&conn, "zhang", "!sso", 7).is_err());
    }

    #[test]
    fn admin_group_is_synced_but_last_admin_is_kept() {
        let conn = db();
        let c = cfg();
        let r = login_identity(&conn, &c, &c.issuer, &who("s1", "boss", Some(true)), 7).unwrap();
        assert!(r.user.admin);
        // 唯一的管理员被移出管理员组：不降级，免得系统里没有管理员
        let r = login_identity(&conn, &c, &c.issuer, &who("s1", "boss", Some(false)), 7).unwrap();
        assert!(r.user.admin);
        // 有了别的管理员后，可以降级
        login_identity(&conn, &c, &c.issuer, &who("s2", "boss2", Some(true)), 7).unwrap();
        let r = login_identity(&conn, &c, &c.issuer, &who("s1", "boss", Some(false)), 7).unwrap();
        assert!(!r.user.admin);
        // 没配管理员组（None）时不动现有权限
        let r = login_identity(&conn, &c, &c.issuer, &who("s2", "boss2", None), 7).unwrap();
        assert!(r.user.admin);
    }

    #[test]
    fn link_by_username_is_opt_in_and_only_for_unbound_accounts() {
        let conn = db();
        let mut c = cfg();
        crate::account::create(&conn, &crate::account::NewAccount { username: "alice".into(), name: "Alice".into(), password: "alice-pass-1".into(), admin: true }).unwrap();
        // 默认：同名不绑，建新账号
        let r = login_identity(&conn, &c, &c.issuer, &who("s1", "alice", Some(false)), 7).unwrap();
        assert_eq!(r.user.id, "alice-2");
        assert!(!r.user.admin);
        // 打开后：第一次绑定到同名的本地账号
        c.link_by_username = true;
        let r = login_identity(&conn, &c, &c.issuer, &who("s2", "alice", None), 7).unwrap();
        assert_eq!(r.user.id, "alice");
        // 已经被别的身份绑过，第三个同名的人不能再绑
        let r = login_identity(&conn, &c, &c.issuer, &who("s3", "alice", None), 7).unwrap();
        assert_eq!(r.user.id, "alice-3");
    }

    #[test]
    fn disabled_and_unprovisioned_are_rejected() {
        let conn = db();
        let mut c = cfg();
        login_identity(&conn, &c, &c.issuer, &who("s1", "dave", None), 7).unwrap();
        conn.execute("UPDATE ledger_account SET disabled = true WHERE id = 'dave'", []).unwrap();
        assert_eq!(login_identity(&conn, &c, &c.issuer, &who("s1", "dave", None), 7).err().map(|f| f.code), Some("disabled"));
        c.auto_create = false;
        assert_eq!(login_identity(&conn, &c, &c.issuer, &who("s9", "erin", None), 7).err().map(|f| f.code), Some("not_provisioned"));
    }

    #[test]
    fn config_rules() {
        let get = |extra: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                let base = [("LEDGER_OIDC_ISSUER", "https://idp.example.com"), ("LEDGER_OIDC_CLIENT_ID", "x"), ("LEDGER_OIDC_REDIRECT_URL", "https://a.example.com/cb")];
                extra.iter().chain(base.iter()).find(|(a, _)| *a == k).map(|(_, v)| v.to_string())
            }
        };
        assert!(OidcConfig::parse(|_| None).unwrap().is_none());
        assert!(OidcConfig::parse(get(&[])).unwrap().is_some());
        // 明文 http 的签发者：回环可以，其它必须显式放行
        assert!(OidcConfig::parse(get(&[("LEDGER_OIDC_ISSUER", "http://idp.corp.local")])).is_err());
        assert!(OidcConfig::parse(get(&[("LEDGER_OIDC_ISSUER", "http://idp.corp.local"), ("LEDGER_OIDC_ALLOW_HTTP", "1")])).is_ok());
        assert!(OidcConfig::parse(get(&[("LEDGER_OIDC_ISSUER", "http://127.0.0.1:9000/")])).is_ok());
        assert!(OidcConfig::parse(get(&[("LEDGER_OIDC_SCOPES", "profile email")])).is_err(), "scope 必须含 openid");
        assert!(OidcConfig::parse(|k| (k == "LEDGER_OIDC_ISSUER").then(|| "https://idp.example.com".to_string())).is_err(), "缺 client_id");
    }
}
