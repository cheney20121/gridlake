//! 数据集回收站：删除数据集时只做标记（deleted_at），数据表 / 视图与计算列都保留，可恢复；
//! 彻底删除时才删表。保留天数与工作簿回收站相同（LEDGER_TRASH_DAYS）
//!
//! - 回收站里的数据集对其他功能都视为不存在：列表、公式、透视、SQL、数据连接与查询的写入目标
//!   （写入会失败并提示「在回收站」，恢复后继续）
//! - 恢复：关联视图要求左右两侧的数据集都在；名称已被新数据集占用时自动加后缀；
//!   恢复后有计算列或视图失效时整体取消（guarded_change）
//! - 彻底删除数据表时，回收站里基于它的关联视图一并彻底删除

use duckdb::{Connection, params};
use serde::Serialize;

use crate::dataset;
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::views::ViewDef;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedDataset {
    pub id: String,
    pub name: String,
    /// table / view
    pub kind: String,
    /// 行数（关联视图为空）
    pub row_count: Option<i64>,
    pub deleted_at: String,
    pub deleted_by: Option<String>,
    pub purge_at: Option<String>,
    /// 现在不能恢复的原因（如关联视图引用的数据集也在回收站）
    pub blocked: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashList {
    pub days: i64,
    pub items: Vec<TrashedDataset>,
}

struct Row {
    id: String,
    name: String,
    kind: String,
    definition: Option<String>,
    deleted_at: String,
    deleted_by: Option<String>,
    purge_at: String,
}

fn trashed(conn: &Connection) -> AppResult<Vec<Row>> {
    let days = crate::workbook::trash_days();
    let mut stmt = conn.prepare(
        "SELECT id, name, COALESCE(kind, 'table'), definition, strftime(deleted_at, '%Y-%m-%d %H:%M:%S'), deleted_by,
                strftime(deleted_at + to_days(CAST(? AS INTEGER)), '%Y-%m-%d %H:%M')
         FROM ledger_dataset WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC",
    )?;
    let rows = stmt
        .query_map(params![days], |r| {
            Ok(Row { id: r.get(0)?, name: r.get(1)?, kind: r.get(2)?, definition: r.get(3)?, deleted_at: r.get(4)?, deleted_by: r.get(5)?, purge_at: r.get(6)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 关联视图 / 对账引用的两个数据集
fn view_sources(r: &Row) -> Option<[String; 2]> {
    match r.kind.as_str() {
        "view" => r.definition.as_deref().and_then(|d| serde_json::from_str::<ViewDef>(d).ok()).map(|v| [v.left, v.right]),
        "recon" => r.definition.as_deref().and_then(crate::recon::sources),
        _ => None,
    }
}

/// 视图类（关联视图、对账）：数据库里是 VIEW，没有自己的行数
fn is_view_kind(kind: &str) -> bool {
    matches!(kind, "view" | "recon")
}

/// 关联视图 / 对账现在能否恢复：引用的两个数据集都要在
fn blocked_reason(conn: &Connection, r: &Row, all: &[Row]) -> Option<String> {
    let v = view_sources(r)?;
    let alive = dataset::datasets(conn).unwrap_or_default();
    for side in &v {
        if alive.iter().any(|d| &d.id == side) {
            continue;
        }
        return Some(match all.iter().find(|x| &x.id == side) {
            Some(x) => format!("它引用的数据集「{}」也在回收站，请先恢复「{}」", x.name, x.name),
            None => "它引用的数据集已被彻底删除，不能恢复".into(),
        });
    }
    None
}

pub fn list(conn: &Connection) -> AppResult<TrashList> {
    let days = crate::workbook::trash_days();
    let rows = trashed(conn)?;
    let items = rows
        .iter()
        .map(|r| TrashedDataset {
            id: r.id.clone(),
            name: r.name.clone(),
            kind: r.kind.clone(),
            row_count: if is_view_kind(&r.kind) { None } else { conn.query_row(&format!("SELECT count(*) FROM {}", r.id), [], |x| x.get(0)).ok() },
            deleted_at: r.deleted_at.clone(),
            deleted_by: r.deleted_by.clone(),
            purge_at: if days > 0 { Some(r.purge_at.clone()) } else { None },
            blocked: blocked_reason(conn, r, &rows),
        })
        .collect();
    Ok(TrashList { days, items })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub id: String,
    pub name: String,
    /// 原名已被占用、改了名
    pub renamed: bool,
}

/// 从回收站恢复
pub fn restore(conn: &Connection, id: &str) -> AppResult<Restored> {
    let rows = trashed(conn)?;
    let r = rows.iter().find(|x| x.id == id).ok_or_else(|| AppError::bad("回收站里没有这个数据集"))?;
    if let Some(why) = blocked_reason(conn, r, &rows) {
        return Err(AppError::bad(why));
    }
    let taken: Vec<String> = dataset::datasets(conn)?.into_iter().map(|d| d.name).collect();
    let mut name = r.name.clone();
    let mut n = 1;
    while taken.contains(&name) {
        name = if n == 1 { format!("{}（恢复）", r.name) } else { format!("{}（恢复{n}）", r.name) };
        n += 1;
    }
    let renamed = name != r.name;
    // 恢复后重建关联视图并检查计算列：有失效的整体取消
    dataset::guarded_change(conn, |c| {
        c.execute("UPDATE ledger_dataset SET deleted_at = NULL, deleted_by = NULL, name = ? WHERE id = ?", params![name, id])?;
        Ok(())
    })
    .map_err(|e| AppError::bad(format!("恢复「{}」失败：{}", r.name, e.message())))?;
    Ok(Restored { id: id.to_string(), name, renamed })
}

/// 彻底删除一组回收站里的数据集（数据表时连同回收站里基于它的视图），返回删除的名称
fn purge_ids(conn: &Connection, ids: &[String]) -> AppResult<Vec<String>> {
    let rows = trashed(conn)?;
    let mut targets: Vec<&Row> = rows.iter().filter(|r| ids.contains(&r.id)).collect();
    // 回收站里依赖这些数据集的视图一并删除（视图依赖视图时逐层展开）
    loop {
        let more: Vec<&Row> = rows
            .iter()
            .filter(|r| !targets.iter().any(|t| t.id == r.id))
            .filter(|r| view_sources(r).is_some_and(|v| targets.iter().any(|t| v.contains(&t.id))))
            .collect();
        if more.is_empty() {
            break;
        }
        targets.extend(more);
    }
    // 视图先删（不依赖顺序也能删，先删更稳妥）
    targets.sort_by_key(|r| if is_view_kind(&r.kind) { 0 } else { 1 });
    conn.execute_batch("BEGIN TRANSACTION")?;
    let res = (|| -> AppResult<()> {
        for r in &targets {
            conn.execute("DELETE FROM ledger_dataset WHERE id = ? AND deleted_at IS NOT NULL", params![r.id])?;
            conn.execute("DELETE FROM ledger_calc_column WHERE dataset_id = ?", params![r.id])?;
            conn.execute("DELETE FROM ledger_writeback WHERE dataset_id = ?", params![r.id])?;
            if r.kind == "recon" {
                crate::recon::purge(conn, &r.id)?;
            }
            conn.execute_batch(&if is_view_kind(&r.kind) { format!("DROP VIEW IF EXISTS {}", r.id) } else { format!("DROP TABLE IF EXISTS {}", r.id) })?;
        }
        Ok(())
    })();
    match res {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(targets.iter().map(|r| r.name.clone()).collect())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 彻底删除（不可恢复）；返回实际删除的数据集名称（含连带删除的视图）
pub fn purge(conn: &Connection, id: &str) -> AppResult<Vec<String>> {
    if !trashed(conn)?.iter().any(|r| r.id == id) {
        return Err(AppError::bad("只能彻底删除回收站里的数据集"));
    }
    purge_ids(conn, &[id.to_string()])
}

pub fn empty(conn: &Connection) -> AppResult<usize> {
    let ids: Vec<String> = trashed(conn)?.into_iter().map(|r| r.id).collect();
    if ids.is_empty() {
        return Ok(0);
    }
    Ok(purge_ids(conn, &ids)?.len())
}

/// 自动清理超过保留天数的数据集
pub fn purge_expired(conn: &Connection) -> AppResult<usize> {
    let days = crate::workbook::trash_days();
    if days == 0 {
        return Ok(0);
    }
    let mut stmt = conn.prepare("SELECT id FROM ledger_dataset WHERE deleted_at IS NOT NULL AND deleted_at < CAST(? AS TIMESTAMP) - to_days(CAST(? AS INTEGER))")?;
    let ids = stmt.query_map(params![now_local(), days], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Ok(0);
    }
    Ok(purge_ids(conn, &ids)?.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::StoredColumn;
    use crate::views::{KeyPair, ViewColumn};

    fn table(conn: &Connection, id: &str, name: &str, cols: &[(&str, &str)], rows: &str) {
        let defs = cols.iter().map(|(c, k)| format!("\"{c}\" {}", if *k == "money" { "DOUBLE" } else { "VARCHAR" })).collect::<Vec<_>>().join(", ");
        conn.execute_batch(&format!("CREATE TABLE {id} ({defs}, search_text VARCHAR); INSERT INTO {id} VALUES {rows};")).unwrap();
        let stored: Vec<StoredColumn> = cols.iter().map(|(c, k)| StoredColumn { key: c.to_string(), title: c.to_string(), kind: k.to_string(), width: 100, source: c.to_string() }).collect();
        dataset::register(conn, id, name, &stored, &[]).unwrap();
    }

    #[test]
    fn trash_restore_purge() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        table(&conn, "ds_order", "订单", &[("商品ID", "text"), ("金额", "money")], "('A', 10, NULL), ('B', 20, NULL)");
        table(&conn, "ds_cost", "成本", &[("商品ID", "text"), ("成本价", "money")], "('A', 4, NULL)");
        let v = ViewDef {
            left: "ds_order".into(),
            right: "ds_cost".into(),
            on: vec![KeyPair { left: "商品ID".into(), right: "商品ID".into() }],
            columns: vec![ViewColumn { source: "成本价".into(), title: "成本价".into() }],
        };
        let vid = crate::views::create(&conn, "订单带成本", &v).unwrap();

        // 被视图引用时不能删；先删视图，再删成本
        assert!(dataset::delete(&conn, "ds_cost", "t").is_err());
        dataset::delete(&conn, &vid, "t").unwrap();
        assert_eq!(dataset::delete(&conn, "ds_cost", "t").unwrap(), 1);
        assert!(dataset::datasets(&conn).unwrap().iter().all(|d| d.id != "ds_cost" && d.id != vid));
        assert!(dataset::dataset(&conn, "成本").err().is_some_and(|e| e.message().contains("回收站")));

        // 视图引用的「成本」也在回收站：不能先恢复视图
        let l = list(&conn).unwrap();
        let view_item = l.items.iter().find(|i| i.id == vid).unwrap();
        assert!(view_item.blocked.as_deref().unwrap().contains("成本"));
        assert_eq!(l.items.iter().find(|i| i.id == "ds_cost").unwrap().row_count, Some(1));
        assert!(restore(&conn, &vid).is_err());

        // 期间有人新建了同名的「成本」：恢复时改名
        table(&conn, "ds_cost2", "成本", &[("商品ID", "text")], "('Z', NULL)");
        let r = restore(&conn, "ds_cost").unwrap();
        assert!(r.renamed);
        assert_eq!(r.name, "成本（恢复）");
        let r = restore(&conn, &vid).unwrap();
        assert!(!r.renamed);
        let joined: f64 = conn.query_row(&format!("SELECT sum(\"成本价\") FROM {vid}"), [], |x| x.get(0)).unwrap();
        assert_eq!(joined, 4.0);

        // 彻底删除数据表时，回收站里基于它的视图一并删除
        dataset::delete(&conn, &vid, "t").unwrap();
        dataset::delete(&conn, "ds_cost", "t").unwrap();
        assert!(purge(&conn, "ds_order").is_err());
        let mut names = purge(&conn, "ds_cost").unwrap();
        names.sort();
        assert_eq!(names, vec!["成本（恢复）".to_string(), "订单带成本".to_string()]);
        assert!(list(&conn).unwrap().items.is_empty());
        assert!(conn.execute_batch("SELECT * FROM ds_cost").is_err());

        // 自动清理：只清超过保留天数的
        dataset::delete(&conn, "ds_cost2", "t").unwrap();
        assert_eq!(purge_expired(&conn).unwrap(), 0);
        conn.execute("UPDATE ledger_dataset SET deleted_at = deleted_at - INTERVAL 31 DAY WHERE id = 'ds_cost2'", []).unwrap();
        assert_eq!(purge_expired(&conn).unwrap(), 1);
    }
}
