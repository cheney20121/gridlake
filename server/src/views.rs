//! 关联视图：左数据集 ⟕ 右数据集（按一个或多个键），带入右侧指定列，生成新的数据集
//!
//! - 右侧按键去重（同一键取最先入库的一行）：一笔订单不会因为成本表里有两行而被算两次
//! - 键统一转文本并去首尾空格后比较（编号列有的存文本、有的存数字）
//! - 以 DuckDB 视图实现，始终反映两侧最新数据；行标识沿用左表的行号（__rid），大数据表的游标、导出照常可用

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::dataset::{self, DatasetDef};
use crate::error::{AppError, AppResult};
use crate::schema::ident;

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KeyPair {
    pub left: String,
    pub right: String,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ViewColumn {
    /// 右侧数据集的列名
    pub source: String,
    /// 在关联视图中的列名
    pub title: String,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ViewDef {
    pub left: String,
    pub right: String,
    pub on: Vec<KeyPair>,
    pub columns: Vec<ViewColumn>,
}

fn col<'a>(def: &'a DatasetDef, name: &str) -> AppResult<&'a dataset::ColumnDef> {
    def.columns.iter().find(|c| c.key == name || c.title == name).ok_or_else(|| AppError::bad(format!("数据集「{}」没有列「{name}」", def.name)))
}

fn key_sql(def: &DatasetDef, name: &str, alias: &str) -> AppResult<String> {
    let c = col(def, name)?;
    // 计算列以表达式形式存在，需要带上表别名：这里只允许实际列做关联键
    if c.expr.is_some() {
        return Err(AppError::bad(format!("「{}」是计算列，暂不能作为关联键", c.title)));
    }
    Ok(format!("NULLIF(trim(CAST({alias}.{} AS VARCHAR)), '')", ident(&c.key)))
}

/// 右侧按键去重后的子查询（列名：__k0..、以及要带入的列）
fn right_subquery(right: &DatasetDef, v: &ViewDef) -> AppResult<String> {
    let mut sel = Vec::new();
    for (i, k) in v.on.iter().enumerate() {
        sel.push(format!("{} AS __k{i}", key_sql(right, &k.right, "r0")?));
    }
    for c in &v.columns {
        let rc = col(right, &c.source)?;
        let expr = match &rc.expr {
            // 右侧的计算列：表达式里的列名不带别名，在 r0 上直接计算
            Some(e) => format!("({e})"),
            None => format!("r0.{}", ident(&rc.key)),
        };
        sel.push(format!("{expr} AS {}", ident(&c.title)));
    }
    let part = (0..v.on.len()).map(|i| format!("__k{i}")).collect::<Vec<_>>().join(", ");
    Ok(format!(
        "SELECT {} FROM {} r0 WHERE {} QUALIFY row_number() OVER (PARTITION BY {part} ORDER BY r0.{rid}) = 1",
        sel.join(", "),
        right.table,
        (0..v.on.len()).map(|i| format!("{} IS NOT NULL", key_sql(right, &v.on[i].right, "r0").unwrap_or_default())).collect::<Vec<_>>().join(" AND "),
        rid = right.rid,
    ))
}

fn join_cond(left: &DatasetDef, v: &ViewDef) -> AppResult<String> {
    v.on.iter()
        .enumerate()
        .map(|(i, k)| Ok(format!("{} = r.__k{i}", key_sql(left, &k.left, "l")?)))
        .collect::<AppResult<Vec<_>>>()
        .map(|v| v.join(" AND "))
}

fn validate(conn: &Connection, v: &ViewDef) -> AppResult<(DatasetDef, DatasetDef)> {
    if v.on.is_empty() {
        return Err(AppError::bad("请至少设置一组关联键"));
    }
    if v.columns.is_empty() {
        return Err(AppError::bad("请至少选择一个要带入的列"));
    }
    let left = dataset::dataset(conn, &v.left)?;
    let right = dataset::dataset(conn, &v.right)?;
    if left.id == right.id {
        return Err(AppError::bad("左右不能是同一个数据集"));
    }
    let mut seen = std::collections::HashSet::new();
    for c in &v.columns {
        let t = c.title.trim();
        if t.is_empty() || !seen.insert(t.to_string()) {
            return Err(AppError::bad(format!("带入列的名称为空或重复：「{t}」")));
        }
        if left.columns.iter().any(|lc| lc.title == t || lc.key == t) {
            return Err(AppError::bad(format!("「{t}」与左侧数据集的列重名，请改个名字（如「{t}_{}」）", right.name)));
        }
        col(&right, &c.source)?;
    }
    Ok((left, right))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinPreview {
    pub left_rows: i64,
    pub right_rows: i64,
    /// 左侧能关联上的行数
    pub matched_rows: i64,
    pub unmatched_rows: i64,
    /// 左侧关联不上的键（出现次数最多的前 20 个）
    pub unmatched_keys: Vec<(String, i64)>,
    /// 右侧同一键有多行的键数（关联时取最先入库的一行）
    pub right_duplicate_keys: i64,
}

pub fn preview(conn: &Connection, v: &ViewDef) -> AppResult<JoinPreview> {
    let (left, right) = validate(conn, v)?;
    let sub = right_subquery(&right, v)?;
    let cond = join_cond(&left, v)?;
    let (left_rows, matched_rows): (i64, i64) = conn.query_row(
        &format!("SELECT count(*), count(r.__k0) FROM {} l LEFT JOIN ({sub}) r ON {cond}", left.table),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let right_rows: i64 = conn.query_row(&format!("SELECT count(*) FROM {}", right.table), [], |r| r.get(0))?;
    let left_key = v.on.iter().map(|k| key_sql(&left, &k.left, "l")).collect::<AppResult<Vec<_>>>()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT concat_ws(' / ', {keys}) AS k, count(*) FROM {t} l LEFT JOIN ({sub}) r ON {cond}
         WHERE r.__k0 IS NULL GROUP BY 1 ORDER BY 2 DESC LIMIT 20",
        keys = left_key.iter().map(|k| format!("COALESCE({k}, '（空）')")).collect::<Vec<_>>().join(", "),
        t = left.table,
    ))?;
    let unmatched_keys = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let right_keys = v.on.iter().map(|k| key_sql(&right, &k.right, "r0")).collect::<AppResult<Vec<_>>>()?;
    let right_duplicate_keys: i64 = conn.query_row(
        &format!(
            "SELECT count(*) FROM (SELECT {k} FROM {t} r0 GROUP BY ALL HAVING count(*) > 1)",
            k = right_keys.join(", "),
            t = right.table
        ),
        [],
        |r| r.get(0),
    )?;
    Ok(JoinPreview { left_rows, right_rows, matched_rows, unmatched_rows: left_rows - matched_rows, unmatched_keys, right_duplicate_keys })
}

/// 视图 SQL：左表全部列（含行号 __rid）+ 右侧带入的列
pub fn view_sql(left: &DatasetDef, right: &DatasetDef, v: &ViewDef) -> AppResult<String> {
    let sub = right_subquery(right, v)?;
    let cond = join_cond(left, v)?;
    let rid = if left.rid == "rowid" { "l.rowid AS __rid, ".to_string() } else { String::new() };
    let brought = v.columns.iter().map(|c| format!("r.{}", ident(c.title.trim()))).collect::<Vec<_>>().join(", ");
    Ok(format!("SELECT l.*, {rid}{brought} FROM {} l LEFT JOIN ({sub}) r ON {cond}", left.table))
}

pub fn create(conn: &Connection, name: &str, v: &ViewDef) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::bad("数据集名称不能为空，且不超过 40 个字符"));
    }
    if dataset::datasets(conn)?.iter().any(|d| d.name == name) {
        return Err(AppError::bad(format!("已存在名为「{name}」的数据集")));
    }
    let (left, right) = validate(conn, v)?;
    let id = format!("dsv_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    let sql = view_sql(&left, &right, v)?;
    let now = crate::db::now_local();
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        conn.execute_batch(&format!("CREATE VIEW {id} AS {sql}"))?;
        conn.execute(
            "INSERT INTO ledger_dataset (id, name, columns, primary_key, kind, definition, created_at, updated_at)
             VALUES (?, ?, '[]', '[]', 'view', ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
            params![id, name, serde_json::to_string(v)?, now, now],
        )?;
        Ok(())
    })();
    match r {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(id)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 某数据集被哪些关联视图引用（删除前检查）
pub fn dependents(conn: &Connection, id: &str) -> AppResult<Vec<String>> {
    // 回收站里的视图不算（彻底删除数据集时一并删除它们）
    let mut stmt = conn.prepare("SELECT name, definition FROM ledger_dataset WHERE kind = 'view' AND deleted_at IS NULL")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, d)| d.as_deref().and_then(|d| serde_json::from_str::<ViewDef>(d).ok()).is_some_and(|v| v.left == id || v.right == id))
        .map(|(n, _)| n)
        .collect())
}

/// 按定义重建所有关联视图（右侧带入的计算列是内联在视图 SQL 里的，计算列修改后需要重建）
pub fn rebuild_all(conn: &Connection) -> AppResult<()> {
    let defs = dataset::datasets(conn)?;
    for d in defs.iter().filter(|d| d.kind == "view") {
        let Some(v) = &d.view else { continue };
        let r = (|| -> AppResult<()> {
            let left = defs.iter().find(|x| x.id == v.left).ok_or_else(|| AppError::bad("左侧数据集不存在"))?;
            let right = defs.iter().find(|x| x.id == v.right).ok_or_else(|| AppError::bad("右侧数据集不存在"))?;
            let sql = view_sql(left, right, v)?;
            conn.execute_batch(&format!("CREATE OR REPLACE VIEW {} AS {sql}", d.id))?;
            Ok(())
        })();
        r.map_err(|e| AppError::bad(format!("关联视图「{}」会失效（{}），已取消", d.name, e.message())))?;
    }
    crate::recon::rebuild_all(conn)
}

/// 修改关联视图（关联键、带入的列、名称）；会让下游视图或计算列失效时整体取消
pub fn update(conn: &Connection, id: &str, name: &str, v: &ViewDef) -> AppResult<()> {
    let defs = dataset::datasets(conn)?;
    let me = defs.iter().find(|d| d.id == id && d.kind == "view").ok_or_else(|| AppError::bad("关联视图不存在"))?;
    let downstream: Vec<&str> = dataset::derived_views(&defs, id).iter().map(|d| d.id.as_str()).collect();
    if v.left == id || v.right == id || downstream.contains(&v.left.as_str()) || downstream.contains(&v.right.as_str()) {
        return Err(AppError::bad("不能关联自身或由它派生的数据集（会形成循环）"));
    }
    let rename = me.name != name.trim();
    validate(conn, v)?;
    let def = serde_json::to_string(v)?;
    dataset::guarded_change(conn, |conn| {
        if rename {
            dataset::rename(conn, id, name)?;
        }
        conn.execute(
            "UPDATE ledger_dataset SET definition = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params![def, crate::db::now_local(), id],
        )?;
        Ok(())
    })
}
