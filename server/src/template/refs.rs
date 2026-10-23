//! 工作簿里对数据集的引用：收集（发布模板时得到「需要哪些数据集的哪些列」）与改写（使用模板时换成使用者的数据集）
//!
//! 引用出现在三处：
//! - 透视表页签：工作表 `custom.pivot`（datasetId / datasetName + 请求里的列）
//! - 填报区：工作表 `custom.inputs[]`（datasetId / datasetName + fields / keys 列名）
//! - 浮动部件：资源 `SHEET_DRAWING_PLUGIN` 中 componentKey 为 webexcel-*（历史键名，保存在工作簿里，不能改）的部件定义（图表的 source、切片器 / 指标卡 / 明细卡）
//! - 公式：DSUMIFS / DCOUNTIFS / DAVERAGEIFS / DMAXIFS / DMINIFS / DCOUNTDISTINCTIFS / DLOOKUP 的数据集名、列名（字符串字面量）
//!
//! 部件定义里，含 datasetId 的对象及其子对象中，键为 column / splitBy / dateColumn / baseField / field（透视计算项所在的字段）的字符串是该数据集的列名，
//! 键为 fields 的字符串数组（数据表显示的列）里每一项也是列名

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value as Json;

const DRAWING_RESOURCE: &str = "SHEET_DRAWING_PLUGIN";
const COLUMN_KEYS: [&str; 5] = ["column", "splitBy", "dateColumn", "baseField", "field"];
const D_FUNCS: [&str; 7] = ["DSUMIFS", "DCOUNTIFS", "DAVERAGEIFS", "DMAXIFS", "DMINIFS", "DCOUNTDISTINCTIFS", "DLOOKUP"];

/// 一个数据集被引用的情况
#[derive(Default, Debug, Clone)]
pub struct Usage {
    /// 部件里引用的 ID（公式只有名称时为 None）
    pub ids: BTreeSet<String>,
    /// 公式里引用的名称
    pub names: BTreeSet<String>,
    pub columns: BTreeSet<String>,
    /// 部件类型 → 个数
    pub parts: BTreeMap<String, usize>,
}

#[derive(Default, Debug)]
pub struct Collected {
    /// 部件按 datasetId 归类；公式按数据集名归类（键为 "name:<名称>"），之后由调用方按目录合并
    pub by_key: BTreeMap<String, Usage>,
    /// 无法自动替换的引用（如公式里数据集名来自单元格）
    pub warnings: Vec<String>,
}

/// 绑定：模板里的一个数据集 → 使用者的数据集与列名映射
#[derive(Debug, Clone)]
pub struct Binding {
    pub dataset_id: String,
    pub dataset_name: String,
    /// 模板列名 → 使用者的列名（未列出的保持原名）
    pub columns: HashMap<String, String>,
}

impl Binding {
    fn column<'a>(&'a self, c: &'a str) -> &'a str {
        self.columns.get(c).map(String::as_str).unwrap_or(c)
    }
}

/// 改写所需：按模板里的 datasetId、数据集名分别查绑定
#[derive(Default)]
pub struct Rebind {
    pub by_id: HashMap<String, Binding>,
    pub by_name: HashMap<String, Binding>,
    /// 清空切片器已选的值（日期范围保留）
    pub clear_slicers: bool,
}

// ── 部件定义的遍历 ──

/// 遍历部件定义：on_dataset(原 datasetId, 所在对象) 可改写对象；on_column(所属 datasetId, 列名字符串) 可改写列名
fn visit(
    v: &mut Json,
    scope: Option<&str>,
    on_dataset: &mut dyn FnMut(&str, &mut serde_json::Map<String, Json>),
    on_column: &mut dyn FnMut(&str, &mut Json),
) {
    match v {
        Json::Object(map) => {
            let own = map.get("datasetId").and_then(Json::as_str).filter(|s| !s.is_empty()).map(String::from);
            if let Some(id) = &own {
                on_dataset(id, map);
            }
            let scope = own.as_deref().or(scope);
            for (k, child) in map.iter_mut() {
                if k == "datasetId" || k == "datasetName" {
                    continue;
                }
                match (scope, child.is_string() && COLUMN_KEYS.contains(&k.as_str())) {
                    (Some(s), true) => on_column(s, child),
                    (Some(s), false) if (k == "fields" || k == "keys") && child.is_array() => {
                        for item in child.as_array_mut().into_iter().flatten().filter(|x| x.is_string()) {
                            on_column(s, item);
                        }
                    }
                    _ => visit(child, scope, on_dataset, on_column),
                }
            }
        }
        Json::Array(items) => items.iter_mut().for_each(|x| visit(x, scope, on_dataset, on_column)),
        _ => {}
    }
}

/// 透视表定义与浮动部件定义：(部件类型, 定义) ；浮动部件资源需改写后写回字符串
fn with_specs(wb: &mut Json, f: &mut dyn FnMut(&str, &mut Json)) {
    if let Some(sheets) = wb.get_mut("sheets").and_then(Json::as_object_mut) {
        for s in sheets.values_mut() {
            if let Some(p) = s.pointer_mut("/custom/pivot") {
                f("pivot", p);
            }
            // 填报区：每个工作表可有多个
            if let Some(items) = s.pointer_mut("/custom/inputs").and_then(Json::as_array_mut) {
                for p in items.iter_mut() {
                    f("input", p);
                }
            }
        }
    }
    let Some(resources) = wb.get_mut("resources").and_then(Json::as_array_mut) else { return };
    for r in resources.iter_mut() {
        if r.get("name").and_then(Json::as_str) != Some(DRAWING_RESOURCE) {
            continue;
        }
        let Some(mut data) = r.get("data").and_then(Json::as_str).and_then(|s| serde_json::from_str::<Json>(s).ok()) else { continue };
        if let Some(sheets) = data.as_object_mut() {
            for sheet in sheets.values_mut() {
                let Some(drawings) = sheet.get_mut("data").and_then(Json::as_object_mut) else { continue };
                for d in drawings.values_mut() {
                    let key = d.get("componentKey").and_then(Json::as_str).unwrap_or("").to_string();
                    if !key.starts_with("webexcel-") {
                        continue;
                    }
                    if let Some(spec) = d.get_mut("data") {
                        f(&key, spec);
                    }
                }
            }
        }
        r["data"] = Json::String(data.to_string());
    }
}

/// 所有公式单元格：f(公式文本) 返回新公式（None 不改）
fn with_formulas(wb: &mut Json, f: &mut dyn FnMut(&str) -> Option<String>) {
    let Some(sheets) = wb.get_mut("sheets").and_then(Json::as_object_mut) else { return };
    for s in sheets.values_mut() {
        let Some(rows) = s.get_mut("cellData").and_then(Json::as_object_mut) else { continue };
        for row in rows.values_mut() {
            let Some(cells) = row.as_object_mut() else { continue };
            for cell in cells.values_mut() {
                let Some(text) = cell.get("f").and_then(Json::as_str) else { continue };
                if let Some(new) = f(text) {
                    cell["f"] = Json::String(new);
                    // 缓存的计算结果已不对应新数据，打开时重算
                    if let Some(o) = cell.as_object_mut() {
                        o.remove("v");
                    }
                }
            }
        }
    }
}

// ── 公式解析 ──

/// 公式中的一个参数：在公式文本中的字节范围，以及是否为字符串字面量
#[derive(Debug, Clone)]
pub struct Arg {
    pub start: usize,
    pub end: usize,
    pub literal: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DCall {
    pub func: String,
    pub args: Vec<Arg>,
}

impl DCall {
    /// 列名参数的下标：DCOUNTIFS 为 1,3,5…；其他为 1，以及条件列 2,4,6…
    pub fn column_args(&self) -> Vec<usize> {
        if self.func == "DCOUNTIFS" {
            (1..self.args.len()).step_by(2).collect()
        } else {
            std::iter::once(1).chain((2..self.args.len()).step_by(2)).filter(|i| *i < self.args.len()).collect()
        }
    }
}

fn unquote(s: &str) -> Option<String> {
    let t = s.trim();
    if t.len() < 2 || !t.starts_with('"') || !t.ends_with('"') {
        return None;
    }
    let inner = &t[1..t.len() - 1];
    // 中间不能有未成对的引号（否则是 "a"&"b" 这样的表达式）
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
                out.push('"');
            } else {
                return None;
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// 找出公式里所有 D 系列函数调用（含嵌套）
pub fn d_calls(f: &str) -> Vec<DCall> {
    let b = f.as_bytes();
    let mut calls = Vec::new();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'"' {
                if b.get(i + 1) == Some(&b'"') {
                    i += 1;
                } else {
                    in_str = false;
                }
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_str = true;
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.')) {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            let name = f[start..i].to_ascii_uppercase();
            if i < b.len() && b[i] == b'(' && D_FUNCS.contains(&name.as_str())
                && let Some(args) = split_args(f, i + 1)
            {
                calls.push(DCall { func: name, args });
            }
            // 从括号后继续扫描，嵌套的调用也能找到
            continue;
        }
        i += 1;
    }
    calls
}

/// 从左括号后开始切分参数，直到配对的右括号
fn split_args(f: &str, from: usize) -> Option<Vec<Arg>> {
    let b = f.as_bytes();
    let mut args = Vec::new();
    let mut depth = 0;
    let mut in_str = false;
    let mut start = from;
    let mut i = from;
    let push = |args: &mut Vec<Arg>, s: usize, e: usize| {
        let raw = &f[s..e];
        let lead = raw.len() - raw.trim_start().len();
        let trail = raw.len() - raw.trim_end().len();
        args.push(Arg { start: s + lead, end: e - trail, literal: unquote(raw) });
    };
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'"' {
                if b.get(i + 1) == Some(&b'"') {
                    i += 1;
                } else {
                    in_str = false;
                }
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'(' | b'{' => depth += 1,
                b')' | b'}' if depth > 0 => depth -= 1,
                b')' => {
                    if i > start || !args.is_empty() {
                        push(&mut args, start, i);
                    }
                    return Some(args);
                }
                b',' if depth == 0 => {
                    push(&mut args, start, i);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// 改写一个公式；返回 (新公式, 是否有变化, 无法替换的说明)
pub fn rewrite_formula(f: &str, rebind: &Rebind) -> (String, bool, Vec<String>) {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut warnings = Vec::new();
    for call in d_calls(f) {
        let Some(ds) = call.args.first() else { continue };
        let Some(name) = &ds.literal else {
            warnings.push(format!("公式 {} 的数据集名来自单元格或表达式，未能自动替换", call.func));
            continue;
        };
        let Some(b) = rebind.by_name.get(name) else { continue };
        if &b.dataset_name != name {
            edits.push((ds.start, ds.end, quote(&b.dataset_name)));
        }
        for i in call.column_args() {
            let a = &call.args[i];
            match &a.literal {
                Some(c) => {
                    let to = b.column(c);
                    if to != c {
                        edits.push((a.start, a.end, quote(to)));
                    }
                }
                None => warnings.push(format!("公式 {} 的列名来自单元格或表达式，未能自动替换", call.func)),
            }
        }
    }
    if edits.is_empty() {
        return (f.to_string(), false, warnings);
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = f.to_string();
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    (out, true, warnings)
}

// ── 对外 ──

/// 收集工作簿对数据集的全部引用
pub fn collect(wb: &Json) -> Collected {
    let mut wb = wb.clone();
    let mut out = Collected::default();
    with_specs(&mut wb, &mut |kind, spec| {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut cols: Vec<(String, String)> = Vec::new();
        visit(
            spec,
            None,
            &mut |id, _| {
                seen.insert(id.to_string());
            },
            &mut |id, c| {
                if let Some(s) = c.as_str().filter(|s| !s.is_empty()) {
                    cols.push((id.to_string(), s.to_string()));
                }
            },
        );
        for id in seen {
            let u = out.by_key.entry(id.clone()).or_default();
            u.ids.insert(id);
            *u.parts.entry(kind.to_string()).or_default() += 1;
        }
        for (id, c) in cols {
            out.by_key.entry(id).or_default().columns.insert(c);
        }
    });
    let mut warnings = BTreeSet::new();
    with_formulas(&mut wb, &mut |f| {
        for call in d_calls(f) {
            let Some(name) = call.args.first().and_then(|a| a.literal.clone()) else {
                warnings.insert(format!("公式 {} 的数据集名来自单元格或表达式，使用模板时无法自动替换", call.func));
                continue;
            };
            let u = out.by_key.entry(format!("name:{name}")).or_default();
            u.names.insert(name);
            *u.parts.entry("formula".into()).or_default() += 1;
            for i in call.column_args() {
                match &call.args[i].literal {
                    Some(c) if !c.is_empty() => {
                        u.columns.insert(c.clone());
                    }
                    Some(_) => {}
                    None => {
                        warnings.insert(format!("公式 {} 的列名来自单元格或表达式，使用模板时无法自动替换", call.func));
                    }
                }
            }
        }
        None
    });
    out.warnings = warnings.into_iter().collect();
    out
}

/// 按绑定改写工作簿；返回无法替换的说明
pub fn rebind(wb: &mut Json, rb: &Rebind) -> Vec<String> {
    with_specs(wb, &mut |kind, spec| {
        visit(
            spec,
            None,
            &mut |id, map| {
                if let Some(b) = rb.by_id.get(id) {
                    map.insert("datasetId".into(), Json::String(b.dataset_id.clone()));
                    if map.contains_key("datasetName") {
                        map.insert("datasetName".into(), Json::String(b.dataset_name.clone()));
                    }
                }
            },
            &mut |id, c| {
                if let (Some(b), Some(s)) = (rb.by_id.get(id), c.as_str()) {
                    let to = b.column(s).to_string();
                    *c = Json::String(to);
                }
            },
        );
        if kind == "webexcel-slicer" && rb.clear_slicers && let Some(o) = spec.as_object_mut() {
            o.insert("values".into(), Json::Array(vec![]));
        }
    });
    let mut warnings = BTreeSet::new();
    with_formulas(wb, &mut |f| {
        let (new, changed, w) = rewrite_formula(f, rb);
        warnings.extend(w);
        changed.then_some(new)
    });
    warnings.into_iter().collect()
}

/// 工作簿概要：工作表数、各类部件数、公式数
pub fn summary(wb: &Json) -> BTreeMap<String, usize> {
    let mut wb = wb.clone();
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    out.insert("sheets".into(), wb.get("sheets").and_then(Json::as_object).map(|s| s.len()).unwrap_or(0));
    with_specs(&mut wb, &mut |kind, _| *out.entry(kind.to_string()).or_default() += 1);
    let mut formulas = 0;
    with_formulas(&mut wb, &mut |_| {
        formulas += 1;
        None
    });
    out.insert("formula".into(), formulas);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(id: &str, name: &str, cols: &[(&str, &str)]) -> Binding {
        Binding { dataset_id: id.into(), dataset_name: name.into(), columns: cols.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() }
    }

    #[test]
    fn formula_calls() {
        let f = r#"=DSUMIFS("销售订单","订单金额","下单时间",">="&B6,"店铺",$B$2)+DCOUNTIFS( "销售订单" , "店铺", "旗舰""店" )"#;
        let calls = d_calls(f);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].args.len(), 6);
        assert_eq!(calls[0].args[0].literal.as_deref(), Some("销售订单"));
        assert_eq!(calls[0].args[3].literal, None);
        assert_eq!(calls[1].args[2].literal.as_deref(), Some("旗舰\"店"));
        assert_eq!(calls[0].column_args(), vec![1, 2, 4]);
        assert_eq!(calls[1].column_args(), vec![1]);
        // 嵌套与字符串里的函数名
        let n = d_calls(r#"=IF(A1="DSUMIFS(",DLOOKUP("成本","成本价","商品ID",DLOOKUP("映射","新ID","旧ID",A2)),0)"#);
        assert_eq!(n.len(), 2);
        assert_eq!(n[1].args[0].literal.as_deref(), Some("映射"));
    }

    #[test]
    fn formula_rewrite() {
        let mut rb = Rebind::default();
        rb.by_name.insert("订单".into(), binding("ds_1", "我的订单", &[("订单金额", "实付金额"), ("下单时间", "支付时间")]));
        let (out, changed, w) = rewrite_formula(r#"=DSUMIFS("订单","订单金额","下单时间",">="&B6,"店铺",$B$2)/DSUMIFS("投放","消耗")"#, &rb);
        assert!(changed);
        assert!(w.is_empty());
        assert_eq!(out, r#"=DSUMIFS("我的订单","实付金额","支付时间",">="&B6,"店铺",$B$2)/DSUMIFS("投放","消耗")"#);
        let (_, _, w) = rewrite_formula(r#"=DSUMIFS(A1,"订单金额")"#, &rb);
        assert_eq!(w.len(), 1);
        // 取最大 / 最小值的函数同样会随数据集绑定改名
        let (out, changed, _) = rewrite_formula(r#"=DMAXIFS("订单","订单金额","店铺",B2)-DMINIFS("订单","订单金额")"#, &rb);
        assert!(changed);
        assert_eq!(out, r#"=DMAXIFS("我的订单","实付金额","店铺",B2)-DMINIFS("我的订单","实付金额")"#);
        // 去重计数的数据集名、去重列、条件列也一样
        let (out, changed, _) = rewrite_formula(r#"=DCOUNTDISTINCTIFS("订单","订单金额","下单时间",">="&B6)"#, &rb);
        assert!(changed);
        assert_eq!(out, r#"=DCOUNTDISTINCTIFS("我的订单","实付金额","支付时间",">="&B6)"#);
    }

    #[test]
    fn specs_collect_and_rebind() {
        let drawing = serde_json::json!({
            "s1": { "data": {
                "d1": { "componentKey": "webexcel-chart", "data": { "id": "c1", "source": { "kind": "dataset", "datasetId": "ds_a", "x": { "column": "下单时间", "granularity": "month" }, "values": [{ "column": "订单金额", "agg": "sum" }], "splitBy": "店铺", "filters": [{ "op": "in", "column": "店铺", "values": ["旗舰店"] }] } } },
                "d2": { "componentKey": "webexcel-slicer", "data": { "id": "s1", "datasetId": "ds_a", "column": "店铺", "values": ["旗舰店"], "mode": "values" } },
                "d3": { "componentKey": "other", "data": { "datasetId": "x" } },
                "d4": { "componentKey": "webexcel-datatable", "data": { "id": "t1", "datasetId": "ds_a", "name": "订单表", "fields": ["店铺", "订单金额"], "sort": [{ "column": "订单金额", "desc": true }], "filters": [] } }
            }}
        });
        let mut wb = serde_json::json!({
            "sheets": {
                "p": { "custom": { "pivot": { "version": 1, "datasetId": "ds_a", "datasetName": "订单", "request": { "rows": [{ "column": "店铺" }], "columns": [], "values": [{ "column": "订单金额", "agg": "sum" }], "baseField": "下单时间", "calcItems": [{ "field": "店铺", "name": "差", "formula": "[甲] - [乙]" }] } } } },
                "f": { "cellData": { "0": { "0": { "f": "=DSUMIFS(\"订单\",\"订单金额\")", "v": 1 } } } }
            },
            "resources": [{ "name": "SHEET_DRAWING_PLUGIN", "data": drawing.to_string() }]
        });
        let c = collect(&wb);
        let a = &c.by_key["ds_a"];
        assert_eq!(a.columns.iter().cloned().collect::<Vec<_>>(), vec!["下单时间", "店铺", "订单金额"]);
        assert_eq!(a.parts["pivot"], 1);
        assert_eq!(a.parts["webexcel-chart"], 1);
        assert_eq!(a.parts["webexcel-slicer"], 1);
        assert_eq!(a.parts["webexcel-datatable"], 1);
        assert_eq!(c.by_key["name:订单"].parts["formula"], 1);
        assert!(!c.by_key.contains_key("x"));

        let b = binding("ds_new", "新订单", &[("订单金额", "实付"), ("店铺", "门店")]);
        let mut rb = Rebind { clear_slicers: true, ..Default::default() };
        rb.by_id.insert("ds_a".into(), b.clone());
        rb.by_name.insert("订单".into(), b);
        rebind(&mut wb, &rb);
        let p = &wb["sheets"]["p"]["custom"]["pivot"];
        assert_eq!(p["datasetId"], "ds_new");
        assert_eq!(p["datasetName"], "新订单");
        assert_eq!(p["request"]["rows"][0]["column"], "门店");
        assert_eq!(p["request"]["baseField"], "下单时间");
        // 计算项所在的字段是列名，跟着改；公式里引用的是取值，不动
        assert_eq!(p["request"]["calcItems"][0]["field"], "门店");
        assert_eq!(p["request"]["calcItems"][0]["formula"], "[甲] - [乙]");
        assert_eq!(wb["sheets"]["f"]["cellData"]["0"]["0"]["f"], "=DSUMIFS(\"新订单\",\"实付\")");
        assert!(wb["sheets"]["f"]["cellData"]["0"]["0"].get("v").is_none());
        let d: Json = serde_json::from_str(wb["resources"][0]["data"].as_str().unwrap()).unwrap();
        let chart = &d["s1"]["data"]["d1"]["data"]["source"];
        assert_eq!(chart["datasetId"], "ds_new");
        assert_eq!(chart["splitBy"], "门店");
        let table = &d["s1"]["data"]["d4"]["data"];
        assert_eq!(table["datasetId"], "ds_new");
        assert_eq!(table["fields"], serde_json::json!(["门店", "实付"]));
        assert_eq!(table["sort"][0]["column"], "实付");
        assert_eq!(chart["filters"][0]["column"], "门店");
        assert_eq!(chart["filters"][0]["values"][0], "旗舰店");
        assert_eq!(d["s1"]["data"]["d2"]["data"]["values"].as_array().unwrap().len(), 0);
        assert_eq!(d["s1"]["data"]["d3"]["data"]["datasetId"], "x");
    }
}
