//! 数据清洗：在服务端整表执行，先预览（多少行受影响、改之前 / 改之后的样子），再执行；可以撤销最近一次
//!
//! - 删除重复行（按选定的列判断，保留最先导入的那一行）、去掉首尾空白（只有空白的变成空值）、查找替换（整格等于 / 包含）、
//!   填充空值（填一个值 / 用上一行的值向下填充）、删除当前筛选出的行
//! - 只限普通数据表（导入 / 填报的），所有者或管理员；数据范围受限的用户不能用（会绕过行级权限改到别人的行）
//! - 撤销：执行前把要改的格子（按行号）或要删的整行存到 `clean_undo` 这个 schema 下与数据集同名的表里，每个数据集只留最近一次；
//!   之后数据集又有改动（导入、同步、填报、别的清洗、改列类型）就不能再撤销（`ledger_dataset.updated_at` 精确到微秒比对）

use duckdb::types::Value;
use duckdb::{Connection, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::dataset::{self, QuerySpec, StoredColumn};
use crate::error::{AppError, AppResult};
use crate::kinds;
use crate::schema::{ident, literal};
use crate::tabular::to_json;

const SAMPLES: usize = 10;
/// 首尾要去掉的空白：空格、制表、换行、全角空格、不换行空格
const BLANKS: &str = " \t\r\n\u{3000}\u{a0}";

#[derive(Deserialize, Serialize, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CleanOp {
    /// 删除重复行：这些列（为空表示全部列）都相同的行只留最先导入的一行
    Dedupe {
        #[serde(default)]
        columns: Vec<String>,
    },
    /// 去掉首尾空白（为空表示全部文本 / 编号列）；只有空白的变成空值
    Trim {
        #[serde(default)]
        columns: Vec<String>,
    },
    /// 查找替换（文本 / 编号列）：whole 为整格等于才替换，否则替换格子里出现的每一处；替换成空字符串即变成空值
    Replace {
        column: String,
        find: String,
        #[serde(default)]
        replace: String,
        #[serde(default)]
        whole: bool,
    },
    /// 填充空值：down 时用上一行（按导入顺序）的非空值，否则填 value
    Fill {
        column: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        down: bool,
    },
    /// 删除当前筛选（列筛选 / 关键字）出的行；至少要有一个条件
    DeleteRows {
        #[serde(default)]
        spec: QuerySpec,
    },
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub before: Vec<Json>,
    /// 改之后（删除行时为空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Vec<Json>>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CleanPreview {
    /// 这次清洗的说明（如「删除重复行（按 店铺、金额）」）
    pub summary: String,
    /// 受影响的行数（删除的行 / 改了至少一格的行）
    pub affected: i64,
    pub rows: i64,
    /// 删除行（否则是修改格子）
    pub deletes: bool,
    /// 样例里各列的列名
    pub columns: Vec<String>,
    pub samples: Vec<Sample>,
    pub applied: bool,
}

/// 一次清洗翻译成的 SQL
struct Plan {
    summary: String,
    /// 受影响的行（用在 WHERE 里，数据表不加别名）
    cond: String,
    args: Vec<Value>,
    deletes: bool,
    /// 修改格子时：（列，新值表达式）
    sets: Vec<(StoredColumn, String)>,
    /// 样例里显示的列
    shown: Vec<StoredColumn>,
}

fn target(conn: &Connection, id: &str) -> AppResult<(dataset::DatasetDef, Vec<StoredColumn>)> {
    let def = dataset::dataset(conn, id)?;
    if def.kind != "table" {
        return Err(AppError::bad(format!("「{}」不是导入 / 填报的数据表（关联视图、汇总、外部文件等的数据由来源决定），不能清洗", def.name)));
    }
    let (_, _, cols) = dataset::stored(conn)?.into_iter().find(|(i, _, _)| i == &def.id).ok_or_else(|| AppError::bad("数据集不存在"))?;
    Ok((def, cols))
}

fn pick<'a>(cols: &'a [StoredColumn], key: &str) -> AppResult<&'a StoredColumn> {
    cols.iter().find(|c| c.title == key || c.key == key).ok_or_else(|| AppError::bad(format!("列不存在：{key}（计算列不能清洗）")))
}

fn names(cols: &[StoredColumn]) -> String {
    cols.iter().map(|c| c.title.as_str()).collect::<Vec<_>>().join("、")
}

/// 填充用的值：按列类型检查并转换
fn fill_value(col: &StoredColumn, raw: &str) -> AppResult<Value> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(AppError::bad("请填写要填充的值"));
    }
    let bad = || AppError::bad(format!("「{s}」不是{}，不能填到「{}」里", kinds::label(&col.kind), col.title));
    Ok(match col.kind.as_str() {
        "integer" => Value::BigInt(kinds::parse_integer(s).ok_or_else(bad)?),
        k if kinds::is_numeric(k) => Value::Double(s.replace([',', '，'], "").parse().map_err(|_| bad())?),
        "boolean" => Value::Boolean(kinds::parse_bool(s).ok_or_else(bad)?),
        _ => Value::Text(s.to_string()),
    })
}

fn plan(def: &dataset::DatasetDef, cols: &[StoredColumn], op: &CleanOp) -> AppResult<Plan> {
    let table = &def.id;
    let textual = |c: &StoredColumn| kinds::is_textual(&c.kind);
    let trimmed = |c: &str| format!("trim({c}, {})", literal(BLANKS));
    Ok(match op {
        CleanOp::Dedupe { columns } => {
            let by: Vec<StoredColumn> = if columns.is_empty() { cols.to_vec() } else { columns.iter().map(|k| pick(cols, k).cloned()).collect::<AppResult<_>>()? };
            if by.is_empty() {
                return Err(AppError::bad("请选择判断重复用的列"));
            }
            let part = by.iter().map(|c| ident(&c.title)).collect::<Vec<_>>().join(", ");
            Plan {
                summary: if columns.is_empty() { "删除重复行（所有列都相同）".into() } else { format!("删除重复行（按 {}）", names(&by)) },
                cond: format!("rowid IN (SELECT rowid FROM {table} QUALIFY row_number() OVER (PARTITION BY {part} ORDER BY rowid) > 1)"),
                args: vec![],
                deletes: true,
                sets: vec![],
                shown: by,
            }
        }
        CleanOp::Trim { columns } => {
            let list: Vec<StoredColumn> = if columns.is_empty() { cols.iter().filter(|c| textual(c)).cloned().collect() } else { columns.iter().map(|k| pick(cols, k).cloned()).collect::<AppResult<_>>()? };
            if let Some(c) = list.iter().find(|c| !textual(c)) {
                return Err(AppError::bad(format!("「{}」是{}列，去空白只用于文本 / 编号列", c.title, kinds::label(&c.kind))));
            }
            if list.is_empty() {
                return Err(AppError::bad("这个数据集没有文本列"));
            }
            let cond = list.iter().map(|c| format!("({0} IS NOT NULL AND {0} <> {1})", ident(&c.title), trimmed(&ident(&c.title)))).collect::<Vec<_>>().join(" OR ");
            let sets = list.iter().map(|c| (c.clone(), format!("NULLIF({}, '')", trimmed(&ident(&c.title))))).collect();
            Plan { summary: format!("去掉首尾空白（{}）", names(&list)), cond, args: vec![], deletes: false, sets, shown: list }
        }
        CleanOp::Replace { column, find, replace, whole } => {
            let c = pick(cols, column)?.clone();
            if !textual(&c) {
                return Err(AppError::bad(format!("「{}」是{}列，查找替换只用于文本 / 编号列（数值列请改列类型或用填充）", c.title, kinds::label(&c.kind))));
            }
            if find.is_empty() {
                return Err(AppError::bad("请填写要查找的内容"));
            }
            let q = ident(&c.title);
            let (cond, set) = if *whole { (format!("{q} = ?"), "NULLIF(?, '')".to_string()) } else { (format!("contains({q}, ?)"), format!("NULLIF(replace({q}, ?, ?), '')")) };
            let args = vec![Value::Text(find.clone())];
            let set_args = if *whole { vec![Value::Text(replace.clone())] } else { vec![Value::Text(find.clone()), Value::Text(replace.clone())] };
            // 新值表达式里的参数放在样例查询 / UPDATE 的 SET 里，按位置先于 WHERE 的参数：用字面量代入，避免两组参数交错
            let set = set_args.iter().fold(set, |acc, v| match v {
                Value::Text(s) => acc.replacen('?', &literal(s), 1),
                _ => acc,
            });
            let how = if *whole { "整格等于" } else { "包含" };
            Plan { summary: format!("「{}」里{how}「{find}」的替换为「{replace}」", c.title), cond, args, deletes: false, sets: vec![(c.clone(), set)], shown: vec![c] }
        }
        CleanOp::Fill { column, value, down } => {
            let c = pick(cols, column)?.clone();
            let q = ident(&c.title);
            let empty = if textual(&c) { format!("({q} IS NULL OR {} = '')", trimmed(&q)) } else { format!("{q} IS NULL") };
            if *down {
                // 上一行（按导入顺序）的非空值；第一行之前没有值的保持空
                let prev = format!("(SELECT v FROM (SELECT rowid AS r, last_value(NULLIF({q}, {blank}) IGNORE NULLS) OVER (ORDER BY rowid ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS v FROM {table}) p WHERE p.r = {table}.rowid)", blank = if textual(&c) { "''" } else { "NULL" });
                Plan {
                    summary: format!("「{}」的空值用上一行的值向下填充", c.title),
                    cond: format!("{empty} AND {prev} IS NOT NULL"),
                    args: vec![],
                    deletes: false,
                    sets: vec![(c.clone(), prev)],
                    shown: vec![c],
                }
            } else {
                let v = fill_value(&c, value.as_deref().unwrap_or(""))?;
                let lit = match &v {
                    Value::BigInt(n) => n.to_string(),
                    Value::Double(f) => format!("{f}"),
                    Value::Boolean(b) => b.to_string(),
                    Value::Text(s) => literal(s),
                    _ => "NULL".into(),
                };
                Plan { summary: format!("「{}」的空值填为「{}」", c.title, value.as_deref().unwrap_or("").trim()), cond: empty, args: vec![], deletes: false, sets: vec![(c.clone(), lit)], shown: vec![c] }
            }
        }
        CleanOp::DeleteRows { spec } => {
            let has_filter = !spec.filters.is_empty() || spec.keyword.as_deref().is_some_and(|k| !k.trim().is_empty()) || spec.violation.is_some();
            if !has_filter {
                return Err(AppError::bad("请先筛选出要删除的行（没有筛选条件时不能整表删除；要清空请删除数据集）"));
            }
            let (cond, args) = dataset::rows_sql(def, spec)?;
            Plan { summary: "删除筛选出的行".into(), cond, args, deletes: true, sets: vec![], shown: cols.to_vec() }
        }
    })
}

fn samples(conn: &Connection, table: &str, p: &Plan) -> AppResult<Vec<Sample>> {
    let mut exprs: Vec<String> = p.shown.iter().map(|c| ident(&c.title)).collect();
    if !p.deletes {
        // 改之后：被改的列用新值，其他列不变
        exprs.extend(p.shown.iter().map(|c| p.sets.iter().find(|(s, _)| s.title == c.title).map_or_else(|| ident(&c.title), |(_, e)| e.clone())));
    }
    let n = p.shown.len();
    let mut stmt = conn.prepare(&format!("SELECT {} FROM {table} WHERE {} ORDER BY rowid LIMIT {SAMPLES}", exprs.join(", "), p.cond))?;
    let rows = stmt
        .query_map(params_from_iter(p.args.iter()), |r| {
            let before = (0..n).map(|i| Ok(to_json(r.get_ref(i)?))).collect::<duckdb::Result<Vec<_>>>()?;
            let after = if p.deletes { None } else { Some((n..2 * n).map(|i| Ok(to_json(r.get_ref(i)?))).collect::<duckdb::Result<Vec<_>>>()?) };
            Ok(Sample { before, after })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

fn inspect(conn: &Connection, def: &dataset::DatasetDef, p: &Plan) -> AppResult<CleanPreview> {
    let table = &def.id;
    let (affected, rows): (i64, i64) = conn.query_row(&format!("SELECT count(*) FILTER (WHERE {}), count(*) FROM {table}", p.cond), params_from_iter(p.args.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(CleanPreview {
        summary: p.summary.clone(),
        affected,
        rows,
        deletes: p.deletes,
        columns: p.shown.iter().map(|c| c.title.clone()).collect(),
        samples: samples(conn, table, p)?,
        applied: false,
    })
}

pub fn preview(conn: &Connection, id: &str, op: &CleanOp) -> AppResult<CleanPreview> {
    let (def, cols) = target(conn, id)?;
    let p = plan(&def, &cols, op)?;
    inspect(conn, &def, &p)
}

fn undo_table(id: &str) -> String {
    format!("clean_undo.{}", ident(id))
}

/// 关键字搜索列（只有带 search_text 的数据表）：文本列变了要重算
fn refresh_search(conn: &Connection, def: &dataset::DatasetDef, cols: &[StoredColumn], rows_sql: &str) -> AppResult<()> {
    if !def.searchable {
        return Ok(());
    }
    let text: Vec<String> = cols.iter().filter(|c| kinds::is_textual(&c.kind)).map(|c| ident(&c.title)).collect();
    let expr = if text.is_empty() { "NULL".into() } else { format!("lower(concat_ws(' ', {}))", text.join(", ")) };
    conn.execute_batch(&format!("UPDATE {} SET search_text = {expr} WHERE {rows_sql}", def.id))?;
    Ok(())
}

fn touch(conn: &Connection, id: &str) -> AppResult<String> {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    conn.execute("UPDATE ledger_dataset SET updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now, id])?;
    Ok(conn.query_row("SELECT strftime(updated_at, '%Y-%m-%d %H:%M:%S.%f') FROM ledger_dataset WHERE id = ?", params![id], |r| r.get(0))?)
}

/// 执行清洗；返回执行时的预览（applied = true）
pub fn apply(conn: &Connection, id: &str, op: &CleanOp, operator: &str) -> AppResult<CleanPreview> {
    let (def, cols) = target(conn, id)?;
    let p = plan(&def, &cols, op)?;
    let mut preview = inspect(conn, &def, &p)?;
    if preview.affected == 0 {
        return Err(AppError::bad("没有需要处理的行"));
    }
    let table = &def.id;
    let undo = undo_table(table);
    let shape = serde_json::to_string(&cols.iter().map(|c| (&c.title, &c.kind)).collect::<Vec<_>>())?;
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        conn.execute_batch(&format!("CREATE SCHEMA IF NOT EXISTS clean_undo; DROP TABLE IF EXISTS {undo}"))?;
        if p.deletes {
            conn.execute(&format!("CREATE TABLE {undo} AS SELECT * FROM {table} WHERE {}", p.cond), params_from_iter(p.args.iter()))?;
            conn.execute(&format!("DELETE FROM {table} WHERE {}", p.cond), params_from_iter(p.args.iter()))?;
        } else {
            let saved = p.sets.iter().map(|(c, _)| ident(&c.title)).collect::<Vec<_>>().join(", ");
            conn.execute(&format!("CREATE TABLE {undo} AS SELECT rowid AS __rid, {saved} FROM {table} WHERE {}", p.cond), params_from_iter(p.args.iter()))?;
            // 先算好新值再写回（向下填充的新值依赖同一列其他行的旧值，不能边改边算）
            let news = p.sets.iter().enumerate().map(|(i, (_, e))| format!("{e} AS __v{i}")).collect::<Vec<_>>().join(", ");
            conn.execute_batch(&format!("CREATE TEMP TABLE __clean_new AS SELECT rowid AS __rid, {news} FROM {table} WHERE rowid IN (SELECT __rid FROM {undo})"))?;
            let assigns = p.sets.iter().enumerate().map(|(i, (c, _))| format!("{} = n.__v{i}", ident(&c.title))).collect::<Vec<_>>().join(", ");
            let r = conn.execute_batch(&format!("UPDATE {table} SET {assigns} FROM __clean_new n WHERE {table}.rowid = n.__rid"));
            conn.execute_batch("DROP TABLE IF EXISTS __clean_new")?;
            r?;
            if p.sets.iter().any(|(c, _)| kinds::is_textual(&c.kind)) {
                refresh_search(conn, &def, &cols, &format!("rowid IN (SELECT __rid FROM {undo})"))?;
            }
        }
        let stamp = touch(conn, table)?;
        conn.execute("DELETE FROM ledger_clean WHERE dataset_id = ?", params![table])?;
        conn.execute(
            "INSERT INTO ledger_clean (dataset_id, summary, op, deletes, affected, shape, stamp, operator, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
            params![table, p.summary, serde_json::to_string(op)?, p.deletes, preview.affected, shape, stamp, operator, crate::db::now_local()],
        )?;
        Ok(())
    })();
    match r {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    preview.applied = true;
    Ok(preview)
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LastClean {
    pub summary: String,
    pub affected: i64,
    pub deletes: bool,
    pub operator: String,
    pub created_at: String,
    pub can_undo: bool,
    /// 不能撤销的原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 最近一次清洗（没有时为 None）与能否撤销
pub fn last(conn: &Connection, id: &str) -> AppResult<Option<LastClean>> {
    let row = conn.query_row(
        "SELECT summary, affected, deletes, COALESCE(operator, ''), strftime(created_at, '%Y-%m-%d %H:%M:%S'), stamp, shape, undone_at IS NOT NULL FROM ledger_clean WHERE dataset_id = ?",
        params![id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, bool>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, bool>(7)?)),
    );
    let (summary, affected, deletes, operator, created_at, stamp, shape, undone) = match row {
        Ok(v) => v,
        Err(duckdb::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let reason = if undone {
        Some("已撤销".to_string())
    } else {
        let now: Option<String> = conn
            .query_row("SELECT strftime(updated_at, '%Y-%m-%d %H:%M:%S.%f'), columns FROM ledger_dataset WHERE id = ? AND deleted_at IS NULL", params![id], |r| {
                let cols: Vec<StoredColumn> = serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default();
                let now_shape = serde_json::to_string(&cols.iter().map(|c| (&c.title, &c.kind)).collect::<Vec<_>>()).unwrap_or_default();
                Ok(if now_shape == shape { r.get::<_, String>(0)? } else { String::new() })
            })
            .ok();
        match now {
            None => Some("数据集已删除或在回收站".into()),
            Some(s) if s.is_empty() => Some("之后数据集的列有变化".into()),
            Some(s) if s != stamp => Some("之后数据集又有改动（导入、同步、填报或别的清洗）".into()),
            Some(_) => None,
        }
    };
    Ok(Some(LastClean { summary, affected, deletes, operator, created_at, can_undo: reason.is_none(), reason }))
}

/// 撤销最近一次清洗：删掉的行插回（排到最后），改过的格子按行号改回去
pub fn undo(conn: &Connection, id: &str, operator: &str) -> AppResult<LastClean> {
    let (def, cols) = target(conn, id)?;
    let l = last(conn, id)?.ok_or_else(|| AppError::bad("这个数据集没有清洗记录"))?;
    if let Some(why) = &l.reason {
        return Err(AppError::bad(format!("不能撤销：{why}")));
    }
    let table = &def.id;
    let undo = undo_table(table);
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        if l.deletes {
            conn.execute_batch(&format!("INSERT INTO {table} SELECT * FROM {undo}"))?;
        } else {
            let saved: Vec<String> = {
                let mut st = conn.prepare("SELECT column_name FROM information_schema.columns WHERE table_schema = 'clean_undo' AND table_name = ? AND column_name <> '__rid' ORDER BY ordinal_position")?;
                st.query_map(params![table], |r| r.get(0))?.collect::<Result<_, _>>()?
            };
            let assigns = saved.iter().map(|c| format!("{0} = u.{0}", ident(c))).collect::<Vec<_>>().join(", ");
            conn.execute_batch(&format!("UPDATE {table} SET {assigns} FROM {undo} u WHERE {table}.rowid = u.__rid"))?;
            if saved.iter().any(|s| cols.iter().any(|c| &c.title == s && kinds::is_textual(&c.kind))) {
                refresh_search(conn, &def, &cols, &format!("rowid IN (SELECT __rid FROM {undo})"))?;
            }
        }
        conn.execute_batch(&format!("DROP TABLE IF EXISTS {undo}"))?;
        touch(conn, table)?;
        conn.execute("UPDATE ledger_clean SET undone_at = CAST(? AS TIMESTAMP), undone_by = ? WHERE dataset_id = ?", params![crate::db::now_local(), operator, table])?;
        Ok(())
    })();
    match r {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    Ok(LastClean { can_undo: false, reason: Some("已撤销".into()), ..l })
}

/// 彻底删除数据集时，清洗记录与撤销数据一起删
pub fn forget(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_clean WHERE dataset_id = ?", params![id])?;
    conn.execute_batch(&format!("DROP TABLE IF EXISTS {}", undo_table(id)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, String) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        let input: crate::writeback::NewInputDataset = serde_json::from_value(serde_json::json!({
            "name": "订单", "columns": [{"title": "店铺", "kind": "text"}, {"title": "备注", "kind": "text"}, {"title": "金额", "kind": "money"}], "primaryKey": []
        }))
        .unwrap();
        let id = crate::writeback::create_dataset(&conn, &input).unwrap();
        let rows = [("A", Some(" 加急 "), Some(10.0)), ("A", Some(" 加急 "), Some(10.0)), ("B", None, None), ("B", Some("\u{3000}"), Some(5.0)), ("C", Some("普通"), None), ("A", Some(" 加急 "), Some(10.0))];
        for (s, n, a) in rows {
            conn.execute(&format!("INSERT INTO {id} (\"店铺\", \"备注\", \"金额\") VALUES (?, ?, ?)"), params![s, n, a]).unwrap();
        }
        (conn, id)
    }

    fn count(conn: &Connection, id: &str, w: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {id} WHERE {w}"), [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn dedupe_trim_fill_and_undo() {
        let (conn, id) = setup();
        // 删除重复行：三行 A / 加急 / 10 只留第一行
        let op = CleanOp::Dedupe { columns: vec![] };
        let p = preview(&conn, &id, &op).unwrap();
        assert_eq!((p.affected, p.rows, p.deletes), (2, 6, true));
        apply(&conn, &id, &op, "测试").unwrap();
        assert_eq!(count(&conn, &id, "TRUE"), 4);
        assert!(last(&conn, &id).unwrap().unwrap().can_undo);
        undo(&conn, &id, "测试").unwrap();
        assert_eq!(count(&conn, &id, "TRUE"), 6, "撤销：删掉的行插回");
        assert!(!last(&conn, &id).unwrap().unwrap().can_undo);

        // 去空白：「 加急 」×3 → 「加急」，全角空格 → 空值
        let op = CleanOp::Trim { columns: vec![] };
        let p = preview(&conn, &id, &op).unwrap();
        assert_eq!(p.affected, 4);
        assert_eq!(p.samples[0].after.as_ref().unwrap()[1], Json::from("加急"));
        apply(&conn, &id, &op, "测试").unwrap();
        assert_eq!(count(&conn, &id, "\"备注\" = '加急'"), 3);
        assert_eq!(count(&conn, &id, "\"备注\" IS NULL"), 2);
        undo(&conn, &id, "测试").unwrap();
        assert_eq!(count(&conn, &id, "\"备注\" = ' 加急 '"), 3, "撤销：改回原样");

        // 向下填充金额。撤销删除重复行时，删掉的两行插回到了最后，所以现在的顺序是：
        // A 10、B 空、B 5、C 空、A 10、A 10 → B 用上一行的 10，C 用上一行的 5
        let op = CleanOp::Fill { column: "金额".into(), value: None, down: true };
        assert_eq!(preview(&conn, &id, &op).unwrap().affected, 2);
        apply(&conn, &id, &op, "测试").unwrap();
        let filled: Vec<f64> = {
            let mut st = conn.prepare(&format!("SELECT \"金额\" FROM {id} ORDER BY rowid")).unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(filled, vec![10.0, 10.0, 5.0, 5.0, 10.0, 10.0]);

        // 之后又有改动（再清洗一次）：上一次不能撤销，只能撤销最近这次
        let op = CleanOp::Fill { column: "备注".into(), value: Some("无".into()), down: false };
        apply(&conn, &id, &op, "测试").unwrap();
        assert_eq!(count(&conn, &id, "\"备注\" = '无'"), 2, "空值和只有空白的都填上");
        conn.execute("UPDATE ledger_dataset SET updated_at = CAST('2030-01-01' AS TIMESTAMP) WHERE id = ?", params![id]).unwrap();
        let l = last(&conn, &id).unwrap().unwrap();
        assert!(!l.can_undo && l.reason.unwrap().contains("又有改动"));
    }

    #[test]
    fn replace_delete_and_validation() {
        let (conn, id) = setup();
        let op = CleanOp::Replace { column: "店铺".into(), find: "A".into(), replace: "甲".into(), whole: true };
        assert_eq!(preview(&conn, &id, &op).unwrap().affected, 3);
        apply(&conn, &id, &op, "测试").unwrap();
        assert_eq!(count(&conn, &id, "\"店铺\" = '甲'"), 3);
        let op = CleanOp::Replace { column: "备注".into(), find: "急".into(), replace: "".into(), whole: false };
        apply(&conn, &id, &op, "测试").unwrap();
        assert_eq!(count(&conn, &id, "\"备注\" = ' 加 '"), 3);

        let spec: QuerySpec = serde_json::from_value(serde_json::json!({"filters": [{"op": "in", "column": "店铺", "values": ["B"]}]})).unwrap();
        let op = CleanOp::DeleteRows { spec };
        assert_eq!(apply(&conn, &id, &op, "测试").unwrap().affected, 2);
        assert_eq!(count(&conn, &id, "TRUE"), 4);

        let err = |op: CleanOp| preview(&conn, &id, &op).err().map(|e| e.message()).unwrap_or_default();
        assert!(err(CleanOp::DeleteRows { spec: QuerySpec::default() }).contains("筛选"));
        assert!(err(CleanOp::Replace { column: "金额".into(), find: "1".into(), replace: "2".into(), whole: false }).contains("文本"));
        assert!(err(CleanOp::Fill { column: "金额".into(), value: Some("很多".into()), down: false }).contains("不是金额"));
        assert!(err(CleanOp::Trim { columns: vec!["不存在".into()] }).contains("不存在"));
        assert!(apply(&conn, &id, &CleanOp::Dedupe { columns: vec!["店铺".into(), "备注".into(), "金额".into()] }, "测试").is_ok());
        assert!(apply(&conn, &id, &CleanOp::Dedupe { columns: vec![] }, "测试").err().unwrap().message().contains("没有需要处理"));
    }
}
