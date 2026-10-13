//! 个人访问令牌（PAT）：给脚本、CI、其他系统调用接口用，不会像登录会话那样几天就过期
//!
//! - 只用于内置账号登录（`LEDGER_AUTH_MODE=local`）：令牌绑定到一个内置账号，权限与该账号完全相同；
//! - 格式 `glp_` + 32 字节随机数（43 个字符）。库里只存 SHA-256 摘要，创建时返回一次，之后看不到；
//! - 用法：`Authorization: Bearer glp_…` 或 `x-user-token: glp_…`；
//! - 可以设有效期（默认 90 天，也可以不过期），随时吊销；账号被停用、删除后令牌立即失效；
//! - **不能用令牌创建 / 吊销令牌**，需要用登录会话操作：令牌泄露后攻击者没法给自己再造一批；
//! - 每个账号最多 20 个。

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use duckdb::{Connection, OptionalExt, params};
use serde::{Deserialize, Serialize};

use crate::account::{expires_after, random_bytes, token_hash, token_of};
use crate::auth::{CurrentUser, run};
use crate::config::AuthMode;
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::{AppState, parse_json};

pub const PREFIX: &str = "glp_";
const MAX_PER_ACCOUNT: i64 = 20;
const DEFAULT_DAYS: i64 = 90;
const MAX_DAYS: i64 = 3650;

/// 令牌的字符串是不是个人访问令牌（而不是登录会话令牌）
pub fn is_pat(token: &str) -> bool {
    token.starts_with(PREFIX)
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Pat {
    pub id: String,
    pub name: String,
    /// 令牌开头的几位，用来在列表里认出是哪一个（不足以还原令牌）
    pub prefix: String,
    pub created_at: String,
    /// 空表示永不过期
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub expired: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    #[serde(flatten)]
    pub pat: Pat,
    /// 完整令牌：只在创建时返回这一次
    pub token: String,
}

const COLS: &str = "id, name, prefix, strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(expires_at, '%Y-%m-%d %H:%M:%S'), strftime(last_used_at, '%Y-%m-%d %H:%M:%S'), \
                    (expires_at IS NOT NULL AND expires_at <= CAST(? AS TIMESTAMP))";

fn read(r: &duckdb::Row) -> duckdb::Result<Pat> {
    Ok(Pat { id: r.get(0)?, name: r.get(1)?, prefix: r.get(2)?, created_at: r.get(3)?, expires_at: r.get(4)?, last_used_at: r.get(5)?, expired: r.get(6)? })
}

pub fn list(conn: &Connection, account_id: &str) -> AppResult<Vec<Pat>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM ledger_pat WHERE account_id = ? ORDER BY created_at DESC, id"))?;
    Ok(stmt.query_map(params![now_local(), account_id], read)?.collect::<Result<Vec<_>, _>>()?)
}

/// 新建令牌；`days` 为 None 表示永不过期
pub fn create(conn: &Connection, account_id: &str, name: &str, days: Option<i64>) -> AppResult<Created> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::bad("令牌名称不能为空，且不超过 40 个字符"));
    }
    if let Some(d) = days
        && !(1..=MAX_DAYS).contains(&d)
    {
        return Err(AppError::bad(format!("有效期应在 1 到 {MAX_DAYS} 天之间")));
    }
    let existing: i64 = conn.query_row("SELECT count(*) FROM ledger_pat WHERE account_id = ?", params![account_id], |r| r.get(0))?;
    if existing >= MAX_PER_ACCOUNT {
        return Err(AppError::bad(format!("每个账号最多 {MAX_PER_ACCOUNT} 个访问令牌，请先吊销不用的")));
    }
    let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(random_bytes::<32>()));
    let id = format!("pat_{}", URL_SAFE_NO_PAD.encode(random_bytes::<9>()));
    conn.execute(
        "INSERT INTO ledger_pat (id, account_id, name, token_hash, prefix, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![id, account_id, name, token_hash(&token), &token[..PREFIX.len() + 4], now_local(), days.map(expires_after)],
    )?;
    let pat = conn.query_row(&format!("SELECT {COLS} FROM ledger_pat WHERE id = ?"), params![now_local(), id], read)?;
    Ok(Created { pat, token })
}

/// 吊销自己的令牌
pub fn revoke(conn: &Connection, account_id: &str, id: &str) -> AppResult<()> {
    let n = conn.execute("DELETE FROM ledger_pat WHERE id = ? AND account_id = ?", params![id, account_id])?;
    if n == 0 {
        return Err(AppError::bad("令牌不存在"));
    }
    Ok(())
}

/// 令牌对应的用户；不存在、过期或账号停用时为 None。顺便记下最近使用时间（一分钟内不重复写）
pub fn user_for(conn: &Connection, token: &str) -> AppResult<Option<CurrentUser>> {
    let now = now_local();
    let hash = token_hash(token);
    let found = conn
        .query_row(
            "SELECT p.id, a.id, a.name, a.admin FROM ledger_pat p JOIN ledger_account a ON a.id = p.account_id
             WHERE p.token_hash = ? AND (p.expires_at IS NULL OR p.expires_at > CAST(? AS TIMESTAMP)) AND NOT a.disabled",
            params![hash, now],
            |r| Ok((r.get::<_, String>(0)?, CurrentUser { id: r.get(1)?, name: r.get(2)?, admin: r.get(3)?, enforced: true })),
        )
        .optional()?;
    let Some((pat_id, user)) = found else { return Ok(None) };
    conn.execute(
        "UPDATE ledger_pat SET last_used_at = CAST(? AS TIMESTAMP) WHERE id = ? AND (last_used_at IS NULL OR last_used_at < CAST(? AS TIMESTAMP) - INTERVAL 1 MINUTE)",
        params![now, pat_id, now],
    )?;
    Ok(Some(user))
}

// ── 接口 ──────────────────────────────────────────────

fn require_local(db: &AppState) -> AppResult<()> {
    if db.auth.mode != AuthMode::Local {
        return Err(AppError::bad("个人访问令牌只在内置账号登录方式下可用（LEDGER_AUTH_MODE=local）"));
    }
    Ok(())
}

/// 不能用访问令牌管理访问令牌
fn require_session(headers: &HeaderMap) -> AppResult<()> {
    if is_pat(&token_of(headers)) {
        return Err(AppError::Forbidden("不能用访问令牌管理访问令牌，请在网页里登录后操作".into()));
    }
    Ok(())
}

/// GET /api/ledger/tokens  我的访问令牌（不含令牌本身）
pub async fn list_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>) -> AppResult<Json<Envelope<Vec<Pat>>>> {
    require_local(&db)?;
    Ok(ok(run(&db, move |c| list(c, &user.id)).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewToken {
    pub name: String,
    /// 有效期天数；不传用 90 天，传 null 或 0 表示永不过期
    #[serde(default = "default_days")]
    pub days: Option<i64>,
}

fn default_days() -> Option<i64> {
    Some(DEFAULT_DAYS)
}

/// POST /api/ledger/tokens  { name, days? }  新建访问令牌；响应里的 token 只显示这一次
pub async fn create_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Envelope<Created>>> {
    require_local(&db)?;
    require_session(&headers)?;
    let b: NewToken = parse_json(&body)?;
    let days = b.days.filter(|d| *d != 0);
    Ok(ok(run(&db, move |c| create(c, &user.id, &b.name, days)).await?))
}

/// DELETE /api/ledger/tokens/{id}  吊销我的访问令牌（立即生效）
pub async fn revoke_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, headers: HeaderMap, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    require_local(&db)?;
    require_session(&headers)?;
    run(&db, move |c| revoke(c, &user.id, &id)).await?;
    db.auth.clear_cache();
    Ok(ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(&schema::create_tables_sql()).unwrap();
        c.execute_batch(&schema::migrate_sql()).unwrap();
        c.execute("INSERT INTO ledger_account (id, username, name, password_hash, admin, disabled, created_at) VALUES ('a1', 'alice', 'Alice', 'x', false, false, CAST(? AS TIMESTAMP))", params![now_local()]).unwrap();
        c
    }

    #[test]
    fn created_token_authenticates_and_is_only_stored_as_a_hash() {
        let c = db();
        let made = create(&c, "a1", "CI", Some(30)).unwrap();
        assert!(made.token.starts_with(PREFIX) && made.token.len() >= 40, "{}", made.token);
        assert_eq!(made.pat.prefix, &made.token[..8]);
        let u = user_for(&c, &made.token).unwrap().expect("应认得这个令牌");
        assert_eq!((u.id.as_str(), u.name.as_str(), u.admin), ("a1", "Alice", false));
        // 库里没有明文
        let stored: String = c.query_row("SELECT token_hash FROM ledger_pat", [], |r| r.get(0)).unwrap();
        assert_ne!(stored, made.token);
        assert_eq!(stored, token_hash(&made.token));
        let leaked: i64 = c.query_row("SELECT count(*) FROM ledger_pat WHERE token_hash = ? OR prefix = ? OR name = ?", params![made.token, made.token, made.token], |r| r.get(0)).unwrap();
        assert_eq!(leaked, 0);
        assert!(user_for(&c, "glp_wrong").unwrap().is_none());
    }

    #[test]
    fn expiry_revocation_and_disabled_accounts() {
        let c = db();
        let made = create(&c, "a1", "t", Some(1)).unwrap();
        c.execute("UPDATE ledger_pat SET expires_at = CAST('2000-01-01 00:00:00' AS TIMESTAMP)", []).unwrap();
        assert!(user_for(&c, &made.token).unwrap().is_none(), "过期");
        assert!(list(&c, "a1").unwrap()[0].expired);

        let forever = create(&c, "a1", "never", None).unwrap();
        assert!(forever.pat.expires_at.is_none());
        assert!(user_for(&c, &forever.token).unwrap().is_some());
        c.execute("UPDATE ledger_account SET disabled = true WHERE id = 'a1'", []).unwrap();
        assert!(user_for(&c, &forever.token).unwrap().is_none(), "账号停用后令牌失效");
        c.execute("UPDATE ledger_account SET disabled = false WHERE id = 'a1'", []).unwrap();

        revoke(&c, "a1", &forever.pat.id).unwrap();
        assert!(user_for(&c, &forever.token).unwrap().is_none(), "吊销后失效");
        assert!(revoke(&c, "a1", &forever.pat.id).is_err());
        // 不能吊销别人的
        let other = create(&c, "a1", "x", None).unwrap();
        assert!(revoke(&c, "someone-else", &other.pat.id).is_err());
    }

    #[test]
    fn validation_and_limits() {
        let c = db();
        assert!(create(&c, "a1", "  ", None).is_err());
        assert!(create(&c, "a1", &"x".repeat(41), None).is_err());
        assert!(create(&c, "a1", "ok", Some(0)).is_err());
        assert!(create(&c, "a1", "ok", Some(MAX_DAYS + 1)).is_err());
        for i in 0..MAX_PER_ACCOUNT {
            create(&c, "a1", &format!("t{i}"), None).unwrap();
        }
        assert!(create(&c, "a1", "one too many", None).is_err());
    }

    #[test]
    fn last_used_is_recorded_without_writing_on_every_call() {
        let c = db();
        let made = create(&c, "a1", "t", Some(30)).unwrap();
        assert!(list(&c, "a1").unwrap()[0].last_used_at.is_none());
        user_for(&c, &made.token).unwrap();
        let first = list(&c, "a1").unwrap()[0].last_used_at.clone();
        assert!(first.is_some());
        user_for(&c, &made.token).unwrap();
        assert_eq!(list(&c, "a1").unwrap()[0].last_used_at, first, "一分钟内不重复写");
    }
}
