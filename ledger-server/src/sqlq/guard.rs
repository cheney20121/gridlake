//! SQL 安全检查：用 DuckDB 自带的解析器（json_serialize_sql）把语句解析成语法树，按白名单检查
//!
//! - 只能是一条 SELECT（含 WITH / UNION / 子查询）；其他语句解析器直接拒绝
//! - 表只能是数据集名称或语句内定义的 CTE；不能带库名 / 模式名（如 main.ledger_detail）；
//!   不能直接读文件（FROM 'a.csv' 也会被当作表名而拒绝）
//! - 表函数只允许生成序列的 range / generate_series / unnest；read_csv、query、duckdb_tables 等一律拒绝
//! - 拒绝读取环境变量、配置、序列、执行 SQL 的函数（getenv、current_setting、nextval、json_execute_serialized_sql…）
//! - 不认识的语法节点类型一律拒绝（宁可误拒，不可放过）

use std::collections::BTreeSet;

use duckdb::Connection;
use serde_json::Value as Json;

use crate::error::{AppError, AppResult};

/// 允许的表引用类型
const TABLE_REF_TYPES: [&str; 7] = ["BASE_TABLE", "JOIN", "SUBQUERY", "TABLE_FUNCTION", "EMPTY", "EXPRESSION_LIST", "PIVOT"];
/// 允许的表函数
const TABLE_FUNCTIONS: [&str; 3] = ["range", "generate_series", "unnest"];
/// 禁用的函数（精确名）
const BLOCKED_FUNCTIONS: [&str; 15] = [
    "getenv",
    "current_setting",
    "nextval",
    "currval",
    "setseed",
    "glob",
    "read_text",
    "read_blob",
    "json_serialize_sql",
    "json_deserialize_sql",
    "json_execute_serialized_sql",
    "json_serialize_plan",
    "query",
    "query_table",
    "which_secret",
];
/// 禁用的函数前缀
const BLOCKED_PREFIXES: [&str; 11] = ["read_", "duckdb_", "pragma_", "parquet_", "sql_", "iceberg_", "delta_", "load", "install", "enable_", "disable_"];

/// 检查通过的语句
#[derive(Debug, Default)]
pub struct Checked {
    /// 去掉结尾分号后的 SQL
    pub sql: String,
    /// 引用到的表名（数据集名，不含 CTE）
    pub tables: BTreeSet<String>,
}

/// 去掉结尾的分号与空白
pub fn trim_sql(sql: &str) -> String {
    let mut s = sql.trim();
    while let Some(x) = s.strip_suffix(';') {
        s = x.trim_end();
    }
    s.to_string()
}

fn blocked_function(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    BLOCKED_FUNCTIONS.contains(&n.as_str()) || BLOCKED_PREFIXES.iter().any(|p| n.starts_with(p))
}

#[derive(Default)]
struct Walk {
    tables: BTreeSet<String>,
    ctes: BTreeSet<String>,
    errors: Vec<String>,
}

impl Walk {
    fn node(&mut self, v: &Json) {
        match v {
            Json::Array(items) => items.iter().for_each(|x| self.node(x)),
            Json::Object(map) => {
                // CTE 名称：cte_map.map[].key
                if let Some(list) = map.get("cte_map").and_then(|c| c.get("map")).and_then(Json::as_array) {
                    for e in list {
                        if let Some(k) = e.get("key").and_then(Json::as_str) {
                            self.ctes.insert(k.to_lowercase());
                        }
                    }
                }
                let ty = map.get("type").and_then(Json::as_str).unwrap_or("");
                // 表引用（带 alias 与 sample 字段的节点）
                if map.contains_key("sample") && map.contains_key("alias") && map.contains_key("query_location") && !map.contains_key("class") && !map.contains_key("select_list") {
                    if !TABLE_REF_TYPES.contains(&ty) {
                        self.errors.push(format!("不支持的语法：{ty}"));
                    }
                    if ty == "BASE_TABLE" {
                        let schema = map.get("schema_name").and_then(Json::as_str).unwrap_or("");
                        let catalog = map.get("catalog_name").and_then(Json::as_str).unwrap_or("");
                        let name = map.get("table_name").and_then(Json::as_str).unwrap_or("").to_string();
                        if !schema.is_empty() || !catalog.is_empty() {
                            self.errors.push(format!("不能指定库或模式：{}{}{name}", if catalog.is_empty() { String::new() } else { format!("{catalog}.") }, if schema.is_empty() { String::new() } else { format!("{schema}.") }));
                        } else if map.get("at_clause").is_some_and(|a| !a.is_null()) {
                            self.errors.push("不支持 AT 时间旅行语法".into());
                        } else {
                            self.tables.insert(name);
                        }
                    }
                    if ty == "TABLE_FUNCTION" {
                        let f = map.get("function").and_then(|f| f.get("function_name")).and_then(Json::as_str).unwrap_or("");
                        if !TABLE_FUNCTIONS.contains(&f.to_ascii_lowercase().as_str()) {
                            self.errors.push(format!("不能使用表函数 {f}（只能查询数据集）"));
                        }
                    }
                }
                // 函数调用
                if map.get("class").and_then(Json::as_str) == Some("FUNCTION") {
                    let name = map.get("function_name").and_then(Json::as_str).unwrap_or("");
                    let schema = map.get("schema").and_then(Json::as_str).unwrap_or("");
                    let catalog = map.get("catalog").and_then(Json::as_str).unwrap_or("");
                    if !schema.is_empty() || !catalog.is_empty() {
                        self.errors.push(format!("不能调用指定模式的函数：{schema}.{name}"));
                    } else if blocked_function(name) {
                        self.errors.push(format!("不能使用函数 {name}"));
                    }
                }
                for (k, child) in map {
                    // 表函数节点下的 function 已在上面检查（range 等），其参数里的子查询仍需遍历
                    if k == "type" || k == "alias" {
                        continue;
                    }
                    self.node(child);
                }
            }
            _ => {}
        }
    }
}

/// 检查 SQL；known 判断一个表名是否为数据集（大小写按原样比较）
pub fn check(conn: &Connection, sql: &str, known: &dyn Fn(&str) -> bool) -> AppResult<Checked> {
    let sql = trim_sql(sql);
    if sql.is_empty() {
        return Err(AppError::bad("请输入 SQL"));
    }
    if sql.len() > 200_000 {
        return Err(AppError::bad("SQL 过长"));
    }
    let ast: String = conn.query_row("SELECT json_serialize_sql(?::VARCHAR)", [&sql], |r| r.get(0)).map_err(|e| AppError::bad(format!("SQL 解析失败：{e}")))?;
    let v: Json = serde_json::from_str(&ast).map_err(|e| AppError::bad(format!("SQL 解析失败：{e}")))?;
    if v.get("error").and_then(Json::as_bool) == Some(true) {
        let msg = v.get("error_message").and_then(Json::as_str).unwrap_or("");
        if msg.contains("Only SELECT") {
            return Err(AppError::bad("只能执行查询（SELECT / WITH）；修改、删除数据或建表等语句不允许"));
        }
        let detail = msg.trim_start_matches("Parser Error: ");
        // 常见原因：别名是关键字（如 cost、value、year）却没写 AS
        let hint = if detail.contains("syntax error at or near") { "。如果这是列的别名，请在前面加 AS（如 sum(花费) AS 消耗）" } else { "" };
        return Err(AppError::bad(format!("SQL 语法错误：{detail}{hint}")));
    }
    let statements = v.get("statements").and_then(Json::as_array).cloned().unwrap_or_default();
    if statements.len() != 1 {
        return Err(AppError::bad("一次只能执行一条查询语句"));
    }
    let mut w = Walk::default();
    w.node(&statements[0]);
    if let Some(e) = w.errors.first() {
        return Err(AppError::bad(e.clone()));
    }
    let mut tables = BTreeSet::new();
    for t in &w.tables {
        if w.ctes.contains(&t.to_lowercase()) {
            continue;
        }
        if !known(t) {
            return Err(AppError::bad(format!("没有数据集「{t}」（只能查询数据集；名称含空格或符号时用双引号括起来）")));
        }
        tables.insert(t.clone());
    }
    Ok(Checked { sql, tables })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sql: &str) -> Result<Vec<String>, String> {
        let c = Connection::open_in_memory().unwrap();
        check(&c, sql, &|t| t == "销售订单" || t == "退款").map(|x| x.tables.into_iter().collect()).map_err(|e| e.message())
    }

    #[test]
    fn allows_queries_on_datasets() {
        assert_eq!(run("SELECT 店铺, sum(订单金额) AS 金额 FROM 销售订单 GROUP BY 1 ORDER BY 2 DESC;").unwrap(), vec!["销售订单"]);
        assert_eq!(run("WITH m AS (SELECT * FROM \"销售订单\") SELECT * FROM m JOIN 退款 USING (订单号)").unwrap(), vec!["退款", "销售订单"]);
        assert!(run("SELECT * FROM 销售订单 WHERE 店铺 IN (SELECT 店铺 FROM 退款) UNION ALL SELECT * FROM 销售订单").is_ok());
        assert!(run("SELECT range FROM range(10)").is_ok());
        assert!(run("SELECT strftime(now(), '%Y'), regexp_replace(店铺, 'a', 'b') FROM 销售订单").is_ok());
    }

    #[test]
    fn rejects_everything_else() {
        let bad = [
            ("DELETE FROM 销售订单", "只能执行查询"),
            ("DROP TABLE 销售订单", "只能执行查询"),
            ("SELECT 1; DROP TABLE x", "只能执行查询"),
            ("SELECT 1; SELECT 2", "一次只能执行一条"),
            ("SELECT * FROM ledger_workbook", "没有数据集"),
            ("SELECT * FROM main.ledger_detail", "不能指定库或模式"),
            ("SELECT * FROM 销售订单 WHERE x IN (SELECT id FROM ledger_dataset)", "没有数据集"),
            ("SELECT * FROM read_csv('/etc/passwd')", "表函数"),
            ("SELECT * FROM '/etc/passwd'", "没有数据集"),
            ("SELECT * FROM query('select 1')", "表函数"),
            ("SELECT * FROM duckdb_tables()", "表函数"),
            ("SELECT getenv('HOME')", "getenv"),
            ("SELECT current_setting('memory_limit')", "current_setting"),
            ("SELECT * FROM 销售订单 WHERE 店铺 = (SELECT read_text FROM read_text('/etc/hosts'))", "表函数"),
            ("COPY (SELECT 1) TO '/tmp/x.csv'", "只能执行查询"),
            ("ATTACH '/tmp/x.db'", "只能执行查询"),
            ("SELECT pg_catalog.version()", "指定模式"),
            ("SELECT nextval('s')", "nextval"),
        ];
        for (sql, expect) in bad {
            let e = run(sql).err().unwrap_or_else(|| panic!("应拒绝：{sql}"));
            assert!(e.contains(expect), "{sql} → {e}");
        }
    }
}
