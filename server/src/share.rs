//! 工作簿的只读分享链接：把一个工作簿（含它引用的数据集）开放给没有账号的人只读查看
//!
//! - 链接里带一个令牌 `gls_` + 32 字节随机数（43 个字符）。库里只存 SHA-256 摘要，创建时返回一次，之后看不到；
//!   可以设有效期，随时吊销；工作簿进回收站、被彻底删除、创建人账号被停用后链接立即失效。
//! - **只读，且只能读这个工作簿**：带 `x-share-token` 的请求不走普通的登录与权限表，只能调用 [`ALLOWED`] 里登记的接口——
//!   工作簿的内容 / 元信息、以及这个工作簿用到的数据集的只读查询（游标、取行、统计、取值、聚合、透视）。
//!   不能导出、不能写入、不能 SQL、不能 AI、不能评论、不能进协作。
//! - **只能读这个工作簿用到的数据集**：每次打开分享页（读取工作簿内容）时，按当前内容重新计算引用了哪些数据集（图表、透视、
//!   指标卡、数据表、公式里的数据集名）并记下；分享链接访问其他数据集一律拒绝。
//! - **数据范围跟随创建人**：分享出去的数据不会比创建人自己能看到的更多——请求在创建人的数据范围内执行
//!   （创建人被限制到某些店铺 / 部门，访客看到的也只有这些）。数据范围受限的用户不能创建分享链接。
//! - 只在开启了登录（`LEDGER_AUTH_MODE` 不是 off）时有意义：没开登录时所有人本来就能打开。

use std::collections::BTreeSet;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, Method};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use duckdb::{Connection, OptionalExt, params};
use serde::{Deserialize, Serialize};

use crate::account::{expires_after, random_bytes, token_hash};
use crate::auth::{CurrentUser, Need, run};
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::{AppState, parse_json};

pub const PREFIX: &str = "gls_";
pub const HEADER: &str = "x-share-token";
const MAX_PER_WORKBOOK: i64 = 20;
const MAX_DAYS: i64 = 3650;

/// 分享链接可以调用的接口（方法，路径模板）。**白名单**：不在表里的一律拒绝
pub const ALLOWED: &[(&str, &str)] = &[
    ("GET", "/ledger/me"),
    ("GET", "/ledger/workbooks/{id}"),
    ("GET", "/ledger/workbooks/{id}/content"),
    ("GET", "/ledger/datasets"),
    ("POST", "/ledger/datasets/{id}/cursor"),
    ("GET", "/ledger/datasets/{id}/rows"),
    ("POST", "/ledger/datasets/{id}/stats"),
    ("POST", "/ledger/datasets/{id}/distinct"),
    ("POST", "/ledger/datasets/{id}/aggregate"),
    ("POST", "/ledger/datasets/{id}/pivot"),
];

/// 一条有效的分享链接
#[derive(Clone, Debug)]
pub struct ShareInfo {
    pub id: String,
    pub workbook_id: String,
    pub workbook_name: String,
    /// 创建人（按他的数据范围执行请求）
    pub creator: CurrentUser,
    pub datasets: BTreeSet<String>,
    pub content_version: i64,
}

impl ShareInfo {
    /// 请求里的身份：不是任何一个账号，只用来在审计、处理函数里区分
    pub fn principal(&self) -> CurrentUser {
        CurrentUser { id: format!("share:{}", self.id), name: "分享链接访客".into(), admin: false, enforced: true }
    }

    pub fn may_read_dataset(&self, id: &str) -> bool {
        self.datasets.contains(id)
    }
}

pub fn is_share_token(token: &str) -> bool {
    token.starts_with(PREFIX)
}

pub fn token_of(headers: &HeaderMap) -> Option<String> {
    headers.get(HEADER).and_then(|v| v.to_str().ok()).map(str::trim).filter(|t| !t.is_empty()).map(String::from)
}

/// 方法 + 路径是不是白名单里的；返回匹配的路径模板与路径参数里的 id
pub fn allowed(method: &Method, path: &str) -> Option<(&'static str, Option<String>)> {
    let m = if method == Method::HEAD { "GET" } else { method.as_str() };
    ALLOWED.iter().filter(|(rm, _)| *rm == m).find_map(|(_, t)| crate::auth::match_path(t, path).map(|p| (*t, p.into_iter().find(|(k, _)| k == "id").map(|(_, v)| v))))
}

// ── 存取 ──────────────────────────────────────────────

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub id: String,
    pub workbook_id: String,
    pub note: String,
    /// 令牌开头的几位，用来在列表里认出是哪一个（不足以还原令牌）
    pub prefix: String,
    pub created_by_name: String,
    pub created_at: String,
    /// 空表示永不过期
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub view_count: i64,
    pub expired: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    #[serde(flatten)]
    pub share: Share,
    /// 完整令牌：只在创建时返回这一次
    pub token: String,
}

const COLS: &str = "id, workbook_id, COALESCE(note, ''), prefix, COALESCE(created_by_name, ''), strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(expires_at, '%Y-%m-%d %H:%M:%S'), \
                    strftime(last_used_at, '%Y-%m-%d %H:%M:%S'), COALESCE(view_count, 0), (expires_at IS NOT NULL AND expires_at <= CAST(? AS TIMESTAMP))";

fn read(r: &duckdb::Row) -> duckdb::Result<Share> {
    Ok(Share {
        id: r.get(0)?,
        workbook_id: r.get(1)?,
        note: r.get(2)?,
        prefix: r.get(3)?,
        created_by_name: r.get(4)?,
        created_at: r.get(5)?,
        expires_at: r.get(6)?,
        last_used_at: r.get(7)?,
        view_count: r.get(8)?,
        expired: r.get(9)?,
    })
}

pub fn list(conn: &Connection, workbook_id: &str) -> AppResult<Vec<Share>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM ledger_share WHERE workbook_id = ? ORDER BY created_at DESC, id"))?;
    Ok(stmt.query_map(params![now_local(), workbook_id], read)?.collect::<Result<Vec<_>, _>>()?)
}

/// 新建分享链接；`days` 为 None 表示永不过期
pub fn create(conn: &Connection, user: &CurrentUser, workbook_id: &str, note: &str, days: Option<i64>) -> AppResult<Created> {
    let note = note.trim();
    if note.chars().count() > 60 {
        return Err(AppError::bad("备注最多 60 个字"));
    }
    if let Some(d) = days
        && !(1..=MAX_DAYS).contains(&d)
    {
        return Err(AppError::bad(format!("有效期应在 1 到 {MAX_DAYS} 天之间")));
    }
    let existing: i64 = conn.query_row("SELECT count(*) FROM ledger_share WHERE workbook_id = ?", params![workbook_id], |r| r.get(0))?;
    if existing >= MAX_PER_WORKBOOK {
        return Err(AppError::bad(format!("一个工作簿最多 {MAX_PER_WORKBOOK} 个分享链接，请先吊销不用的")));
    }
    let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(random_bytes::<32>()));
    let id = format!("sh_{}", URL_SAFE_NO_PAD.encode(random_bytes::<9>()));
    conn.execute(
        "INSERT INTO ledger_share (id, workbook_id, note, token_hash, prefix, created_by, created_by_name, creator_admin, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![id, workbook_id, note, token_hash(&token), &token[..PREFIX.len() + 4], user.id, user.name, user.admin, now_local(), days.map(expires_after)],
    )?;
    let share = conn.query_row(&format!("SELECT {COLS} FROM ledger_share WHERE id = ?"), params![now_local(), id], read)?;
    Ok(Created { share, token })
}

/// 吊销；返回它属于哪个工作簿（调用方据此检查权限）
pub fn workbook_of(conn: &Connection, id: &str) -> AppResult<String> {
    conn.query_row("SELECT workbook_id FROM ledger_share WHERE id = ?", params![id], |r| r.get(0)).optional()?.ok_or_else(|| AppError::bad("分享链接不存在"))
}

pub fn revoke(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_share WHERE id = ?", params![id])?;
    Ok(())
}

/// 工作簿被彻底删除时一并清掉
pub fn purge_workbook(conn: &Connection, workbook_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_share WHERE workbook_id = ?", params![workbook_id])?;
    Ok(())
}

/// 令牌对应的有效分享链接：不存在、过期、工作簿在回收站或不存在、创建人账号被停用，都是 None。
/// 创建人的管理员身份以账号表为准（内置账号登录），其他登录方式用创建时记下的
pub fn resolve(conn: &Connection, token: &str) -> AppResult<Option<ShareInfo>> {
    type Row = (String, String, String, Option<String>, Option<String>, bool, String, i64);
    let found: Option<Row> = conn
        .query_row(
            "SELECT s.id, s.workbook_id, w.name, s.created_by, s.created_by_name, COALESCE(s.creator_admin, false), COALESCE(s.datasets, '[]'), COALESCE(s.content_version, 0)
             FROM ledger_share s JOIN ledger_workbook w ON w.id = s.workbook_id
             WHERE s.token_hash = ? AND (s.expires_at IS NULL OR s.expires_at > CAST(? AS TIMESTAMP)) AND w.deleted_at IS NULL",
            params![token_hash(token), now_local()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .optional()?;
    let Some((id, workbook_id, workbook_name, by, by_name, mut admin, datasets, content_version)) = found else { return Ok(None) };
    let by = by.unwrap_or_default();
    // 内置账号：账号停用 / 降权后立即反映
    let account: Option<(bool, bool)> = conn.query_row("SELECT admin, disabled FROM ledger_account WHERE id = ?", params![by], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    if let Some((a, disabled)) = account {
        if disabled {
            return Ok(None);
        }
        admin = a;
    }
    let datasets: BTreeSet<String> = serde_json::from_str::<Vec<String>>(&datasets).unwrap_or_default().into_iter().collect();
    Ok(Some(ShareInfo {
        id,
        workbook_id,
        workbook_name,
        creator: CurrentUser { id: by, name: by_name.unwrap_or_default(), admin, enforced: true },
        datasets,
        content_version,
    }))
}

/// 工作簿内容里用到的数据集（部件、透视、填报区里的 datasetId，以及公式里写的数据集名）：返回它们的 id 与名称
pub fn referenced_datasets(conn: &Connection, workbook_json: &serde_json::Value) -> AppResult<BTreeSet<String>> {
    let collected = crate::template::refs::collect(workbook_json);
    let catalog = crate::dataset::datasets(conn)?;
    let mut out = BTreeSet::new();
    for usage in collected.by_key.into_values() {
        out.extend(usage.ids.into_iter().filter(|id| catalog.iter().any(|d| d.id == *id)));
        for name in &usage.names {
            out.extend(catalog.iter().filter(|d| d.name == *name).map(|d| d.id.clone()));
        }
    }
    // 公式下推按数据集名称请求（路径里是名称而不是 id）：被允许的数据集的名称也算
    let names: Vec<String> = catalog.iter().filter(|d| out.contains(&d.id)).map(|d| d.name.clone()).collect();
    out.extend(names);
    Ok(out)
}

/// 打开分享页：返回工作簿内容，并在工作簿有新版本时重新计算它用到的数据集；顺便记一次访问
pub fn open(conn: &Connection, share: &ShareInfo) -> AppResult<(crate::workbook::WorkbookMeta, Vec<u8>, ShareInfo)> {
    let (meta, gz) = crate::workbook::content(conn, &share.workbook_id)?;
    let mut share = share.clone();
    if meta.version != share.content_version || share.datasets.is_empty() {
        let json = crate::workbook::gunzip_json(&gz)?;
        let value: serde_json::Value = serde_json::from_slice(&json).map_err(|e| AppError::bad(format!("工作簿内容无法解析：{e}")))?;
        share.datasets = referenced_datasets(conn, &value)?;
        share.content_version = meta.version;
        conn.execute(
            "UPDATE ledger_share SET datasets = ?, content_version = ? WHERE id = ?",
            params![serde_json::to_string(&share.datasets.iter().collect::<Vec<_>>())?, meta.version, share.id],
        )?;
    }
    conn.execute("UPDATE ledger_share SET view_count = COALESCE(view_count, 0) + 1, last_used_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now_local(), share.id])?;
    Ok((meta, gz, share))
}

// ── 接口（需要登录）─────────────────────────────────────

fn require_auth(db: &AppState) -> AppResult<()> {
    if !db.auth.enabled() {
        return Err(AppError::bad("没有开启登录（LEDGER_AUTH_MODE=off）：所有能访问页面的人本来就可以打开，不需要分享链接"));
    }
    Ok(())
}

/// GET /api/ledger/workbooks/{id}/shares  这个工作簿的分享链接（所有者与管理员）
pub async fn list_route(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Vec<Share>>>> {
    require_auth(&db)?;
    Ok(ok(run(&db, move |c| list(c, &id)).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewShare {
    #[serde(default)]
    pub note: String,
    /// 有效期天数；不传用 30 天，传 null 或 0 表示永不过期
    #[serde(default = "default_days")]
    pub days: Option<i64>,
}

fn default_days() -> Option<i64> {
    Some(30)
}

/// POST /api/ledger/workbooks/{id}/shares  { note?, days? }  新建分享链接；响应里的 token 只显示这一次
pub async fn create_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: Bytes) -> AppResult<Json<Envelope<Created>>> {
    require_auth(&db)?;
    let b: NewShare = parse_json(&body)?;
    let days = b.days.filter(|d| *d != 0);
    Ok(ok(run(&db, move |c| create(c, &user, &id, &b.note, days)).await?))
}

/// DELETE /api/ledger/shares/{id}  吊销分享链接（立即生效；工作簿的所有者或管理员）
pub async fn revoke_route(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    require_auth(&db)?;
    run(&db, move |c| {
        let wb = workbook_of(c, &id)?;
        crate::workbook::require(c, &wb, &user, Need::Manage, true)?;
        revoke(c, &id)
    })
    .await?;
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
        c.execute("INSERT INTO ledger_workbook (id, name, version, size, stored_size, data, created_at, updated_at) VALUES ('wb1', '看板', 1, 0, 0, ''::BLOB, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))", params![now_local(), now_local()]).unwrap();
        c
    }

    fn alice() -> CurrentUser {
        CurrentUser { id: "a1".into(), name: "Alice".into(), admin: false, enforced: true }
    }

    #[test]
    fn token_is_only_stored_as_a_hash_and_resolves() {
        let c = db();
        let made = create(&c, &alice(), "wb1", "给客户", Some(7)).unwrap();
        assert!(made.token.starts_with(PREFIX) && made.token.len() >= 40, "{}", made.token);
        assert_eq!(made.share.prefix, &made.token[..8]);
        let s = resolve(&c, &made.token).unwrap().expect("应认得这个令牌");
        assert_eq!((s.workbook_id.as_str(), s.workbook_name.as_str(), s.creator.id.as_str()), ("wb1", "看板", "a1"));
        assert!(s.datasets.is_empty() && !s.creator.admin);
        let leaked: i64 = c.query_row("SELECT count(*) FROM ledger_share WHERE token_hash = ? OR prefix = ? OR note = ?", params![made.token, made.token, made.token], |r| r.get(0)).unwrap();
        assert_eq!(leaked, 0);
        assert!(resolve(&c, "gls_wrong").unwrap().is_none());
        // 摘要不能当令牌用
        let stored: String = c.query_row("SELECT token_hash FROM ledger_share", [], |r| r.get(0)).unwrap();
        assert!(resolve(&c, &stored).unwrap().is_none());
    }

    #[test]
    fn expiry_revocation_trash_and_disabled_creator() {
        let c = db();
        let made = create(&c, &alice(), "wb1", "", Some(1)).unwrap();
        c.execute("UPDATE ledger_share SET expires_at = CAST('2000-01-01 00:00:00' AS TIMESTAMP)", []).unwrap();
        assert!(resolve(&c, &made.token).unwrap().is_none(), "过期");
        assert!(list(&c, "wb1").unwrap()[0].expired);

        let forever = create(&c, &alice(), "wb1", "", None).unwrap();
        assert!(forever.share.expires_at.is_none());
        assert!(resolve(&c, &forever.token).unwrap().is_some());
        // 工作簿进回收站：失效；恢复后又有效
        c.execute("UPDATE ledger_workbook SET deleted_at = CAST(? AS TIMESTAMP) WHERE id = 'wb1'", params![now_local()]).unwrap();
        assert!(resolve(&c, &forever.token).unwrap().is_none());
        c.execute("UPDATE ledger_workbook SET deleted_at = NULL WHERE id = 'wb1'", []).unwrap();
        // 创建人账号停用：失效
        c.execute("UPDATE ledger_account SET disabled = true WHERE id = 'a1'", []).unwrap();
        assert!(resolve(&c, &forever.token).unwrap().is_none());
        c.execute("UPDATE ledger_account SET disabled = false WHERE id = 'a1'", []).unwrap();
        // 创建人的管理员身份以账号表为准
        c.execute("UPDATE ledger_account SET admin = true WHERE id = 'a1'", []).unwrap();
        assert!(resolve(&c, &forever.token).unwrap().unwrap().creator.admin);

        revoke(&c, &forever.share.id).unwrap();
        assert!(resolve(&c, &forever.token).unwrap().is_none(), "吊销后失效");
        assert!(workbook_of(&c, &forever.share.id).is_err());
    }

    #[test]
    fn limits_and_validation() {
        let c = db();
        assert!(create(&c, &alice(), "wb1", &"x".repeat(61), None).is_err());
        assert!(create(&c, &alice(), "wb1", "", Some(0)).is_err());
        assert!(create(&c, &alice(), "wb1", "", Some(MAX_DAYS + 1)).is_err());
        for _ in 0..MAX_PER_WORKBOOK {
            create(&c, &alice(), "wb1", "", None).unwrap();
        }
        assert!(create(&c, &alice(), "wb1", "", None).is_err(), "数量上限");
        purge_workbook(&c, "wb1").unwrap();
        assert!(list(&c, "wb1").unwrap().is_empty());
    }

    #[test]
    fn allowlist_is_read_only_and_extracts_the_dataset_id() {
        let get = Method::GET;
        let post = Method::POST;
        assert_eq!(allowed(&get, "/ledger/workbooks/wb1/content"), Some(("/ledger/workbooks/{id}/content", Some("wb1".into()))));
        assert_eq!(allowed(&post, "/ledger/datasets/ds_1/pivot"), Some(("/ledger/datasets/{id}/pivot", Some("ds_1".into()))));
        assert_eq!(allowed(&get, "/ledger/me"), Some(("/ledger/me", None)));
        assert_eq!(allowed(&Method::HEAD, "/ledger/datasets"), Some(("/ledger/datasets", None)));
        // 写、导出、SQL、AI、评论、协作、别的工作簿的接口：都不在白名单里
        for (m, p) in [
            (Method::PUT, "/ledger/workbooks/wb1/content"),
            (Method::DELETE, "/ledger/workbooks/wb1"),
            (Method::POST, "/ledger/datasets/ds_1/writeback"),
            (Method::GET, "/ledger/datasets/ds_1/export"),
            (Method::POST, "/ledger/sql/run"),
            (Method::POST, "/ledger/ai/threads"),
            (Method::GET, "/ledger/workbooks/wb1/comments"),
            (Method::GET, "/ledger/collab/wb1/events"),
            (Method::GET, "/ledger/workbooks"),
            (Method::GET, "/ledger/workbooks/wb1/versions"),
            (Method::GET, "/ledger/workbooks/wb1/shares"),
            (Method::POST, "/ledger/workbooks/ranges"),
            (Method::GET, "/ledger/audit"),
        ] {
            assert!(allowed(&m, p).is_none(), "{m} {p} 不应对分享链接开放");
        }
    }
}
