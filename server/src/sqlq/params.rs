//! 参数化查询：SQL 里的 `{{名称}}` 占位，运行时按类型安全地换成字面量
//!
//! - 文本 → 单引号字符串（引号加倍）；数字 → 数值字面量；日期 → `'yyyy-mm-dd'`（校验过格式；字符串字面量能同时与文本日期列、DATE 比较）。值是填入的而不是拼进 SQL 的文本，不会变成 SQL 语法
//! - 占位符不要自己加引号（`'{{店铺}}'` 会被拒绝）：引号由类型决定
//! - 文本参数可以限定可选值（choices）
//! - 一次替换：值里就算含有 `{{x}}` 也不会再被展开

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

const MAX_PARAMS: usize = 12;
const NAME_MAX: usize = 20;
const VALUE_MAX: usize = 200;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParamDef {
    pub name: String,
    /// text / number / date
    pub kind: String,
    #[serde(default)]
    pub default: Option<String>,
    /// 文本参数的可选值（为空不限制）
    #[serde(default)]
    pub choices: Vec<String>,
}

/// SQL 里出现的占位符名称（去重，按出现顺序）
pub fn placeholders(sql: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, _, _) in scan(sql) {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// (名称, 起, 止)，止不含 `}}`
fn scan(sql: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = sql[i..].find("{{") {
        let start = i + p;
        let Some(q) = sql[start + 2..].find("}}") else { break };
        let end = start + 2 + q + 2;
        out.push((sql[start + 2..start + 2 + q].trim().to_string(), start, end));
        i = end;
    }
    out
}

/// 参数定义自身是否合规，并与 SQL 里的占位符一一对应
pub fn check_defs(sql: &str, defs: &[ParamDef]) -> AppResult<()> {
    if defs.len() > MAX_PARAMS {
        return Err(AppError::bad(format!("参数最多 {MAX_PARAMS} 个")));
    }
    let mut seen = HashSet::new();
    for d in defs {
        let n = d.name.trim();
        if n.is_empty() || n.chars().count() > NAME_MAX || n.contains(['{', '}', '\'', '"']) {
            return Err(AppError::bad(format!("参数名「{n}」不合法：不能为空，不超过 {NAME_MAX} 个字，不含括号和引号")));
        }
        if !seen.insert(n.to_string()) {
            return Err(AppError::bad(format!("参数名「{n}」重复")));
        }
        if !matches!(d.kind.as_str(), "text" | "number" | "date") {
            return Err(AppError::bad(format!("参数「{n}」的类型应为文本 / 数字 / 日期")));
        }
        if !d.choices.is_empty() && d.kind != "text" {
            return Err(AppError::bad(format!("参数「{n}」：只有文本参数可以限定可选值")));
        }
        if let Some(v) = d.default.as_deref().filter(|v| !v.is_empty()) {
            literal(d, v)?;
        }
    }
    let used = placeholders(sql);
    for n in &used {
        if !defs.iter().any(|d| &d.name == n) {
            return Err(AppError::bad(format!("SQL 里用了参数 {{{{{n}}}}}，但没有定义它")));
        }
    }
    for d in defs {
        if !used.contains(&d.name) {
            return Err(AppError::bad(format!("参数「{}」没有在 SQL 里使用", d.name)));
        }
    }
    for (name, s, e) in scan(sql) {
        let before = sql[..s].chars().next_back();
        let after = sql[e..].chars().next();
        if before == Some('\'') && after == Some('\'') {
            return Err(AppError::bad(format!("{{{{{name}}}}} 不要自己加引号：文本参数会自动加引号")));
        }
    }
    Ok(())
}

/// 一个值换成 SQL 字面量（已校验类型与可选值）
fn literal(d: &ParamDef, value: &str) -> AppResult<String> {
    let v = value.trim();
    if v.chars().count() > VALUE_MAX {
        return Err(AppError::bad(format!("参数「{}」的值太长（最多 {VALUE_MAX} 个字）", d.name)));
    }
    match d.kind.as_str() {
        "number" => {
            let n: f64 = v.replace(',', "").parse().map_err(|_| AppError::bad(format!("参数「{}」应为数字：{v}", d.name)))?;
            if !n.is_finite() {
                return Err(AppError::bad(format!("参数「{}」应为有限的数字", d.name)));
            }
            Ok(format!("{n}"))
        }
        "date" => {
            let day = NaiveDate::parse_from_str(v, "%Y-%m-%d").map_err(|_| AppError::bad(format!("参数「{}」应为日期，格式 2026-09-01：{v}", d.name)))?;
            // 写成字符串字面量：数据集里的日期列是文本（2026-09-01 或带时分秒），文本比较和 CAST(列 AS DATE) 比较都成立
            Ok(format!("'{}'", day.format("%Y-%m-%d")))
        }
        _ => {
            if v.chars().any(char::is_control) {
                return Err(AppError::bad(format!("参数「{}」的值含有不允许的控制字符", d.name)));
            }
            if !d.choices.is_empty() && !d.choices.iter().any(|c| c == v) {
                return Err(AppError::bad(format!("参数「{}」只能取：{}", d.name, d.choices.join("、"))));
            }
            Ok(format!("'{}'", v.replace('\'', "''")))
        }
    }
}

/// 类型对应的样例值：保存时只检查语法，不要求已经填了值
fn sample(d: &ParamDef) -> String {
    match d.kind.as_str() {
        "number" => "0".into(),
        "date" => "'2000-01-01'".into(),
        _ => format!("'{}'", d.choices.first().map(|c| c.replace('\'', "''")).unwrap_or_default()),
    }
}

fn fill(sql: &str, defs: &[ParamDef], pick: impl Fn(&ParamDef) -> AppResult<String>) -> AppResult<String> {
    let mut out = String::with_capacity(sql.len());
    let mut last = 0;
    for (name, s, e) in scan(sql) {
        let d = defs.iter().find(|d| d.name == name).ok_or_else(|| AppError::bad(format!("SQL 里用了参数 {{{{{name}}}}}，但没有定义它")))?;
        out.push_str(&sql[last..s]);
        out.push_str(&pick(d)?);
        last = e;
    }
    out.push_str(&sql[last..]);
    Ok(out)
}

/// 取值（填的 > 默认值）替换成可执行的 SQL；缺值时说明是哪个参数
pub fn bind(sql: &str, defs: &[ParamDef], values: &HashMap<String, String>) -> AppResult<String> {
    check_defs(sql, defs)?;
    fill(sql, defs, |d| {
        let v = values.get(&d.name).map(String::as_str).filter(|v| !v.trim().is_empty()).or(d.default.as_deref().filter(|v| !v.is_empty()));
        match v {
            Some(v) => literal(d, v),
            None => Err(AppError::bad(format!("请填写参数「{}」", d.name))),
        }
    })
}

/// 只用于检查语法与引用：已填的值照常校验，没填的用样例
pub fn bind_for_check(sql: &str, defs: &[ParamDef], values: &HashMap<String, String>) -> AppResult<String> {
    check_defs(sql, defs)?;
    fill(sql, defs, |d| match values.get(&d.name).map(String::as_str).filter(|v| !v.trim().is_empty()).or(d.default.as_deref().filter(|v| !v.is_empty())) {
        Some(v) => literal(d, v),
        None => Ok(sample(d)),
    })
}

/// 实例的名称后缀：（店铺=A，月份=2026-09）
pub fn describe(defs: &[ParamDef], values: &HashMap<String, String>) -> String {
    let parts: Vec<String> = defs
        .iter()
        .filter_map(|d| {
            let v = values.get(&d.name).map(String::as_str).filter(|v| !v.trim().is_empty()).or(d.default.as_deref().filter(|v| !v.is_empty()))?;
            Some(format!("{}={}", d.name, v.trim()))
        })
        .collect();
    format!("（{}）", parts.join("，"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str, kind: &str) -> ParamDef {
        ParamDef { name: name.into(), kind: kind.into(), default: None, choices: vec![] }
    }

    fn vals(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn finds_placeholders_in_order_once() {
        assert_eq!(placeholders("WHERE 店铺 = {{店铺}} AND 日期 >= {{ 起 }} OR 店铺 = {{店铺}}"), vec!["店铺", "起"]);
        assert!(placeholders("SELECT 1").is_empty());
        assert!(placeholders("{{没有结尾").is_empty());
    }

    #[test]
    fn typed_literals_are_safe() {
        let defs = [def("店铺", "text"), def("下限", "number"), def("起", "date")];
        let sql = "SELECT * FROM 订单 WHERE 店铺 = {{店铺}} AND 金额 >= {{下限}} AND 日期 >= {{起}}";
        let out = bind(sql, &defs, &vals(&[("店铺", "A'; DROP TABLE x; --"), ("下限", "1,000.5"), ("起", "2026-09-01")])).unwrap();
        assert_eq!(out, "SELECT * FROM 订单 WHERE 店铺 = 'A''; DROP TABLE x; --' AND 金额 >= 1000.5 AND 日期 >= '2026-09-01'");
        // 值里的 {{x}} 不会被再次展开
        let out = bind("SELECT {{店铺}}", &[def("店铺", "text")], &vals(&[("店铺", "{{店铺}}")])).unwrap();
        assert_eq!(out, "SELECT '{{店铺}}'");
    }

    #[test]
    fn rejects_bad_values_with_clear_messages() {
        let e = |defs: &[ParamDef], v: &[(&str, &str)]| bind("SELECT {{x}}", defs, &vals(v)).err().unwrap().message().to_string();
        assert!(e(&[def("x", "number")], &[("x", "abc")]).contains("应为数字"));
        assert!(e(&[def("x", "number")], &[("x", "inf")]).contains("有限"));
        assert!(e(&[def("x", "date")], &[("x", "2026/09/01")]).contains("2026-09-01"));
        assert!(e(&[def("x", "date")], &[("x", "2026-13-40")]).contains("应为日期"));
        assert!(e(&[def("x", "text")], &[("x", "a\nb")]).contains("控制字符"));
        assert!(e(&[def("x", "text")], &[]).contains("请填写参数「x」"));
        let mut d = def("x", "text");
        d.choices = vec!["A".into(), "B".into()];
        assert!(e(std::slice::from_ref(&d), &[("x", "C")]).contains("只能取：A、B"));
        assert!(bind("SELECT {{x}}", &[d], &vals(&[("x", "B")])).is_ok());
    }

    #[test]
    fn defaults_fill_missing_values() {
        let mut d = def("x", "text");
        d.default = Some("A".into());
        assert_eq!(bind("SELECT {{x}}", std::slice::from_ref(&d), &vals(&[])).unwrap(), "SELECT 'A'");
        assert_eq!(bind("SELECT {{x}}", std::slice::from_ref(&d), &vals(&[("x", "  ")])).unwrap(), "SELECT 'A'", "空白当作没填");
        assert_eq!(describe(&[d], &vals(&[])), "（x=A）");
    }

    #[test]
    fn definitions_must_match_the_sql() {
        let err = |sql: &str, defs: &[ParamDef]| check_defs(sql, defs).err().unwrap().message().to_string();
        assert!(err("SELECT {{x}}", &[]).contains("没有定义"));
        assert!(err("SELECT 1", &[def("x", "text")]).contains("没有在 SQL 里使用"));
        assert!(err("SELECT {{x}}", &[def("x", "text"), def("x", "text")]).contains("重复"));
        assert!(err("SELECT {{x}}", &[def("x", "bool")]).contains("类型"));
        assert!(err("SELECT '{{x}}'", &[def("x", "text")]).contains("不要自己加引号"));
        assert!(err("SELECT {{x}}", &[def("", "text")]).contains("不合法"));
        let mut d = def("x", "number");
        d.choices = vec!["1".into()];
        assert!(err("SELECT {{x}}", &[d]).contains("只有文本参数"));
        let mut d = def("x", "date");
        d.default = Some("昨天".into());
        assert!(err("SELECT {{x}}", &[d]).contains("应为日期"));
        // 同一个参数用在多个位置、带空格的写法都可以
        assert!(check_defs("SELECT {{ x }}, {{x}}", &[def("x", "text")]).is_ok());
    }

    #[test]
    fn check_binding_uses_samples_for_unfilled() {
        let out = bind_for_check("SELECT {{a}}, {{b}}, {{c}}", &[def("a", "text"), def("b", "number"), def("c", "date")], &vals(&[])).unwrap();
        assert_eq!(out, "SELECT '', 0, '2000-01-01'");
        // 填了的值照常校验
        assert!(bind_for_check("SELECT {{b}}", &[def("b", "number")], &vals(&[("b", "x")])).is_err());
    }
}
