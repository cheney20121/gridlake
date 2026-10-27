//! 修改已有数据集（导入 / 填报 / SQL 另存的数据表）某一列的类型
//!
//! 导入时按样本推断的类型不对（订单号被当成数值、日期被当成文本、数量想按整数存……）时，不用删掉重新导入：
//! 1. 预览：按新类型转换后，有多少个值转不过来（会变成空值）、举几个例子，以及转换后的样子
//! 2. 执行：`ALTER TABLE … ALTER COLUMN … SET DATA TYPE … USING 转换表达式`，就地改存储类型（行号不变，游标、填报批次都还认得）；
//!    有值转不过来时必须明确确认才执行。改动会让计算列、关联视图失效时整体取消（见 [`dataset::guarded_change`]）
//!
//! 转换规则与导入一致：文本 → 数值 / 金额认千分位、货币符号；文本 → 整数只认没有小数的写法；文本 → 是否认 true / 是 / 1 等；
//! 文本 → 日期时间统一成「2026-09-15 10:00:00」，认不出的保留原文（不丢数据）。数值 → 文本时整数不带「.0」

use duckdb::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::dataset::{self, StoredColumn};
use crate::error::{AppError, AppResult};
use crate::kinds;
use crate::schema::ident;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KindChange {
    pub column: String,
    pub kind: String,
    /// 有值转不过来（会变成空值）时，确认仍然执行
    #[serde(default)]
    pub accept_loss: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindPreview {
    pub column: String,
    pub from: String,
    pub to: String,
    /// 非空值个数
    pub values: i64,
    /// 转不过来、会变成空值的个数
    pub lost: i64,
    /// 转不过来的值（举例，最多 5 个）
    pub lost_samples: Vec<String>,
    /// 认不出、按原文保留的个数（改成日期时间时）
    pub kept: i64,
    /// 转换前后的样子（最多 8 行，不同的值）
    pub samples: Vec<(String, Json)>,
    /// 改完之后要注意的地方（如数值列改成文本后不能再求和）
    pub warnings: Vec<String>,
    /// 已经执行（预览时为 false）
    pub applied: bool,
}

/// 现有存储类型的值 → 新类型的 SQL 表达式；c 为带引号的列名
fn convert_expr(from: &str, to: &str, c: &str) -> AppResult<String> {
    let numeric_to_text = format!("CASE WHEN {c} = trunc({c}) AND abs({c}) < 1e15 THEN CAST(CAST({c} AS BIGINT) AS VARCHAR) ELSE CAST({c} AS VARCHAR) END");
    let unsupported = || AppError::bad(format!("「{}」列不能直接改成「{}」", kinds::label(from), kinds::label(to)));
    Ok(match (kinds::sql_type(from), to) {
        // 文本类（文本 / 编号 / 日期时间都存成文本）：按导入时的规则转换
        ("VARCHAR", t) if kinds::is_textual(t) => c.to_string(),
        ("VARCHAR", t) => crate::dataset_import::convert_sql(t, c),
        // 小数（数值 / 金额）
        ("DOUBLE", "number" | "money") => c.to_string(),
        ("DOUBLE", "integer") => format!("CASE WHEN {c} = trunc({c}) AND abs({c}) < 9.2e18 THEN CAST({c} AS BIGINT) END"),
        ("DOUBLE", t) if kinds::is_textual(t) => numeric_to_text,
        ("DOUBLE", "boolean") => format!("({c} <> 0)"),
        // 整数
        ("BIGINT", "number" | "money") => format!("CAST({c} AS DOUBLE)"),
        ("BIGINT", t) if kinds::is_textual(t) => format!("CAST({c} AS VARCHAR)"),
        ("BIGINT", "boolean") => format!("({c} <> 0)"),
        // 是否
        ("BOOLEAN", t) if kinds::is_textual(t) => format!("CASE WHEN {c} THEN 'true' WHEN NOT {c} THEN 'false' END"),
        ("BOOLEAN", "integer") => format!("CAST({c} AS BIGINT)"),
        ("BOOLEAN", "number" | "money") => format!("CAST({c} AS DOUBLE)"),
        _ => return Err(unsupported()),
    })
}

/// 要改的数据表与列
fn target(conn: &Connection, id: &str, change: &KindChange) -> AppResult<(String, Vec<StoredColumn>, usize, &'static str)> {
    let to = kinds::parse(&change.kind).ok_or_else(|| AppError::bad(format!("未知的列类型：{}", change.kind)))?;
    let def = dataset::dataset(conn, id)?;
    if def.kind != "table" {
        return Err(AppError::bad(format!("「{}」不是导入 / 填报的数据表（关联视图、汇总、外部文件等的列类型由来源决定），不能改列类型", def.name)));
    }
    let (_, name, cols) = dataset::stored(conn)?.into_iter().find(|(i, _, _)| i == &def.id).ok_or_else(|| AppError::bad("数据集不存在"))?;
    let i = cols.iter().position(|c| c.title == change.column || c.key == change.column).ok_or_else(|| AppError::bad(format!("「{name}」没有列「{}」", change.column)))?;
    if cols[i].kind == to {
        return Err(AppError::bad(format!("「{}」已经是{}列", cols[i].title, kinds::label(to))));
    }
    Ok((def.id, cols, i, to))
}

fn inspect(conn: &Connection, table: &str, col: &StoredColumn, to: &'static str) -> AppResult<(KindPreview, String)> {
    let c = ident(&col.title);
    let expr = convert_expr(&col.kind, to, &c)?;
    // 认不出、按原文保留：只有改成日期时间（转换失败时退回原文）会出现
    let kept_sql = if to == "datetime" && kinds::sql_type(&col.kind) == "VARCHAR" {
        format!("count(*) FILTER (WHERE NULLIF(trim({c}), '') IS NOT NULL AND NOT regexp_full_match(({expr}), '\\d{{4}}-\\d{{2}}-\\d{{2}}( \\d{{2}}:\\d{{2}}:\\d{{2}})?'))")
    } else {
        "0".into()
    };
    let non_empty = if kinds::sql_type(&col.kind) == "VARCHAR" { format!("NULLIF(trim({c}), '') IS NOT NULL") } else { format!("{c} IS NOT NULL") };
    let (values, lost, kept): (i64, i64, i64) = conn.query_row(
        &format!("SELECT count(*) FILTER (WHERE {non_empty}), count(*) FILTER (WHERE {non_empty} AND ({expr}) IS NULL), {kept_sql} FROM {table}"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let lost_samples: Vec<String> = {
        let mut st = conn.prepare(&format!("SELECT DISTINCT CAST({c} AS VARCHAR) FROM {table} WHERE {non_empty} AND ({expr}) IS NULL LIMIT 5"))?;
        st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let samples: Vec<(String, Json)> = {
        let mut st = conn.prepare(&format!("SELECT DISTINCT CAST({c} AS VARCHAR) AS b, ({expr}) AS a FROM {table} WHERE {non_empty} LIMIT 8"))?;
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, crate::tabular::to_json(r.get_ref(1)?))))?.collect::<Result<_, _>>()?
    };
    let mut warnings = Vec::new();
    if kinds::is_numeric(&col.kind) && !kinds::is_numeric(to) {
        warnings.push(format!("改成{}后不能再求和、求平均：用「{}」求和 / 平均的透视表、指标卡、图表和 DSUMIFS 等公式会报错，请一并调整", kinds::label(to), col.title));
    }
    let preview = KindPreview { column: col.title.clone(), from: col.kind.clone(), to: to.into(), values, lost, lost_samples, kept, samples, warnings, applied: false };
    Ok((preview, expr))
}

/// 预览：转换后会怎样（不改数据）
pub fn preview(conn: &Connection, id: &str, change: &KindChange) -> AppResult<KindPreview> {
    let (table, cols, i, to) = target(conn, id, change)?;
    Ok(inspect(conn, &table, &cols[i], to)?.0)
}

/// 执行：改存储类型与登记的列类型；文本列变了的要重算关键字搜索
pub fn apply(conn: &Connection, id: &str, change: &KindChange) -> AppResult<KindPreview> {
    let (table, mut cols, i, to) = target(conn, id, change)?;
    let (mut preview, expr) = inspect(conn, &table, &cols[i], to)?;
    if preview.lost > 0 && !change.accept_loss {
        return Err(AppError::bad(format!("有 {} 个值转不成{}（如「{}」），会变成空值；确认后再执行", preview.lost, kinds::label(to), preview.lost_samples.join("」「"))));
    }
    let textual_changed = kinds::is_textual(&cols[i].kind) != kinds::is_textual(to);
    let c = ident(&cols[i].title);
    cols[i].kind = to.into();
    let text_cols: Vec<String> = cols.iter().filter(|c| kinds::is_textual(&c.kind)).map(|c| ident(&c.title)).collect();
    let stored = serde_json::to_string(&cols)?;
    dataset::guarded_change(conn, |conn| {
        conn.execute_batch(&format!("ALTER TABLE {table} ALTER COLUMN {c} SET DATA TYPE {} USING ({expr})", kinds::sql_type(to)))?;
        conn.execute("UPDATE ledger_dataset SET columns = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?", duckdb::params![stored, crate::db::now_local(), table])?;
        if textual_changed {
            let search = if text_cols.is_empty() { "NULL".into() } else { format!("lower(concat_ws(' ', {}))", text_cols.join(", ")) };
            conn.execute_batch(&format!("UPDATE {table} SET search_text = {search}"))?;
        }
        Ok(())
    })?;
    preview.applied = true;
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, String) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        let id = crate::dataset_import::create_empty(
            &conn,
            "订单",
            &[("单号".into(), "number".into()), ("金额".into(), "text".into()), ("日期".into(), "text".into()), ("件数".into(), "money".into()), ("备注".into(), "text".into())],
            &[],
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO {id} VALUES (690001, '1,200', '2026/9/1', 3, '甲', NULL), (690002, 'x', '昨天', 2.5, '乙', NULL), (NULL, '', '2026-09-02 10:00', NULL, '丙', NULL)"
        ))
        .unwrap();
        (conn, id)
    }

    fn change(column: &str, kind: &str, accept_loss: bool) -> KindChange {
        KindChange { column: column.into(), kind: kind.into(), accept_loss }
    }

    fn values(conn: &Connection, id: &str, col: &str) -> Vec<Json> {
        let mut st = conn.prepare(&format!("SELECT {} FROM {id} ORDER BY rowid", ident(col))).unwrap();
        st.query_map([], |r| Ok(crate::tabular::to_json(r.get_ref(0)?))).unwrap().map(Result::unwrap).collect()
    }

    #[test]
    fn number_to_id_keeps_every_digit_without_dot_zero() {
        let (conn, id) = setup();
        let p = apply(&conn, &id, &change("单号", "id", false)).unwrap();
        assert_eq!((p.values, p.lost, p.applied), (2, 0, true));
        assert_eq!(values(&conn, &id, "单号"), vec![Json::from("690001"), Json::from("690002"), Json::Null]);
        // 变成文本列：参与关键字搜索
        let hit: i64 = conn.query_row(&format!("SELECT count(*) FROM {id} WHERE search_text LIKE '%690002%'"), [], |r| r.get(0)).unwrap();
        assert_eq!(hit, 1);
        assert_eq!(dataset::dataset(&conn, &id).unwrap().columns[0].kind, "id");
    }

    #[test]
    fn text_to_money_needs_confirmation_when_values_would_be_lost() {
        let (conn, id) = setup();
        let p = preview(&conn, &id, &change("金额", "money", false)).unwrap();
        assert_eq!((p.values, p.lost, p.lost_samples.clone()), (2, 1, vec!["x".to_string()]));
        assert!(!p.applied);
        let e = apply(&conn, &id, &change("金额", "money", false)).err().unwrap();
        assert!(e.message().contains("转不成金额") && e.message().contains("x"), "{}", e.message());
        assert_eq!(dataset::dataset(&conn, &id).unwrap().columns[1].kind, "text", "没确认就不改");
        apply(&conn, &id, &change("金额", "money", true)).unwrap();
        assert_eq!(values(&conn, &id, "金额"), vec![Json::from(1200.0), Json::Null, Json::Null]);
    }

    #[test]
    fn text_to_datetime_keeps_unrecognized_text() {
        let (conn, id) = setup();
        let p = apply(&conn, &id, &change("日期", "datetime", false)).unwrap();
        assert_eq!((p.values, p.lost, p.kept), (3, 0, 1));
        assert_eq!(values(&conn, &id, "日期"), vec![Json::from("2026-09-01 00:00:00"), Json::from("昨天"), Json::from("2026-09-02 10:00:00")]);
    }

    #[test]
    fn money_to_integer_drops_fractions_only_after_confirmation() {
        let (conn, id) = setup();
        let p = preview(&conn, &id, &change("件数", "integer", false)).unwrap();
        assert_eq!((p.lost, p.lost_samples.clone()), (1, vec!["2.5".to_string()]));
        apply(&conn, &id, &change("件数", "integer", true)).unwrap();
        assert_eq!(values(&conn, &id, "件数"), vec![Json::from(3), Json::Null, Json::Null]);
        let ty: String = conn.query_row(&format!("SELECT data_type FROM information_schema.columns WHERE table_name = '{id}' AND column_name = '件数'"), [], |r| r.get(0)).unwrap();
        assert_eq!(ty, "BIGINT");
    }

    #[test]
    fn calc_columns_keep_working_and_numeric_to_text_warns() {
        let (conn, id) = setup();
        dataset::add_calc(&conn, &id, "两倍", "=[件数]*2", None).unwrap();
        // 计算列里文本按数字转换：改成文本后计算列照常能算；但提醒用它求和的地方会报错
        let p = apply(&conn, &id, &change("件数", "text", false)).unwrap();
        assert!(p.warnings.iter().any(|w| w.contains("不能再求和")), "{:?}", p.warnings);
        let def = dataset::dataset(&conn, &id).unwrap();
        let calc = def.columns.iter().find(|c| c.title == "两倍").unwrap();
        let first: f64 = conn.query_row(&format!("SELECT {} FROM {id} ORDER BY rowid LIMIT 1", dataset::column_sql(calc)), [], |r| r.get(0)).unwrap();
        assert_eq!(first, 6.0);
    }

    #[test]
    fn refuses_unsupported_changes() {
        let (conn, id) = setup();
        assert!(apply(&conn, &id, &change("件数", "datetime", false)).err().unwrap().message().contains("不能直接改成"));
        assert!(apply(&conn, &id, &change("备注", "text", false)).err().unwrap().message().contains("已经是"));
        assert!(apply(&conn, &id, &change("没有", "text", false)).is_err());
        assert!(apply(&conn, &id, &change("备注", "date", false)).is_err());
    }
}
