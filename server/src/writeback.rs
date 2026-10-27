//! 填报回写：工作表里的填报区提交到数据集
//!
//! - 整批校验：任何一格有误（数字 / 日期格式不对、主键为空或重复）都不写入，返回出错的格子
//! - upsert：按主键新增或更新，只更新提交了的列（其他列保持原值）；append：追加
//! - 每次提交记一条记录（含撤销用的前像）；数据集在这之后没有别的改动时可撤销，恢复到提交前

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::NaiveDateTime;
use duckdb::types::Value;
use duckdb::{Connection, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::dataset::{self, StoredColumn};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::kinds;
use crate::schema::{ident, literal};

/// 单次提交的行数上限
const MAX_ROWS: usize = 20_000;
/// 返回的错误格子上限
const MAX_ERRORS: usize = 200;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WritebackRequest {
    /// upsert / append
    pub mode: String,
    /// 每行各值对应的数据集列名
    pub columns: Vec<String>,
    /// 行（值为文本 / 数字 / null）
    pub rows: Vec<Vec<Json>>,
    /// 数据集未登记主键时，按这些列更新
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub operator: String,
    /// 来源（工作簿、页签、填报区名称），记入提交记录
    #[serde(default)]
    pub source: Option<Json>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CellError {
    /// 请求 rows 中的下标
    pub row: usize,
    /// 列名；为空表示整行的问题
    pub column: String,
    pub message: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WritebackOutcome {
    /// 写入成功时的提交记录 id
    pub batch_id: Option<String>,
    pub dataset_name: String,
    pub inserted: i64,
    pub updated: i64,
    /// 与已有数据完全相同、无需改动的行
    pub unchanged: i64,
    /// 删除的行（mode = delete）
    pub deleted: i64,
    /// 要删除的主键在数据集里找不到的个数
    pub not_found: i64,
    pub errors: Vec<CellError>,
    /// 错误总数（errors 最多列出 200 个）
    pub error_count: usize,
    pub elapsed_ms: i64,
}

/// 撤销用的前像：行均为数据集全部列（按 columns 顺序）
#[derive(Serialize, Deserialize, Default)]
struct Undo {
    columns: Vec<String>,
    keys: Vec<String>,
    /// 被更新的行在提交前的样子
    before: Vec<Vec<Json>>,
    /// 新增的行
    inserted: Vec<Vec<Json>>,
    /// 删除的行（整行）
    #[serde(default)]
    removed: Vec<Vec<Json>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WritebackRecord {
    pub id: String,
    pub dataset_id: String,
    pub dataset_name: Option<String>,
    pub mode: String,
    pub operator: String,
    pub created_at: String,
    pub inserted: i64,
    pub updated: i64,
    pub unchanged: i64,
    pub deleted: i64,
    pub source: Option<Json>,
    pub undone_at: Option<String>,
    pub undone_by: Option<String>,
    /// 现在能否撤销
    pub can_undo: bool,
    /// 不能撤销的原因
    pub undo_blocked: Option<String>,
}

fn kind_of(c: &StoredColumn) -> &str {
    c.kind.as_str()
}

fn numeric(c: &StoredColumn) -> bool {
    kinds::is_numeric(kind_of(c))
}

/// 单元格值 → 文本（空白为 None）
fn cell_text(v: &Json) -> Option<String> {
    let s = match v {
        Json::Null => return None,
        Json::String(s) => s.trim().to_string(),
        Json::Number(n) => n.to_string(),
        Json::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        other => other.to_string(),
    };
    (!s.is_empty()).then_some(s)
}

/// 数字：去掉千分位、货币符号、「元」
fn parse_number(s: &str) -> Option<f64> {
    let cleaned: String = s.chars().filter(|c| !matches!(c, ',' | '，' | '¥' | '￥' | '元' | ' ')).collect();
    cleaned.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// 日期 → 统一成 2026-10-06 00:00:00（与导入一致）
fn parse_datetime(s: &str) -> Option<String> {
    let t = s.replace(['年', '月'], "-").replace('日', "").replace('/', "-");
    let t = t.trim();
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(t, f) {
            return Some(d.format("%Y-%m-%d %H:%M:%S").to_string());
        }
    }
    for f in ["%Y-%m-%d", "%Y%m%d"] {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(t, f) {
            return Some(format!("{} 00:00:00", d.format("%Y-%m-%d")));
        }
    }
    None
}

/// 可写入的数据集：(名称, 列, 主键)
fn target(conn: &Connection, id: &str) -> AppResult<(String, Vec<StoredColumn>, Vec<String>)> {
    let def = dataset::dataset(conn, id)?;
    match def.kind.as_str() {
        "view" => return Err(AppError::bad(format!("「{}」是关联视图，不能写入；请填报到它关联的原始数据集", def.name))),
        "recon" => return Err(AppError::bad(format!("「{}」是对账结果，不能写入；勾对请在对账页面操作", def.name))),
        "summary" => return Err(AppError::bad(format!("「{}」是透视表转成的汇总数据集，不能写入；请填报到它的源数据集", def.name))),
        "builtin" => return Err(AppError::bad(format!("「{}」是内置数据集，由专门的导入流程维护，不能填报写入", def.name))),
        "external" => return Err(AppError::bad(format!("「{}」是外部数据集（直接读取 Parquet 文件），只读，不能写入", def.name))),
        _ => {}
    }
    let (_, name, cols) = dataset::stored(conn)?.into_iter().find(|(i, _, _)| i == id).ok_or_else(|| AppError::bad("数据集不存在"))?;
    // 数据连接整体替换的数据集：写入的数据会在下次同步时被覆盖
    if let Some(c) = crate::connector::replacing_connector(conn, id)? {
        return Err(AppError::bad(format!("「{name}」由数据连接「{c}」每次整体替换，填报的数据会在下次同步时丢失，不能作为填报目标")));
    }
    Ok((name, cols, def.primary_key))
}

fn search_expr(cols: &[StoredColumn], prefix: &str) -> String {
    let text: Vec<String> = cols.iter().filter(|c| kinds::is_textual(kind_of(c))).map(|c| format!("{prefix}{}", ident(&c.title))).collect();
    if text.is_empty() { "NULL".into() } else { format!("lower(concat_ws(' ', {}))", text.join(", ")) }
}

fn to_value(c: &StoredColumn, v: &Json) -> Value {
    match v {
        Json::Null => Value::Null,
        Json::Number(n) if kind_of(c) == "integer" => n.as_i64().map(Value::BigInt).or_else(|| n.as_f64().map(Value::Double)).unwrap_or(Value::Null),
        Json::Number(n) if numeric(c) => n.as_f64().map(Value::Double).unwrap_or(Value::Null),
        Json::Bool(b) if kind_of(c) == "boolean" => Value::Boolean(*b),
        Json::String(s) => Value::Text(s.clone()),
        other => Value::Text(other.to_string()),
    }
}

/// 一个单元格按列类型取值（撤销快照用：整数不经过浮点，是否列为布尔值）
fn cell_json(r: &duckdb::Row, i: usize, c: &StoredColumn) -> duckdb::Result<Json> {
    Ok(match kind_of(c) {
        "integer" => r.get::<_, Option<i64>>(i)?.map(Json::from),
        "boolean" => r.get::<_, Option<bool>>(i)?.map(Json::Bool),
        _ if numeric(c) => r.get::<_, Option<f64>>(i)?.map(Json::from),
        _ => r.get::<_, Option<String>>(i)?.map(Json::String),
    }
    .unwrap_or(Json::Null))
}

/// 读出若干行（数据集全部列）作为 JSON
fn read_rows(conn: &Connection, table: &str, cols: &[StoredColumn], where_sql: &str) -> AppResult<Vec<Vec<Json>>> {
    let sel = cols.iter().map(|c| ident(&c.title)).collect::<Vec<_>>().join(", ");
    let mut stmt = conn.prepare(&format!("SELECT {sel} FROM {table} d WHERE {where_sql}"))?;
    let mut rs = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rs.next()? {
        let row = cols
            .iter()
            .enumerate()
            .map(|(i, c)| cell_json(r, i, c))
            .collect::<duckdb::Result<Vec<_>>>()?;
        out.push(row);
    }
    Ok(out)
}

/// 把整行（数据集全部列）插入数据集
fn insert_full(conn: &Connection, table: &str, cols: &[StoredColumn], rows: &[Vec<Json>]) -> AppResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let list = cols.iter().map(|c| ident(&c.title)).collect::<Vec<_>>().join(", ");
    let ph = vec!["?"; cols.len()].join(", ");
    let mut stmt = conn.prepare(&format!("INSERT INTO {table} ({list}) VALUES ({ph})"))?;
    for r in rows {
        stmt.execute(params_from_iter(cols.iter().zip(r).map(|(c, v)| to_value(c, v))))?;
    }
    Ok(())
}

/// 重算搜索文本（where_sql 中用 d 指代数据集表）
fn refresh_search(conn: &Connection, table: &str, cols: &[StoredColumn], where_sql: &str) -> AppResult<()> {
    conn.execute(&format!("UPDATE {table} AS d SET search_text = {} WHERE {where_sql}", search_expr(cols, "")), [])?;
    Ok(())
}

fn stamp(conn: &Connection, id: &str) -> AppResult<String> {
    Ok(conn.query_row("SELECT COALESCE(strftime(updated_at, '%Y-%m-%d %H:%M:%S.%f'), '') FROM ledger_dataset WHERE id = ?", params![id], |r| r.get(0))?)
}

fn touch(conn: &Connection, id: &str) -> AppResult<String> {
    // 精确到微秒：撤销时据此判断提交之后数据集有没有别的改动
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    conn.execute("UPDATE ledger_dataset SET updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now, id])?;
    stamp(conn, id)
}

/// 提交的主键对上了已有的、但不在 shops 范围内的行（返回提交里的行下标）
fn foreign_rows(conn: &Connection, table: &str, given: &[&StoredColumn], keys: &[String], values: &[Vec<Json>], scope_title: &str, shops: &[String]) -> AppResult<Vec<usize>> {
    let key_cols: Vec<(usize, &StoredColumn)> = keys.iter().filter_map(|k| given.iter().position(|g| &g.title == k).map(|i| (i, given[i]))).collect();
    let defs = key_cols.iter().map(|(_, c)| format!("{} {}", ident(&c.title), kinds::sql_type(kind_of(c)))).collect::<Vec<_>>().join(", ");
    conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE wb_chk (__line BIGINT, {defs})"))?;
    let r = (|| -> AppResult<Vec<usize>> {
        let ph = vec!["?"; key_cols.len() + 1].join(", ");
        let mut stmt = conn.prepare(&format!("INSERT INTO wb_chk VALUES ({ph})"))?;
        for (ri, row) in values.iter().enumerate() {
            let mut args = vec![Value::BigInt(ri as i64)];
            args.extend(key_cols.iter().map(|(i, c)| to_value(c, &row[*i])));
            stmt.execute(params_from_iter(args))?;
        }
        let on = keys.iter().map(|k| format!("d.{k} IS NOT DISTINCT FROM n.{k}", k = ident(k))).collect::<Vec<_>>().join(" AND ");
        let list = if shops.is_empty() { "NULL".to_string() } else { shops.iter().map(|s| literal(s)).collect::<Vec<_>>().join(", ") };
        let mut q = conn.prepare(&format!("SELECT DISTINCT n.__line FROM wb_chk n JOIN {table} d ON {on} WHERE COALESCE(CAST(d.{} AS VARCHAR), '') NOT IN ({list}) ORDER BY 1", ident(scope_title)))?;
        let rows = q.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|i| i as usize).collect())
    })();
    let _ = conn.execute_batch("DROP TABLE IF EXISTS wb_chk");
    r
}

pub fn submit(conn: &Connection, dataset_id: &str, req: &WritebackRequest) -> AppResult<WritebackOutcome> {
    let started = Instant::now();
    let (name, cols, registered_pk) = target(conn, dataset_id)?;
    // delete 与 upsert 一样按主键定位行
    let (upsert, delete) = match req.mode.as_str() {
        "upsert" => (true, false),
        "append" => (false, false),
        "delete" => (true, true),
        other => return Err(AppError::bad(format!("未知的提交方式：{other}"))),
    };
    // 数据范围受限的用户：只能填自己范围内的行（范围列必须在填报区里）
    let limit = crate::scope::write_limit(conn, dataset_id)?
        .map(|(key, shops)| -> AppResult<(String, Vec<String>)> {
            let title = cols.iter().find(|c| c.key == key).map(|c| c.title.clone()).ok_or_else(|| AppError::Forbidden("数据集的范围列不存在".into()))?;
            Ok((title, shops))
        })
        .transpose()?;
    if let Some((title, _)) = &limit
        && !delete
        && !req.columns.contains(title)
    {
        return Err(AppError::bad(format!("你的数据范围受限：填报区必须包含「{title}」列，并且只能填自己范围内的值")));
    }
    if req.rows.len() > MAX_ROWS {
        return Err(AppError::bad(format!("一次最多提交 {MAX_ROWS} 行，当前 {} 行，请分批提交", req.rows.len())));
    }
    // 提交的列
    let mut seen = HashSet::new();
    let mut given: Vec<&StoredColumn> = Vec::new();
    for t in &req.columns {
        let c = cols.iter().find(|c| &c.title == t).ok_or_else(|| AppError::bad(format!("数据集「{name}」没有「{t}」列（计算列不能填写）")))?;
        if !seen.insert(t.as_str()) {
            return Err(AppError::bad(format!("「{t}」列重复")));
        }
        given.push(c);
    }
    if given.is_empty() {
        return Err(AppError::bad("填报区没有任何列"));
    }
    let keys: Vec<String> = if !upsert {
        vec![]
    } else if !registered_pk.is_empty() {
        registered_pk.clone()
    } else {
        req.keys.clone()
    };
    if upsert && keys.is_empty() {
        return Err(AppError::bad(format!("数据集「{name}」没有主键，按主键更新时请指定主键列")));
    }
    for k in &keys {
        if !req.columns.contains(k) {
            return Err(AppError::bad(format!("主键列「{k}」必须放在填报区里")));
        }
    }

    // ── 校验与转换 ──
    let mut errors: Vec<CellError> = Vec::new();
    let mut values: Vec<Vec<Json>> = Vec::with_capacity(req.rows.len());
    let key_idx: Vec<usize> = keys.iter().filter_map(|k| req.columns.iter().position(|c| c == k)).collect();
    let mut key_seen: HashMap<Vec<String>, usize> = HashMap::new();
    for (ri, row) in req.rows.iter().enumerate() {
        let mut out = Vec::with_capacity(given.len());
        for (ci, c) in given.iter().enumerate() {
            // 删除只看主键列，其他列的内容不检查
            if delete && !keys.contains(&c.title) {
                out.push(Json::Null);
                continue;
            }
            let text = row.get(ci).and_then(cell_text);
            let v = match (text, kind_of(c)) {
                (None, _) => Json::Null,
                (Some(s), "integer") => match kinds::parse_integer(&s) {
                    Some(n) => Json::from(n),
                    None => {
                        errors.push(CellError { row: ri, column: c.title.clone(), message: format!("「{s}」不是有效的整数") });
                        Json::Null
                    }
                },
                (Some(s), "boolean") => match kinds::parse_bool(&s) {
                    Some(b) => Json::Bool(b),
                    None => {
                        errors.push(CellError { row: ri, column: c.title.clone(), message: format!("「{s}」不是有效的是 / 否，请写 TRUE / FALSE 或 是 / 否") });
                        Json::Null
                    }
                },
                (Some(s), "number" | "money") => match parse_number(&s) {
                    Some(n) => Json::from(n),
                    None => {
                        errors.push(CellError { row: ri, column: c.title.clone(), message: format!("「{s}」不是有效的数字") });
                        Json::Null
                    }
                },
                (Some(s), "datetime") => match parse_datetime(&s) {
                    Some(d) => Json::String(d),
                    None => {
                        errors.push(CellError { row: ri, column: c.title.clone(), message: format!("无法识别的日期「{s}」，请写成 2026-10-06 或 2026-10-06 14:30 这样的格式") });
                        Json::Null
                    }
                },
                (Some(s), _) => Json::String(s),
            };
            out.push(v);
        }
        if let Some((title, shops)) = &limit
            && !delete
            && let Some(i) = given.iter().position(|c| &c.title == title)
        {
            match cell_text(&out[i]) {
                None => errors.push(CellError { row: ri, column: title.clone(), message: format!("「{title}」不能为空：只能填你的数据范围内的值") }),
                Some(v) if !shops.contains(&v) => errors.push(CellError { row: ri, column: title.clone(), message: format!("「{v}」不在你的数据范围内（你只能填：{}）", if shops.is_empty() { "没有任何店铺".into() } else { shops.join("、") }) }),
                Some(_) => {}
            }
        }
        if upsert {
            let key: Vec<String> = key_idx.iter().map(|&i| cell_text(&out[i]).unwrap_or_default()).collect();
            for (&i, k) in key_idx.iter().zip(&key) {
                if k.is_empty() {
                    errors.push(CellError { row: ri, column: req.columns[i].clone(), message: "主键不能为空".into() });
                }
            }
            if key.iter().all(|k| !k.is_empty()) {
                if let Some(first) = key_seen.get(&key) {
                    errors.push(CellError { row: ri, column: keys[0].clone(), message: format!("与第 {} 行主键重复（{}）", first + 1, key.join(" / ")) });
                } else {
                    key_seen.insert(key, ri);
                }
            }
        }
        values.push(out);
    }
    if !errors.is_empty() || values.is_empty() {
        let error_count = errors.len();
        errors.truncate(MAX_ERRORS);
        if values.is_empty() && errors.is_empty() {
            return Err(AppError::bad("没有要提交的数据"));
        }
        return Ok(WritebackOutcome { batch_id: None, dataset_name: name, inserted: 0, updated: 0, unchanged: 0, deleted: 0, not_found: 0, errors, error_count, elapsed_ms: started.elapsed().as_millis() as i64 });
    }
    // 受限用户不能改 / 删范围外的行：主键对上了已有的、但属于别人范围的行
    if let Some((title, shops)) = &limit
        && upsert
    {
        let foreign = foreign_rows(conn, dataset_id, &given, &keys, &values, title, shops)?;
        if !foreign.is_empty() {
            let error_count = foreign.len();
            let errors = foreign
                .into_iter()
                .take(MAX_ERRORS)
                .map(|ri| CellError { row: ri, column: keys[0].clone(), message: "这个主键对应的已有数据不在你的数据范围内，不能修改或删除".into() })
                .collect();
            return Ok(WritebackOutcome { batch_id: None, dataset_name: name, inserted: 0, updated: 0, unchanged: 0, deleted: 0, not_found: 0, errors, error_count, elapsed_ms: started.elapsed().as_millis() as i64 });
        }
    }

    // ── 写入 ──
    let table = dataset_id.to_string();
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<(i64, i64, i64, i64, Undo)> {
        let defs = given.iter().map(|c| format!("{} {}", ident(&c.title), kinds::sql_type(kind_of(c)))).collect::<Vec<_>>().join(", ");
        conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE wb_in (__line BIGINT, {defs})"))?;
        {
            let ph = vec!["?"; given.len() + 1].join(", ");
            let mut stmt = conn.prepare(&format!("INSERT INTO wb_in VALUES ({ph})"))?;
            for (i, row) in values.iter().enumerate() {
                let mut args = vec![Value::BigInt(i as i64)];
                args.extend(given.iter().zip(row).map(|(c, v)| to_value(c, v)));
                stmt.execute(params_from_iter(args))?;
            }
        }
        let list = given.iter().map(|c| ident(&c.title)).collect::<Vec<_>>().join(", ");
        let n_list = given.iter().map(|c| format!("n.{}", ident(&c.title))).collect::<Vec<_>>().join(", ");
        let all_cols: Vec<String> = cols.iter().map(|c| c.title.clone()).collect();
        let mut undo = Undo { columns: all_cols, keys: keys.clone(), ..Default::default() };
        if delete {
            let key_match = keys.iter().map(|k| format!("d.{k} IS NOT DISTINCT FROM n.{k}", k = ident(k))).collect::<Vec<_>>().join(" AND ");
            let exists = format!("EXISTS (SELECT 1 FROM wb_in n WHERE {key_match})");
            undo.removed = read_rows(conn, &table, &cols, &exists)?;
            let n = conn.execute(&format!("DELETE FROM {table} AS d WHERE {exists}"), [])? as i64;
            return Ok((0, 0, 0, n, undo));
        }
        if !upsert {
            let n = conn.execute(&format!("INSERT INTO {table} ({list}, search_text) SELECT {n_list}, {} FROM wb_in n ORDER BY n.__line", search_expr(&given.iter().map(|c| (*c).clone()).collect::<Vec<_>>(), "n.")), [])? as i64;
            // 追加的行：数据集全部列，未提交的列为空
            undo.inserted = values
                .iter()
                .map(|row| cols.iter().map(|c| given.iter().position(|g| g.title == c.title).map(|i| row[i].clone()).unwrap_or(Json::Null)).collect())
                .collect();
            return Ok((n, 0, 0, 0, undo));
        }
        let key_match = keys.iter().map(|k| format!("d.{k} IS NOT DISTINCT FROM n.{k}", k = ident(k))).collect::<Vec<_>>().join(" AND ");
        let same = given.iter().map(|c| format!("d.{k} IS NOT DISTINCT FROM n.{k}", k = ident(&c.title))).collect::<Vec<_>>().join(" AND ");
        // 与已有行完全相同的不动
        let total = values.len() as i64;
        conn.execute(&format!("DELETE FROM wb_in n WHERE EXISTS (SELECT 1 FROM {table} d WHERE {key_match} AND {same})"), [])?;
        let changed: i64 = conn.query_row("SELECT count(*) FROM wb_in", [], |r| r.get(0))?;
        let exists = format!("EXISTS (SELECT 1 FROM wb_in n WHERE {key_match})");
        undo.before = read_rows(conn, &table, &cols, &exists)?;
        let set = given.iter().filter(|c| !keys.contains(&c.title)).map(|c| format!("{k} = n.{k}", k = ident(&c.title))).collect::<Vec<_>>();
        let updated = if set.is_empty() {
            0
        } else {
            let n = conn.execute(&format!("UPDATE {table} AS d SET {} FROM wb_in n WHERE {key_match}", set.join(", ")), [])? as i64;
            refresh_search(conn, &table, &cols, &format!("EXISTS (SELECT 1 FROM wb_in n WHERE {key_match})"))?;
            n
        };
        let inserted = conn.execute(
            &format!("INSERT INTO {table} ({list}) SELECT {n_list} FROM wb_in n WHERE NOT EXISTS (SELECT 1 FROM {table} d WHERE {key_match}) ORDER BY n.__line"),
            [],
        )? as i64;
        if inserted > 0 {
            // 新增行的搜索文本（含未提交的列：为空）
            refresh_search(conn, &table, &cols, "search_text IS NULL")?;
            let key_given: Vec<usize> = keys.iter().filter_map(|k| given.iter().position(|g| &g.title == k)).collect();
            let before_keys: HashSet<Vec<String>> = undo
                .before
                .iter()
                .map(|r| keys.iter().map(|k| cols.iter().position(|c| &c.title == k).and_then(|i| cell_text(&r[i])).unwrap_or_default()).collect())
                .collect();
            let mut stmt = conn.prepare("SELECT * EXCLUDE (__line) FROM wb_in ORDER BY __line")?;
            let mut rs = stmt.query([])?;
            while let Some(row) = rs.next()? {
                let vals: Vec<Json> = given
                    .iter()
                    .enumerate()
                    .map(|(i, c)| cell_json(row, i, c))
                    .collect::<duckdb::Result<_>>()?;
                let key: Vec<String> = key_given.iter().map(|&i| cell_text(&vals[i]).unwrap_or_default()).collect();
                if !before_keys.contains(&key) {
                    undo.inserted.push(cols.iter().map(|c| given.iter().position(|g| g.title == c.title).map(|i| vals[i].clone()).unwrap_or(Json::Null)).collect());
                }
            }
        }
        Ok((inserted, if set.is_empty() { 0 } else { updated }, total - changed, 0, undo))
    })();
    let _ = conn.execute_batch("DROP TABLE IF EXISTS wb_in");
    let (inserted, updated, unchanged, deleted, undo) = match r {
        Ok(v) => v,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    };
    let batch_id = format!("wb_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    let saved = (|| -> AppResult<()> {
        let st = if inserted + updated + deleted > 0 { touch(conn, dataset_id)? } else { stamp(conn, dataset_id)? };
        conn.execute(
            "INSERT INTO ledger_writeback (id, dataset_id, mode, operator, created_at, inserted, updated, unchanged, source, undo, stamp, deleted)
             VALUES (?, ?, ?, ?, CAST(? AS TIMESTAMP), ?, ?, ?, ?, ?, ?, ?)",
            params![
                batch_id,
                dataset_id,
                req.mode,
                req.operator.trim(),
                now_local(),
                inserted,
                updated,
                unchanged,
                req.source.as_ref().map(|s| s.to_string()),
                serde_json::to_string(&undo)?,
                st,
                deleted
            ],
        )?;
        Ok(())
    })();
    if let Err(e) = saved {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    conn.execute_batch("COMMIT")?;
    Ok(WritebackOutcome {
        batch_id: Some(batch_id),
        dataset_name: name,
        inserted,
        updated,
        unchanged,
        deleted,
        not_found: if delete { values.len() as i64 - deleted } else { 0 },
        errors: vec![],
        error_count: 0,
        elapsed_ms: started.elapsed().as_millis() as i64,
    })
}

struct Stored {
    id: String,
    dataset_id: String,
    mode: String,
    operator: String,
    created_at: String,
    inserted: i64,
    updated: i64,
    unchanged: i64,
    source: Option<String>,
    stamp: String,
    undone_at: Option<String>,
    undone_by: Option<String>,
    deleted: i64,
}

const COLS: &str = "id, dataset_id, mode, COALESCE(operator, ''), strftime(created_at, '%Y-%m-%d %H:%M:%S'), inserted, updated, unchanged, source, COALESCE(stamp, ''), strftime(undone_at, '%Y-%m-%d %H:%M:%S'), undone_by, COALESCE(deleted, 0)";

fn from_row(r: &duckdb::Row<'_>) -> duckdb::Result<Stored> {
    Ok(Stored {
        id: r.get(0)?,
        dataset_id: r.get(1)?,
        mode: r.get(2)?,
        operator: r.get(3)?,
        created_at: r.get(4)?,
        inserted: r.get(5)?,
        updated: r.get(6)?,
        unchanged: r.get(7)?,
        source: r.get(8)?,
        stamp: r.get(9)?,
        undone_at: r.get(10)?,
        undone_by: r.get(11)?,
        deleted: r.get(12)?,
    })
}

/// 不能撤销的原因；None 表示可以撤销
fn undo_blocked(conn: &Connection, s: &Stored) -> AppResult<Option<String>> {
    if s.undone_at.is_some() {
        return Ok(Some("已撤销".into()));
    }
    if s.inserted + s.updated + s.deleted == 0 {
        return Ok(Some("这次提交没有改动数据".into()));
    }
    let alive: Option<String> = conn
        .query_row("SELECT COALESCE(strftime(updated_at, '%Y-%m-%d %H:%M:%S.%f'), '') FROM ledger_dataset WHERE id = ? AND deleted_at IS NULL", params![s.dataset_id], |r| r.get(0))
        .ok();
    match alive {
        None => Ok(Some("数据集已删除或在回收站".into())),
        Some(now) if now != s.stamp => Ok(Some("这次提交之后数据集又有改动（导入、同步或其他提交），不能撤销".into())),
        Some(_) => Ok(None),
    }
}

pub fn list(conn: &Connection, dataset_id: Option<&str>, limit: i64) -> AppResult<Vec<WritebackRecord>> {
    let names: HashMap<String, String> = dataset::stored(conn)?.into_iter().map(|(id, n, _)| (id, n)).collect();
    let (sql, args): (String, Vec<Value>) = match dataset_id {
        Some(id) => (format!("SELECT {COLS} FROM ledger_writeback WHERE dataset_id = ? ORDER BY created_at DESC, id DESC LIMIT ?"), vec![Value::Text(id.into()), Value::BigInt(limit)]),
        None => (format!("SELECT {COLS} FROM ledger_writeback ORDER BY created_at DESC, id DESC LIMIT ?"), vec![Value::BigInt(limit)]),
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), from_row)?.collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|s| {
            let blocked = undo_blocked(conn, &s)?;
            Ok(WritebackRecord {
                dataset_name: names.get(&s.dataset_id).cloned(),
                can_undo: blocked.is_none(),
                undo_blocked: blocked,
                source: s.source.as_deref().and_then(|x| serde_json::from_str(x).ok()),
                id: s.id,
                dataset_id: s.dataset_id,
                mode: s.mode,
                operator: s.operator,
                created_at: s.created_at,
                inserted: s.inserted,
                updated: s.updated,
                unchanged: s.unchanged,
                deleted: s.deleted,
                undone_at: s.undone_at,
                undone_by: s.undone_by,
            })
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    pub dataset_id: String,
    pub dataset_name: String,
    /// 删除的新增行
    pub removed: usize,
    /// 恢复的被更新行
    pub restored: usize,
    /// 重新插回的被删除行
    pub reinserted: usize,
}

pub fn undo(conn: &Connection, batch_id: &str, operator: &str) -> AppResult<UndoOutcome> {
    let s = conn.query_row(&format!("SELECT {COLS} FROM ledger_writeback WHERE id = ?"), params![batch_id], from_row).map_err(|_| AppError::bad("提交记录不存在"))?;
    if let Some(why) = undo_blocked(conn, &s)? {
        return Err(AppError::bad(format!("不能撤销：{why}")));
    }
    let (name, cols, _) = target(conn, &s.dataset_id)?;
    let undo: Undo = conn.query_row("SELECT undo FROM ledger_writeback WHERE id = ?", params![batch_id], |r| r.get::<_, String>(0)).map_err(AppError::from).and_then(|j| Ok(serde_json::from_str(&j)?))?;
    if undo.columns != cols.iter().map(|c| c.title.clone()).collect::<Vec<_>>() {
        return Err(AppError::bad("数据集的列在提交之后有变化，不能撤销"));
    }
    let table = s.dataset_id.clone();
    let tuple_where = |cols_idx: &[usize], row: &[Json], args: &mut Vec<Value>| -> String {
        cols_idx
            .iter()
            .map(|&i| {
                let c = &cols[i];
                match to_value(c, &row[i]) {
                    Value::Null => format!("{} IS NULL", ident(&c.title)),
                    v => {
                        args.push(v);
                        format!("{} = ?", ident(&c.title))
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<(usize, usize, usize)> {
        if s.mode == "delete" {
            // 删除的行原样插回（提交之后数据集没有别的改动，所以不会有主键冲突）
            insert_full(conn, &table, &cols, &undo.removed)?;
        } else if s.mode == "upsert" {
            let key_idx: Vec<usize> = undo.keys.iter().filter_map(|k| cols.iter().position(|c| &c.title == k)).collect();
            // 新增的行删掉；被更新的行先删掉现状再插回提交前的样子
            for row in undo.inserted.iter().chain(&undo.before) {
                let mut args = vec![];
                let w = tuple_where(&key_idx, row, &mut args);
                conn.execute(&format!("DELETE FROM {table} WHERE {w}"), params_from_iter(args))?;
            }
            insert_full(conn, &table, &cols, &undo.before)?;
        } else {
            // 追加：按整行内容删除；数据集里原本就有相同内容的行时只删掉追加的份数
            let all: Vec<usize> = (0..cols.len()).collect();
            let mut groups: Vec<(&Vec<Json>, usize)> = Vec::new();
            for row in &undo.inserted {
                match groups.iter_mut().find(|(r, _)| *r == row) {
                    Some(g) => g.1 += 1,
                    None => groups.push((row, 1)),
                }
            }
            for (row, n) in groups {
                let mut args = vec![];
                let w = tuple_where(&all, row, &mut args);
                let existing: i64 = conn.query_row(&format!("SELECT count(*) FROM {table} WHERE {w}"), params_from_iter(args.clone()), |r| r.get(0))?;
                conn.execute(&format!("DELETE FROM {table} WHERE {w}"), params_from_iter(args))?;
                let keep = (existing as usize).saturating_sub(n);
                insert_full(conn, &table, &cols, &vec![row.clone(); keep])?;
            }
        }
        refresh_search(conn, &table, &cols, "search_text IS NULL")?;
        touch(conn, &table)?;
        conn.execute("UPDATE ledger_writeback SET undone_at = CAST(? AS TIMESTAMP), undone_by = ? WHERE id = ?", params![now_local(), operator.trim(), batch_id])?;
        Ok((undo.inserted.len(), undo.before.len(), undo.removed.len()))
    })();
    match r {
        Ok((removed, restored, reinserted)) => {
            conn.execute_batch("COMMIT")?;
            Ok(UndoOutcome { dataset_id: s.dataset_id, dataset_name: name, removed, restored, reinserted })
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewInputDataset {
    pub name: String,
    pub columns: Vec<NewColumn>,
    #[serde(default)]
    pub primary_key: Vec<String>,
}

#[derive(Deserialize)]
pub struct NewColumn {
    pub title: String,
    pub kind: String,
}

/// 新建一个空的填报数据集
pub fn create_dataset(conn: &Connection, input: &NewInputDataset) -> AppResult<String> {
    let cols: Vec<(String, String)> = input.columns.iter().map(|c| (c.title.clone(), c.kind.clone())).collect();
    crate::dataset_import::create_empty(conn, &input.name, &cols, &input.primary_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        conn
    }

    fn req(mode: &str, columns: &[&str], rows: Vec<Vec<Json>>) -> WritebackRequest {
        WritebackRequest { mode: mode.into(), columns: columns.iter().map(|s| s.to_string()).collect(), rows, keys: vec![], operator: "测试".into(), source: None }
    }

    fn table(conn: &Connection, id: &str) -> Vec<(String, String, Option<f64>, Option<String>)> {
        let mut st = conn.prepare(&format!("SELECT \"店铺\", \"月份\", \"预算\", \"备注\" FROM {id} ORDER BY 1, 2")).unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
    }

    #[test]
    fn parse_values() {
        assert_eq!(parse_number("¥1,234.50"), Some(1234.5));
        assert_eq!(parse_number("12元"), Some(12.0));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_datetime("2026/10/6"), Some("2026-10-06 00:00:00".into()));
        assert_eq!(parse_datetime("2026年10月6日"), Some("2026-10-06 00:00:00".into()));
        assert_eq!(parse_datetime("2026-10-06 14:30"), Some("2026-10-06 14:30:00".into()));
        assert_eq!(parse_datetime("下周一"), None);
    }

    #[test]
    fn upsert_append_and_undo() {
        let conn = setup();
        let input = NewInputDataset {
            name: "预算".into(),
            columns: vec![
                NewColumn { title: "店铺".into(), kind: "text".into() },
                NewColumn { title: "月份".into(), kind: "text".into() },
                NewColumn { title: "预算".into(), kind: "money".into() },
                NewColumn { title: "备注".into(), kind: "text".into() },
            ],
            primary_key: vec!["店铺".into(), "月份".into()],
        };
        let id = create_dataset(&conn, &input).unwrap();

        // 校验：数字错误、主键为空、主键重复，整批不写入
        let bad = submit(
            &conn,
            &id,
            &req("upsert", &["店铺", "月份", "预算"], vec![
                vec![Json::from("旗舰店"), Json::from("2026-10"), Json::from("十万")],
                vec![Json::Null, Json::from("2026-10"), Json::from(5)],
                vec![Json::from("旗舰店"), Json::from("2026-10"), Json::from(6)],
            ]),
        )
        .unwrap();
        assert!(bad.batch_id.is_none());
        assert_eq!(bad.error_count, 3, "{:?}", bad.errors);
        assert!(table(&conn, &id).is_empty());

        // 首次提交：新增 2 行
        let r1 = submit(
            &conn,
            &id,
            &req("upsert", &["店铺", "月份", "预算", "备注"], vec![
                vec![Json::from("旗舰店"), Json::from("2026-10"), Json::from("100,000"), Json::from("初稿")],
                vec![Json::from("专卖店"), Json::from("2026-10"), Json::from(50000), Json::Null],
            ]),
        )
        .unwrap();
        assert_eq!((r1.inserted, r1.updated, r1.unchanged), (2, 0, 0));

        // 只提交预算列：更新 1 行、新增 1 行、1 行不变；备注保持原值
        let r2 = submit(
            &conn,
            &id,
            &req("upsert", &["店铺", "月份", "预算"], vec![
                vec![Json::from("旗舰店"), Json::from("2026-10"), Json::from(120000)],
                vec![Json::from("专卖店"), Json::from("2026-10"), Json::from(50000)],
                vec![Json::from("折扣店"), Json::from("2026-10"), Json::from(30000)],
            ]),
        )
        .unwrap();
        assert_eq!((r2.inserted, r2.updated, r2.unchanged), (1, 1, 1));
        let rows = table(&conn, &id);
        assert!(rows.contains(&("旗舰店".into(), "2026-10".into(), Some(120000.0), Some("初稿".into()))));
        assert_eq!(rows.len(), 3);
        // 搜索文本随更新重算
        let st: String = conn.query_row(&format!("SELECT search_text FROM {id} WHERE \"店铺\" = '折扣店'"), [], |r| r.get(0)).unwrap();
        assert!(st.contains("折扣店"));

        // 第一次提交已不是最后一次改动：不能撤销
        let recs = list(&conn, Some(&id), 10).unwrap();
        assert_eq!(recs.len(), 2);
        assert!(recs[0].can_undo && !recs[1].can_undo, "{:?}", recs.iter().map(|r| &r.undo_blocked).collect::<Vec<_>>());

        // 撤销第二次：恢复到第一次提交后的样子
        let u = undo(&conn, r2.batch_id.as_deref().unwrap(), "测试").unwrap();
        assert_eq!((u.removed, u.restored), (1, 1));
        let rows = table(&conn, &id);
        assert_eq!(rows, vec![("专卖店".into(), "2026-10".into(), Some(50000.0), None), ("旗舰店".into(), "2026-10".into(), Some(100000.0), Some("初稿".into()))]);
        assert!(undo(&conn, r2.batch_id.as_deref().unwrap(), "测试").is_err());
        // 撤销本身也算一次改动：第一次提交仍不能撤销
        assert!(!list(&conn, Some(&id), 10).unwrap()[1].can_undo);

        // 追加：与已有内容相同的行，撤销时只删追加的份数
        let a = submit(&conn, &id, &req("append", &["店铺", "月份", "预算"], vec![vec![Json::from("专卖店"), Json::from("2026-10"), Json::from(50000)], vec![Json::from("海外店"), Json::from("2026-11"), Json::from(1)]])).unwrap();
        assert_eq!(a.inserted, 2);
        assert_eq!(table(&conn, &id).len(), 4);
        undo(&conn, a.batch_id.as_deref().unwrap(), "测试").unwrap();
        assert_eq!(table(&conn, &id), rows);
    }

    #[test]
    fn rejects_views_and_unknown_columns() {
        let conn = setup();
        let id = create_dataset(&conn, &NewInputDataset { name: "调整".into(), columns: vec![NewColumn { title: "科目".into(), kind: "text".into() }, NewColumn { title: "金额".into(), kind: "money".into() }], primary_key: vec![] }).unwrap();
        let e = submit(&conn, &id, &req("upsert", &["科目", "金额"], vec![vec![Json::from("运费"), Json::from(1)]])).unwrap_err();
        assert!(e.message().contains("没有主键"), "{}", e.message());
        let e = submit(&conn, &id, &req("append", &["科目", "不存在"], vec![vec![Json::from("运费"), Json::from(1)]])).unwrap_err();
        assert!(e.message().contains("没有「不存在」列"));
        let e = submit(&conn, "ledger_detail", &req("append", &["店铺"], vec![vec![Json::from("x")]])).unwrap_err();
        assert!(e.message().contains("内置数据集"), "{}", e.message());
    }

    fn budget(conn: &Connection, name: &str, pk: &[&str]) -> String {
        let cols = [("店铺", "text"), ("月份", "text"), ("预算", "money"), ("备注", "text")];
        create_dataset(conn, &NewInputDataset { name: name.into(), columns: cols.iter().map(|(t, k)| NewColumn { title: (*t).into(), kind: (*k).into() }).collect(), primary_key: pk.iter().map(|s| s.to_string()).collect() }).unwrap()
    }

    fn row(shop: &str, month: &str, v: i64) -> Vec<Json> {
        vec![Json::from(shop), Json::from(month), Json::from(v)]
    }

    #[test]
    fn delete_by_key_and_undo() {
        let conn = setup();
        let id = budget(&conn, "预算", &["店铺", "月份"]);
        submit(&conn, &id, &req("upsert", &["店铺", "月份", "预算"], vec![row("A", "2026-10", 1), row("B", "2026-10", 2), row("C", "2026-10", 3)])).unwrap();
        // 删除只看主键列；找不到的主键报告个数；整批有主键重复 / 为空时一行都不删
        let bad = submit(&conn, &id, &req("delete", &["店铺", "月份"], vec![vec![Json::from("A"), Json::from("2026-10")], vec![Json::from("A"), Json::from("2026-10")]])).unwrap();
        assert!(bad.batch_id.is_none() && bad.error_count == 1, "{:?}", bad.errors);
        let none = submit(&conn, &id, &req("delete", &["店铺", "月份"], vec![vec![Json::from("A"), Json::Null]])).unwrap();
        assert_eq!(none.error_count, 1);
        assert_eq!(table(&conn, &id).len(), 3);
        let d = submit(&conn, &id, &req("delete", &["店铺", "月份", "预算"], vec![vec![Json::from("A"), Json::from("2026-10"), Json::from("随便写")], vec![Json::from("Z"), Json::from("2026-10"), Json::Null]])).unwrap();
        assert_eq!((d.deleted, d.not_found, d.inserted, d.updated), (1, 1, 0, 0), "预算列在删除时不检查：{:?}", d.errors);
        assert_eq!(table(&conn, &id).iter().map(|r| r.0.clone()).collect::<Vec<_>>(), vec!["B", "C"]);
        // 记录里有「删除」，可撤销，撤销把原行（含未提交的列）插回
        let rec = &list(&conn, Some(&id), 5).unwrap()[0];
        assert_eq!((rec.mode.as_str(), rec.deleted, rec.can_undo), ("delete", 1, true));
        let u = undo(&conn, d.batch_id.as_deref().unwrap(), "测试").unwrap();
        assert_eq!(u.reinserted, 1);
        assert_eq!(table(&conn, &id).len(), 3);
        // 删除没有主键的数据集：要求指定主键
        let plain = create_dataset(&conn, &NewInputDataset { name: "无主键".into(), columns: vec![NewColumn { title: "科目".into(), kind: "text".into() }], primary_key: vec![] }).unwrap();
        assert!(submit(&conn, &plain, &req("delete", &["科目"], vec![vec![Json::from("x")]])).unwrap_err().message().contains("没有主键"));
        // 删光后撤销：先删除再撤销，搜索文本也恢复（关键字搜索能找到）
        let all = submit(&conn, &id, &req("delete", &["店铺", "月份"], vec![vec![Json::from("A"), Json::from("2026-10")], vec![Json::from("B"), Json::from("2026-10")], vec![Json::from("C"), Json::from("2026-10")]])).unwrap();
        assert_eq!(all.deleted, 3);
        assert!(table(&conn, &id).is_empty());
        undo(&conn, all.batch_id.as_deref().unwrap(), "测试").unwrap();
        let hits: i64 = conn.query_row(&format!("SELECT count(*) FROM {id} WHERE search_text LIKE '%a%'"), [], |r| r.get(0)).unwrap();
        assert_eq!(hits, 1);
    }

    fn restricted<T>(shops: &[&str], f: impl FnOnce() -> T) -> T {
        crate::scope::run_with(Some(std::sync::Arc::new(crate::scope::Scope::shops(shops.iter().map(|s| s.to_string()).collect()))), f)
    }

    #[test]
    fn restricted_users_write_only_their_own_rows() {
        let conn = setup();
        // 主键只有「月份」：不同店铺的行会撞主键，用来检查「不能改别人范围内的行」
        let id = budget(&conn, "分店预算", &["月份"]);
        conn.execute("UPDATE ledger_dataset SET scope_column = '店铺' WHERE id = ?", [&id]).unwrap();
        submit(&conn, &id, &req("upsert", &["店铺", "月份", "预算"], vec![row("B", "2026-10", 200), row("A", "2026-11", 10)])).unwrap();

        // 范围内的新增 / 更新可以
        let ok = restricted(&["A"], || submit(&conn, &id, &req("upsert", &["店铺", "月份", "预算"], vec![row("A", "2026-11", 11), row("A", "2026-12", 12)]))).unwrap();
        assert_eq!((ok.inserted, ok.updated), (1, 1), "{:?}", ok.errors);
        // 范围外的店铺值：逐格报错
        let bad = restricted(&["A"], || submit(&conn, &id, &req("upsert", &["店铺", "月份", "预算"], vec![row("B", "2026-09", 1), vec![Json::Null, Json::from("2026-08"), Json::from(1)]]))).unwrap();
        assert!(bad.batch_id.is_none() && bad.error_count == 2, "{:?}", bad.errors);
        assert!(bad.errors[0].message.contains("不在你的数据范围内"));
        assert!(bad.errors[1].message.contains("不能为空"));
        // 主键撞上别人范围内的行（B 的 2026-10）：不能改，也不能删
        let clash = restricted(&["A"], || submit(&conn, &id, &req("upsert", &["店铺", "月份", "预算"], vec![row("A", "2026-10", 999)]))).unwrap();
        assert!(clash.batch_id.is_none() && clash.errors[0].message.contains("不在你的数据范围内"), "{:?}", clash.errors);
        let del = restricted(&["A"], || submit(&conn, &id, &req("delete", &["月份"], vec![vec![Json::from("2026-10")]]))).unwrap();
        assert!(del.batch_id.is_none() && del.error_count == 1);
        assert_eq!(table(&conn, &id).iter().find(|r| r.1 == "2026-10").unwrap().2, Some(200.0), "B 的数据没被动过");
        // 自己范围内的行可以删
        let own = restricted(&["A"], || submit(&conn, &id, &req("delete", &["月份"], vec![vec![Json::from("2026-12")]]))).unwrap();
        assert_eq!(own.deleted, 1);
        // 追加也要在范围内；填报区没有范围列直接拒绝
        assert!(restricted(&["A"], || submit(&conn, &id, &req("append", &["月份", "预算"], vec![vec![Json::from("2027-01"), Json::from(1)]]))).unwrap_err().message().contains("必须包含「店铺」"));
        let app = restricted(&["A"], || submit(&conn, &id, &req("append", &["店铺", "月份", "预算"], vec![row("B", "2027-01", 1)]))).unwrap();
        assert_eq!(app.error_count, 1);
        // 没有任何店铺的范围：什么都不能填
        let none = restricted(&[], || submit(&conn, &id, &req("append", &["店铺", "月份", "预算"], vec![row("A", "2027-01", 1)]))).unwrap();
        assert_eq!(none.error_count, 1);

        // 没有绑定范围列的数据集、所有人可见的共享数据集：受限用户不能填
        let open = budget(&conn, "共享预算", &["店铺", "月份"]);
        let e = restricted(&["A"], || submit(&conn, &open, &req("upsert", &["店铺", "月份", "预算"], vec![row("A", "2026-10", 1)]))).unwrap_err();
        assert!(e.message().contains("不存在"), "没有绑定的数据集对受限用户不可见：{}", e.message());
        conn.execute("UPDATE ledger_dataset SET scope_open = true WHERE id = ?", [&open]).unwrap();
        let e = restricted(&["A"], || submit(&conn, &open, &req("upsert", &["店铺", "月份", "预算"], vec![row("A", "2026-10", 1)]))).unwrap_err();
        assert!(e.message().contains("只能填报按某一列划分范围"), "{}", e.message());
        // 不受限的用户不受影响
        assert!(submit(&conn, &open, &req("upsert", &["店铺", "月份", "预算"], vec![row("A", "2026-10", 1)])).is_ok());
    }
}
