//! 个人访问令牌（PAT）：给脚本、CI、其他系统调用接口用，不会像登录会话那样几天就过期
//!
//! - 只用于内置账号登录（`LEDGER_AUTH_MODE=local`）：令牌绑定到一个内置账号，权限与该账号完全相同；
//! - 格式 `glp_` + 32 字节随机数（43 个字符）。库里只存 SHA-256 摘要，创建时返回一次，之后看不到；
//! - **只读令牌**以 `glr_` 开头：只能读取数据（GET / HEAD，以及 [`crate::auth::READ_ONLY_POST`] 里登记的计算类 POST），其余写操作一律 403。
//!   「只读」写在令牌本身的前缀里而不是单独的表列：旧版程序不认识 `glr_`，降级后只会拒绝这类令牌，不会把它当成完全权限的令牌；
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

use std::collections::BTreeSet;

use crate::account::{expires_after, random_bytes, token_hash, token_of};
use crate::auth::{CurrentUser, run};
use crate::config::AuthMode;
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::{AppState, parse_json};

pub const PREFIX: &str = "glp_";
/// 只读令牌的前缀
pub const PREFIX_READ_ONLY: &str = "glr_";
const MAX_PER_ACCOUNT: i64 = 20;
const DEFAULT_DAYS: i64 = 90;
const MAX_DAYS: i64 = 3650;
const MAX_DATASETS: usize = 50;

/// 令牌的字符串是不是个人访问令牌（而不是登录会话令牌）
pub fn is_pat(token: &str) -> bool {
    token.starts_with(PREFIX) || token.starts_with(PREFIX_READ_ONLY)
}

/// 是不是只读令牌（只看前缀；令牌内容被改动就对不上库里的摘要，认证时会失败）
pub fn is_read_only(token: &str) -> bool {
    token.starts_with(PREFIX_READ_ONLY)
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
    /// 只读令牌：只能读取，不能修改任何数据
    pub read_only: bool,
    /// 限定能访问的数据集（空表示不限）；名称在列表时按当前数据集目录补上，数据集被删除后为 None
    pub datasets: Vec<LimitedDataset>,
}

#[derive(Serialize, Debug)]
pub struct LimitedDataset {
    pub id: String,
    pub name: Option<String>,
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
                    (expires_at IS NOT NULL AND expires_at <= CAST(? AS TIMESTAMP)), COALESCE(datasets, '')";

fn read(r: &duckdb::Row) -> duckdb::Result<Pat> {
    let prefix: String = r.get(2)?;
    Ok(Pat { id: r.get(0)?, name: r.get(1)?, read_only: prefix.starts_with(PREFIX_READ_ONLY), prefix, created_at: r.get(3)?, expires_at: r.get(4)?, last_used_at: r.get(5)?, expired: r.get(6)?, datasets: ids_of(&r.get::<_, String>(7)?).into_iter().map(|id| LimitedDataset { id, name: None }).collect() })
}

fn ids_of(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// 给限定的数据集补上名称（数据集被删除或改名后列表里显示当前名称）
fn fill_names(conn: &Connection, pats: &mut [Pat]) {
    if pats.iter().all(|p| p.datasets.is_empty()) {
        return;
    }
    let Ok((defs, _)) = crate::dataset::load(conn) else { return };
    for d in pats.iter_mut().flat_map(|p| p.datasets.iter_mut()) {
        d.name = defs.iter().find(|x| x.id == d.id).map(|x| x.name.clone());
    }
}

pub fn list(conn: &Connection, account_id: &str) -> AppResult<Vec<Pat>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM ledger_pat WHERE account_id = ? ORDER BY created_at DESC, id"))?;
    let mut out = stmt.query_map(params![now_local(), account_id], read)?.collect::<Result<Vec<_>, _>>()?;
    fill_names(conn, &mut out);
    Ok(out)
}

/// 新建令牌；`days` 为 None 表示永不过期；`read_only` 为真时生成只读令牌（`glr_` 开头）；`datasets` 非空时令牌只能访问这些数据集
pub fn create(conn: &Connection, account_id: &str, name: &str, days: Option<i64>, read_only: bool, datasets: &[String]) -> AppResult<Created> {
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
    let mut limited: Vec<String> = datasets.iter().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect();
    limited.sort();
    limited.dedup();
    if limited.len() > MAX_DATASETS {
        return Err(AppError::bad(format!("一个令牌最多限定 {MAX_DATASETS} 个数据集")));
    }
    if !limited.is_empty() {
        // 只能限定到自己看得到的数据集
        let known = crate::dataset::datasets(conn)?;
        if let Some(bad) = limited.iter().find(|id| !known.iter().any(|d| &d.id == *id)) {
            return Err(AppError::bad(format!("数据集不存在或无权访问：{bad}")));
        }
    }
    let lead = if read_only { PREFIX_READ_ONLY } else { PREFIX };
    let token = format!("{lead}{}", URL_SAFE_NO_PAD.encode(random_bytes::<32>()));
    let id = format!("pat_{}", URL_SAFE_NO_PAD.encode(random_bytes::<9>()));
    conn.execute(
        "INSERT INTO ledger_pat (id, account_id, name, token_hash, prefix, created_at, expires_at, datasets)
         VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP), ?)",
        params![id, account_id, name, token_hash(&token), &token[..lead.len() + 4], now_local(), days.map(expires_after), (!limited.is_empty()).then(|| serde_json::to_string(&limited)).transpose()?],
    )?;
    let mut pat = conn.query_row(&format!("SELECT {COLS} FROM ledger_pat WHERE id = ?"), params![now_local(), id], read)?;
    fill_names(conn, std::slice::from_mut(&mut pat));
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

/// 令牌限定的数据集：id 和它们当前的名称（公式下推的路径里写的是名称）。None 表示不限
pub type DatasetLimit = Option<std::sync::Arc<BTreeSet<String>>>;

/// 令牌对应的用户（以及它限定的数据集）；不存在、过期或账号停用时为 None。顺便记下最近使用时间（一分钟内不重复写）
pub fn user_for(conn: &Connection, token: &str) -> AppResult<Option<(CurrentUser, DatasetLimit)>> {
    let now = now_local();
    let hash = token_hash(token);
    let found = conn
        .query_row(
            "SELECT p.id, a.id, a.name, a.admin, COALESCE(p.datasets, '') FROM ledger_pat p JOIN ledger_account a ON a.id = p.account_id
             WHERE p.token_hash = ? AND (p.expires_at IS NULL OR p.expires_at > CAST(? AS TIMESTAMP)) AND NOT a.disabled",
            params![hash, now],
            |r| Ok((r.get::<_, String>(0)?, CurrentUser { id: r.get(1)?, name: r.get(2)?, admin: r.get(3)?, enforced: true }, r.get::<_, String>(4)?)),
        )
        .optional()?;
    let Some((pat_id, user, limited)) = found else { return Ok(None) };
    conn.execute(
        "UPDATE ledger_pat SET last_used_at = CAST(? AS TIMESTAMP) WHERE id = ? AND (last_used_at IS NULL OR last_used_at < CAST(? AS TIMESTAMP) - INTERVAL 1 MINUTE)",
        params![now, pat_id, now],
    )?;
    let ids = ids_of(&limited);
    let limit = if ids.is_empty() {
        None
    } else {
        // 限定了数据集：id 和当前名称都算（名称给公式下推用）。读不出目录时只留 id，宁可少放不可多放
        let mut set: BTreeSet<String> = ids.iter().cloned().collect();
        if let Ok((defs, _)) = crate::dataset::load(conn) {
            set.extend(defs.iter().filter(|d| ids.contains(&d.id)).map(|d| d.name.clone()));
        }
        Some(std::sync::Arc::new(set))
    };
    Ok(Some((user, limit)))
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
    /// 只读令牌：只能读取数据，写操作一律拒绝（适合报表、监控、给第三方看数据的脚本）
    #[serde(default)]
    pub read_only: bool,
    /// 限定能访问的数据集 id；不传或空表示不限
    #[serde(default)]
    pub datasets: Vec<String>,
}

fn default_days() -> Option<i64> {
    Some(DEFAULT_DAYS)
}

/// POST /api/ledger/tokens  { name, days?, readOnly?, datasets? }  新建访问令牌；响应里的 token 只显示这一次
pub async fn create_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Envelope<Created>>> {
    require_local(&db)?;
    require_session(&headers)?;
    let b: NewToken = parse_json(&body)?;
    let days = b.days.filter(|d| *d != 0);
    Ok(ok(run(&db, move |c| create(c, &user.id, &b.name, days, b.read_only, &b.datasets)).await?))
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
        let made = create(&c, "a1", "CI", Some(30), false, &[]).unwrap();
        assert!(made.token.starts_with(PREFIX) && made.token.len() >= 40, "{}", made.token);
        assert_eq!(made.pat.prefix, &made.token[..8]);
        let (u, limit) = user_for(&c, &made.token).unwrap().expect("应认得这个令牌");
        assert!(limit.is_none(), "没限定数据集");
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
        let made = create(&c, "a1", "t", Some(1), false, &[]).unwrap();
        c.execute("UPDATE ledger_pat SET expires_at = CAST('2000-01-01 00:00:00' AS TIMESTAMP)", []).unwrap();
        assert!(user_for(&c, &made.token).unwrap().is_none(), "过期");
        assert!(list(&c, "a1").unwrap()[0].expired);

        let forever = create(&c, "a1", "never", None, false, &[]).unwrap();
        assert!(forever.pat.expires_at.is_none());
        assert!(user_for(&c, &forever.token).unwrap().is_some());
        c.execute("UPDATE ledger_account SET disabled = true WHERE id = 'a1'", []).unwrap();
        assert!(user_for(&c, &forever.token).unwrap().is_none(), "账号停用后令牌失效");
        c.execute("UPDATE ledger_account SET disabled = false WHERE id = 'a1'", []).unwrap();

        revoke(&c, "a1", &forever.pat.id).unwrap();
        assert!(user_for(&c, &forever.token).unwrap().is_none(), "吊销后失效");
        assert!(revoke(&c, "a1", &forever.pat.id).is_err());
        // 不能吊销别人的
        let other = create(&c, "a1", "x", None, false, &[]).unwrap();
        assert!(revoke(&c, "someone-else", &other.pat.id).is_err());
    }

    #[test]
    fn read_only_tokens_are_marked_by_their_prefix() {
        let c = db();
        let ro = create(&c, "a1", "report", None, true, &[]).unwrap();
        assert!(ro.token.starts_with("glr_") && ro.pat.read_only && ro.pat.prefix.starts_with("glr_"));
        assert!(is_pat(&ro.token) && is_read_only(&ro.token));
        // 认证照常，权限由中间件按前缀限制
        assert!(user_for(&c, &ro.token).unwrap().is_some());
        // 把前缀改成 glp_ 冒充完全权限令牌：摘要对不上
        let forged = format!("glp_{}", &ro.token[4..]);
        assert!(is_pat(&forged) && !is_read_only(&forged));
        assert!(user_for(&c, &forged).unwrap().is_none());
        let full = create(&c, "a1", "full", None, false, &[]).unwrap();
        assert!(!full.pat.read_only && !is_read_only(&full.token));
        assert!(list(&c, "a1").unwrap().iter().any(|p| p.read_only));
    }

    #[test]
    fn validation_and_limits() {
        let c = db();
        assert!(create(&c, "a1", "  ", None, false, &[]).is_err());
        assert!(create(&c, "a1", &"x".repeat(41), None, false, &[]).is_err());
        assert!(create(&c, "a1", "ok", Some(0), false, &[]).is_err());
        assert!(create(&c, "a1", "ok", Some(MAX_DAYS + 1), false, &[]).is_err());
        for i in 0..MAX_PER_ACCOUNT {
            create(&c, "a1", &format!("t{i}"), None, false, &[]).unwrap();
        }
        assert!(create(&c, "a1", "one too many", None, false, &[]).is_err());
    }

    #[test]
    fn last_used_is_recorded_without_writing_on_every_call() {
        let c = db();
        let made = create(&c, "a1", "t", Some(30), false, &[]).unwrap();
        assert!(list(&c, "a1").unwrap()[0].last_used_at.is_none());
        user_for(&c, &made.token).unwrap();
        let first = list(&c, "a1").unwrap()[0].last_used_at.clone();
        assert!(first.is_some());
        user_for(&c, &made.token).unwrap();
        assert_eq!(list(&c, "a1").unwrap()[0].last_used_at, first, "一分钟内不重复写");
    }

    #[test]
    fn dataset_limited_tokens_validate_and_resolve_ids_and_names() {
        let c = db();
        let orders = crate::dataset_import::create_empty(&c, "订单", &[("店铺".into(), "text".into())], &[]).unwrap();
        let costs = crate::dataset_import::create_empty(&c, "成本", &[("店铺".into(), "text".into())], &[]).unwrap();
        assert!(create(&c, "a1", "bad", None, false, &["ds_nope".to_string()]).is_err(), "数据集必须存在");
        assert!(create(&c, "a1", "many", None, false, &(0..=MAX_DATASETS).map(|i| format!("ds_{i}")).collect::<Vec<_>>()).is_err(), "数量上限");

        let made = create(&c, "a1", "只写订单", None, false, &[orders.clone(), orders.clone(), " ".into()]).unwrap();
        assert_eq!(made.pat.datasets.len(), 1, "去重、忽略空白");
        assert_eq!((made.pat.datasets[0].id.as_str(), made.pat.datasets[0].name.as_deref()), (orders.as_str(), Some("订单")));
        let (_, limit) = user_for(&c, &made.token).unwrap().expect("应认得这个令牌");
        let limit = limit.expect("限定了数据集");
        assert!(limit.contains(&orders) && limit.contains("订单"), "id 和名称都算（公式下推按名称请求）：{limit:?}");
        assert!(!limit.contains(&costs) && !limit.contains("成本"));

        // 不限定的令牌没有限制；列表里限定的和不限定的并存
        let free = create(&c, "a1", "全部", None, false, &[]).unwrap();
        assert!(user_for(&c, &free.token).unwrap().unwrap().1.is_none());
        let listed = list(&c, "a1").unwrap();
        assert_eq!(listed.iter().filter(|p| !p.datasets.is_empty()).count(), 1);

        // 数据集改名后，列表和限定范围都跟着当前名称走
        c.execute("UPDATE ledger_dataset SET name = '销售订单' WHERE id = ?", params![orders]).unwrap();
        assert_eq!(list(&c, "a1").unwrap().iter().find(|p| !p.datasets.is_empty()).unwrap().datasets[0].name.as_deref(), Some("销售订单"));
        let limit = user_for(&c, &made.token).unwrap().unwrap().1.unwrap();
        assert!(limit.contains("销售订单") && !limit.contains("订单"));
    }
}
