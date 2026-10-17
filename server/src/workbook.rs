//! 工作簿保存到服务端：登记、gzip 内容、版本历史、乐观并发
//!
//! - 内容以 gzip 压缩的 Univer 工作簿 JSON 存储（压缩率约 10 倍）；读取时原样返回压缩字节并声明
//!   Content-Encoding: gzip，由浏览器解压，服务端不做重复压缩
//! - 每次保存写入一个版本：手动保存与带说明的版本永久保留，自动保存只保留最近 AUTO_KEEP 个
//! - 乐观并发：保存时带上「基于的版本号」，期间别人保存过则返回冲突（不覆盖），由用户选择
//!   加载对方版本 / 覆盖 / 另存为副本 —— 多人协作的基础
//! - 组织：文件夹（可嵌套）、星标；删除先进回收站（deleted_at），可恢复，超过保留天数后自动彻底删除。
//!   回收站里的工作簿对其他功能（打开、保存、推送、模板、MCP）都视为不存在

use std::io::{Read, Write};

use duckdb::{Connection, OptionalExt, params};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};

use crate::db::now_local;
use crate::error::{AppError, AppResult};

/// 自动保存版本保留个数（手动 / 命名版本不受限）
const AUTO_KEEP: i64 = 30;
/// 解压后的工作簿 JSON 上限
const MAX_JSON_BYTES: usize = 512 * 1024 * 1024;
/// 文件夹最多嵌套层数
const MAX_FOLDER_DEPTH: usize = 5;

/// 回收站保留天数（LEDGER_TRASH_DAYS，默认 30；0 表示不自动清理）
pub fn trash_days() -> i64 {
    std::env::var("LEDGER_TRASH_DAYS").ok().and_then(|v| v.trim().parse().ok()).filter(|d: &i64| *d >= 0).unwrap_or(30)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbookMeta {
    pub id: String,
    pub name: String,
    pub version: i64,
    /// 解压后的 JSON 字节数
    pub size: i64,
    /// 压缩后存储字节数
    pub stored_size: i64,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_by: Option<String>,
    pub updated_at: String,
    /// 所在文件夹（空为根目录）
    pub folder_id: Option<String>,
    pub starred: bool,
    /// 所有者（登录体系启用前创建的没有所有者，只有管理员能管理）
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
    /// 共享范围：private 仅自己 / view 所有人可看 / edit 所有人可编辑
    pub visibility: String,
}

const META_COLS: &str = "id, name, version, size, stored_size, created_by, strftime(created_at, '%Y-%m-%d %H:%M:%S'), updated_by, strftime(updated_at, '%Y-%m-%d %H:%M:%S'), folder_id, COALESCE(starred, false), owner_id, owner_name, COALESCE(visibility, 'edit')";
/// 不在回收站
const ALIVE: &str = "deleted_at IS NULL";

fn read_meta(r: &duckdb::Row) -> duckdb::Result<WorkbookMeta> {
    Ok(WorkbookMeta {
        id: r.get(0)?,
        name: r.get(1)?,
        version: r.get(2)?,
        size: r.get(3)?,
        stored_size: r.get(4)?,
        created_by: r.get(5)?,
        created_at: r.get(6)?,
        updated_by: r.get(7)?,
        updated_at: r.get(8)?,
        folder_id: r.get(9)?,
        starred: r.get(10)?,
        owner_id: r.get(11)?,
        owner_name: r.get(12)?,
        visibility: r.get(13)?,
    })
}

// ── 所有者与共享范围 ──────────────────────────────────

use crate::auth::{CurrentUser, Need, level};

/// 检查用户对工作簿的权限，返回工作簿名称（审计用）。trashed_ok：回收站里的也算（恢复 / 彻底删除）
pub fn require(conn: &Connection, id: &str, user: &CurrentUser, need: Need, trashed_ok: bool) -> AppResult<String> {
    let row: Option<(String, Option<String>, Option<String>, String, bool)> = conn
        .query_row(
            "SELECT name, owner_id, owner_name, COALESCE(visibility, 'edit'), deleted_at IS NOT NULL FROM ledger_workbook WHERE id = ?",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((name, owner_id, owner_name, visibility, trashed)) = row else {
        return Err(AppError::bad("工作簿不存在或已删除"));
    };
    if trashed && !trashed_ok {
        return Err(missing(conn, id));
    }
    let have = level(user, owner_id.as_deref(), if trashed { "private" } else { &visibility });
    if have >= need as u8 {
        return Ok(name);
    }
    // 被指定为这个工作簿审批人的用户，可以查看（含审批时的版本）
    if need == Need::View && !trashed && crate::approval::grants_view(conn, id, &user.id) {
        return Ok(name);
    }
    let owner = owner_name.filter(|n| !n.is_empty()).map(|n| format!("所有者：{n}")).unwrap_or_else(|| "没有所有者，仅管理员可管理".into());
    Err(AppError::Forbidden(match (have, need) {
        (0, _) => format!("无权访问工作簿「{name}」（{owner}）"),
        (_, Need::Manage) => format!("只有所有者或管理员可以执行这个操作（工作簿「{name}」，{owner}）"),
        _ => format!("工作簿「{name}」对你是只读的，需要所有者开放编辑（{owner}）"),
    }))
}

/// 登录用户创建的工作簿归自己，默认仅自己可见（未启用认证时不改动）
pub fn set_owner(conn: &Connection, id: &str, user: &CurrentUser) -> AppResult<()> {
    if user.enforced {
        conn.execute("UPDATE ledger_workbook SET owner_id = ?, owner_name = ?, visibility = 'private' WHERE id = ?", params![user.id, user.name, id])?;
    }
    Ok(())
}

pub fn set_visibility(conn: &Connection, id: &str, visibility: &str) -> AppResult<()> {
    if !matches!(visibility, "private" | "view" | "edit") {
        return Err(AppError::bad("共享范围应为 private / view / edit"));
    }
    conn.execute(&format!("UPDATE ledger_workbook SET visibility = ? WHERE id = ? AND {ALIVE}"), params![visibility, id])?;
    Ok(())
}

/// 用户能看到的工作簿
pub fn list_for(conn: &Connection, user: &CurrentUser) -> AppResult<Vec<WorkbookMeta>> {
    Ok(list(conn)?.into_iter().filter(|m| level(user, m.owner_id.as_deref(), &m.visibility) >= Need::View as u8).collect())
}

pub fn list(conn: &Connection) -> AppResult<Vec<WorkbookMeta>> {
    let mut stmt = conn.prepare(&format!("SELECT {META_COLS} FROM ledger_workbook WHERE {ALIVE} ORDER BY updated_at DESC"))?;
    let rows = stmt.query_map([], read_meta)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn meta(conn: &Connection, id: &str) -> AppResult<WorkbookMeta> {
    conn.query_row(&format!("SELECT {META_COLS} FROM ledger_workbook WHERE id = ? AND {ALIVE}"), params![id], read_meta)
        .optional()?
        .ok_or_else(|| missing(conn, id))
}

/// 找不到（未在回收站之外）时的错误：区分「在回收站」与「不存在」
fn missing(conn: &Connection, id: &str) -> AppError {
    let trashed = conn
        .query_row("SELECT count(*) FROM ledger_workbook WHERE id = ? AND deleted_at IS NOT NULL", params![id], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        > 0;
    AppError::bad(if trashed { "工作簿已移到回收站，可在「工作簿 → 回收站」中恢复" } else { "工作簿不存在或已删除" })
}

/// 工作簿存在且不在回收站（定时推送等据此跳过）
pub fn is_alive(conn: &Connection, id: &str) -> bool {
    conn.query_row(&format!("SELECT count(*) FROM ledger_workbook WHERE id = ? AND {ALIVE}"), params![id], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
}

/// 解压工作簿内容（超过上限按损坏处理）
pub fn gunzip_json(gz: &[u8]) -> AppResult<Vec<u8>> {
    let mut json = Vec::new();
    GzDecoder::new(gz).take(MAX_JSON_BYTES as u64 + 1).read_to_end(&mut json).map_err(|_| AppError::bad("工作簿内容已损坏"))?;
    if json.len() > MAX_JSON_BYTES {
        return Err(AppError::bad("工作簿内容过大"));
    }
    Ok(json)
}

/// 校验上传内容：gzip → 合法 JSON 对象（含 sheets）；返回 (压缩字节, 原始大小)
fn validate(gz: &[u8]) -> AppResult<(Vec<u8>, i64)> {
    let mut json = Vec::new();
    GzDecoder::new(gz)
        .take(MAX_JSON_BYTES as u64 + 1)
        .read_to_end(&mut json)
        .map_err(|_| AppError::bad("工作簿内容不是有效的 gzip 数据"))?;
    if json.len() > MAX_JSON_BYTES {
        return Err(AppError::bad("工作簿超过 512 MB，请把明细数据导入为数据集后再保存"));
    }
    let v: serde_json::Value = serde_json::from_slice(&json).map_err(|e| AppError::bad(format!("工作簿内容不是有效的 JSON：{e}")))?;
    if !v.get("sheets").is_some_and(|s| s.is_object()) {
        return Err(AppError::bad("工作簿内容缺少 sheets"));
    }
    Ok((gz.to_vec(), json.len() as i64))
}

fn new_id() -> String {
    format!("wb_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"))
}

fn clean_name(name: &str) -> AppResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 80 {
        return Err(AppError::bad("工作簿名称不能为空，且不超过 80 个字符"));
    }
    Ok(n.to_string())
}

fn write_version(conn: &Connection, id: &str, version: i64, gz: &[u8], size: i64, operator: &str, description: &str, is_auto: bool) -> AppResult<()> {
    conn.execute(
        "INSERT INTO ledger_workbook_version (workbook_id, version, data, size, saved_by, saved_at, description, is_auto)
         VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?, ?)",
        params![id, version, gz, size, operator, now_local(), description, is_auto],
    )?;
    // 自动保存版本只保留最近 AUTO_KEEP 个
    conn.execute(
        "DELETE FROM ledger_workbook_version WHERE workbook_id = ? AND is_auto AND version NOT IN (
           SELECT version FROM ledger_workbook_version WHERE workbook_id = ? AND is_auto ORDER BY version DESC LIMIT ?)",
        params![id, id, AUTO_KEEP],
    )?;
    Ok(())
}

fn in_tx<T>(conn: &Connection, f: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
    conn.execute_batch("BEGIN TRANSACTION")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 新建工作簿（内容为 gzip 压缩的 JSON）
pub fn create(conn: &Connection, name: &str, gz: &[u8], operator: &str) -> AppResult<WorkbookMeta> {
    create_in(conn, name, gz, operator, None)
}

/// 新建到指定文件夹
pub fn create_in(conn: &Connection, name: &str, gz: &[u8], operator: &str, folder_id: Option<&str>) -> AppResult<WorkbookMeta> {
    if let Some(f) = folder_id {
        folder_exists(conn, f)?;
    }
    let name = clean_name(name)?;
    let (gz, size) = validate(gz)?;
    let id = new_id();
    in_tx(conn, || {
        let now = now_local();
        conn.execute(
            "INSERT INTO ledger_workbook (id, name, version, size, stored_size, data, created_by, created_at, updated_by, updated_at, folder_id, starred)
             VALUES (?, ?, 1, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?, CAST(? AS TIMESTAMP), ?, false)",
            params![id, name, size, gz.len() as i64, gz, operator, now, operator, now, folder_id],
        )?;
        write_version(conn, &id, 1, &gz, size, operator, "创建", false)
    })?;
    meta(conn, &id)
}

/// 读取内容（gzip 字节）与元数据
pub fn content(conn: &Connection, id: &str) -> AppResult<(WorkbookMeta, Vec<u8>)> {
    let m = meta(conn, id)?;
    let data: Vec<u8> = conn.query_row("SELECT data FROM ledger_workbook WHERE id = ?", params![id], |r| r.get(0))?;
    Ok((m, data))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub saved: bool,
    /// 保存后的版本号（冲突时为服务端当前版本）
    pub version: i64,
    /// 冲突：基于的版本已被别人更新
    pub conflict: Option<WorkbookMeta>,
    pub meta: Option<WorkbookMeta>,
}

pub struct SaveRequest<'a> {
    pub base_version: i64,
    pub force: bool,
    pub operator: &'a str,
    pub description: &'a str,
    pub is_auto: bool,
}

/// 保存：base_version 与服务端当前版本不一致时返回冲突（force 时直接覆盖，旧内容仍在版本历史里）
pub fn save(conn: &Connection, id: &str, gz: &[u8], req: &SaveRequest) -> AppResult<SaveResult> {
    let (gz, size) = validate(gz)?;
    in_tx(conn, || {
        let current: i64 = conn
            .query_row(&format!("SELECT version FROM ledger_workbook WHERE id = ? AND {ALIVE}"), params![id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| missing(conn, id))?;
        if current != req.base_version && !req.force {
            return Ok(SaveResult { saved: false, version: current, conflict: Some(meta(conn, id)?), meta: None });
        }
        let next = current + 1;
        conn.execute(
            "UPDATE ledger_workbook SET version = ?, size = ?, stored_size = ?, data = ?, updated_by = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params![next, size, gz.len() as i64, gz, req.operator, now_local(), id],
        )?;
        let desc = if req.description.trim().is_empty() {
            if req.is_auto { "自动保存" } else { "手动保存" }
        } else {
            req.description.trim()
        };
        write_version(conn, id, next, &gz, size, req.operator, desc, req.is_auto)?;
        Ok(SaveResult { saved: true, version: next, conflict: None, meta: Some(meta(conn, id)?) })
    })
}

pub fn rename(conn: &Connection, id: &str, name: &str) -> AppResult<WorkbookMeta> {
    let name = clean_name(name)?;
    if conn.execute(&format!("UPDATE ledger_workbook SET name = ? WHERE id = ? AND {ALIVE}"), params![name, id])? == 0 {
        return Err(missing(conn, id));
    }
    meta(conn, id)
}

/// 复制为新工作簿（放在原工作簿所在的文件夹）
pub fn duplicate(conn: &Connection, id: &str, name: &str, operator: &str) -> AppResult<WorkbookMeta> {
    let (src, gz) = content(conn, id)?;
    let m = create(conn, name, &gz, operator)?;
    if src.folder_id.is_some() {
        conn.execute("UPDATE ledger_workbook SET folder_id = ? WHERE id = ?", params![src.folder_id, m.id])?;
    }
    meta(conn, &m.id)
}

// ── 星标、移动 ────────────────────────────────────────

pub fn set_starred(conn: &Connection, id: &str, starred: bool) -> AppResult<()> {
    if conn.execute(&format!("UPDATE ledger_workbook SET starred = ? WHERE id = ? AND {ALIVE}"), params![starred, id])? == 0 {
        return Err(missing(conn, id));
    }
    Ok(())
}

/// 把若干工作簿移到文件夹（None 为根目录）
pub fn move_to(conn: &Connection, ids: &[String], folder_id: Option<&str>) -> AppResult<usize> {
    if let Some(f) = folder_id {
        folder_exists(conn, f)?;
    }
    in_tx(conn, || {
        let mut n = 0;
        for id in ids {
            n += conn.execute(&format!("UPDATE ledger_workbook SET folder_id = ? WHERE id = ? AND {ALIVE}"), params![folder_id, id])?;
        }
        Ok(n)
    })
}

// ── 回收站 ────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    pub id: String,
    pub name: String,
    pub size: i64,
    pub version: i64,
    /// 删除前所在文件夹的名称（文件夹已删除时为空）
    pub folder_name: Option<String>,
    pub deleted_at: String,
    pub deleted_by: Option<String>,
    /// 到这个时间会被自动彻底删除（不自动清理时为空）
    pub purge_at: Option<String>,
}

/// 移到回收站
pub fn move_to_trash(conn: &Connection, id: &str, operator: &str) -> AppResult<()> {
    let n = conn.execute(
        &format!("UPDATE ledger_workbook SET deleted_at = CAST(? AS TIMESTAMP), deleted_by = ? WHERE id = ? AND {ALIVE}"),
        params![now_local(), operator, id],
    )?;
    if n == 0 {
        return Err(missing(conn, id));
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashList {
    /// 保留天数（0 为不自动清理）
    pub days: i64,
    pub items: Vec<TrashItem>,
}

pub fn trash(conn: &Connection) -> AppResult<TrashList> {
    Ok(TrashList { days: trash_days(), items: trash_list(conn)? })
}

/// 回收站：普通用户只看到自己的（管理员看全部）
pub fn trash_for(conn: &Connection, user: &CurrentUser) -> AppResult<TrashList> {
    let mut t = trash(conn)?;
    if user.enforced && !user.admin {
        let mine: std::collections::HashSet<String> = {
            let mut stmt = conn.prepare("SELECT id FROM ledger_workbook WHERE deleted_at IS NOT NULL AND owner_id = ?")?;
            stmt.query_map(params![user.id], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?
        };
        t.items.retain(|i| mine.contains(&i.id));
    }
    Ok(t)
}

pub fn trash_list(conn: &Connection) -> AppResult<Vec<TrashItem>> {
    let days = trash_days();
    let mut stmt = conn.prepare(
        "SELECT w.id, w.name, w.size, w.version, f.name, strftime(w.deleted_at, '%Y-%m-%d %H:%M:%S'), w.deleted_by,
                strftime(w.deleted_at + to_days(CAST(? AS INTEGER)), '%Y-%m-%d %H:%M')
         FROM ledger_workbook w LEFT JOIN ledger_folder f ON f.id = w.folder_id
         WHERE w.deleted_at IS NOT NULL ORDER BY w.deleted_at DESC",
    )?;
    let rows = stmt
        .query_map(params![days], |r| {
            Ok(TrashItem {
                id: r.get(0)?,
                name: r.get(1)?,
                size: r.get(2)?,
                version: r.get(3)?,
                folder_name: r.get(4)?,
                deleted_at: r.get(5)?,
                deleted_by: r.get(6)?,
                purge_at: if days > 0 { r.get(7)? } else { None },
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 从回收站恢复；原文件夹已删除时放回根目录
pub fn untrash(conn: &Connection, id: &str) -> AppResult<WorkbookMeta> {
    let n = conn.execute(
        "UPDATE ledger_workbook SET deleted_at = NULL, deleted_by = NULL,
           folder_id = CASE WHEN folder_id IN (SELECT id FROM ledger_folder) THEN folder_id ELSE NULL END
         WHERE id = ? AND deleted_at IS NOT NULL",
        params![id],
    )?;
    if n == 0 {
        return Err(AppError::bad("回收站里没有这个工作簿"));
    }
    meta(conn, id)
}

fn purge_ids(conn: &Connection, ids: &[String]) -> AppResult<usize> {
    in_tx(conn, || {
        for id in ids {
            conn.execute("DELETE FROM ledger_workbook_version WHERE workbook_id = ?", params![id])?;
            crate::comment::purge_workbook(conn, id)?;
            crate::share::purge_workbook(conn, id)?;
            conn.execute("DELETE FROM ledger_workbook WHERE id = ? AND deleted_at IS NOT NULL", params![id])?;
        }
        Ok(ids.len())
    })
}

fn trashed_ids(conn: &Connection, extra: &str, args: &[&dyn duckdb::ToSql]) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(&format!("SELECT id FROM ledger_workbook WHERE deleted_at IS NOT NULL {extra}"))?;
    let ids = stmt.query_map(args, |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// 彻底删除回收站里的一个工作簿（含全部版本，不可恢复）
pub fn purge(conn: &Connection, id: &str) -> AppResult<()> {
    let ids = trashed_ids(conn, "AND id = ?", &[&id])?;
    if ids.is_empty() {
        return Err(AppError::bad("只能彻底删除回收站里的工作簿"));
    }
    purge_ids(conn, &ids)?;
    Ok(())
}

/// 清空回收站，返回删除个数
pub fn empty_trash(conn: &Connection) -> AppResult<usize> {
    let ids = trashed_ids(conn, "", &[])?;
    purge_ids(conn, &ids)
}

/// 自动清理超过保留天数的工作簿
pub fn purge_expired(conn: &Connection) -> AppResult<usize> {
    let days = trash_days();
    if days == 0 {
        return Ok(0);
    }
    let ids = trashed_ids(conn, "AND deleted_at < CAST(? AS TIMESTAMP) - to_days(CAST(? AS INTEGER))", &[&now_local(), &days])?;
    if ids.is_empty() {
        return Ok(0);
    }
    purge_ids(conn, &ids)
}

// ── 文件夹 ────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInput {
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub operator: String,
}

pub fn folders(conn: &Connection) -> AppResult<Vec<Folder>> {
    let mut stmt = conn.prepare("SELECT id, name, parent_id, created_by, strftime(created_at, '%Y-%m-%d %H:%M:%S') FROM ledger_folder ORDER BY name")?;
    let rows = stmt
        .query_map([], |r| Ok(Folder { id: r.get(0)?, name: r.get(1)?, parent_id: r.get(2)?, created_by: r.get(3)?, created_at: r.get(4)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn folder_exists(conn: &Connection, id: &str) -> AppResult<()> {
    let n: i64 = conn.query_row("SELECT count(*) FROM ledger_folder WHERE id = ?", params![id], |r| r.get(0))?;
    if n == 0 {
        return Err(AppError::bad("文件夹不存在或已删除"));
    }
    Ok(())
}

/// 某文件夹到根的路径（含自身，自下而上）
fn ancestors(all: &[Folder], id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = Some(id.to_string());
    while let Some(c) = cur {
        if out.contains(&c) {
            break;
        }
        cur = all.iter().find(|f| f.id == c).and_then(|f| f.parent_id.clone());
        out.push(c);
    }
    out
}

/// 某文件夹及其全部子孙
fn descendants(all: &[Folder], id: &str) -> Vec<String> {
    let mut out = vec![id.to_string()];
    let mut i = 0;
    while i < out.len() {
        let cur = out[i].clone();
        out.extend(all.iter().filter(|f| f.parent_id.as_deref() == Some(cur.as_str())).map(|f| f.id.clone()));
        i += 1;
    }
    out
}

/// 子树高度（只有自己为 1）
fn subtree_height(all: &[Folder], id: &str) -> usize {
    1 + all.iter().filter(|f| f.parent_id.as_deref() == Some(id)).map(|f| subtree_height(all, &f.id)).max().unwrap_or(0)
}

fn check_folder(all: &[Folder], self_id: Option<&str>, name: &str, parent: Option<&str>) -> AppResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 40 {
        return Err(AppError::bad("文件夹名称不能为空，且不超过 40 个字符"));
    }
    if n.contains('/') {
        return Err(AppError::bad("文件夹名称不能包含 /"));
    }
    if let Some(p) = parent {
        if !all.iter().any(|f| f.id == p) {
            return Err(AppError::bad("上级文件夹不存在或已删除"));
        }
        if let Some(me) = self_id
            && ancestors(all, p).iter().any(|a| a == me) {
                return Err(AppError::bad("不能把文件夹移到它自己或它的子文件夹里"));
            }
    }
    let depth = parent.map(|p| ancestors(all, p).len()).unwrap_or(0) + self_id.map(|me| subtree_height(all, me)).unwrap_or(1);
    if depth > MAX_FOLDER_DEPTH {
        return Err(AppError::bad(format!("文件夹最多嵌套 {MAX_FOLDER_DEPTH} 层")));
    }
    if all.iter().any(|f| f.parent_id.as_deref() == parent && Some(f.id.as_str()) != self_id && f.name.to_lowercase() == n.to_lowercase()) {
        return Err(AppError::bad(format!("这里已有名为「{n}」的文件夹")));
    }
    Ok(n.to_string())
}

pub fn create_folder(conn: &Connection, input: &FolderInput) -> AppResult<Folder> {
    let all = folders(conn)?;
    let parent = input.parent_id.as_deref().filter(|s| !s.is_empty());
    let name = check_folder(&all, None, &input.name, parent)?;
    let id = format!("fd_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    conn.execute(
        "INSERT INTO ledger_folder (id, name, parent_id, created_by, created_at) VALUES (?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params![id, name, parent, input.operator, now_local()],
    )?;
    folders(conn)?.into_iter().find(|f| f.id == id).ok_or_else(|| AppError::bad("文件夹创建失败"))
}

/// 改名 / 移动文件夹
pub fn update_folder(conn: &Connection, id: &str, input: &FolderInput) -> AppResult<()> {
    let all = folders(conn)?;
    if !all.iter().any(|f| f.id == id) {
        return Err(AppError::bad("文件夹不存在或已删除"));
    }
    let parent = input.parent_id.as_deref().filter(|s| !s.is_empty());
    let name = check_folder(&all, Some(id), &input.name, parent)?;
    conn.execute("UPDATE ledger_folder SET name = ?, parent_id = ? WHERE id = ?", params![name, parent, id])?;
    Ok(())
}

/// 删除文件夹（含子文件夹）：其中的工作簿移到回收站（恢复时回到根目录），返回移入回收站的个数
pub fn delete_folder(conn: &Connection, id: &str, operator: &str) -> AppResult<usize> {
    let all = folders(conn)?;
    if !all.iter().any(|f| f.id == id) {
        return Err(AppError::bad("文件夹不存在或已删除"));
    }
    let ids = descendants(&all, id);
    in_tx(conn, || {
        let now = now_local();
        let mut moved = 0;
        for f in &ids {
            moved += conn.execute(
                &format!("UPDATE ledger_workbook SET deleted_at = CAST(? AS TIMESTAMP), deleted_by = ? WHERE folder_id = ? AND {ALIVE}"),
                params![now, operator, f],
            )?;
            conn.execute("DELETE FROM ledger_folder WHERE id = ?", params![f])?;
        }
        Ok(moved)
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub version: i64,
    pub size: i64,
    pub saved_by: Option<String>,
    pub saved_at: String,
    pub description: Option<String>,
    pub is_auto: bool,
}

pub fn versions(conn: &Connection, id: &str) -> AppResult<Vec<VersionInfo>> {
    meta(conn, id)?;
    let mut stmt = conn.prepare(
        "SELECT version, size, saved_by, strftime(saved_at, '%Y-%m-%d %H:%M:%S'), description, is_auto
         FROM ledger_workbook_version WHERE workbook_id = ? ORDER BY version DESC",
    )?;
    let rows = stmt
        .query_map(params![id], |r| {
            Ok(VersionInfo { version: r.get(0)?, size: r.get(1)?, saved_by: r.get(2)?, saved_at: r.get(3)?, description: r.get(4)?, is_auto: r.get(5)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn version_content(conn: &Connection, id: &str, version: i64) -> AppResult<Vec<u8>> {
    conn.query_row(
        "SELECT data FROM ledger_workbook_version WHERE workbook_id = ? AND version = ?",
        params![id, version],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| AppError::bad(format!("版本 {version} 不存在（自动保存版本只保留最近 {AUTO_KEEP} 个）")))
}

/// 恢复到某个历史版本：以该版本内容生成一个新版本（历史不丢）
pub fn restore(conn: &Connection, id: &str, version: i64, operator: &str) -> AppResult<WorkbookMeta> {
    let gz = version_content(conn, id, version)?;
    let current = meta(conn, id)?.version;
    let r = save(
        conn,
        id,
        &gz,
        &SaveRequest { base_version: current, force: true, operator, description: &format!("恢复到版本 {version}"), is_auto: false },
    )?;
    r.meta.ok_or_else(|| AppError::bad("恢复失败"))
}

/// 测试辅助：JSON → gzip
#[allow(dead_code)]
pub fn gzip(json: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::fast());
    let _ = e.write_all(json);
    e.finish().unwrap_or_default()
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    fn f(id: &str, name: &str, parent: Option<&str>) -> Folder {
        Folder { id: id.into(), name: name.into(), parent_id: parent.map(String::from), created_by: None, created_at: String::new() }
    }

    #[test]
    fn folder_rules() {
        // a / b / c / d
        let all = vec![f("a", "财务", None), f("b", "月报", Some("a")), f("c", "2026", Some("b")), f("d", "10月", Some("c")), f("x", "运营", None)];
        assert_eq!(descendants(&all, "b"), vec!["b", "c", "d"]);
        assert_eq!(ancestors(&all, "d"), vec!["d", "c", "b", "a"]);
        assert_eq!(subtree_height(&all, "a"), 4);
        // 同级重名（不区分大小写）
        assert!(check_folder(&all, None, "月报", Some("a")).is_err());
        assert!(check_folder(&all, None, "月报", None).is_ok());
        // 不能移到自己或子孙里
        assert!(check_folder(&all, Some("b"), "月报", Some("d")).is_err());
        assert!(check_folder(&all, Some("b"), "月报", Some("b")).is_err());
        // 层数：d 下再建一层为第 5 层，可以；再深不行
        assert!(check_folder(&all, None, "第5层", Some("d")).is_ok());
        let mut deep = all.clone();
        deep.push(f("e", "第5层", Some("d")));
        assert!(check_folder(&deep, None, "第6层", Some("e")).is_err());
        // 把 4 层高的 a 移到 x 下面会变成 5 层：可以；移到 5 层深的位置不行
        assert!(check_folder(&deep, Some("x"), "运营", Some("d")).is_ok());
        assert!(check_folder(&deep, Some("a"), "财务", Some("x")).is_err());
        assert!(check_folder(&all, Some("a"), "财务", Some("x")).is_ok());
        // 名称
        assert!(check_folder(&all, None, "  ", None).is_err());
        assert!(check_folder(&all, None, "a/b", None).is_err());
        assert_eq!(check_folder(&all, None, "  新建  ", None).unwrap(), "新建");
    }
}

#[cfg(test)]
mod trash_tests {
    use super::*;

    #[test]
    fn trash_lifecycle_and_expiry() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        let gz = gzip(br#"{"sheets":{}}"#);
        let fd = create_folder(&conn, &FolderInput { name: "财务".into(), parent_id: None, operator: "t".into() }).unwrap();
        let a = create_in(&conn, "旧", &gz, "t", Some(&fd.id)).unwrap();
        let b = create(&conn, "新", &gz, "t").unwrap();
        assert_eq!(a.folder_id.as_deref(), Some(fd.id.as_str()));
        move_to_trash(&conn, &a.id, "t").unwrap();
        move_to_trash(&conn, &b.id, "t").unwrap();
        assert!(list(&conn).unwrap().is_empty());
        assert!(save(&conn, &a.id, &gz, &SaveRequest { base_version: 1, force: false, operator: "t", description: "", is_auto: true }).is_err());
        // a 删除于 31 天前：自动清理只删 a
        conn.execute("UPDATE ledger_workbook SET deleted_at = deleted_at - INTERVAL 31 DAY WHERE id = ?", params![a.id]).unwrap();
        assert_eq!(purge_expired(&conn).unwrap(), 1);
        let left: Vec<String> = trash_list(&conn).unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(left, vec![b.id.clone()]);
        let versions: i64 = conn.query_row("SELECT count(*) FROM ledger_workbook_version WHERE workbook_id = ?", params![a.id], |r| r.get(0)).unwrap();
        assert_eq!(versions, 0);
        untrash(&conn, &b.id).unwrap();
        assert_eq!(list(&conn).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod access_tests {
    use super::*;
    use crate::auth::{CurrentUser, Need};

    fn user(id: &str, admin: bool) -> CurrentUser {
        CurrentUser { id: id.into(), name: format!("用户{id}"), admin, enforced: true }
    }

    #[test]
    fn workbook_and_dataset_permissions() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        let gz = gzip(br#"{"sheets":{}}"#);
        let (a, b, admin) = (user("a", false), user("b", false), user("root", true));

        // a 新建：默认仅自己可见
        let w = create(&conn, "利润表", &gz, &a.name).unwrap();
        set_owner(&conn, &w.id, &a).unwrap();
        assert!(require(&conn, &w.id, &a, Need::Manage, false).is_ok());
        assert!(require(&conn, &w.id, &admin, Need::Manage, false).is_ok());
        let denied = require(&conn, &w.id, &b, Need::View, false).unwrap_err().message();
        assert!(denied.contains("无权访问") && denied.contains("用户a"), "{denied}");
        assert!(list_for(&conn, &b).unwrap().is_empty());
        assert_eq!(list_for(&conn, &a).unwrap().len(), 1);

        // 共享为只读：b 能看、不能改、不能删
        set_visibility(&conn, &w.id, "view").unwrap();
        assert!(require(&conn, &w.id, &b, Need::View, false).is_ok());
        assert!(require(&conn, &w.id, &b, Need::Edit, false).unwrap_err().message().contains("只读"));
        assert!(require(&conn, &w.id, &b, Need::Manage, false).is_err());
        assert_eq!(list_for(&conn, &b).unwrap().len(), 1);

        // 共享为可编辑：b 能改，仍不能删 / 改共享范围
        set_visibility(&conn, &w.id, "edit").unwrap();
        assert!(require(&conn, &w.id, &b, Need::Edit, false).is_ok());
        assert!(require(&conn, &w.id, &b, Need::Manage, false).unwrap_err().message().contains("所有者或管理员"));
        assert!(set_visibility(&conn, &w.id, "public").is_err());

        // 回收站：只有所有者 / 管理员能恢复，别人看不到
        move_to_trash(&conn, &w.id, &a.name).unwrap();
        assert!(require(&conn, &w.id, &b, Need::Edit, false).is_err());
        assert!(require(&conn, &w.id, &b, Need::Manage, true).is_err());
        assert!(require(&conn, &w.id, &a, Need::Manage, true).is_ok());
        assert_eq!(trash_for(&conn, &a).unwrap().items.len(), 1);
        assert!(trash_for(&conn, &b).unwrap().items.is_empty());
        assert_eq!(trash_for(&conn, &admin).unwrap().items.len(), 1);

        // 登录体系启用前创建的（没有所有者）：按共享范围用，只有管理员能管理
        let old = create(&conn, "旧表", &gz, "某人").unwrap();
        assert!(require(&conn, &old.id, &b, Need::Edit, false).is_ok());
        assert!(require(&conn, &old.id, &b, Need::Manage, false).is_err());
        assert!(require(&conn, &old.id, &admin, Need::Manage, false).is_ok());

        // 未开启认证：一律放行，不改动所有者
        let open = CurrentUser::open();
        let w2 = create(&conn, "无认证", &gz, "x").unwrap();
        set_owner(&conn, &w2.id, &open).unwrap();
        assert!(meta(&conn, &w2.id).unwrap().owner_id.is_none());
        assert!(require(&conn, &w2.id, &open, Need::Manage, false).is_ok());

        // 不存在
        assert!(require(&conn, "wb_none", &a, Need::View, false).unwrap_err().message().contains("不存在"));

        // 数据集：所有者与管理员可以改，其他人不行；内置 / 没有所有者的只有管理员
        conn.execute("INSERT INTO ledger_dataset (id, name, columns) VALUES ('ds_1', '订单', '[]'), ('ds_old', '老数据', '[]')", []).unwrap();
        crate::dataset::set_owner(&conn, "ds_1", &a).unwrap();
        assert_eq!(crate::dataset::require_manage(&conn, "ds_1", &a).unwrap(), "订单");
        assert!(crate::dataset::require_manage(&conn, "ds_1", &b).unwrap_err().message().contains("用户a"));
        assert!(crate::dataset::require_manage(&conn, "ds_1", &admin).is_ok());
        assert!(crate::dataset::require_manage(&conn, "ds_old", &a).is_err());
        assert!(crate::dataset::require_manage(&conn, "ledger_detail", &a).is_err());
        assert!(crate::dataset::require_manage(&conn, "ds_old", &admin).is_ok());
        // 已有所有者的数据集不会被后来者「认领」
        crate::dataset::set_owner(&conn, "ds_1", &b).unwrap();
        assert!(crate::dataset::require_manage(&conn, "ds_1", &b).is_err());
    }
}
