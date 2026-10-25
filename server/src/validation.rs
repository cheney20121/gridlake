//! 数据校验规则：给数据集的列设必填、不能重复、取值范围、格式（手机号、邮箱、日期…）、只能是某几个值，
//! 一次扫描数出每条规则不合格的行数并给出样例；数据表可以「只看不合格的行」（[`QuerySpec::violation`]，
//! 和其他筛选一样能透视、导出）。导入、同步、外部文件刷新、清洗之后自动重新检查（[`recheck`]），
//! 有不合格时发 `dataset.validation_failed` 事件
//!
//! 规则里的值（正则、允许的取值、范围）一律作为参数传给 SQL，不拼进语句

use duckdb::types::Value;
use duckdb::{Connection, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::auth::CurrentUser;
use crate::dataset::{self, ColumnDef, DatasetDef};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::kinds;

const MAX_RULES: usize = 50;
const MAX_VALUES: usize = 500;
const SAMPLES: usize = 5;

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    /// 必填：空值、只有空白都不行
    Required,
    /// 不能重复（空值不算）
    Unique,
    /// 取值范围（含两端）：数值列按数字比，日期时间列按「2026-09-15 10:00:00」写法比；只写一端也行
    Range {
        #[serde(default)]
        min: Option<Json>,
        #[serde(default)]
        max: Option<Json>,
    },
    /// 格式：phone 手机号 / email 邮箱 / date 日期 / digits 纯数字 / idcard 身份证号 / regex 自定义正则（pattern）
    Format {
        format: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
    },
    /// 只能是这几个值之一
    Allowed { values: Vec<String> },
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub column: String,
    #[serde(flatten)]
    pub check: Check,
}

const FORMATS: &[(&str, &str, &str)] = &[
    ("phone", "手机号", r"1[3-9]\d{9}"),
    ("email", "邮箱", r"[^@\s]+@[^@\s]+\.[^@\s]+"),
    ("date", "日期", r"\d{4}-\d{2}-\d{2}( \d{2}:\d{2}(:\d{2})?)?"),
    ("digits", "纯数字", r"\d+"),
    ("idcard", "身份证号", r"\d{17}[\dXx]"),
];

fn format_of(format: &str, pattern: Option<&str>) -> AppResult<(&'static str, String)> {
    if format == "regex" {
        let p = pattern.map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| AppError::bad("自定义格式要填正则表达式"))?;
        if p.chars().count() > 200 {
            return Err(AppError::bad("正则表达式太长（最多 200 个字符）"));
        }
        return Ok(("指定格式", p.to_string()));
    }
    FORMATS.iter().find(|f| f.0 == format).map(|f| (f.1, f.2.to_string())).ok_or_else(|| AppError::bad(format!("不认识的格式：{format}")))
}

fn text_of(v: &Json) -> String {
    v.as_str().map(String::from).unwrap_or_else(|| v.to_string())
}

/// 规则的说明：「手机号」必填 / 「金额」在 0 ~ 10000 之间 / …
pub fn label(rule: &Rule) -> String {
    let c = &rule.column;
    match &rule.check {
        Check::Required => format!("「{c}」必填"),
        Check::Unique => format!("「{c}」不能重复"),
        Check::Range { min, max } => match (min.as_ref().filter(|v| !v.is_null()), max.as_ref().filter(|v| !v.is_null())) {
            (Some(a), Some(b)) => format!("「{c}」在 {} ~ {} 之间", text_of(a), text_of(b)),
            (Some(a), None) => format!("「{c}」不小于 {}", text_of(a)),
            (None, Some(b)) => format!("「{c}」不大于 {}", text_of(b)),
            (None, None) => format!("「{c}」的范围"),
        },
        Check::Format { format, pattern } => match format_of(format, pattern.as_deref()) {
            Ok((_, p)) if format == "regex" => format!("「{c}」符合 {p}"),
            Ok((name, _)) => format!("「{c}」是{name}格式"),
            Err(_) => format!("「{c}」的格式"),
        },
        Check::Allowed { values } => {
            let shown: Vec<&str> = values.iter().take(8).map(String::as_str).collect();
            format!("「{c}」只能是 {}{}", shown.join(" / "), if values.len() > 8 { " …" } else { "" })
        }
    }
}

fn blank(col: &ColumnDef, c: &str) -> String {
    if kinds::sql_type(col.kind) == "VARCHAR" { format!("({c} IS NULL OR trim(CAST({c} AS VARCHAR)) = '')") } else { format!("{c} IS NULL") }
}

/// 不合格的行的条件（数据表不加别名）与参数
pub fn violation_sql(def: &DatasetDef, rule: &Rule) -> AppResult<(String, Vec<Value>)> {
    let col = dataset::find_column(def, &rule.column)?;
    let c = format!("({})", dataset::column_sql(col));
    let empty = blank(col, &c);
    let text = format!("trim(CAST({c} AS VARCHAR))");
    Ok(match &rule.check {
        Check::Required => (empty, vec![]),
        Check::Unique => {
            let v = format!("({})", dataset::column_sql(col));
            let inner_blank = blank(col, &v);
            (format!("(NOT {empty} AND {c} IN (SELECT {v} FROM {} __u WHERE NOT {inner_blank} GROUP BY {v} HAVING count(*) > 1))", def.table), vec![])
        }
        Check::Range { min, max } => {
            let numeric = kinds::is_numeric(col.kind);
            let mut ors = Vec::new();
            let mut args = Vec::new();
            for (bound, op) in [(min, "<"), (max, ">")] {
                let Some(v) = bound.as_ref().filter(|v| !v.is_null()) else { continue };
                if numeric {
                    let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).ok_or_else(|| AppError::bad(format!("「{}」的范围要写数字", col.title)))?;
                    ors.push(format!("{c} {op} ?"));
                    args.push(Value::Double(n));
                } else {
                    let mut s = text_of(v).trim().to_string();
                    // 结束日期只写到日：算到当天结束
                    if op == ">" && s.len() == 10 {
                        s.push_str(" 23:59:59");
                    }
                    ors.push(format!("CAST({c} AS VARCHAR) {op} ?"));
                    args.push(Value::Text(s));
                }
            }
            if ors.is_empty() {
                return Err(AppError::bad(format!("「{}」的范围至少要写最小值或最大值", col.title)));
            }
            (format!("(NOT {empty} AND ({}))", ors.join(" OR ")), args)
        }
        Check::Format { format, pattern } => {
            let (_, p) = format_of(format, pattern.as_deref())?;
            (format!("(NOT {empty} AND NOT regexp_full_match({text}, ?))"), vec![Value::Text(p)])
        }
        Check::Allowed { values } => {
            if values.is_empty() {
                return Err(AppError::bad(format!("「{}」允许的取值不能为空", col.title)));
            }
            (format!("(NOT {empty} AND {text} NOT IN ({}))", vec!["?"; values.len()].join(", ")), values.iter().map(|v| Value::Text(v.trim().to_string())).collect())
        }
    })
}

/// 保存前检查规则：列存在、范围只用于数值 / 日期列、正则写得对
fn check_rules(conn: &Connection, def: &DatasetDef, rules: &[Rule]) -> AppResult<()> {
    if rules.len() > MAX_RULES {
        return Err(AppError::bad(format!("一个数据集最多 {MAX_RULES} 条校验规则")));
    }
    for r in rules {
        let col = dataset::find_column(def, &r.column)?;
        match &r.check {
            Check::Range { min, max } => {
                if !(kinds::is_numeric(col.kind) || col.kind == "datetime") {
                    return Err(AppError::bad(format!("「{}」是{}列，取值范围只能用于数值、金额、整数和日期时间列", col.title, kinds::label(col.kind))));
                }
                if kinds::is_numeric(col.kind) {
                    let num = |v: &Option<Json>| v.as_ref().filter(|v| !v.is_null()).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())));
                    if let (Some(a), Some(b)) = (num(min), num(max))
                        && a > b
                    {
                        return Err(AppError::bad(format!("「{}」的最小值不能大于最大值", col.title)));
                    }
                }
            }
            Check::Format { format, pattern } => {
                let (_, p) = format_of(format, pattern.as_deref())?;
                conn.query_row("SELECT regexp_full_match('', ?)", params![p], |r| r.get::<_, bool>(0)).map_err(|_| AppError::bad(format!("「{}」的正则表达式写得不对：{p}", col.title)))?;
            }
            Check::Allowed { values } => {
                if values.iter().all(|v| v.trim().is_empty()) {
                    return Err(AppError::bad(format!("「{}」允许的取值不能为空", col.title)));
                }
                if values.len() > MAX_VALUES {
                    return Err(AppError::bad(format!("「{}」允许的取值最多 {MAX_VALUES} 个", col.title)));
                }
            }
            _ => {}
        }
        violation_sql(def, r)?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RuleResult {
    pub rule: Rule,
    pub label: String,
    /// 不合格的行数
    pub violations: i64,
    /// 不合格的值（举例，不同的值）
    pub samples: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub rows: i64,
    /// 至少违反一条规则的行数
    pub bad_rows: i64,
    /// 有不合格的规则条数
    pub failed: usize,
    pub results: Vec<RuleResult>,
    pub checked_at: String,
    pub elapsed_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Validation {
    pub rules: Vec<Rule>,
    /// 最近一次检查的结果（规则改过、数据变过后会重新检查）
    pub report: Option<Report>,
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
}

/// 数据集列表里的小结（数据表上显示「校验：N 项不通过」）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub rules: usize,
    pub failed: usize,
    pub bad_rows: i64,
    pub checked_at: Option<String>,
}

fn load(conn: &Connection, id: &str) -> AppResult<Option<(Vec<Rule>, Option<Report>, Option<String>, Option<String>)>> {
    let row: Option<(String, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT rules, report, updated_by, strftime(updated_at, '%Y-%m-%d %H:%M:%S') FROM ledger_validation WHERE dataset_id = ?",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    let Some((rules, report, by, at)) = row else { return Ok(None) };
    Ok(Some((serde_json::from_str(&rules)?, report.and_then(|r| serde_json::from_str(&r).ok()), by, at)))
}

pub fn get(conn: &Connection, id: &str) -> AppResult<Validation> {
    let def = dataset::dataset(conn, id)?;
    Ok(match load(conn, &def.id)? {
        Some((rules, report, updated_by, updated_at)) => Validation { rules, report, updated_by, updated_at },
        None => Validation { rules: vec![], report: None, updated_by: None, updated_at: None },
    })
}

/// 所有数据集的小结（列数据集时用）
pub fn summaries(conn: &Connection) -> AppResult<std::collections::HashMap<String, Summary>> {
    let mut st = conn.prepare("SELECT dataset_id, rules, report FROM ledger_validation")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?;
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let (id, rules, report) = row?;
        let rules: Vec<Rule> = serde_json::from_str(&rules).unwrap_or_default();
        if rules.is_empty() {
            continue;
        }
        let report: Option<Report> = report.and_then(|r| serde_json::from_str(&r).ok());
        out.insert(id, Summary { rules: rules.len(), failed: report.as_ref().map_or(0, |r| r.failed), bad_rows: report.as_ref().map_or(0, |r| r.bad_rows), checked_at: report.map(|r| r.checked_at) });
    }
    Ok(out)
}

/// 按规则检查一遍（数据范围受限的用户只检查自己能看到的行）
pub fn run(conn: &Connection, def: &DatasetDef, rules: &[Rule]) -> AppResult<Report> {
    let started = std::time::Instant::now();
    let mut conds = Vec::new();
    let mut args = Vec::new();
    for r in rules {
        let (c, a) = violation_sql(def, r)?;
        conds.push(c);
        args.extend(a);
    }
    let mut selects = vec!["count(*)".to_string()];
    selects.extend(conds.iter().map(|c| format!("count(*) FILTER (WHERE {c})")));
    let any = if conds.is_empty() { "FALSE".to_string() } else { conds.join(" OR ") };
    selects.push(format!("count(*) FILTER (WHERE {any})"));
    let mut all_args = args.clone();
    all_args.extend(args.iter().cloned());
    let n = conds.len();
    let counts: Vec<i64> = conn.query_row(&format!("SELECT {} FROM {}", selects.join(", "), def.table), params_from_iter(all_args.iter()), |r| (0..n + 2).map(|i| r.get(i)).collect())?;
    let mut results = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        let violations = counts[i + 1];
        let samples = if violations > 0 {
            let (c, a) = violation_sql(def, r)?;
            let col = dataset::find_column(def, &r.column)?;
            let v = format!("CAST(({}) AS VARCHAR)", dataset::column_sql(col));
            let mut st = conn.prepare(&format!("SELECT DISTINCT {v} FROM {} WHERE {c} LIMIT {SAMPLES}", def.table))?;
            st.query_map(params_from_iter(a.iter()), |row| row.get::<_, Option<String>>(0))?.map(|x| x.map(|o| o.unwrap_or_default())).collect::<Result<Vec<_>, _>>()?
        } else {
            vec![]
        };
        results.push(RuleResult { rule: r.clone(), label: label(r), violations, samples });
    }
    Ok(Report {
        rows: counts[0],
        bad_rows: counts[n + 1],
        failed: results.iter().filter(|r| r.violations > 0).count(),
        results,
        checked_at: now_local(),
        elapsed_ms: started.elapsed().as_millis() as i64,
    })
}

fn store_report(conn: &Connection, id: &str, report: &Report) -> AppResult<()> {
    conn.execute("UPDATE ledger_validation SET report = ? WHERE dataset_id = ?", params![serde_json::to_string(report)?, id])?;
    Ok(())
}

/// 保存规则并立即检查一遍
pub fn save(conn: &Connection, id: &str, rules: Vec<Rule>, user: &CurrentUser) -> AppResult<Validation> {
    let def = dataset::dataset(conn, id)?;
    let rules: Vec<Rule> = rules
        .into_iter()
        .map(|mut r| {
            // 列名统一成列标题
            if let Ok(c) = dataset::find_column(&def, &r.column) {
                r.column = c.title.clone();
            }
            r
        })
        .collect();
    check_rules(conn, &def, &rules)?;
    let operator = if user.name.is_empty() { None } else { Some(user.name.clone()) };
    conn.execute(
        "INSERT INTO ledger_validation (dataset_id, rules, report, updated_by, updated_at) VALUES (?, ?, NULL, ?, CAST(? AS TIMESTAMP))
         ON CONFLICT (dataset_id) DO UPDATE SET rules = excluded.rules, report = NULL, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![def.id, serde_json::to_string(&rules)?, operator, now_local()],
    )?;
    if !rules.is_empty() && def.scope_sig.is_empty() {
        let report = run(conn, &def, &rules)?;
        store_report(conn, &def.id, &report)?;
    }
    get(conn, &def.id)
}

/// 手动检查：数据范围受限的用户只检查自己能看到的行，结果不保存（不代表整个数据集）
pub fn check(conn: &Connection, id: &str) -> AppResult<Report> {
    let def = dataset::dataset(conn, id)?;
    let Some((rules, ..)) = load(conn, &def.id)? else {
        return Err(AppError::bad(format!("「{}」还没有设置校验规则", def.name)));
    };
    if rules.is_empty() {
        return Err(AppError::bad(format!("「{}」还没有设置校验规则", def.name)));
    }
    let report = run(conn, &def, &rules)?;
    if def.scope_sig.is_empty() {
        store_report(conn, &def.id, &report)?;
    }
    Ok(report)
}

/// 数据变了以后重新检查（后台、不受数据范围限制）；返回检查结果，没有规则时为 None
pub fn recheck(conn: &Connection, id: &str) -> AppResult<Option<(DatasetDef, Report)>> {
    let Some((rules, ..)) = load(conn, id)? else { return Ok(None) };
    if rules.is_empty() {
        return Ok(None);
    }
    let def = dataset::dataset(conn, id)?;
    let report = run(conn, &def, &rules)?;
    store_report(conn, id, &report)?;
    Ok(Some((def, report)))
}

/// 数据集彻底删除时清掉它的规则
pub fn forget(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_validation WHERE dataset_id = ?", params![id])?;
    Ok(())
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
            "会员",
            &[("编号".into(), "id".into()), ("手机号".into(), "text".into()), ("金额".into(), "money".into()), ("日期".into(), "datetime".into()), ("等级".into(), "text".into())],
            &[],
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO {id} VALUES
             ('A1', '13800000000', 10, '2026-09-01 10:00:00', '金卡', NULL),
             ('A2', '1380000', -5, '2026-13-01', '银卡', NULL),
             ('A2', '', 20000, '2026-09-30 23:00:00', '钻石', NULL),
             ('A3', NULL, NULL, '2027-01-01 00:00:00', ' ', NULL)"
        ))
        .unwrap();
        (conn, id)
    }

    fn rule(v: Json) -> Rule {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn counts_each_rule_in_one_scan() {
        let (conn, id) = setup();
        let def = dataset::dataset(&conn, &id).unwrap();
        let rules = vec![
            rule(serde_json::json!({ "column": "手机号", "kind": "required" })),
            rule(serde_json::json!({ "column": "编号", "kind": "unique" })),
            rule(serde_json::json!({ "column": "金额", "kind": "range", "min": 0, "max": "10000" })),
            rule(serde_json::json!({ "column": "手机号", "kind": "format", "format": "phone" })),
            rule(serde_json::json!({ "column": "等级", "kind": "allowed", "values": ["金卡", "银卡"] })),
            rule(serde_json::json!({ "column": "日期", "kind": "range", "min": "2026-09-01", "max": "2026-12-31" })),
        ];
        check_rules(&conn, &def, &rules).unwrap();
        let r = run(&conn, &def, &rules).unwrap();
        let v: Vec<i64> = r.results.iter().map(|x| x.violations).collect();
        // 手机号空 2（空字符串、NULL）；编号 A2 重复 2 行；金额 -5、20000；手机号格式 1380000（空的不算）；
        // 等级「钻石」（只有空白的不算）；日期 2026-13-01 字面上大于 12-31 23:59:59、2027-01-01
        assert_eq!(v, vec![2, 2, 2, 1, 1, 2]);
        assert_eq!((r.rows, r.failed), (4, 6));
        assert_eq!(r.bad_rows, 3, "A1 那一行全部合格");
        assert_eq!(r.results[1].samples, vec!["A2"]);
        assert_eq!(r.results[2].label, "「金额」在 0 ~ 10000 之间");
        assert_eq!(r.results[3].label, "「手机号」是手机号格式");
        assert_eq!(r.results[4].label, "「等级」只能是 金卡 / 银卡");
    }

    #[test]
    fn rejects_bad_rules() {
        let (conn, id) = setup();
        let def = dataset::dataset(&conn, &id).unwrap();
        let err = |v: Json| check_rules(&conn, &def, &[rule(v)]).unwrap_err().message();
        assert!(err(serde_json::json!({ "column": "等级", "kind": "range", "min": 1 })).contains("取值范围只能用于"));
        assert!(err(serde_json::json!({ "column": "金额", "kind": "range" })).contains("至少要写"));
        assert!(err(serde_json::json!({ "column": "金额", "kind": "range", "min": 9, "max": 1 })).contains("不能大于"));
        assert!(err(serde_json::json!({ "column": "手机号", "kind": "format", "format": "regex", "pattern": "(" })).contains("正则表达式写得不对"));
        assert!(err(serde_json::json!({ "column": "手机号", "kind": "format", "format": "qq" })).contains("不认识的格式"));
        assert!(err(serde_json::json!({ "column": "等级", "kind": "allowed", "values": [" "] })).contains("不能为空"));
        assert!(err(serde_json::json!({ "column": "没有", "kind": "required" })).contains("列不存在"));
    }
}
