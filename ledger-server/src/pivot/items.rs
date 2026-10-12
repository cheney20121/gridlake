//! 透视表的「计算项」：在某个行 / 列字段的取值之间做运算，新增一个虚拟的取值
//! （如列字段「单据类型」里新增一项「期末」= `[入库] - [出库]`；行字段「月份」里新增「同比差」= `[本月] - [上月]`）。
//!
//! 和「计算值」（`pivot_calc`，在值之间运算）的区别：计算值新增一列**值**，计算项新增一个**取值**（行或列）。
//! 公式对每个值（金额、数量……）分别、对每个格子（明细、小计、总计）分别计算，引用的是同一字段下兄弟取值的汇总。
//! 总计列（跨列合计）与跨行的总计不含计算项，避免把「入库 + 出库 + 期末」加在一起。
//! 引用的取值在某一格没有记录时按 0 算（这个商品没有出库，期末 = 期初 + 入库）；引用的取值全都没有记录则为空；除数为 0 为空。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{Bucket, Grid, Key, PivotRequest};
use crate::error::{AppError, AppResult};
use crate::pivot_calc::{self, Expr};

#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CalcItem {
    /// 行或列字段的列名
    pub field: String,
    /// 新增的取值名称，如「期末」
    pub name: String,
    /// 公式，如 `[入库] - [出库]`；`[名称]` 是该字段下已有的取值
    pub formula: String,
}

pub struct Compiled {
    /// 在 行字段 + 列字段 里的位置
    pub pos: usize,
    pub field: String,
    pub name: String,
    expr: Expr,
    /// 公式里引用的取值名称（下标与表达式里的 Value(i) 对应）
    refs: Vec<String>,
}

/// 编译并做不依赖数据的校验；返回按请求顺序排列的计算项
pub fn compile(req: &PivotRequest) -> AppResult<Vec<Compiled>> {
    if req.calc_items.is_empty() {
        return Ok(vec![]);
    }
    if req.values.iter().any(|v| !matches!(v.show.as_deref(), None | Some("" | "raw"))) {
        return Err(AppError::bad("有计算项时，值不能设置占比 / 同比 / 环比（计算项会新增取值，这些口径会混乱）；请把它们改为「原值」，或去掉计算项"));
    }
    if req.top.is_some() {
        return Err(AppError::bad("有计算项时不能同时使用「前 N 项」（排名在数据库里算，计算项在汇总后才有）"));
    }
    let fields: Vec<&str> = req.rows.iter().chain(&req.columns).map(|f| f.column.as_str()).collect();
    let mut out: Vec<Compiled> = Vec::new();
    for item in &req.calc_items {
        let name = item.name.trim();
        if name.is_empty() {
            return Err(AppError::bad("计算项需要一个名称"));
        }
        let pos = fields.iter().position(|f| *f == item.field).ok_or_else(|| AppError::bad(format!("计算项「{name}」所在的字段「{}」不在行或列里", item.field)))?;
        if out.iter().any(|c| c.pos == pos && c.name == name) {
            return Err(AppError::bad(format!("「{}」下有两个同名的计算项「{name}」", item.field)));
        }
        let refs: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let resolve = |n: &str| -> AppResult<usize> {
            let mut r = refs.borrow_mut();
            if let Some(i) = r.iter().position(|x| x == n) {
                return Ok(i);
            }
            r.push(n.to_string());
            Ok(r.len() - 1)
        };
        let expr = pivot_calc::parse(&item.formula, &resolve).map_err(|e| match e {
            AppError::BadRequest(m) => AppError::bad(format!("计算项「{name}」：{m}")),
            other => other,
        })?;
        out.push(Compiled { pos, field: item.field.clone(), name: name.to_string(), expr, refs: refs.into_inner() });
    }
    // 计算项不能引用同一字段下的另一个计算项
    for c in &out {
        if let Some(r) = c.refs.iter().find(|r| out.iter().any(|o| o.pos == c.pos && &o.name == *r)) {
            return Err(AppError::bad(format!("计算项「{}」引用了另一个计算项「{r}」，计算项只能引用该字段下已有的取值", c.name)));
        }
        if c.refs.is_empty() {
            return Err(AppError::bad(format!("计算项「{}」的公式里没有引用任何取值（用 [取值名称] 引用）", c.name)));
        }
    }
    Ok(out)
}

fn show_names(names: &HashSet<String>) -> String {
    let mut v: Vec<&String> = names.iter().collect();
    v.sort();
    let shown: Vec<String> = v.iter().take(20).map(|n| format!("[{n}]")).collect();
    format!("{}{}", shown.join("、"), if v.len() > 20 { " 等" } else { "" })
}

impl Grid {
    /// 把计算项补进汇总结果：行字段上的补成新的行（含小计层级），列字段上的补成新的列。
    /// `round` 标出哪些值是金额（求和 / 最值），结果保留到分
    pub(super) fn apply_items(&mut self, items: &[Compiled], k: usize, nvals: usize, round: &[bool]) -> AppResult<()> {
        for item in items {
            let t = item.pos;
            // 该字段现有的取值
            let existing: HashSet<String> = if t < k {
                self.buckets.keys().filter_map(|p| p.get(t).and_then(|x| x.clone())).collect()
            } else {
                self.column_keys.iter().filter_map(|ck| ck.get(t - k).and_then(|x| x.clone())).collect()
            };
            if existing.contains(&item.name) {
                return Err(AppError::bad(format!("计算项「{}」与「{}」里已有的取值重名，请换一个名称", item.name, item.field)));
            }
            if let Some(missing) = item.refs.iter().find(|r| !existing.contains(*r)) {
                return Err(AppError::bad(format!("计算项「{}」引用的「{missing}」不是「{}」里的取值（现有：{}）", item.name, item.field, show_names(&existing))));
            }
            let calc = |sibs: &HashMap<&str, &Vec<Option<f64>>>| -> Vec<Option<f64>> {
                (0..nvals)
                    .map(|vi| {
                        let refvals: Vec<Option<f64>> = item.refs.iter().map(|r| sibs.get(r.as_str()).and_then(|v| v.get(vi).copied().flatten())).collect();
                        // 某个兄弟取值在这一格没有记录（如这个商品没有出库）按 0 算，和 Excel 透视表的空格一致；全都没有记录则为空
                        if refvals.iter().all(Option::is_none) {
                            return None;
                        }
                        let refvals: Vec<Option<f64>> = refvals.into_iter().map(|v| Some(v.unwrap_or(0.0))).collect();
                        pivot_calc::eval(&item.expr, &refvals).map(|x| if round[vi] { (x * 100.0).round() / 100.0 + 0.0 } else { x })
                    })
                    .collect()
            };
            if t < k {
                // 行：同一「去掉该字段」的前缀下，兄弟取值各是一个桶；对每个列组合分别算
                let mut groups: HashMap<Key, HashMap<&str, &Bucket>> = HashMap::new();
                for (p, b) in &self.buckets {
                    if let Some(Some(x)) = p.get(t).filter(|_| p.len() > t)
                        && item.refs.contains(x) {
                            let mut rest = p.clone();
                            rest.remove(t);
                            groups.entry(rest).or_default().insert(x.as_str(), b);
                        }
                }
                let mut made: Vec<(Key, Bucket)> = Vec::new();
                for (rest, sibs) in &groups {
                    let mut np = rest.clone();
                    np.insert(t, Some(item.name.clone()));
                    let col_keys: HashSet<&Option<Key>> = sibs.values().flat_map(|b| b.by_col.keys()).collect();
                    let mut nb = Bucket::default();
                    for ck in col_keys {
                        let per_ref: HashMap<&str, &Vec<Option<f64>>> = sibs.iter().filter_map(|(n, b)| b.by_col.get(ck).map(|v| (*n, v))).collect();
                        nb.by_col.insert(ck.clone(), calc(&per_ref));
                    }
                    made.push((np, nb));
                }
                for (p, b) in made {
                    self.buckets.insert(p, b);
                }
            } else {
                // 列：每个行前缀里，去掉该列字段后相同的列组合互为兄弟
                let tc = t - k;
                let mut made: Vec<(Key, Key, Vec<Option<f64>>)> = Vec::new();
                for (p, b) in &self.buckets {
                    let mut groups: HashMap<Key, HashMap<&str, &Vec<Option<f64>>>> = HashMap::new();
                    for (ck, v) in &b.by_col {
                        let Some(ck) = ck else { continue };
                        if let Some(Some(x)) = ck.get(tc)
                            && item.refs.contains(x) {
                                let mut rest = ck.clone();
                                rest.remove(tc);
                                groups.entry(rest).or_default().insert(x.as_str(), v);
                            }
                    }
                    for (rest, sibs) in groups {
                        let mut nck = rest;
                        nck.insert(tc, Some(item.name.clone()));
                        made.push((p.clone(), nck, calc(&sibs)));
                    }
                }
                for (p, nck, vals) in made {
                    self.column_keys.insert(nck.clone());
                    if let Some(b) = self.buckets.get_mut(&p) {
                        b.by_col.insert(Some(nck), vals);
                    }
                }
            }
        }
        Ok(())
    }
}

/// 每个行层级上的计算项名称（展示时排在同级最后）
pub fn row_names(items: &[Compiled], k: usize) -> Vec<HashSet<String>> {
    let mut out = vec![HashSet::new(); k];
    for c in items.iter().filter(|c| c.pos < k) {
        out[c.pos].insert(c.name.clone());
    }
    out
}

/// 每个列层级上的计算项名称
pub fn col_names(items: &[Compiled], k: usize, m: usize) -> Vec<HashSet<String>> {
    let mut out = vec![HashSet::new(); m];
    for c in items.iter().filter(|c| c.pos >= k) {
        out[c.pos - k].insert(c.name.clone());
    }
    out
}
