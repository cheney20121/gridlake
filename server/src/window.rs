//! 窗口计算：累计、排名、上一行、移动平均、组内合计 / 占比……生成一个派生数据集
//!
//! 窗口函数不能放进计算列：计算列是在筛选、透视、公式下推的 SQL 里就地展开的表达式，而 `sum(…) OVER (…)` 不能出现在
//! WHERE 或别的聚合里。所以这里把结构化的设置翻译成一条只读 SQL（`SELECT *, <窗口表达式> FROM 数据集 [QUALIFY …]`，
//! 标识符一律转义，不拼接用户输入的 SQL），保存为 SQL 查询并另存为数据集；默认设为「源数据集更新后重算」，
//! 导入、同步、外部文件有变化后自动跟上。结果就是普通的列：能筛选、透视、做概况、写公式，SQL 工作台里也能继续改
//!
//! 排序列有并列值时，累计、上一行这类结果的先后不确定，可以多加一个更细的排序列（如时间之后再按单号）

use std::collections::HashMap;

use duckdb::Connection;
use serde::Deserialize;

use crate::dataset::{self, DatasetDef};
use crate::error::{AppError, AppResult};
use crate::kinds;
use crate::schema::ident;

const MAX_CALCS: usize = 20;
const MAX_MOVING_ROWS: u32 = 1000;
const PREVIEW_ROWS: usize = 100;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OrderSpec {
    pub column: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WindowCalc {
    /// running_sum 累计 / row_number 序号 / rank 排名（大的在前）/ rank_asc 排名（小的在前）/ prev 上一行的值 /
    /// diff 与上一行的差 / pct_change 较上一行的变化率 / moving_avg 移动平均 / group_sum 组内合计 / group_avg 组内平均 /
    /// group_count 组内行数 / share 占组内合计的比例
    pub func: String,
    /// 值列（序号、组内行数不需要）
    #[serde(default)]
    pub column: Option<String>,
    /// 新列名
    pub name: String,
    /// 移动平均的行数（含当前行，默认 7）
    #[serde(default)]
    pub rows: Option<u32>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct WindowSpec {
    pub calcs: Vec<WindowCalc>,
    /// 分组（为空表示整个数据集一组）
    #[serde(default)]
    pub partition_by: Vec<String>,
    /// 组内的先后顺序（累计、序号、上一行、移动平均、每组前 N 行要用）
    #[serde(default)]
    pub order_by: Vec<OrderSpec>,
    /// 每组只保留按 order_by 排在前面的 N 行
    #[serde(default)]
    pub top_n: Option<u32>,
}

/// 生成的 SQL 与各新列的类型
pub struct Plan {
    pub sql: String,
    pub column_kinds: HashMap<String, String>,
}

fn needs_order(func: &str) -> bool {
    matches!(func, "running_sum" | "row_number" | "prev" | "diff" | "pct_change" | "moving_avg")
}

fn needs_value(func: &str) -> bool {
    !matches!(func, "row_number" | "group_count")
}

fn label(func: &str) -> &'static str {
    match func {
        "running_sum" => "累计",
        "row_number" => "序号",
        "rank" | "rank_asc" => "排名",
        "prev" => "上一行的值",
        "diff" => "与上一行的差",
        "pct_change" => "较上一行的变化率",
        "moving_avg" => "移动平均",
        "group_sum" => "组内合计",
        "group_avg" => "组内平均",
        "group_count" => "组内行数",
        "share" => "占比",
        _ => "",
    }
}

/// 把设置翻译成 SQL（只读，标识符转义）
pub fn plan(def: &DatasetDef, spec: &WindowSpec) -> AppResult<Plan> {
    if spec.calcs.is_empty() {
        return Err(AppError::bad("请至少添加一个计算"));
    }
    if spec.calcs.len() > MAX_CALCS {
        return Err(AppError::bad(format!("一次最多 {MAX_CALCS} 个计算")));
    }
    let col = |key: &str| dataset::find_column(def, key);
    let partition: Vec<String> = spec.partition_by.iter().map(|k| col(k).map(|c| ident(&c.title))).collect::<AppResult<_>>()?;
    let order: Vec<String> = spec.order_by.iter().map(|o| col(&o.column).map(|c| format!("{} {} NULLS LAST", ident(&c.title), if o.desc { "DESC" } else { "ASC" }))).collect::<AppResult<_>>()?;
    let over = |order_sql: &str, frame: &str| -> String {
        let mut parts = Vec::new();
        if !partition.is_empty() {
            parts.push(format!("PARTITION BY {}", partition.join(", ")));
        }
        if !order_sql.is_empty() {
            parts.push(format!("ORDER BY {order_sql}"));
        }
        if !frame.is_empty() {
            parts.push(frame.to_string());
        }
        format!("OVER ({})", parts.join(" "))
    };
    let ordered = order.join(", ");
    let mut names: Vec<String> = Vec::new();
    let mut selects = Vec::new();
    let mut kinds_out = HashMap::new();
    for c in &spec.calcs {
        let what = label(&c.func);
        if what.is_empty() {
            return Err(AppError::bad(format!("不认识的计算：{}", c.func)));
        }
        let name = c.name.trim().to_string();
        if name.is_empty() || name.chars().count() > 40 {
            return Err(AppError::bad("新列名不能为空，且不超过 40 个字"));
        }
        if def.columns.iter().any(|x| x.title == name) || names.contains(&name) {
            return Err(AppError::bad(format!("列名「{name}」重复了，请换一个")));
        }
        if needs_order(&c.func) && order.is_empty() {
            return Err(AppError::bad(format!("「{name}」（{what}）要按某一列排序，请选择排序列")));
        }
        let value = if needs_value(&c.func) {
            let key = c.column.as_deref().filter(|s| !s.trim().is_empty()).ok_or_else(|| AppError::bad(format!("「{name}」（{what}）请选择值列")))?;
            let v = col(key)?;
            let numeric = kinds::is_numeric(v.kind);
            if !numeric && !matches!(c.func.as_str(), "prev") {
                return Err(AppError::bad(format!("「{name}」（{what}）的值列要是数值列，「{}」是{}列", v.title, kinds::label(v.kind))));
            }
            Some(v)
        } else {
            None
        };
        let v = value.map(|v| ident(&v.title)).unwrap_or_default();
        let prev = format!("lag({v}) {}", over(&ordered, ""));
        let expr = match c.func.as_str() {
            "running_sum" => format!("sum({v}) {}", over(&ordered, "ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW")),
            "row_number" => format!("row_number() {}", over(&ordered, "")),
            "rank" => format!("rank() {}", over(&format!("{v} DESC NULLS LAST"), "")),
            "rank_asc" => format!("rank() {}", over(&format!("{v} ASC NULLS LAST"), "")),
            "prev" => prev,
            "diff" => format!("{v} - {prev}"),
            "pct_change" => format!("({v} - {prev}) / NULLIF({prev}, 0)"),
            "moving_avg" => {
                let n = c.rows.unwrap_or(7);
                if !(2..=MAX_MOVING_ROWS).contains(&n) {
                    return Err(AppError::bad(format!("移动平均的行数应在 2 ~ {MAX_MOVING_ROWS} 之间")));
                }
                format!("avg({v}) {}", over(&ordered, &format!("ROWS BETWEEN {} PRECEDING AND CURRENT ROW", n - 1)))
            }
            "group_sum" => format!("sum({v}) {}", over("", "")),
            "group_avg" => format!("avg({v}) {}", over("", "")),
            "group_count" => format!("count(*) {}", over("", "")),
            _ => format!("{v} / NULLIF(sum({v}) {}, 0)", over("", "")),
        };
        let source_kind = value.map(|v| v.kind).unwrap_or("integer");
        let kind = match c.func.as_str() {
            "row_number" | "rank" | "rank_asc" | "group_count" => "integer",
            "pct_change" | "share" => "number",
            "group_avg" | "moving_avg" if source_kind == "integer" => "number",
            "prev" => source_kind,
            _ => source_kind,
        };
        kinds_out.insert(name.clone(), kind.to_string());
        selects.push(format!("{expr} AS {}", ident(&name)));
        names.push(name);
    }
    let mut sql = format!("SELECT *, {} FROM {}", selects.join(", "), ident(&def.name));
    if let Some(n) = spec.top_n {
        if n == 0 {
            return Err(AppError::bad("每组保留的行数至少是 1"));
        }
        if order.is_empty() {
            return Err(AppError::bad("每组只保留前几行时要选择排序列"));
        }
        sql.push_str(&format!(" QUALIFY row_number() {} <= {n}", over(&ordered, "")));
    }
    // 结果按分组、组内顺序排好：新数据集打开时一组一组、按先后排列
    let sort: Vec<String> = partition.iter().cloned().chain(order.iter().cloned()).collect();
    if !sort.is_empty() {
        sql.push_str(&format!(" ORDER BY {}", sort.join(", ")));
    }
    Ok(Plan { sql, column_kinds: kinds_out })
}

/// 预览：生成的 SQL 与前 100 行
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub sql: String,
    pub result: crate::sqlq::RunResult,
}

pub fn preview(conn: &Connection, dataset_id: &str, spec: &WindowSpec) -> AppResult<Preview> {
    let def = dataset::dataset(conn, dataset_id)?;
    let p = plan(&def, spec)?;
    let result = crate::sqlq::run_scoped(conn, &p.sql, PREVIEW_ROWS, None)?;
    Ok(Preview { sql: p.sql, result })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInput {
    #[serde(flatten)]
    pub spec: WindowSpec,
    /// 新数据集（也是 SQL 查询）的名称
    pub name: String,
    /// 源数据集更新后自动重算（默认是）
    #[serde(default = "yes")]
    pub auto_refresh: bool,
    #[serde(default)]
    pub operator: Option<String>,
}

fn yes() -> bool {
    true
}

/// 保存为 SQL 查询并写入新数据集，返回保存的查询（含数据集 id）
pub fn create(state: &crate::routes::AppState, conn: &Connection, dataset_id: &str, input: &CreateInput) -> AppResult<crate::sqlq::SavedQuery> {
    let def = dataset::dataset(conn, dataset_id)?;
    let p = plan(&def, &input.spec)?;
    let name = input.name.trim();
    let schedule = if input.auto_refresh { serde_json::json!({ "kind": "after_data", "datasetId": def.id }) } else { serde_json::json!({ "kind": "manual" }) };
    let q: crate::sqlq::QueryInput = serde_json::from_value(serde_json::json!({
        "name": name,
        "sql": p.sql,
        "description": format!("由「{}」的窗口计算生成：{}", def.name, input.spec.calcs.iter().map(|c| c.name.trim()).collect::<Vec<_>>().join("、")),
        "saveAsDataset": name,
        "columnKinds": p.column_kinds,
        "schedule": schedule,
        "operator": input.operator,
    }))?;
    crate::sqlq::save(state, conn, None, q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::ColumnDef;

    fn def() -> DatasetDef {
        let col = |title: &str, kind: &'static str| ColumnDef { key: title.into(), title: title.into(), kind, width: 100, expr: None, computed: false, formula: None, origin: None };
        DatasetDef {
            id: "ds1".into(),
            name: "订单".into(),
            table: "ds1".into(),
            columns: vec![col("店铺", "text"), col("下单时间", "datetime"), col("金额", "money"), col("数量", "integer")],
            searchable: false,
            supports_rule: false,
            builtin: false,
            primary_key: vec![],
            rid: "rowid".into(),
            kind: "table".into(),
            scope_sig: String::new(),
            view: None,
            summary: None,
            external: None,
        }
    }

    fn calc(func: &str, column: Option<&str>, name: &str) -> WindowCalc {
        WindowCalc { func: func.into(), column: column.map(String::from), name: name.into(), rows: None }
    }

    #[test]
    fn builds_sql_and_kinds() {
        let spec = WindowSpec {
            calcs: vec![calc("running_sum", Some("金额"), "累计金额"), calc("rank", Some("金额"), "排名"), calc("share", Some("金额"), "占比"), calc("moving_avg", Some("数量"), "七笔平均")],
            partition_by: vec!["店铺".into()],
            order_by: vec![OrderSpec { column: "下单时间".into(), desc: false }],
            top_n: Some(3),
        };
        let p = plan(&def(), &spec).unwrap();
        assert_eq!(
            p.sql,
            "SELECT *, sum(\"金额\") OVER (PARTITION BY \"店铺\" ORDER BY \"下单时间\" ASC NULLS LAST ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS \"累计金额\", \
             rank() OVER (PARTITION BY \"店铺\" ORDER BY \"金额\" DESC NULLS LAST) AS \"排名\", \
             \"金额\" / NULLIF(sum(\"金额\") OVER (PARTITION BY \"店铺\"), 0) AS \"占比\", \
             avg(\"数量\") OVER (PARTITION BY \"店铺\" ORDER BY \"下单时间\" ASC NULLS LAST ROWS BETWEEN 6 PRECEDING AND CURRENT ROW) AS \"七笔平均\" \
             FROM \"订单\" QUALIFY row_number() OVER (PARTITION BY \"店铺\" ORDER BY \"下单时间\" ASC NULLS LAST) <= 3 \
             ORDER BY \"店铺\", \"下单时间\" ASC NULLS LAST"
        );
        assert_eq!(p.column_kinds["累计金额"], "money");
        assert_eq!(p.column_kinds["排名"], "integer");
        assert_eq!(p.column_kinds["占比"], "number");
        assert_eq!(p.column_kinds["七笔平均"], "number");
    }

    #[test]
    fn validation() {
        let bad = |spec: WindowSpec| plan(&def(), &spec).err().map(|e| e.message()).unwrap_or_default();
        let one = |c: WindowCalc| WindowSpec { calcs: vec![c], ..Default::default() };
        assert!(bad(WindowSpec::default()).contains("至少"));
        assert!(bad(one(calc("running_sum", Some("金额"), "累计"))).contains("排序列"));
        assert!(bad(one(calc("group_sum", Some("店铺"), "合计"))).contains("数值列"));
        assert!(bad(one(calc("group_sum", Some("金额"), "金额"))).contains("重复"));
        assert!(bad(one(calc("explode", Some("金额"), "x"))).contains("不认识"));
        assert!(bad(one(calc("group_sum", Some("不存在"), "x"))).contains("不存在"));
        assert!(bad(WindowSpec { top_n: Some(2), ..one(calc("group_count", None, "行数")) }).contains("排序列"));
        // 标识符转义：列名里的引号不会变成 SQL
        let mut d = def();
        d.columns[2].title = "金\"额".into();
        d.columns[2].key = "金\"额".into();
        let p = plan(&d, &one(calc("group_sum", Some("金\"额"), "合计\" FROM x; --"))).unwrap();
        assert!(p.sql.contains("sum(\"金\"\"额\")") && p.sql.contains("AS \"合计\"\" FROM x; --\""), "{}", p.sql);
    }
}
