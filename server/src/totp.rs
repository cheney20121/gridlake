//! 两步验证（TOTP，RFC 6238）：内置账号登录时，密码之后再输入验证器 App（Google Authenticator、Microsoft Authenticator、
//! 1Password、Authy 等）里的 6 位动态码
//!
//! - 算法：HMAC-SHA1、6 位、30 秒一步（所有验证器 App 的默认值）；允许前后各一步的时钟误差；
//! - 一个动态码只能用一次：记下账号最近一次用过的时间步，同一步或更早的码一律拒绝（防止偷看到的码被重放）；
//! - 密钥只以密文存库（与数据连接共用 `secret.key` 加密）；启用时生成 10 个一次性恢复码（手机丢了用），库里只存它们的 SHA-256；
//! - 输错的动态码和密码错误一样计入登录失败次数（10 分钟内 5 次锁定），所以 6 位数字没法被暴力穷举；
//! - 只用于内置账号的密码登录；单点登录（OIDC）账号由身份提供方负责二次验证；个人访问令牌是独立的凭证，不受影响；
//! - 管理员可以重置某个账号的两步验证（手机丢了又没有恢复码），重置会让该账号已登录的会话失效。
//!
//! SHA-1 / HMAC-SHA1 / Base32 都在本文件里自己实现（SHA-1 只在这里需要，不为它引入依赖），并用 RFC 的测试向量验证。

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use duckdb::{Connection, OptionalExt, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::account::{random_bytes, token_of, verify_password};
use crate::auth::{CurrentUser, run};
use crate::config::AuthMode;
use crate::connector::secret::Secret;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::{AppState, parse_json};

const STEP_SECS: i64 = 30;
const DIGITS: u32 = 6;
const RECOVERY_CODES: usize = 10;
const ISSUER: &str = "Gridlake";

// ── 密码学小零件 ──────────────────────────────────────

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, b) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0);
    for &b in data {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let (mut buf, mut bits, mut out) = (0u32, 0, Vec::new());
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let v = B32.iter().position(|&x| x as char == c.to_ascii_uppercase())? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buf >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

// ── TOTP ─────────────────────────────────────────────

/// 某个时间步的动态码
pub fn code_at(secret: &[u8], step: i64) -> u32 {
    let h = hmac_sha1(secret, &(step as u64).to_be_bytes());
    let o = (h[19] & 0x0f) as usize;
    let n = u32::from_be_bytes([h[o] & 0x7f, h[o + 1], h[o + 2], h[o + 3]]);
    n % 10u32.pow(DIGITS)
}

fn step_of(unix_secs: i64) -> i64 {
    unix_secs.div_euclid(STEP_SECS)
}

/// 校验动态码：允许前后各一步；`last_step` 之前（含）用过的步一律不收。返回匹配的步（用来记下已用）
pub fn verify_code(secret: &[u8], code: &str, now_secs: i64, last_step: Option<i64>) -> Option<i64> {
    let code = code.trim().replace(' ', "");
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let want: u32 = code.parse().ok()?;
    let now = step_of(now_secs);
    (now - 1..=now + 1).find(|&s| last_step.is_none_or(|l| s > l) && code_at(secret, s) == want)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn pct(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// 验证器 App 扫码 / 点击用的地址
pub fn otpauth_uri(username: &str, secret_b32: &str) -> String {
    format!("otpauth://totp/{ISSUER}:{}?secret={secret_b32}&issuer={ISSUER}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}", pct(username))
}

// ── 存取 ─────────────────────────────────────────────

fn hash_recovery(code: &str) -> String {
    let norm: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    format!("{:x}", Sha256::digest(norm.as_bytes()))
}

fn new_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODES)
        .map(|_| {
            let s = base32_encode(&random_bytes::<7>()).to_lowercase();
            format!("{}-{}", &s[..5], &s[5..10])
        })
        .collect()
}

struct State2fa {
    enabled: bool,
    secret_enc: Option<String>,
    last_step: Option<i64>,
}

fn state(conn: &Connection, account_id: &str) -> AppResult<State2fa> {
    conn.query_row("SELECT totp_enabled, totp_secret_enc, totp_last_step FROM ledger_account WHERE id = ?", params![account_id], |r| {
        Ok(State2fa { enabled: r.get::<_, Option<bool>>(0)?.unwrap_or(false), secret_enc: r.get(1)?, last_step: r.get(2)? })
    })
    .optional()?
    .ok_or_else(|| AppError::bad("账号不存在"))
}

/// 这个账号有没有启用两步验证
pub fn is_enabled(conn: &Connection, account_id: &str) -> AppResult<bool> {
    Ok(state(conn, account_id)?.enabled)
}

/// 登录时的第二步：动态码或恢复码。对就记下已用（动态码记时间步，恢复码作废）；不对返回 false
pub fn check_login_code(conn: &Connection, secret: &Secret, account_id: &str, code: &str) -> AppResult<bool> {
    let st = state(conn, account_id)?;
    let Some(enc) = st.secret_enc.filter(|_| st.enabled) else { return Ok(true) };
    let key = secret.decrypt(&enc).map_err(|e| AppError::bad(e.to_string()))?;
    let key = base32_decode(&key).ok_or_else(|| AppError::bad("两步验证密钥已损坏，请联系管理员重置"))?;
    if let Some(step) = verify_code(&key, code, now_secs(), st.last_step) {
        conn.execute("UPDATE ledger_account SET totp_last_step = ? WHERE id = ?", params![step, account_id])?;
        return Ok(true);
    }
    let used = conn.execute("DELETE FROM ledger_totp_recovery WHERE account_id = ? AND code_hash = ?", params![account_id, hash_recovery(code)])?;
    Ok(used > 0)
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    /// 还剩几个没用过的恢复码
    pub recovery_left: i64,
}

pub fn status(conn: &Connection, account_id: &str) -> AppResult<Status> {
    let enabled = state(conn, account_id)?.enabled;
    let recovery_left = conn.query_row("SELECT count(*) FROM ledger_totp_recovery WHERE account_id = ?", params![account_id], |r| r.get(0))?;
    Ok(Status { enabled, recovery_left })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    /// Base32 密钥，扫码不方便时手工输入
    pub secret: String,
    /// otpauth:// 地址（生成二维码、或手机上直接点开）
    pub uri: String,
}

/// 开始设置：生成新密钥（还没启用，验证一次动态码才算数）。已经启用时要先停用
pub fn start_setup(conn: &Connection, secret: &Secret, account_id: &str, username: &str) -> AppResult<Setup> {
    if state(conn, account_id)?.enabled {
        return Err(AppError::bad("已经启用了两步验证；要换手机请先停用再重新设置"));
    }
    let b32 = base32_encode(&random_bytes::<20>());
    conn.execute("UPDATE ledger_account SET totp_secret_enc = ?, totp_enabled = false, totp_last_step = NULL WHERE id = ?", params![secret.encrypt(&b32)?, account_id])?;
    Ok(Setup { uri: otpauth_uri(username, &b32), secret: b32 })
}

/// 用第一个动态码确认设置，启用并生成恢复码（只在这一次返回明文）
pub fn enable(conn: &Connection, secret: &Secret, account_id: &str, code: &str) -> AppResult<Vec<String>> {
    let st = state(conn, account_id)?;
    if st.enabled {
        return Err(AppError::bad("已经启用了两步验证"));
    }
    let enc = st.secret_enc.ok_or_else(|| AppError::bad("请先点「开始设置」生成密钥"))?;
    let key = base32_decode(&secret.decrypt(&enc).map_err(|e| AppError::bad(e.to_string()))?).ok_or_else(|| AppError::bad("请重新开始设置"))?;
    let step = verify_code(&key, code, now_secs(), None).ok_or_else(|| AppError::bad("验证码不正确：请确认手机时间准确，并输入验证器 App 里现在显示的 6 位数字"))?;
    let codes = new_recovery_codes();
    conn.execute("DELETE FROM ledger_totp_recovery WHERE account_id = ?", params![account_id])?;
    for c in &codes {
        conn.execute("INSERT INTO ledger_totp_recovery (account_id, code_hash) VALUES (?, ?)", params![account_id, hash_recovery(c)])?;
    }
    conn.execute("UPDATE ledger_account SET totp_enabled = true, totp_last_step = ? WHERE id = ?", params![step, account_id])?;
    Ok(codes)
}

/// 清掉两步验证（用户自己停用、或管理员重置）
pub fn clear(conn: &Connection, account_id: &str) -> AppResult<()> {
    conn.execute("UPDATE ledger_account SET totp_enabled = false, totp_secret_enc = NULL, totp_last_step = NULL WHERE id = ?", params![account_id])?;
    conn.execute("DELETE FROM ledger_totp_recovery WHERE account_id = ?", params![account_id])?;
    Ok(())
}

// ── 接口 ──────────────────────────────────────────────

fn require_local(db: &AppState) -> AppResult<()> {
    if db.auth.mode != AuthMode::Local {
        return Err(AppError::bad("两步验证只在内置账号登录方式下可用（LEDGER_AUTH_MODE=local）"));
    }
    Ok(())
}

/// 两步验证要用登录会话操作，个人访问令牌不行（令牌泄露后攻击者不能改掉你的两步验证）
fn require_session(headers: &HeaderMap) -> AppResult<()> {
    if crate::pat::is_pat(&token_of(headers)) {
        return Err(AppError::Forbidden("不能用访问令牌设置两步验证，请在网页里登录后操作".into()));
    }
    Ok(())
}

fn account_username(conn: &Connection, id: &str) -> AppResult<String> {
    conn.query_row("SELECT username FROM ledger_account WHERE id = ?", params![id], |r| r.get(0)).map_err(|_| AppError::bad("账号不存在"))
}

/// GET /api/ledger/auth/2fa  我的两步验证状态
pub async fn status_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>) -> AppResult<Json<Envelope<Status>>> {
    require_local(&db)?;
    Ok(ok(run(&db, move |c| status(c, &user.id)).await?))
}

/// POST /api/ledger/auth/2fa/setup  开始设置两步验证：返回密钥和 otpauth 地址（还没启用，要再验证一次动态码）
pub async fn setup_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, headers: HeaderMap) -> AppResult<Json<Envelope<Setup>>> {
    require_local(&db)?;
    require_session(&headers)?;
    let st = db.clone();
    Ok(ok(run(&db, move |c| start_setup(c, &st.connectors.secret, &user.id, &account_username(c, &user.id)?)).await?))
}

#[derive(Deserialize)]
pub struct CodeBody {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enabled {
    /// 一次性恢复码：只显示这一次，请保存好
    pub recovery_codes: Vec<String>,
}

/// POST /api/ledger/auth/2fa/enable  { code }  用验证器里的 6 位数字确认并启用；返回 10 个一次性恢复码（只显示这一次）
pub async fn enable_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Envelope<Enabled>>> {
    require_local(&db)?;
    require_session(&headers)?;
    let b: CodeBody = parse_json(&body)?;
    let st = db.clone();
    let codes = run(&db, move |c| enable(c, &st.connectors.secret, &user.id, &b.code)).await?;
    Ok(ok(Enabled { recovery_codes: codes }))
}

/// POST /api/ledger/auth/2fa/disable  { password, code }  停用两步验证（要再输一次密码和动态码 / 恢复码）
pub async fn disable_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Envelope<bool>>> {
    require_local(&db)?;
    require_session(&headers)?;
    let b: CodeBody = parse_json(&body)?;
    let st = db.clone();
    run(&db, move |c| {
        let hash: String = c.query_row("SELECT password_hash FROM ledger_account WHERE id = ?", params![user.id], |r| r.get(0))?;
        if !verify_password(&b.password, &hash) {
            return Err(AppError::bad("密码不正确"));
        }
        if !check_login_code(c, &st.connectors.secret, &user.id, &b.code)? {
            return Err(AppError::bad("验证码不正确"));
        }
        clear(c, &user.id)
    })
    .await?;
    Ok(ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_and_hmac_match_the_rfc_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(&vec![b'a'; 1_000_000])), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
        assert_eq!(hex(&sha1(b"The quick brown fox jumps over the lazy dog")), "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12");
        // RFC 2202
        assert_eq!(hex(&hmac_sha1(&[0x0b; 20], b"Hi There")), "b617318655057264e28bc0b6fb378c8ef146be00");
        assert_eq!(hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")), "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79");
        // 密钥比块长：先哈希
        assert_eq!(hex(&hmac_sha1(&[0xaa; 80], b"Test Using Larger Than Block-Size Key - Hash Key First")), "aa4ae5e15272d00e95705637ce8a3b55ed402112");
    }

    #[test]
    fn totp_matches_rfc6238_vectors() {
        // RFC 6238 附录 B（SHA-1，密钥 "12345678901234567890"，取 6 位）
        let key = b"12345678901234567890";
        for (t, want) in [(59, 287_082), (1_111_111_109, 81_804), (1_111_111_111, 50_471), (1_234_567_890, 5_924), (2_000_000_000, 279_037), (20_000_000_000i64, 353_130)] {
            assert_eq!(code_at(key, step_of(t)), want, "t={t}");
        }
    }

    #[test]
    fn base32_roundtrip_and_known_value() {
        assert_eq!(base32_encode(b"12345678901234567890"), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(base32_decode("gezd gnbv GY3TQOJQ GEZDGNBVGY3TQOJQ").unwrap(), b"12345678901234567890");
        assert_eq!(base32_decode(&base32_encode(&[1, 2, 3, 250, 251])).unwrap(), vec![1, 2, 3, 250, 251]);
        assert!(base32_decode("not base32!").is_none());
    }

    #[test]
    fn verify_allows_one_step_of_drift_and_rejects_replays() {
        let key = b"12345678901234567890";
        let now = 1_111_111_109;
        let c = format!("{:06}", code_at(key, step_of(now)));
        assert_eq!(verify_code(key, &c, now, None), Some(step_of(now)));
        // 前后各一步可以，两步不行
        let prev = format!("{:06}", code_at(key, step_of(now) - 1));
        assert_eq!(verify_code(key, &prev, now, None), Some(step_of(now) - 1));
        let old = format!("{:06}", code_at(key, step_of(now) - 2));
        assert_eq!(verify_code(key, &old, now, None), None);
        // 同一步用过就不能再用；更早的步也不行
        assert_eq!(verify_code(key, &c, now, Some(step_of(now))), None);
        assert_eq!(verify_code(key, &prev, now, Some(step_of(now))), None);
        // 格式不对
        for bad in ["", "12345", "1234567", "abcdef", "12 345"] {
            assert_eq!(verify_code(key, bad, now, None), None, "{bad}");
        }
        // 验证器里常把 6 位拆成 3+3 显示
        assert_eq!(verify_code(key, &format!("{} {}", &c[..3], &c[3..]), now, None), Some(step_of(now)));
    }

    fn db() -> (Connection, Secret, tempfile::TempDir) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        crate::account::create(&conn, &crate::account::NewAccount { username: "alice".into(), name: "Alice".into(), password: "alice-pass-1".into(), admin: false }).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let secret = Secret::load(dir.path()).unwrap();
        (conn, secret, dir)
    }

    fn live_code(b32: &str, step_offset: i64) -> String {
        format!("{:06}", code_at(&base32_decode(b32).unwrap(), step_of(now_secs()) + step_offset))
    }

    #[test]
    fn full_flow_setup_enable_login_replay_recovery_reset() {
        let (conn, secret, _dir) = db();
        use crate::account::login;
        // 没启用：只要密码
        assert!(!login(&conn, &secret, "alice", "alice-pass-1", None, 7).unwrap().need_code);
        // 设置：还没确认前不算启用，登录不受影响；确认码不对不能启用
        let setup = start_setup(&conn, &secret, "alice", "alice").unwrap();
        assert!(setup.uri.contains(&setup.secret) && !is_enabled(&conn, "alice").unwrap());
        assert!(!login(&conn, &secret, "alice", "alice-pass-1", None, 7).unwrap().need_code);
        assert!(enable(&conn, &secret, "alice", "abcdef").is_err());
        // 密钥在库里是密文
        let stored: String = conn.query_row("SELECT totp_secret_enc FROM ledger_account WHERE id = 'alice'", [], |r| r.get(0)).unwrap();
        assert!(!stored.contains(&setup.secret));
        let codes = enable(&conn, &secret, "alice", &live_code(&setup.secret, 0)).unwrap();
        assert_eq!(codes.len(), RECOVERY_CODES);
        assert_eq!(status(&conn, "alice").unwrap(), Status { enabled: true, recovery_left: RECOVERY_CODES as i64 });
        assert!(start_setup(&conn, &secret, "alice", "alice").is_err(), "启用后不能直接重新生成密钥");
        // 启用后：密码对了但没给码 → 要求验证码，没有令牌
        let r = login(&conn, &secret, "alice", "alice-pass-1", None, 7).unwrap();
        assert!(r.need_code && r.token.is_empty());
        // 启用时用掉的那一步不能再用；错码被拒绝；下一步的码可以
        assert!(login(&conn, &secret, "alice", "alice-pass-1", Some(&live_code(&setup.secret, 0)), 7).is_err(), "启用时用过的码不能重放");
        assert!(login(&conn, &secret, "alice", "alice-pass-1", Some("abcdef"), 7).is_err());
        let ok = login(&conn, &secret, "alice", "alice-pass-1", Some(&live_code(&setup.secret, 1)), 7).unwrap();
        assert!(!ok.need_code && ok.token.len() >= 40);
        assert!(login(&conn, &secret, "alice", "alice-pass-1", Some(&live_code(&setup.secret, 1)), 7).is_err(), "同一个码只能用一次");
        // 密码错了不会泄露「有没有启用两步验证」：照旧只说用户名或密码不对
        assert_eq!(login(&conn, &secret, "alice", "wrong-pass-1", None, 7).unwrap_err().message(), "用户名或密码不正确");
        // 恢复码：能登录一次，用过作废，格式宽松（大小写、连字符）
        let rc = codes[0].to_uppercase().replace('-', "");
        assert!(login(&conn, &secret, "alice", "alice-pass-1", Some(&rc), 7).is_ok());
        assert!(login(&conn, &secret, "alice", "alice-pass-1", Some(&codes[0]), 7).is_err());
        assert_eq!(status(&conn, "alice").unwrap().recovery_left, RECOVERY_CODES as i64 - 1);
        // 停用 / 重置：清掉密钥和恢复码，又只要密码
        clear(&conn, "alice").unwrap();
        assert_eq!(status(&conn, "alice").unwrap(), Status { enabled: false, recovery_left: 0 });
        assert!(!login(&conn, &secret, "alice", "alice-pass-1", None, 7).unwrap().need_code);
    }

    #[test]
    fn wrong_codes_count_towards_the_lockout() {
        let (conn, secret, _dir) = db();
        let setup = start_setup(&conn, &secret, "alice", "alice").unwrap();
        enable(&conn, &secret, "alice", &live_code(&setup.secret, 0)).unwrap();
        for _ in 0..5 {
            assert!(crate::account::login(&conn, &secret, "alice", "alice-pass-1", Some("999999"), 7).is_err());
        }
        // 第 6 次：即使码对了也被锁定
        let msg = crate::account::login(&conn, &secret, "alice", "alice-pass-1", Some(&live_code(&setup.secret, 1)), 7).unwrap_err().message();
        assert!(msg.contains("失败次数过多"), "{msg}");
    }

    #[test]
    fn admin_reset_clears_two_factor_and_sessions() {
        let (conn, secret, _dir) = db();
        let setup = start_setup(&conn, &secret, "alice", "alice").unwrap();
        enable(&conn, &secret, "alice", &live_code(&setup.secret, 0)).unwrap();
        let s = crate::account::start_session(&conn, "alice", 7).unwrap();
        let admin = CurrentUser { id: "admin".into(), name: "管理员".into(), admin: true, enforced: true };
        let a = crate::account::update(&conn, &admin, "alice", &crate::account::AccountPatch { reset_totp: Some(true), ..Default::default() }).unwrap();
        assert!(!a.two_factor);
        assert!(crate::account::session_user(&conn, &s.token).unwrap().is_none(), "重置后已登录的会话失效");
        assert_eq!(status(&conn, "alice").unwrap().recovery_left, 0);
    }

    #[test]
    fn otpauth_uri_is_escaped() {
        let u = otpauth_uri("张三 a@b", "ABC");
        assert!(u.starts_with("otpauth://totp/Gridlake:%E5%BC%A0%E4%B8%89%20a%40b?secret=ABC&issuer=Gridlake"), "{u}");
    }

    #[test]
    fn recovery_codes_normalize() {
        assert_eq!(hash_recovery("ABCDE-fghij"), hash_recovery("abcdefghij"));
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODES);
        assert!(codes.iter().all(|c| c.len() == 11 && c.as_bytes()[5] == b'-'));
        assert_eq!(codes.iter().collect::<std::collections::HashSet<_>>().len(), RECOVERY_CODES);
    }
}
