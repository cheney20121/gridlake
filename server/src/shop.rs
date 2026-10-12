//! 店铺档案：导入、汇总、完整性检查都以档案中的店铺为准
//!
//! 明细表以店铺名关联（汇总表列标题与公式条件也是店铺名），改名时在事务内同步更新历史数据

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::import::validate_shop;
use crate::schema::ident;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopInfo {
    pub name: String,
    pub douyin_shop_id: Option<String>,
    /// active 运营中 / closed 已关店（关店后不再提示缺数）
    pub status: String,
    pub remark: Option<String>,
    pub created_at: String,
    pub row_count: i64,
    pub periods: Vec<String>,
    /// 出现过的商户主体（支持主体变更后一个店铺对应多个主体）
    pub entities: Vec<String>,
    pub product_count: i64,
    pub first_time: Option<String>,
    pub last_time: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopInput {
    pub name: String,
    pub douyin_shop_id: Option<String>,
    pub remark: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopUpdate {
    /// 当前店铺名
    pub name: String,
    pub new_name: Option<String>,
    pub douyin_shop_id: Option<String>,
    pub status: Option<String>,
    pub remark: Option<String>,
}

fn split(s: Option<String>) -> Vec<String> {
    s.filter(|s| !s.is_empty()).map(|s| s.split('\u{1}').map(String::from).collect()).unwrap_or_default()
}

pub fn list(conn: &Connection) -> AppResult<Vec<ShopInfo>> {
    let sql = format!(
        "WITH stats AS (
           SELECT shop, count(*) AS row_count,
                  string_agg(DISTINCT period, chr(1) ORDER BY period) AS periods,
                  string_agg(DISTINCT {entity}, chr(1) ORDER BY {entity}) AS entities,
                  count(DISTINCT {product}) AS product_count,
                  min({time}) AS first_time, max({time}) AS last_time
           FROM ledger_detail GROUP BY shop
         )
         SELECT s.name, s.douyin_shop_id, s.status, s.remark, strftime(s.created_at, '%Y-%m-%d %H:%M:%S'),
                COALESCE(t.row_count, 0), t.periods, t.entities, COALESCE(t.product_count, 0), t.first_time, t.last_time
         FROM ledger_shop s LEFT JOIN stats t ON t.shop = s.name
         ORDER BY s.status, s.name",
        entity = ident("商户主体名称"),
        product = ident("商品ID"),
        time = ident("动账时间"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ShopInfo {
                name: r.get(0)?,
                douyin_shop_id: r.get(1)?,
                status: r.get(2)?,
                remark: r.get(3)?,
                created_at: r.get(4)?,
                row_count: r.get(5)?,
                periods: split(r.get(6)?),
                entities: split(r.get(7)?),
                product_count: r.get(8)?,
                first_time: r.get(9)?,
                last_time: r.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn exists(conn: &Connection, name: &str) -> AppResult<bool> {
    let n: i64 = conn.query_row("SELECT count(*) FROM ledger_shop WHERE name = ?", params![name], |r| r.get(0))?;
    Ok(n > 0)
}

fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn create(conn: &Connection, input: ShopInput) -> AppResult<Vec<ShopInfo>> {
    let name = validate_shop(&input.name)?;
    if exists(conn, &name)? {
        return Err(AppError::bad(format!("店铺「{name}」已存在")));
    }
    conn.execute(
        "INSERT INTO ledger_shop (name, douyin_shop_id, remark, created_at) VALUES (?, ?, ?, CAST(? AS TIMESTAMP))",
        params![name, clean(input.douyin_shop_id), clean(input.remark), crate::db::now_local()],
    )?;
    list(conn)
}

pub fn update(conn: &Connection, input: ShopUpdate) -> AppResult<Vec<ShopInfo>> {
    if !exists(conn, &input.name)? {
        return Err(AppError::bad(format!("店铺「{}」不存在", input.name)));
    }
    if let Some(status) = &input.status
        && status != "active"
        && status != "closed"
    {
        return Err(AppError::bad("店铺状态只能是 active 或 closed"));
    }
    let new_name = match input.new_name.as_deref().map(str::trim).filter(|n| !n.is_empty() && *n != input.name) {
        Some(n) => {
            let n = validate_shop(n)?;
            if exists(conn, &n)? {
                return Err(AppError::bad(format!("店铺「{n}」已存在，不能重名")));
            }
            Some(n)
        }
        None => None,
    };

    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> anyhow::Result<()> {
        conn.execute(
            "UPDATE ledger_shop SET douyin_shop_id = COALESCE(?, douyin_shop_id), status = COALESCE(?, status),
                    remark = COALESCE(?, remark) WHERE name = ?",
            params![input.douyin_shop_id.map(|s| s.trim().to_string()), input.status, input.remark.map(|s| s.trim().to_string()), input.name],
        )?;
        if let Some(n) = &new_name {
            for table in [
                "ledger_shop SET name",
                "ledger_detail SET shop",
                "ledger_import SET shop",
                "ledger_balance SET shop",
                "ledger_withdraw_match SET shop",
                "ledger_period_close SET shop",
                "ledger_close_log SET shop",
                "ledger_ad_spend SET shop",
            ] {
                let (t, col) = table.split_once(" SET ").unwrap_or_default();
                conn.execute(&format!("UPDATE {t} SET {col} = ? WHERE {col} = ?"), params![n, input.name])?;
            }
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
    list(conn)
}
