//! 查询：聚合（供前端汇总表公式引用）、明细分页、月份与导入记录

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::error::{AppError, AppResult};
use crate::ecommerce::import::EntityCount;
use crate::schema::{NUMERIC_COLUMNS, ident};

// ── 聚合 ──────────────────────────────────────────────

/// 聚合结果的维度列，之后依次是各金额列合计，最后是佣金正负拆分
pub const PIVOT_DIMENSIONS: &[&str] = &["商户主体名称", "店铺", "动账场景", "动账方向", "笔数"];
pub const PIVOT_COMMISSION_SPLIT: &[&str] = &["佣金(负)", "佣金(正)"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PivotResult {
    pub period: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Json>>,
    pub shops: Vec<String>,
    /// 按流水笔数降序，前端据此排列汇总表的主体列（主力主体在前）
    pub entities: Vec<EntityCount>,
    pub total_rows: i64,
}

pub fn pivot(conn: &Connection, period: &str) -> AppResult<PivotResult> {
    let sums = NUMERIC_COLUMNS
        .iter()
        .map(|c| format!("COALESCE(sum({}), 0)::DOUBLE", ident(c)))
        .chain([
            format!("COALESCE(sum({c}) FILTER (WHERE {c} < 0), 0)::DOUBLE", c = ident("佣金")),
            format!("COALESCE(sum({c}) FILTER (WHERE {c} > 0), 0)::DOUBLE", c = ident("佣金")),
        ])
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT COALESCE({entity}, ''), shop, COALESCE({scene}, ''), COALESCE({direction}, ''), count(*), {sums}
         FROM ledger_detail WHERE period = ?
         GROUP BY 1, 2, 3, 4 ORDER BY 1, 2, 3, 4",
        entity = ident("商户主体名称"),
        scene = ident("动账场景"),
        direction = ident("动账方向"),
    );

    let value_count = NUMERIC_COLUMNS.len() + PIVOT_COMMISSION_SPLIT.len();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![period], |row| {
            let mut out: Vec<Json> = Vec::with_capacity(5 + value_count);
            for i in 0..4 {
                out.push(Json::String(row.get::<_, String>(i)?));
            }
            out.push(Json::from(row.get::<_, i64>(4)?));
            for i in 0..value_count {
                let v: f64 = row.get(5 + i)?;
                // 金额列为 DECIMAL(18,2)，合计转 DOUBLE 后保留两位，消除二进制表示的尾差
                out.push(Json::from((v * 100.0).round() / 100.0));
            }
            Ok(out)
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut stmt = conn.prepare(&format!(
        "SELECT d.entity, d.cnt, a.alias
         FROM (SELECT COALESCE({e}, '') AS entity, count(*) AS cnt FROM ledger_detail WHERE period = ? GROUP BY 1) d
         LEFT JOIN ledger_entity_alias a ON a.entity = d.entity
         ORDER BY d.cnt DESC, d.entity",
        e = ident("商户主体名称")
    ))?;
    let entities = stmt
        .query_map(params![period], |r| Ok(EntityCount { name: r.get(0)?, row_count: r.get(1)?, alias: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut stmt = conn.prepare("SELECT DISTINCT shop FROM ledger_detail WHERE period = ? ORDER BY 1")?;
    let shops = stmt.query_map(params![period], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let total_rows = entities.iter().map(|e| e.row_count).sum();

    let columns = PIVOT_DIMENSIONS
        .iter()
        .chain(NUMERIC_COLUMNS.iter())
        .chain(PIVOT_COMMISSION_SPLIT.iter())
        .map(|s| s.to_string())
        .collect();
    Ok(PivotResult { period: period.to_string(), columns, rows, shops, entities, total_rows })
}

// ── 主体简称 ──────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityAlias {
    pub entity: String,
    pub alias: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct EntityAliasInput {
    pub entity: String,
    /// 为空表示删除简称，恢复为前端默认推导
    pub alias: Option<String>,
}

pub fn entity_aliases(conn: &Connection) -> AppResult<Vec<EntityAlias>> {
    let mut stmt = conn.prepare(
        "SELECT entity, alias, strftime(updated_at, '%Y-%m-%d %H:%M:%S') FROM ledger_entity_alias ORDER BY entity",
    )?;
    let rows = stmt
        .query_map([], |r| Ok(EntityAlias { entity: r.get(0)?, alias: r.get(1)?, updated_at: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 批量保存（事务）：alias 为空则删除
pub fn save_entity_aliases(conn: &Connection, items: &[EntityAliasInput]) -> AppResult<Vec<EntityAlias>> {
    for item in items {
        if item.entity.trim().is_empty() {
            return Err(AppError::bad("商户主体名称不能为空"));
        }
        if item.alias.as_deref().map(|a| a.trim().chars().count()).unwrap_or(0) > 30 {
            return Err(AppError::bad(format!("「{}」的简称不能超过 30 个字符", item.entity)));
        }
    }
    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> anyhow::Result<()> {
        for item in items {
            let entity = item.entity.trim();
            match item.alias.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                Some(alias) => conn.execute(
                    "INSERT INTO ledger_entity_alias (entity, alias, updated_at) VALUES (?, ?, CAST(? AS TIMESTAMP))
                     ON CONFLICT (entity) DO UPDATE SET alias = excluded.alias, updated_at = excluded.updated_at",
                    params![entity, alias, crate::db::now_local()],
                )?,
                None => conn.execute("DELETE FROM ledger_entity_alias WHERE entity = ?", params![entity])?,
            };
        }
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e.into());
        }
    }
    entity_aliases(conn)
}

// ── 月份 / 导入记录 / 删除 ────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodInfo {
    pub period: String,
    pub shops: Vec<String>,
    pub row_count: i64,
}

pub fn periods(conn: &Connection) -> AppResult<Vec<PeriodInfo>> {
    let mut stmt = conn.prepare(
        "SELECT period, string_agg(DISTINCT shop, chr(1) ORDER BY shop), count(*)
         FROM ledger_detail GROUP BY period ORDER BY period DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let shops: String = r.get(1)?;
            Ok(PeriodInfo { period: r.get(0)?, shops: shops.split('\u{1}').map(String::from).collect(), row_count: r.get(2)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecord {
    pub import_id: String,
    pub shop: String,
    pub file_name: Option<String>,
    pub periods: Vec<String>,
    pub entities: Vec<String>,
    pub row_count: i64,
    pub replaced_rows: i64,
    pub missing_columns: Vec<String>,
    pub extra_columns: Vec<String>,
    pub elapsed_ms: i64,
    pub imported_at: String,
    pub mode: Option<String>,
    pub inserted_rows: Option<i64>,
    pub skipped_rows: Option<i64>,
    pub changed_rows: Option<i64>,
    pub file_hash: Option<String>,
}

pub fn imports(conn: &Connection) -> AppResult<Vec<ImportRecord>> {
    let parse = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
    let mut stmt = conn.prepare(
        "SELECT import_id, shop, file_name, periods, entities, row_count, replaced_rows, missing_columns, extra_columns,
                elapsed_ms, strftime(imported_at, '%Y-%m-%d %H:%M:%S'), mode, inserted_rows, skipped_rows, changed_rows, file_hash
         FROM ledger_import ORDER BY imported_at DESC LIMIT 200",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ImportRecord {
                import_id: r.get(0)?,
                shop: r.get(1)?,
                file_name: r.get(2)?,
                periods: parse(r.get(3)?),
                entities: parse(r.get(4)?),
                row_count: r.get(5)?,
                replaced_rows: r.get(6)?,
                missing_columns: parse(r.get(7)?),
                extra_columns: parse(r.get(8)?),
                elapsed_ms: r.get(9)?,
                imported_at: r.get(10)?,
                mode: r.get(11)?,
                inserted_rows: r.get(12)?,
                skipped_rows: r.get(13)?,
                changed_rows: r.get(14)?,
                file_hash: r.get(15)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn delete_period(conn: &Connection, shop: &str, period: &str) -> AppResult<i64> {
    crate::ecommerce::close::ensure_editable(conn, shop, period)?;
    let deleted = conn.execute("DELETE FROM ledger_detail WHERE shop = ? AND period = ?", params![shop, period])?;
    if deleted == 0 {
        return Err(AppError::bad(format!("没有找到「{shop}」{period} 的数据")));
    }
    Ok(deleted as i64)
}
