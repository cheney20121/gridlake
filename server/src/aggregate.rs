//! 公式下推：表格里的 DSUMIFS / DCOUNTIFS / DAVERAGEIFS / DMAXIFS / DMINIFS 批量在服务端计算
//!
//! 工作表里的数据表（TSUM / TCOUNT / TAVERAGE / TMIN / TMAX / TVALUE）也走这里：请求带上数据表的
//! 筛选 / 关键字 / 排序作为范围（query），条件在范围之内再筛；TVALUE 取范围内按排序的第 N 行
//!
//! 条件语义与 Excel SUMIFS 一致：
//! - 比较运算 `>` `>=` `<` `<=` `=` `<>`，数值列按数值比较
//! - 通配符 `*` `?`，`~` 转义；不区分大小写
//! - `""` 只匹配空单元格；`"*"` 匹配任意非空文本；`"<>"` 匹配非空
//!
//! 批量策略：按「文本条件列集合 + 数值条件」分组为签名，每个签名只扫描一次表：
//! `GROUP BY 文本条件列` 得到分组（汇总表场景：主体 × 店铺 × 场景 × 月份，数千组），
//! 再在内存里按各调用的条件（含通配符）匹配分组累加。上千个公式单元格一次请求、几十毫秒算完

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

use duckdb::types::Value;
use duckdb::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::dataset::{self, ColumnDef, DatasetDef};
use crate::error::AppResult;

/// 单个签名的分组上限：超过说明条件列取值过多（如按订单号做条件），改为逐条 SQL
const MAX_GROUPS: usize = 200_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateCall {
    /// sum / count / average / min / max / lookup / nth
    pub func: String,
    /// 求值的列（count 可省略）
    pub column: Option<String>,
    #[serde(default)]
    pub criteria: Vec<CriterionInput>,
    /// nth：第几行（从 1 开始，按范围的排序）
    #[serde(default)]
    pub row: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionInput {
    pub column: String,
    /// 公式中的条件值（字符串或数字）
    pub value: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateResult {
    /// 与 calls 一一对应（数值；DLOOKUP 可能是文本）；出错时为 None，原因见 errors
    pub values: Vec<Option<serde_json::Value>>,
    pub errors: Vec<Option<String>>,
    pub signatures: usize,
    pub elapsed_ms: i64,
}

// ── 条件解析 ──────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    fn sql(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "<>",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Lt => "<",
            Op::Le => "<=",
        }
    }
}

/// 在分组键上匹配的文本条件
#[derive(Clone, Debug)]
enum TextMatch {
    /// 空单元格（"" 或 "="）
    Blank,
    /// 非空（"<>"）
    NotBlank,
    /// 相等 / 不等（已转小写）；pattern 含通配符时按模式匹配
    Equals { value: String, wildcard: bool, negate: bool },
    /// 文本比较（"<b" 之类，按字典序）
    Compare { op: Op, value: String },
    /// 文本列上收到 Excel 日期序列号（表格把 2026-10 之类的输入识别成了日期）：
    /// 匹配 YYYY-MM-DD；若为当月 1 日，也匹配 YYYY-MM（如「月份」列）；同时匹配数字原文
    DateLike { day: String, month: Option<String>, raw: String },
}

enum Parsed {
    /// 数值列上的数值比较：下推到 SQL
    Numeric { op: Op, value: f64 },
    Text(TextMatch),
}

fn split_op(s: &str) -> (Op, &str) {
    for (p, op) in [("<>", Op::Ne), (">=", Op::Ge), ("<=", Op::Le), ("=", Op::Eq), (">", Op::Gt), ("<", Op::Lt)] {
        if let Some(rest) = s.strip_prefix(p) {
            return (op, rest);
        }
    }
    (Op::Eq, s)
}

fn parse_criterion(value: &serde_json::Value, numeric_column: bool) -> Parsed {
    let raw = match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => {
            let v = n.as_f64().unwrap_or(0.0);
            if numeric_column {
                return Parsed::Numeric { op: Op::Eq, value: v };
            }
            if let Some(m) = date_like(v) {
                return Parsed::Text(m);
            }
            n.to_string()
        }
        serde_json::Value::Bool(b) => if *b { "TRUE".into() } else { "FALSE".into() },
        other => other.to_string(),
    };
    let had_op = raw.starts_with(['=', '<', '>']);
    let (op, operand) = split_op(&raw);
    if numeric_column && let Ok(v) = operand.trim().parse::<f64>() {
        return Parsed::Numeric { op, value: v };
    }
    if operand.is_empty() {
        return Parsed::Text(match (had_op, op) {
            (true, Op::Ne) => TextMatch::NotBlank,
            _ => TextMatch::Blank,
        });
    }
    match op {
        Op::Eq | Op::Ne => {
            let wildcard = operand.contains(['*', '?']);
            Parsed::Text(TextMatch::Equals { value: operand.to_lowercase(), wildcard, negate: op == Op::Ne })
        }
        _ => Parsed::Text(TextMatch::Compare { op, value: operand.to_lowercase() }),
    }
}

/// Excel 日期序列号（1900 日期系统）→ 日期文本；只接受 1954~2119 年范围内的整数，避免误判普通数字
fn date_like(v: f64) -> Option<TextMatch> {
    if v.fract() != 0.0 || !(20_000.0..80_000.0).contains(&v) {
        return None;
    }
    let date = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)? + chrono::Duration::days(v as i64);
    let day = date.format("%Y-%m-%d").to_string();
    let month = (chrono::Datelike::day(&date) == 1).then(|| date.format("%Y-%m").to_string());
    Some(TextMatch::DateLike { day, month, raw: format!("{}", v as i64) })
}

/// Excel 通配符匹配（* 任意串，? 任意单字符，~ 转义），输入均已转小写
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some(pi);
                    mark = ti;
                    pi += 1;
                    continue;
                }
                '?' => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                '~' if pi + 1 < p.len() && matches!(p[pi + 1], '*' | '?' | '~') => {
                    if p[pi + 1] == t[ti] {
                        pi += 2;
                        ti += 1;
                        continue;
                    }
                }
                c if c == t[ti] => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some(s) => {
                pi = s + 1;
                mark += 1;
                ti = mark;
            }
            None => return false,
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

impl TextMatch {
    /// key 为分组键（小写；None 表示空单元格）
    fn matches(&self, key: Option<&str>) -> bool {
        let key = key.filter(|k| !k.is_empty());
        match self {
            TextMatch::Blank => key.is_none(),
            TextMatch::NotBlank => key.is_some(),
            TextMatch::Equals { value, wildcard, negate } => {
                let hit = match key {
                    // 与 Excel 一致：通配符模式不匹配空单元格
                    None => false,
                    Some(k) => if *wildcard { wildcard_match(value, k) } else { k == value },
                };
                hit != *negate
            }
            TextMatch::DateLike { day, month, raw } => match key {
                None => false,
                Some(k) => k == day || k.starts_with(&format!("{day} ")) || month.as_deref() == Some(k) || k == raw,
            },
            TextMatch::Compare { op, value } => match key {
                None => false,
                Some(k) => match op {
                    Op::Gt => k > value.as_str(),
                    Op::Ge => k >= value.as_str(),
                    Op::Lt => k < value.as_str(),
                    Op::Le => k <= value.as_str(),
                    _ => false,
                },
            },
        }
    }

    /// 精确值（可用于 SQL IN 预筛）；通配符 / 比较 / 不等返回 None
    fn exact(&self) -> Option<Option<&str>> {
        match self {
            TextMatch::Blank => Some(None),
            TextMatch::Equals { value, wildcard: false, negate: false } => Some(Some(value)),
            _ => None,
        }
    }
}

// ── 批量计算 ──────────────────────────────────────────

struct Prepared {
    func: String,
    /// 求值列的 SQL 表达式（count 为 None）
    value_expr: Option<String>,
    /// (文本条件列 SQL, 条件)
    text: Vec<(String, TextMatch)>,
    /// 数值条件 SQL 片段（进入签名，下推到 WHERE）
    numeric: Vec<String>,
}

fn find_column<'a>(def: &'a DatasetDef, name: &str) -> Option<&'a ColumnDef> {
    def.columns.iter().find(|c| c.key == name || c.title == name)
}

fn prepare(def: &DatasetDef, call: &AggregateCall) -> Result<Prepared, String> {
    let func = call.func.to_lowercase();
    if !matches!(func.as_str(), "sum" | "count" | "average" | "min" | "max" | "lookup" | "nth") {
        return Err(format!("不支持的函数：{}", call.func));
    }
    if func == "nth" {
        if !call.criteria.is_empty() {
            return Err("TVALUE 不支持附加条件".into());
        }
        if call.row.is_none_or(|r| r < 1) {
            return Err("行号必须是大于 0 的整数".into());
        }
    }
    let value_expr = if func == "count" {
        None
    } else {
        let name = call.column.as_deref().ok_or("缺少求和列")?;
        let col = find_column(def, name).ok_or_else(|| format!("数据集「{}」没有列「{name}」", def.name))?;
        let numeric = matches!(col.kind, "money" | "number");
        if func == "lookup" || func == "nth" {
            // 查找 / 取第 N 行：返回原值（数值或文本）
            Some(if numeric { format!("({})::DOUBLE", dataset::column_sql(col)) } else { format!("CAST(({}) AS VARCHAR)", dataset::column_sql(col)) })
        } else {
            if !numeric {
                let what = match func.as_str() {
                    "average" => "求平均",
                    "min" => "取最小值",
                    "max" => "取最大值",
                    _ => "求和",
                };
                return Err(format!("列「{name}」不是数值列，不能{what}"));
            }
            Some(format!("({})::DOUBLE", dataset::column_sql(col)))
        }
    };
    let mut text = Vec::new();
    let mut numeric = Vec::new();
    for c in &call.criteria {
        let col = find_column(def, &c.column).ok_or_else(|| format!("数据集「{}」没有列「{}」", def.name, c.column))?;
        let is_num = matches!(col.kind, "money" | "number");
        let expr = dataset::column_sql(col);
        match parse_criterion(&c.value, is_num) {
            Parsed::Numeric { op, value } => numeric.push(format!("({expr})::DOUBLE {} {value}", op.sql())),
            Parsed::Text(m) => {
                // 数值列上的文本条件（如 ""）按文本处理
                text.push((format!("CAST(({expr}) AS VARCHAR)"), m));
            }
        }
    }
    numeric.sort();
    Ok(Prepared { func, value_expr, text, numeric })
}

/// scope：范围（数据表的筛选 / 关键字 / 排序）；None 为整个数据集
pub fn aggregate(conn: &Connection, dataset_id: &str, calls: &[AggregateCall], scope: Option<&dataset::QuerySpec>) -> AppResult<AggregateResult> {
    let started = Instant::now();
    let def = dataset::dataset(conn, dataset_id)?;
    let empty = dataset::QuerySpec::default();
    let scope = scope.unwrap_or(&empty);
    // 范围的 WHERE 放在最前，参数也排在最前
    let (base_where, base_args) = dataset::filter_sql(conn, &def, scope)?;
    let mut values: Vec<Option<serde_json::Value>> = vec![None; calls.len()];
    let mut errors: Vec<Option<String>> = vec![None; calls.len()];

    // 签名：(文本条件列集合, 数值条件) → 调用下标
    let mut prepared: Vec<Option<Prepared>> = Vec::with_capacity(calls.len());
    let mut groups: BTreeMap<(Vec<String>, Vec<String>), Vec<usize>> = BTreeMap::new();
    let mut lookup_groups: BTreeMap<(Vec<String>, Vec<String>), Vec<usize>> = BTreeMap::new();
    let mut nth_calls: Vec<usize> = Vec::new();
    for (i, call) in calls.iter().enumerate() {
        match prepare(&def, call) {
            Ok(p) if p.func == "nth" => {
                nth_calls.push(i);
                prepared.push(Some(p));
            }
            Ok(p) => {
                let cols: BTreeSet<String> = p.text.iter().map(|(c, _)| c.clone()).collect();
                let target = if p.func == "lookup" { &mut lookup_groups } else { &mut groups };
                target.entry((cols.into_iter().collect(), p.numeric.clone())).or_default().push(i);
                prepared.push(Some(p));
            }
            Err(e) => {
                errors[i] = Some(e);
                prepared.push(None);
            }
        }
    }

    for ((cols, numeric), idxs) in &groups {
        // 本签名需要的求值列
        let value_exprs: Vec<String> = idxs
            .iter()
            .filter_map(|&i| prepared[i].as_ref().and_then(|p| p.value_expr.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let value_pos: HashMap<&str, usize> = value_exprs.iter().enumerate().map(|(i, e)| (e.as_str(), i)).collect();

        // 预筛：某列在所有调用里都是精确值时，用 IN 缩小扫描范围
        let mut where_parts: Vec<String> = std::iter::once(base_where.clone()).chain(numeric.iter().cloned()).collect();
        let mut args: Vec<Value> = base_args.clone();
        for col in cols {
            let mut exacts: BTreeSet<Option<String>> = BTreeSet::new();
            let mut all_exact = true;
            for &i in idxs {
                let p = prepared[i].as_ref().expect("prepared");
                for (c, m) in &p.text {
                    if c == col {
                        match m.exact() {
                            Some(v) => {
                                exacts.insert(v.map(String::from));
                            }
                            None => all_exact = false,
                        }
                    }
                }
            }
            if all_exact && !exacts.is_empty() {
                let named: Vec<&String> = exacts.iter().flatten().collect();
                let mut ors = Vec::new();
                if !named.is_empty() {
                    ors.push(format!("lower({col}) IN ({})", vec!["?"; named.len()].join(", ")));
                    args.extend(named.into_iter().map(|v| Value::Text(v.clone())));
                }
                if exacts.contains(&None) {
                    ors.push(format!("({col} IS NULL OR {col} = '')"));
                }
                where_parts.push(format!("({})", ors.join(" OR ")));
            }
        }
        let where_sql = if where_parts.is_empty() { "TRUE".to_string() } else { where_parts.join(" AND ") };
        let key_select: Vec<String> = cols.iter().map(|c| format!("lower({c})")).collect();
        let mut agg_select = vec!["count(*)::DOUBLE".to_string()];
        for e in &value_exprs {
            agg_select.push(format!("COALESCE(sum({e}), 0)"));
            agg_select.push(format!("count({e})::DOUBLE"));
            agg_select.push(format!("COALESCE(min({e}), 0)"));
            agg_select.push(format!("COALESCE(max({e}), 0)"));
        }
        let select = key_select.iter().chain(agg_select.iter()).cloned().collect::<Vec<_>>().join(", ");
        let group_by = if cols.is_empty() { String::new() } else { format!(" GROUP BY {}", (1..=cols.len()).map(|n| n.to_string()).collect::<Vec<_>>().join(", ")) };
        let sql = format!("SELECT {select} FROM {} WHERE {where_sql}{group_by} LIMIT {}", def.table, MAX_GROUPS + 1);

        let rows: Vec<(Vec<Option<String>>, Vec<f64>)> = {
            let mut stmt = conn.prepare(&sql)?;
            let k = cols.len();
            let n_agg = agg_select.len();
            stmt.query_map(params_from_iter(args.iter()), |r| {
                let keys = (0..k).map(|i| r.get::<_, Option<String>>(i)).collect::<duckdb::Result<Vec<_>>>()?;
                let aggs = (0..n_agg).map(|i| Ok(r.get::<_, Option<f64>>(k + i)?.unwrap_or(0.0))).collect::<duckdb::Result<Vec<_>>>()?;
                Ok((keys, aggs))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        if rows.len() > MAX_GROUPS {
            for &i in idxs {
                errors[i] = Some("条件列取值过多（超过 20 万组），请改用取值较少的列做条件".into());
            }
            continue;
        }

        let col_pos: HashMap<&str, usize> = cols.iter().enumerate().map(|(i, c)| (c.as_str(), i)).collect();
        for &i in idxs {
            let p = prepared[i].as_ref().expect("prepared");
            let vp = p.value_expr.as_deref().map(|e| value_pos[e]);
            let (mut sum, mut cnt, mut rows_hit) = (0.0f64, 0.0f64, 0.0f64);
            let (mut lo, mut hi): (Option<f64>, Option<f64>) = (None, None);
            for (keys, aggs) in &rows {
                let ok = p.text.iter().all(|(c, m)| m.matches(keys[col_pos[c.as_str()]].as_deref()));
                if !ok {
                    continue;
                }
                rows_hit += aggs[0];
                if let Some(v) = vp {
                    let n = aggs[2 + v * 4];
                    sum += aggs[1 + v * 4];
                    cnt += n;
                    // 该组没有数值时 min / max 是占位的 0，不参与比较
                    if n > 0.0 {
                        lo = Some(lo.map_or(aggs[3 + v * 4], |x| x.min(aggs[3 + v * 4])));
                        hi = Some(hi.map_or(aggs[4 + v * 4], |x| x.max(aggs[4 + v * 4])));
                    }
                }
            }
            let v = match p.func.as_str() {
                "count" => rows_hit,
                "sum" => (sum * 100.0).round() / 100.0 + 0.0,
                // 与 Excel 一致：没有数值时 MIN / MAX 为 0
                "min" => lo.unwrap_or(0.0) + 0.0,
                "max" => hi.unwrap_or(0.0) + 0.0,
                _ => {
                    if cnt == 0.0 {
                        errors[i] = Some("#DIV/0!".into());
                        continue;
                    }
                    sum / cnt
                }
            };
            values[i] = Some(serde_json::json!(v));
        }
    }

    // ── DLOOKUP：每个签名按条件列分组，取每组最先入库的一行，再在内存里匹配 ──
    for ((cols, numeric), idxs) in &lookup_groups {
        let value_exprs: Vec<String> = idxs.iter().filter_map(|&i| prepared[i].as_ref().and_then(|p| p.value_expr.clone())).collect::<BTreeSet<_>>().into_iter().collect();
        let value_pos: HashMap<&str, usize> = value_exprs.iter().enumerate().map(|(i, e)| (e.as_str(), i)).collect();
        let mut where_parts: Vec<String> = std::iter::once(base_where.clone()).chain(numeric.iter().cloned()).collect();
        let mut args: Vec<Value> = base_args.clone();
        for col in cols {
            let mut exacts: BTreeSet<Option<String>> = BTreeSet::new();
            let mut all_exact = true;
            for &i in idxs {
                for (c, m) in &prepared[i].as_ref().expect("prepared").text {
                    if c == col {
                        match m.exact() {
                            Some(v) => {
                                exacts.insert(v.map(String::from));
                            }
                            None => all_exact = false,
                        }
                    }
                }
            }
            if all_exact && !exacts.is_empty() {
                let named: Vec<&String> = exacts.iter().flatten().collect();
                let mut ors = Vec::new();
                if !named.is_empty() {
                    ors.push(format!("lower({col}) IN ({})", vec!["?"; named.len()].join(", ")));
                    args.extend(named.into_iter().map(|v| Value::Text(v.clone())));
                }
                if exacts.contains(&None) {
                    ors.push(format!("({col} IS NULL OR {col} = '')"));
                }
                where_parts.push(format!("({})", ors.join(" OR ")));
            }
        }
        let where_sql = if where_parts.is_empty() { "TRUE".to_string() } else { where_parts.join(" AND ") };
        let rid = &def.rid;
        let key_select: Vec<String> = cols.iter().map(|c| format!("lower({c})")).collect();
        let mut agg = vec![format!("min({rid})")];
        agg.extend(value_exprs.iter().map(|e| format!("first({e} ORDER BY {rid})")));
        let group_by = if cols.is_empty() { String::new() } else { format!(" GROUP BY {}", (1..=cols.len()).map(|n| n.to_string()).collect::<Vec<_>>().join(", ")) };
        let sql = format!(
            "SELECT {} FROM {} WHERE {where_sql}{group_by} LIMIT {}",
            key_select.iter().chain(agg.iter()).cloned().collect::<Vec<_>>().join(", "),
            def.table,
            MAX_GROUPS + 1
        );
        let k = cols.len();
        let rows: Vec<(Vec<Option<String>>, i64, Vec<serde_json::Value>)> = {
            let mut stmt = conn.prepare(&sql)?;
            stmt.query_map(params_from_iter(args.iter()), |r| {
                let keys = (0..k).map(|i| r.get::<_, Option<String>>(i)).collect::<duckdb::Result<Vec<_>>>()?;
                let first: i64 = r.get(k)?;
                let vals = (0..value_exprs.len()).map(|i| Ok(crate::tabular::to_json(r.get_ref(k + 1 + i)?))).collect::<duckdb::Result<Vec<_>>>()?;
                Ok((keys, first, vals))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        if rows.len() > MAX_GROUPS {
            for &i in idxs {
                errors[i] = Some("条件列取值过多（超过 20 万组），请改用取值较少的列做条件".into());
            }
            continue;
        }
        let col_pos: HashMap<&str, usize> = cols.iter().enumerate().map(|(i, c)| (c.as_str(), i)).collect();
        for &i in idxs {
            let p = prepared[i].as_ref().expect("prepared");
            let vp = p.value_expr.as_deref().map(|e| value_pos[e]).unwrap_or(0);
            // 多组匹配（通配符条件）时取最先入库的那一行，与 Excel 查找「返回第一个匹配」一致
            let hit = rows
                .iter()
                .filter(|(keys, _, _)| p.text.iter().all(|(c, m)| m.matches(keys[col_pos[c.as_str()]].as_deref())))
                .min_by_key(|(_, first, _)| *first);
            match hit {
                Some((_, _, vals)) => values[i] = Some(vals[vp].clone()),
                None => errors[i] = Some("#N/A".into()),
            }
        }
    }

    // ── TVALUE：范围内按排序的第 N 行 ──
    if !nth_calls.is_empty() {
        let order = dataset::order_sql(&def, scope)?;
        for &i in &nth_calls {
            let p = prepared[i].as_ref().expect("prepared");
            let expr = p.value_expr.as_deref().expect("nth 有求值列");
            let n = calls[i].row.unwrap_or(1);
            let sql = format!("SELECT {expr} FROM {} WHERE {base_where} ORDER BY {order} LIMIT 1 OFFSET {}", def.table, n - 1);
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(base_args.iter()))?;
            match rows.next()? {
                Some(r) => values[i] = Some(crate::tabular::to_json(r.get_ref(0)?)),
                None => errors[i] = Some("#N/A".into()),
            }
        }
    }

    Ok(AggregateResult { values, errors, signatures: groups.len() + lookup_groups.len() + nth_calls.len(), elapsed_ms: started.elapsed().as_millis() as i64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard() {
        assert!(wildcard_match("*退款*", "退款-结算后退款-退用户"));
        assert!(wildcard_match("上门取件*", "上门取件-支付快递费"));
        assert!(!wildcard_match("上门取件*", "快递拦截费"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(wildcard_match("~*x", "*x"));
        assert!(!wildcard_match("~*x", "ax"));
        assert!(wildcard_match("*", "anything"));
    }

    #[test]
    fn criteria() {
        let m = |v: &str| match parse_criterion(&serde_json::json!(v), false) {
            Parsed::Text(m) => m,
            Parsed::Numeric { .. } => panic!("numeric"),
        };
        assert!(m("").matches(None));
        assert!(m("").matches(Some("")));
        assert!(!m("").matches(Some("x")));
        assert!(!m("*").matches(None));
        assert!(m("*").matches(Some("x")));
        assert!(m("<>").matches(Some("x")));
        assert!(!m("<>").matches(None));
        assert!(m("<>入账").matches(Some("出账")));
        assert!(m("<>入账").matches(None));
        assert!(m("入账").matches(Some("入账")));
        assert!(matches!(parse_criterion(&serde_json::json!("<0"), true), Parsed::Numeric { op: Op::Lt, .. }));
        assert!(matches!(parse_criterion(&serde_json::json!(5), true), Parsed::Numeric { op: Op::Eq, .. }));
        // 2026-10 被表格识别为日期 46296（2026-10-01）
        let Parsed::Text(d) = parse_criterion(&serde_json::json!(46296), false) else { panic!("date") };
        assert!(d.matches(Some("2026-10")));
        assert!(d.matches(Some("2026-10-01")));
        assert!(!d.matches(Some("2026-09")));
    }
}
