//! 汇总数据集：透视表「转为数据表」（对标 Row Zero 透视表的数据表模式）
//!
//! 透视结果按行展平成一个数据集：行字段、列字段都成为分组列，普通汇总值与计算值成为数值列。
//! 之后它就是一个普通的数据集——放进工作簿当数据表，按值筛选（「金额合计 > 1000 的店铺」）、排序、搜索，
//! 加计算列、做图表 / 透视、写 SQL、导出都直接可用。
//!
//! - **始终最新**：不存结果，数据来源是加载数据集时现生成的 `GROUP BY` 子查询（与关联视图一样随源数据变化，
//!   源数据集新增的计算列、筛选条件都照常生效）
//! - **数据范围**：受限用户看到的是按他可见的行汇总的结果（子查询建在已经按范围过滤过的源数据上），源数据集对他不可见时汇总也不可见
//! - **不带入**：占比 / 同比 / 环比（按行展平后没有「总计」「上一期」可比）、计算项、小计 / 总计行、「前 N 项」
//! - 计算值按公式换成 SQL：除数为 0 或任一值为空时结果为空，与透视表一致；金额格式的计算值保留到分

use duckdb::types::Value;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::dataset::{self, ColumnDef, DatasetDef, QuerySpec};
use crate::error::{AppError, AppResult};
use crate::pivot::{self, PivotField, PivotValue};
use crate::pivot_calc::{self, Expr};
use crate::schema::{ident, literal};

/// 汇总数据集的定义（存在 ledger_dataset.definition）
#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SummaryDef {
    /// 源数据集 id
    pub source: String,
    /// 分组字段（透视表的行字段在前、列字段在后）
    pub rows: Vec<PivotField>,
    /// 汇总值与计算值（不能设置显示方式）
    pub values: Vec<PivotValue>,
    /// 源数据的筛选（与透视表的筛选同构）
    #[serde(default, flatten)]
    pub query: QuerySpec,
}

const MAX_DIMS: usize = 6;

/// 编译结果：结果列与数据来源子查询
struct Built {
    columns: Vec<ColumnDef>,
    table: String,
    primary_key: Vec<String>,
}

fn value_literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Text(s) => literal(s),
        Value::Double(n) => format!("{n:?}"),
        Value::Float(n) => format!("{n:?}"),
        Value::BigInt(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        other => literal(&format!("{other:?}")),
    }
}

/// 把 `?` 占位换成字面量（跳过字符串与带引号的列名），数据来源子查询不能带参数
fn inline_args(sql: &str, args: &[Value]) -> AppResult<String> {
    let mut out = String::with_capacity(sql.len() + 16 * args.len());
    let mut it = args.iter();
    let mut quote: Option<char> = None;
    for ch in sql.chars() {
        match quote {
            Some(q) => {
                out.push(ch);
                if ch == q {
                    quote = None;
                }
            }
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                out.push(ch);
            }
            None if ch == '?' => out.push_str(&value_literal(it.next().ok_or_else(|| AppError::bad("筛选条件的参数个数不对"))?)),
            None => out.push(ch),
        }
    }
    if it.next().is_some() {
        return Err(AppError::bad("筛选条件的参数个数不对"));
    }
    Ok(out)
}

/// 计算值的公式 → SQL（引用同一行里已算好的汇总值列）
fn calc_sql(e: &Expr, names: &[String]) -> String {
    match e {
        Expr::Num(n) => format!("({n:?})::DOUBLE"),
        Expr::Value(i) => ident(&names[*i]),
        Expr::Neg(x) => format!("(-{})", calc_sql(x, names)),
        Expr::Bin('/', a, b) => format!("({} / NULLIF({}, 0))", calc_sql(a, names), calc_sql(b, names)),
        Expr::Bin(op, a, b) => format!("({} {op} {})", calc_sql(a, names), calc_sql(b, names)),
    }
}

fn kind_static(kind: &str) -> &'static str {
    match kind {
        "money" => "money",
        "datetime" => "datetime",
        "id" => "id",
        "integer" => "integer",
        "number" | "percent" => "number",
        _ => "text",
    }
}

/// 按源数据集的定义编译（source 已按当前请求的数据范围处理过）
fn build(def: &SummaryDef, source: &DatasetDef) -> AppResult<Built> {
    if def.rows.is_empty() {
        return Err(AppError::bad("至少要有一个行字段或列字段（按什么分组）"));
    }
    if def.rows.len() > MAX_DIMS {
        return Err(AppError::bad(format!("分组字段最多 {MAX_DIMS} 个")));
    }
    if def.values.is_empty() {
        return Err(AppError::bad("至少要有一个汇总值"));
    }
    let mut columns: Vec<ColumnDef> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |title: String, kind: &'static str, columns: &mut Vec<ColumnDef>| -> AppResult<()> {
        if !seen.insert(title.clone()) {
            return Err(AppError::bad(format!("列名「{title}」重复，请在透视表里给其中一个改名（双击表头即可改名）")));
        }
        columns.push(ColumnDef { key: title.clone(), title, kind, width: 120, expr: None, computed: false, formula: None, origin: None });
        Ok(())
    };
    let mut dims = Vec::new();
    for f in &def.rows {
        let (expr, name) = pivot::dim_sql(source, f)?;
        let col = source.columns.iter().find(|c| c.key == f.column || c.title == f.column);
        let kind = if f.granularity.as_deref().is_some_and(|g| !g.is_empty()) { "text" } else { col.map(|c| c.kind).filter(|k| *k != "boolean").unwrap_or("text") };
        push(name.clone(), kind, &mut columns)?;
        dims.push((expr, name));
    }
    // 普通汇总值（计算值另算）
    let mut plain_names: Vec<String> = Vec::new();
    let mut plain = Vec::new();
    let mut calcs = Vec::new();
    for v in &def.values {
        if !matches!(v.show.as_deref(), None | Some("" | "raw")) {
            return Err(AppError::bad("转为数据表时不能带占比 / 同比 / 环比（按行展平后没有可比的总计或上一期），请先改回「原值」"));
        }
        if v.agg == "calc" {
            calcs.push(v);
            continue;
        }
        let (sql, info) = pivot::value_sql(source, v)?;
        let sql = if info.kind == "money" && pivot::keeps_cents(&info.agg) { format!("ROUND({sql}, 2)") } else { sql };
        push(info.label.clone(), kind_static(&info.kind), &mut columns)?;
        plain_names.push(info.label.clone());
        plain.push((sql, info.label));
    }
    if plain.is_empty() {
        return Err(AppError::bad("计算值要引用普通的汇总值，请至少添加一个普通的值字段"));
    }
    let mut calc_cols = Vec::new();
    for (n, v) in calcs.into_iter().enumerate() {
        let label = v.label.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| format!("计算值{}", n + 1));
        let formula = v.formula.as_deref().map(str::trim).filter(|f| !f.is_empty()).ok_or_else(|| AppError::bad(format!("「{label}」缺少公式")))?;
        let resolve = |name: &str| -> AppResult<usize> {
            plain_names.iter().position(|x| x == name).ok_or_else(|| AppError::bad(format!("公式里的「{name}」不是透视表里的值（可用：{}）", plain_names.iter().map(|x| format!("[{x}]")).collect::<Vec<_>>().join("、"))))
        };
        let e = pivot_calc::parse(formula, &resolve)?;
        let money = v.format.as_deref() == Some("money");
        let sql = calc_sql(&e, &plain_names);
        push(label.clone(), if money { "money" } else { "number" }, &mut columns)?;
        calc_cols.push((if money { format!("ROUND({sql}, 2)") } else { sql }, label));
    }

    let (where_sql, args) = dataset::filter_sql_for(source, &def.query)?;
    let where_sql = inline_args(&where_sql, &args)?;
    let dsel = dims.iter().map(|(e, n)| format!("{e} AS {}", ident(n))).collect::<Vec<_>>().join(", ");
    let vsel = plain.iter().map(|(e, n)| format!("{e} AS {}", ident(n))).collect::<Vec<_>>().join(", ");
    let group = (1..=dims.len()).map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
    let csel = calc_cols.iter().map(|(e, n)| format!(", {e} AS {}", ident(n))).collect::<String>();
    let order = dims.iter().map(|(_, n)| format!("{} NULLS LAST", ident(n))).collect::<Vec<_>>().join(", ");
    let table = format!(
        "(SELECT *{csel}, (row_number() OVER (ORDER BY {order}) - 1)::BIGINT AS __rid FROM (SELECT {dsel}, {vsel} FROM (SELECT *, {rid} AS __ord FROM {src} WHERE {where_sql}) s GROUP BY {group}) g)",
        rid = source.rid,
        src = source.table,
    );
    Ok(Built { columns, table, primary_key: dims.into_iter().map(|(_, n)| n).collect() })
}

/// 数据集清单里的定义（`dataset::load` 调用；源数据集要已加载）
pub fn dataset_def(id: String, name: String, def: SummaryDef, source: &DatasetDef) -> AppResult<DatasetDef> {
    let b = build(&def, source)?;
    Ok(DatasetDef {
        id,
        name,
        table: b.table,
        columns: b.columns,
        searchable: false,
        supports_rule: false,
        builtin: false,
        primary_key: b.primary_key,
        rid: "__rid".into(),
        kind: "summary".into(),
        scope_sig: String::new(),
        view: None,
        summary: Some(def),
        external: None,
    })
}

/// 按受限后的源数据集重建数据来源（`scope::apply` 调用）
pub fn rescope(d: &mut DatasetDef, source: &DatasetDef) -> AppResult<()> {
    let def = d.summary.clone().ok_or_else(|| AppError::bad("汇总数据集的定义缺失"))?;
    d.table = build(&def, source)?.table;
    Ok(())
}

/// 新建：校验（用调用者能看到的源数据集）、试算一次、登记；返回数据集 id
pub fn create(conn: &Connection, name: &str, def: &SummaryDef) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(AppError::bad("名称不能为空，且不超过 60 个字符"));
    }
    if dataset::load(conn)?.0.iter().any(|d| d.name == name) {
        return Err(AppError::bad(format!("已存在名为「{name}」的数据集")));
    }
    let visible = dataset::datasets(conn)?;
    let source = visible.iter().find(|d| d.id == def.source).ok_or_else(|| AppError::bad("源数据集不存在"))?;
    let b = build(def, source)?;
    conn.query_row(&format!("SELECT count(*) FROM {}", b.table), [], |r| r.get::<_, i64>(0))?;
    let id = format!("sm_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    let now = crate::db::now_local();
    conn.execute(
        "INSERT INTO ledger_dataset (id, name, columns, primary_key, kind, definition, created_at, updated_at)
         VALUES (?, ?, '[]', '[]', 'summary', ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![id, name, serde_json::to_string(def)?, now, now],
    )?;
    Ok(id)
}

/// 定义里的源数据集 id（回收站判断能否恢复用）
pub fn source_of(definition: &str) -> Option<String> {
    serde_json::from_str::<SummaryDef>(definition).ok().map(|d| d.source)
}

/// 某数据集被哪些汇总数据集引用（删除前检查）
pub fn dependents(conn: &Connection, dataset_id: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name, definition FROM ledger_dataset WHERE kind = 'summary' AND deleted_at IS NULL")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter(|(_, d)| d.as_deref().and_then(source_of).as_deref() == Some(dataset_id)).map(|(n, _)| n).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inlines_args_outside_quotes() {
        let sql = r#"("店?" IN (?, ?)) AND x LIKE ? AND y > ? AND z = '?'"#;
        let out = inline_args(sql, &[Value::Text("A".into()), Value::Text("O'Neil".into()), Value::Text("%a%".into()), Value::Double(1.5)]).unwrap();
        assert_eq!(out, r#"("店?" IN ('A', 'O''Neil')) AND x LIKE '%a%' AND y > 1.5 AND z = '?'"#);
        assert!(inline_args("a = ?", &[]).is_err());
        assert!(inline_args("a = 1", &[Value::Double(1.0)]).is_err());
    }

    #[test]
    fn calc_formula_to_sql() {
        let conn = Connection::open_in_memory().unwrap();
        let names = vec!["金额".to_string(), "成本".to_string()];
        let resolve = |n: &str| names.iter().position(|x| x == n).ok_or_else(|| AppError::bad("x"));
        let e = pivot_calc::parse("([金额] - [成本]) / [金额]", &resolve).unwrap();
        let sql = calc_sql(&e, &names);
        let q = |a: &str, b: &str| -> Option<f64> { conn.query_row(&format!("SELECT {sql} FROM (SELECT {a}::DOUBLE AS \"金额\", {b}::DOUBLE AS \"成本\")"), [], |r| r.get(0)).unwrap() };
        assert_eq!(q("200", "50"), Some(0.75));
        assert_eq!(q("0", "50"), None, "除数为 0 为空");
        assert_eq!(q("NULL", "50"), None, "任一为空则为空");
        let neg = pivot_calc::parse("-[成本] * 2", &resolve).unwrap();
        let v: f64 = conn.query_row(&format!("SELECT {} FROM (SELECT 1::DOUBLE AS \"金额\", 3::DOUBLE AS \"成本\")", calc_sql(&neg, &names)), [], |r| r.get(0)).unwrap();
        assert_eq!(v, -6.0);
    }
}
