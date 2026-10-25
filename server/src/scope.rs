//! 数据范围（行级权限）：按「维度」限制用户能看到的数据
//!
//! - **维度**：一个名字（店铺 / 地区 / 部门…），代表「按哪类取值划分范围」。
//! - **规则**：管理员给用户（或「默认规则」`*`，用于没有单独规则的用户）在某个维度上指定可见的取值。一个用户可以在几个维度上同时受限；
//!   没有任何规则的用户、管理员不受限制，所以开启后不会把所有人锁在外面
//! - **数据集绑定**：每个数据集由管理员指定「按哪个维度的哪一列过滤」或「不限范围（所有人可见）」；内置的资金流水固定按「店铺」维度的店铺列；
//!   关联视图没有单独设置时沿用左表的绑定；**没有绑定的数据集对受限用户不可见**（默认安全）。
//!   数据集绑定的维度上用户不受限（没有这个维度的规则）时，整个数据集可见
//! - **怎么生效**：受限用户的请求在「加载数据集定义」这个统一出口把数据来源换成带过滤条件的子查询，
//!   透视、公式下推、SQL、AI、导出、游标等所有走数据集定义的读取自动受限；游标缓存按范围区分
//! - **绕过过滤的写操作**（导入、建视图、定时推送、保存 SQL…）对受限用户一律关闭：`auth.rs` 的 `RESTRICTED_OK` 是白名单，未登记的接口默认关闭
//! - 不覆盖：已经写进工作簿单元格的数值（共享工作簿由所有者负责）；MCP 令牌与定时推送按服务账号执行，不受创建人范围限制

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize, Serializer};

use crate::auth::CurrentUser;
use crate::dataset::{DatasetDef, column_sql};
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::AppState;
use crate::schema::literal;

/// 没有单独规则的用户适用的默认规则
pub const DEFAULT_SUBJECT: &str = "*";

/// 默认维度：不写维度时用它（最早只有「店铺」这一个维度，老规则都迁移到这里；内置的资金流水固定用它）
pub const SHOP_DIMENSION: &str = "店铺";

const MAX_DIMENSION_CHARS: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    /// 是否受限（至少有一个维度受限）
    pub restricted: bool,
    /// 受限的维度 → 可见的取值
    pub dims: BTreeMap<String, Vec<String>>,
}

/// 给前端：`restricted`、`dimensions`（维度 → 可见取值），以及兼容老前端的 `shops`（「店铺」维度的取值）
impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("Scope", 3)?;
        st.serialize_field("restricted", &self.restricted)?;
        st.serialize_field("shops", self.dims.get(SHOP_DIMENSION).map_or(&[][..], Vec::as_slice))?;
        st.serialize_field("dimensions", &self.dims)?;
        st.end()
    }
}

fn clean(mut values: Vec<String>) -> Vec<String> {
    values.retain(|s| !s.trim().is_empty());
    values.sort();
    values.dedup();
    values
}

impl Scope {
    pub fn all() -> Self {
        Self { restricted: false, dims: BTreeMap::new() }
    }

    /// 只在一个维度上受限
    #[cfg(test)]
    pub fn dimension(name: &str, values: Vec<String>) -> Self {
        Self { restricted: true, dims: BTreeMap::from([(name.to_string(), clean(values))]) }
    }

    /// 「店铺」维度受限
    #[cfg(test)]
    pub fn shops(values: Vec<String>) -> Self {
        Self::dimension(SHOP_DIMENSION, values)
    }

    /// 这个取值在这个维度的范围内吗：维度不受限时都在范围内
    pub fn allows_in(&self, dimension: &str, value: &str) -> bool {
        self.dims.get(dimension).is_none_or(|v| v.iter().any(|s| s == value))
    }

    /// 这个店铺在范围内吗
    pub fn allows(&self, shop: &str) -> bool {
        self.allows_in(SHOP_DIMENSION, shop)
    }

    /// 范围的签名：游标缓存按它区分（不受限为空）
    pub fn sig(&self) -> String {
        if !self.restricted {
            return String::new();
        }
        let parts: Vec<String> = self.dims.iter().map(|(d, v)| format!("{d}={}", v.join("\u{1f}"))).collect();
        format!("r:{}", parts.join("\u{1e}"))
    }

    /// 给用户看的描述：「店铺：A、B」；几个维度用分号隔开
    pub fn describe(&self) -> String {
        let one = |v: &Vec<String>| if v.is_empty() { "没有任何取值".to_string() } else { v.join("、") };
        self.dims.iter().map(|(d, v)| format!("{d}：{}", one(v))).collect::<Vec<_>>().join("；")
    }
}

/// 放进请求扩展里，供处理函数取用
#[derive(Clone)]
pub struct ScopeExt(pub Arc<Scope>);

tokio::task_local! {
    /// 当前请求的数据范围（异步任务内）
    pub static CURRENT: Arc<Scope>;
}

thread_local! {
    /// 阻塞线程里正在执行的数据库操作所属请求的数据范围
    static AMBIENT: RefCell<Option<Arc<Scope>>> = const { RefCell::new(None) };
}

/// 异步任务里当前请求的数据范围（只返回受限的）
pub fn current_task_scope() -> Option<Arc<Scope>> {
    CURRENT.try_with(Arc::clone).ok().filter(|s| s.restricted)
}

/// 在阻塞线程里执行 f，期间 `datasets()` 按这个范围加载数据集
pub fn run_with<T>(scope: Option<Arc<Scope>>, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<Arc<Scope>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            AMBIENT.with(|a| *a.borrow_mut() = self.0.take());
        }
    }
    let prev = AMBIENT.with(|a| a.replace(scope));
    let _reset = Reset(prev);
    f()
}

fn ambient() -> Option<Arc<Scope>> {
    AMBIENT.with(|a| a.borrow().clone()).filter(|s| s.restricted)
}

/// 阻塞线程里当前请求的用户数据范围受限吗（AI 工具判断能不能给出清洗 / 生成数据集的建议）
pub fn restricted_now() -> bool {
    ambient().is_some()
}

// ── 规则 ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleRow {
    pub subject_id: String,
    pub subject_name: String,
    /// 维度
    pub dimension: String,
    /// all 不限 / values 指定取值
    pub mode: String,
    pub values: Vec<String>,
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
}

/// 用户 id → 维度 → 规则
type RuleMap = HashMap<String, HashMap<String, RuleRow>>;

#[derive(Default)]
pub struct ScopeStore {
    cache: Mutex<Option<Arc<RuleMap>>>,
}

impl ScopeStore {
    pub fn invalidate(&self) {
        if let Ok(mut c) = self.cache.lock() {
            *c = None;
        }
    }

    fn rules(&self, conn: &Connection) -> AppResult<Arc<RuleMap>> {
        if let Some(r) = self.cache.lock().ok().and_then(|c| c.clone()) {
            return Ok(r);
        }
        let mut map: RuleMap = HashMap::new();
        for r in list_rules(conn)? {
            map.entry(r.subject_id.clone()).or_default().insert(r.dimension.clone(), r);
        }
        let arc = Arc::new(map);
        if let Ok(mut c) = self.cache.lock() {
            *c = Some(arc.clone());
        }
        Ok(arc)
    }

    /// 用户的数据范围：管理员 / 未开启登录校验不受限；否则每个维度取用户自己的规则，没有就取默认规则，都没有则这个维度不受限
    pub fn resolve(&self, conn: &Connection, user: &CurrentUser) -> AppResult<Scope> {
        if !user.enforced || user.admin {
            return Ok(Scope::all());
        }
        let rules = self.rules(conn)?;
        let (own, default) = (rules.get(&user.id), rules.get(DEFAULT_SUBJECT));
        let mut dims: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let names: std::collections::BTreeSet<&String> = own.iter().chain(default.iter()).flat_map(|m| m.keys()).collect();
        for dim in names {
            // 用户自己在这个维度有规则就以它为准（可以是「不限」），否则用默认规则
            let Some(rule) = own.and_then(|m| m.get(dim)).or_else(|| default.and_then(|m| m.get(dim))) else { continue };
            if rule.mode == "values" {
                dims.insert(dim.clone(), clean(rule.values.clone()));
            }
        }
        Ok(if dims.is_empty() { Scope::all() } else { Scope { restricted: true, dims } })
    }
}

fn list_rules(conn: &Connection) -> AppResult<Vec<RuleRow>> {
    let mut stmt = conn.prepare(
        "SELECT subject_id, COALESCE(subject_name, ''), dimension, mode, COALESCE(vals, '[]'), updated_by, strftime(updated_at, '%Y-%m-%d %H:%M:%S')
         FROM ledger_scope_rule ORDER BY subject_id = '*' DESC, subject_name, subject_id, dimension",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let vals: String = r.get(4)?;
            Ok(RuleRow { subject_id: r.get(0)?, subject_name: r.get(1)?, dimension: r.get(2)?, mode: r.get(3)?, values: serde_json::from_str(&vals).unwrap_or_default(), updated_by: r.get(5)?, updated_at: r.get(6)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ── 数据集绑定 ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Binding {
    /// 所有人可见
    Open,
    /// 按这一列（列的 key）、这个维度过滤
    Column { key: String, dimension: String },
    /// 没有绑定：受限用户看不到
    Hidden,
}

/// 数据集 id → (过滤列, 所有人可见, 维度)
type StoredBindings = HashMap<String, (Option<String>, bool, Option<String>)>;

fn stored_bindings(conn: &Connection) -> AppResult<StoredBindings> {
    let mut stmt = conn.prepare("SELECT id, scope_column, COALESCE(scope_open, false), scope_dimension FROM ledger_dataset")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, Option<String>>(1)?, r.get::<_, bool>(2)?, r.get::<_, Option<String>>(3)?))))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// 数据集自己设置的绑定（内置的资金流水固定按店铺列、店铺维度）
fn own_binding(d: &DatasetDef, stored: &StoredBindings) -> (Option<String>, bool, String) {
    if d.builtin {
        return (Some("shop".to_string()), false, SHOP_DIMENSION.to_string());
    }
    let (col, open, dim) = stored.get(&d.id).cloned().unwrap_or((None, false, None));
    (col, open, dim.filter(|x| !x.trim().is_empty()).unwrap_or_else(|| SHOP_DIMENSION.to_string()))
}

/// 数据集的有效绑定（defs 需按依赖顺序：被引用的在前）
fn effective(defs: &[DatasetDef], stored: &StoredBindings) -> HashMap<String, Binding> {
    let mut out: HashMap<String, Binding> = HashMap::new();
    for d in defs {
        // 汇总数据集沿用源数据集的范围（数据来源在 apply 里按受限后的源重建）
        if let Some(src) = d.summary.as_ref().and_then(|s| out.get(&s.source)).cloned() {
            out.insert(d.id.clone(), src);
            continue;
        }
        let (col, open, dimension) = own_binding(d, stored);
        let b = if let Some(key) = col.filter(|c| d.columns.iter().any(|x| &x.key == c)) {
            Binding::Column { key, dimension }
        } else if open {
            Binding::Open
        } else if let Some(Binding::Column { key, dimension }) = d.view.as_ref().and_then(|v| out.get(&v.left)).cloned().filter(|b| matches!(b, Binding::Column { .. })) {
            // 关联视图没有单独设置：沿用左表的绑定（视图里保留了左表的列）
            if d.columns.iter().any(|x| x.key == key) { Binding::Column { key, dimension } } else { Binding::Hidden }
        } else {
            Binding::Hidden
        };
        out.insert(d.id.clone(), b);
    }
    out
}

/// 受限用户：把数据来源换成带过滤条件的子查询，没有绑定的数据集去掉
pub fn apply(conn: &Connection, defs: Vec<DatasetDef>, scope: &Scope) -> AppResult<Vec<DatasetDef>> {
    let stored = stored_bindings(conn)?;
    let bindings = effective(&defs, &stored);
    let sig = scope.sig();
    let mut out: Vec<DatasetDef> = Vec::new();
    for mut d in defs {
        // 汇总数据集：源数据集对他可见才可见，数据来源按受限后的源重建（汇总的是他能看到的行）
        if let Some(src) = d.summary.as_ref().map(|s| s.source.clone()) {
            let Some(source) = out.iter().find(|x| x.id == src).cloned() else { continue };
            if !source.scope_sig.is_empty() {
                if crate::summary::rescope(&mut d, &source).is_err() {
                    continue;
                }
                d.scope_sig = sig.clone();
            }
            out.push(d);
            continue;
        }
        match bindings.get(&d.id) {
            Some(Binding::Open) => out.push(d),
            Some(Binding::Column { key, dimension }) => {
                // 用户在这个维度上不受限：整个数据集可见
                let Some(values) = scope.dims.get(dimension) else {
                    out.push(d);
                    continue;
                };
                let Some(col) = d.columns.iter().find(|c| &c.key == key) else { continue };
                let list = values.iter().map(|s| literal(s)).collect::<Vec<_>>().join(", ");
                let pred = if values.is_empty() { "false".to_string() } else { format!("CAST(({}) AS VARCHAR) IN ({list})", column_sql(col)) };
                // 表用 rowid 定位行，子查询里要把它带出来；视图自带 __rid
                let head = if d.rid == "rowid" { "rowid AS rowid, *" } else { "*" };
                d.table = format!("(SELECT {head} FROM {} WHERE {pred})", d.table);
                d.scope_sig = sig.clone();
                out.push(d);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// 填报时的范围限制：None = 不受限；Some((范围列的 key, 可以填的取值))。
/// 受限用户只能填报「按某个维度的某一列划分范围」的数据集，而且只能填自己范围内的行（所有人可见的共享数据集、没有绑定的都不行）；
/// 数据集绑定的维度上用户本来就不受限时，没有额外限制
pub fn write_limit(conn: &Connection, dataset_id: &str) -> AppResult<Option<(String, Vec<String>)>> {
    let Some(s) = ambient() else { return Ok(None) };
    let (defs, _) = crate::dataset::load(conn)?;
    let stored = stored_bindings(conn)?;
    match effective(&defs, &stored).get(dataset_id) {
        Some(Binding::Column { key, dimension }) => Ok(s.dims.get(dimension).map(|v| (key.clone(), v.clone()))),
        _ => Err(AppError::Forbidden(format!("你的数据范围受限，只能填报按某一列划分范围的数据集。{}", restriction_note(&s).unwrap_or_default()))),
    }
}

/// 当前线程的请求若受限，对数据集定义做范围处理（`dataset::datasets` 调用）
pub fn restrict(conn: &Connection, defs: Vec<DatasetDef>) -> AppResult<Vec<DatasetDef>> {
    match ambient() {
        Some(s) => apply(conn, defs, &s),
        None => Ok(defs),
    }
}

// ── 管理接口（仅管理员，路由表限制）──────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetBinding {
    pub id: String,
    pub name: String,
    /// builtin / table / view
    pub kind: String,
    /// 设置的过滤列（列 key）
    pub scope_column: Option<String>,
    /// 设置的维度（没设置时按「店铺」）
    pub scope_dimension: String,
    /// 设置为所有人可见
    pub open: bool,
    /// 实际生效：column 按列过滤 / open 所有人可见 / inherit 沿用左表 / hidden 受限用户看不到
    pub effective: String,
    pub effective_column: Option<String>,
    pub effective_dimension: Option<String>,
    pub columns: Vec<ColumnOption>,
}

#[derive(Serialize)]
pub struct ColumnOption {
    pub key: String,
    pub title: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeOverview {
    pub rules: Vec<RuleRow>,
    pub datasets: Vec<DatasetBinding>,
    /// 已经用到的维度（规则或数据集绑定里出现过的，加上默认的「店铺」）
    pub dimensions: Vec<String>,
    /// 各维度的取值候选（目前只有「店铺」有：来自店铺档案）；其他维度由管理员自己输入
    pub candidates: BTreeMap<String, Vec<String>>,
    /// 兼容老前端：「店铺」维度的候选
    pub shops: Vec<String>,
}

pub fn overview(conn: &Connection) -> AppResult<ScopeOverview> {
    let defs = crate::dataset::datasets(conn)?;
    let stored = stored_bindings(conn)?;
    let eff = effective(&defs, &stored);
    let mut dimensions: std::collections::BTreeSet<String> = std::collections::BTreeSet::from([SHOP_DIMENSION.to_string()]);
    let datasets = defs
        .iter()
        .map(|d| {
            let (col, open, dim) = own_binding(d, &stored);
            let (effective, effective_column, effective_dimension) = match eff.get(&d.id) {
                Some(Binding::Open) => ("open", None, None),
                Some(Binding::Column { key, dimension }) if col.as_deref() == Some(key) => ("column", Some(key.clone()), Some(dimension.clone())),
                Some(Binding::Column { key, dimension }) => ("inherit", Some(key.clone()), Some(dimension.clone())),
                _ => ("hidden", None, None),
            };
            if let Some(x) = &effective_dimension {
                dimensions.insert(x.clone());
            }
            DatasetBinding {
                id: d.id.clone(),
                name: d.name.clone(),
                kind: d.kind.clone(),
                scope_column: col,
                scope_dimension: dim,
                open,
                effective: effective.into(),
                effective_column,
                effective_dimension,
                columns: d.columns.iter().map(|c| ColumnOption { key: c.key.clone(), title: c.title.clone() }).collect(),
            }
        })
        .collect();
    let rules = list_rules(conn)?;
    dimensions.extend(rules.iter().map(|r| r.dimension.clone()));
    let mut stmt = conn.prepare("SELECT name FROM ledger_shop ORDER BY name")?;
    let shops = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let candidates = if shops.is_empty() { BTreeMap::new() } else { BTreeMap::from([(SHOP_DIMENSION.to_string(), shops.clone())]) };
    Ok(ScopeOverview { rules, datasets, dimensions: dimensions.into_iter().collect(), candidates, shops })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleInput {
    pub subject_id: String,
    #[serde(default)]
    pub subject_name: String,
    /// 维度；不写按「店铺」
    #[serde(default)]
    pub dimension: Option<String>,
    /// all / values（老写法 shops 也认）
    pub mode: String,
    /// 可见的取值（老字段名 shops 也认）
    #[serde(default, alias = "shops")]
    pub values: Vec<String>,
    #[serde(default)]
    pub operator: String,
}

pub(crate) fn check_dimension(dim: Option<&str>) -> AppResult<String> {
    let d = dim.map(str::trim).filter(|d| !d.is_empty()).unwrap_or(SHOP_DIMENSION);
    if d.chars().count() > MAX_DIMENSION_CHARS || d.chars().any(char::is_control) {
        return Err(AppError::bad(format!("维度名称不超过 {MAX_DIMENSION_CHARS} 个字符，不含控制字符")));
    }
    Ok(d.to_string())
}

pub fn save_rule(conn: &Connection, input: &RuleInput) -> AppResult<()> {
    let id = input.subject_id.trim();
    if id.is_empty() {
        return Err(AppError::bad("请选择用户"));
    }
    let mode = match input.mode.as_str() {
        "all" => "all",
        "values" | "shops" => "values",
        _ => return Err(AppError::bad("范围应为 all（不限）或 values（指定取值）")),
    };
    let dimension = check_dimension(input.dimension.as_deref())?;
    let values = clean(input.values.clone());
    let name = if id == DEFAULT_SUBJECT {
        "默认规则（没有单独规则的用户）".to_string()
    } else {
        crate::approval::user_name(conn, id)?.unwrap_or_else(|| input.subject_name.trim().to_string())
    };
    conn.execute(
        "INSERT INTO ledger_scope_rule (subject_id, dimension, subject_name, mode, vals, updated_by, updated_at) VALUES (?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))
         ON CONFLICT (subject_id, dimension) DO UPDATE SET subject_name = excluded.subject_name, mode = excluded.mode, vals = excluded.vals, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![id, dimension, name, mode, serde_json::to_string(&values)?, input.operator, now_local()],
    )?;
    Ok(())
}

/// 删除规则：不指定维度删掉这个用户的全部规则
pub fn delete_rule(conn: &Connection, subject_id: &str, dimension: Option<&str>) -> AppResult<()> {
    match dimension.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => conn.execute("DELETE FROM ledger_scope_rule WHERE subject_id = ? AND dimension = ?", params![subject_id, d])?,
        None => conn.execute("DELETE FROM ledger_scope_rule WHERE subject_id = ?", params![subject_id])?,
    };
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingInput {
    /// 按这一列过滤（列 key）；为空表示不按列
    #[serde(default)]
    pub scope_column: Option<String>,
    /// 按哪个维度的规则过滤；不写按「店铺」
    #[serde(default)]
    pub scope_dimension: Option<String>,
    /// 所有人可见（不限范围）
    #[serde(default)]
    pub open: bool,
}

pub fn save_binding(conn: &Connection, dataset_id: &str, input: &BindingInput) -> AppResult<()> {
    let def = crate::dataset::datasets(conn)?.into_iter().find(|d| d.id == dataset_id).ok_or_else(|| AppError::bad("数据集不存在"))?;
    if def.builtin {
        return Err(AppError::bad("资金流水固定按「店铺」列过滤，不能修改"));
    }
    let col = input.scope_column.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let dimension = check_dimension(input.scope_dimension.as_deref())?;
    if let Some(c) = col {
        if input.open {
            return Err(AppError::bad("「按列过滤」与「所有人可见」只能选一种"));
        }
        if !def.columns.iter().any(|x| x.key == c) {
            return Err(AppError::bad("数据集里没有这一列"));
        }
    }
    let dim = col.map(|_| dimension);
    let changed = conn.execute("UPDATE ledger_dataset SET scope_column = ?, scope_open = ?, scope_dimension = ? WHERE id = ?", params![col, input.open, dim, dataset_id])?;
    if changed == 0 {
        return Err(AppError::bad("数据集不存在"));
    }
    Ok(())
}

/// 受限用户的说明（界面提示用）；不受限返回 None
pub fn restriction_note(scope: &Scope) -> Option<String> {
    scope.restricted.then(|| format!("你的数据范围被限制在{}", scope.describe()))
}

async fn run<T: Send + 'static>(db: &AppState, f: impl FnOnce(&Connection) -> AppResult<T> + Send + 'static) -> AppResult<T> {
    crate::auth::run(db, f).await
}

/// GET /api/ledger/data-scope  规则、数据集绑定、维度与取值候选
pub async fn get_overview(State(db): State<AppState>) -> AppResult<Json<Envelope<ScopeOverview>>> {
    Ok(ok(run(&db, overview).await?))
}

/// PUT /api/ledger/data-scope/rules  { subjectId, subjectName?, dimension?, mode: all | values, values }  subjectId 为 * 是默认规则；dimension 不写按「店铺」
pub async fn put_rule(State(db): State<AppState>, Extension(_): Extension<CurrentUser>, Json(input): Json<RuleInput>) -> AppResult<Json<Envelope<bool>>> {
    run(&db, move |c| save_rule(c, &input)).await?;
    db.scope.invalidate();
    Ok(ok(true))
}

#[derive(Deserialize)]
pub struct DeleteRuleQuery {
    #[serde(default)]
    pub dimension: Option<String>,
}

/// DELETE /api/ledger/data-scope/rules/{id}?dimension=  删除规则（该用户回到默认规则）；不写 dimension 删掉这个用户的全部规则
pub async fn remove_rule(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<DeleteRuleQuery>) -> AppResult<Json<Envelope<bool>>> {
    run(&db, move |c| delete_rule(c, &id, q.dimension.as_deref())).await?;
    db.scope.invalidate();
    Ok(ok(true))
}

/// PUT /api/ledger/data-scope/datasets/{id}  { scopeColumn?, scopeDimension?, open }
pub async fn put_binding(State(db): State<AppState>, Path(id): Path<String>, Json(input): Json<BindingInput>) -> AppResult<Json<Envelope<bool>>> {
    run(&db, move |c| save_binding(c, &id, &input)).await?;
    Ok(ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::datasets;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        conn.execute_batch(
            r#"CREATE TABLE ds_orders ("店铺" VARCHAR, "金额" DOUBLE, search_text VARCHAR);
               INSERT INTO ds_orders VALUES ('A',1,'a'),('B',2,'b'),('A',3,'a'),('C',4,'c');
               CREATE TABLE ds_cost ("店铺" VARCHAR, "成本" DOUBLE, search_text VARCHAR);
               INSERT INTO ds_cost VALUES ('A',10,'a'),('B',20,'b');
               INSERT INTO ledger_dataset (id, name, columns, primary_key, kind) VALUES
                 ('ds_orders', '订单', '[{"key":"店铺","title":"店铺","kind":"text","width":100},{"key":"金额","title":"金额","kind":"money","width":100}]', '[]', 'table'),
                 ('ds_cost', '成本', '[{"key":"店铺","title":"店铺","kind":"text","width":100},{"key":"成本","title":"成本","kind":"money","width":100}]', '[]', 'table');"#,
        )
        .unwrap();
        conn
    }

    fn count(conn: &Connection, def: &DatasetDef) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {}", def.table), [], |r| r.get(0)).unwrap()
    }

    fn user(id: &str, admin: bool) -> CurrentUser {
        CurrentUser { id: id.into(), name: format!("用户{id}"), admin, enforced: true }
    }

    #[test]
    fn unbound_hidden_bound_filtered_open_visible() {
        let conn = setup();
        let scope = Scope::shops(vec!["A".into(), "B".into(), "A".into()]);
        assert_eq!(scope.dims["店铺"], vec!["A", "B"]);
        // 没有绑定：受限用户一个数据集都看不到（流水固定按店铺列，在）
        let names = |defs: &[DatasetDef]| defs.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
        let r = run_with(Some(Arc::new(scope.clone())), || datasets(&conn)).unwrap();
        assert_eq!(names(&r), vec!["资金流水"]);
        // 不受限：全部可见
        assert_eq!(datasets(&conn).unwrap().len(), 3);

        // 订单按店铺列过滤，成本所有人可见
        save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("店铺".into()), scope_dimension: None, open: false }).unwrap();
        save_binding(&conn, "ds_cost", &BindingInput { scope_column: None, scope_dimension: None, open: true }).unwrap();
        let r = run_with(Some(Arc::new(scope.clone())), || datasets(&conn)).unwrap();
        assert_eq!(names(&r), vec!["资金流水", "订单", "成本"]);
        let orders = r.iter().find(|d| d.name == "订单").unwrap();
        let cost = r.iter().find(|d| d.name == "成本").unwrap();
        assert_eq!(count(&conn, orders), 3); // A,B,A
        assert_eq!(count(&conn, cost), 2);
        assert!(!orders.scope_sig.is_empty() && cost.scope_sig.is_empty());
        // 行号可用：游标 / 取行用 rowid 连接
        let rows: Vec<f64> = conn.prepare(&format!("SELECT t.\"金额\" FROM (SELECT rowid AS rid FROM ds_orders) c JOIN {} t ON t.rowid = c.rid ORDER BY 1", orders.table)).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(rows, vec![1.0, 2.0, 3.0]);
        // 空范围：什么都看不到
        let none = run_with(Some(Arc::new(Scope::shops(vec![]))), || datasets(&conn)).unwrap();
        assert_eq!(count(&conn, none.iter().find(|d| d.name == "订单").unwrap()), 0);
        // 作用域结束后恢复
        assert_eq!(datasets(&conn).unwrap().len(), 3);
        assert!(ambient().is_none());
    }

    #[test]
    fn binding_validation_and_builtin() {
        let conn = setup();
        assert!(save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("不存在".into()), scope_dimension: None, open: false }).unwrap_err().message().contains("没有这一列"));
        assert!(save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("店铺".into()), scope_dimension: None, open: true }).unwrap_err().message().contains("只能选一种"));
        assert!(save_binding(&conn, "ledger_detail", &BindingInput { scope_column: None, scope_dimension: None, open: true }).is_err());
        assert!(save_binding(&conn, "nope", &BindingInput { scope_column: None, scope_dimension: None, open: true }).is_err());
        let ov = overview(&conn).unwrap();
        let find = |n: &str| ov.datasets.iter().find(|d| d.name == n).unwrap();
        assert_eq!(find("资金流水").effective, "column");
        assert_eq!(find("订单").effective, "hidden");
        save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("店铺".into()), scope_dimension: None, open: false }).unwrap();
        assert_eq!(overview(&conn).unwrap().datasets.iter().find(|d| d.name == "订单").unwrap().effective, "column");
    }

    #[test]
    fn rules_resolution() {
        let conn = setup();
        let store = ScopeStore::default();
        let (a, b, root) = (user("a", false), user("b", false), user("root", true));
        // 没有规则：不受限
        assert!(!store.resolve(&conn, &a).unwrap().restricted);
        // 给 a 指定店铺；b 没有规则仍不受限
        crate::approval::touch_user(&conn, &a).unwrap();
        save_rule(&conn, &RuleInput { subject_id: "a".into(), subject_name: String::new(), dimension: None, mode: "values".into(), values: vec!["B".into(), "A".into()], operator: "x".into() }).unwrap();
        store.invalidate();
        let sa = store.resolve(&conn, &a).unwrap();
        assert_eq!((sa.restricted, sa.dims["店铺"].clone()), (true, vec!["A".to_string(), "B".to_string()]));
        assert!(sa.allows("A") && !sa.allows("C"));
        assert!(!store.resolve(&conn, &b).unwrap().restricted);
        // 默认规则：没有单独规则的人都受限；有单独规则的以自己的为准（可以是「不限」）
        save_rule(&conn, &RuleInput { subject_id: "*".into(), subject_name: String::new(), dimension: None, mode: "values".into(), values: vec!["A".into()], operator: "x".into() }).unwrap();
        save_rule(&conn, &RuleInput { subject_id: "c".into(), subject_name: "小陈".into(), dimension: None, mode: "all".into(), values: vec![], operator: "x".into() }).unwrap();
        store.invalidate();
        assert_eq!(store.resolve(&conn, &b).unwrap().dims["店铺"].clone(), vec!["A".to_string()]);
        assert!(!store.resolve(&conn, &user("c", false)).unwrap().restricted);
        // 管理员与未开启登录校验不受任何规则影响
        assert!(!store.resolve(&conn, &root).unwrap().restricted);
        assert!(!store.resolve(&conn, &CurrentUser::open()).unwrap().restricted);
        // 删除后回到默认规则
        delete_rule(&conn, "a", None).unwrap();
        store.invalidate();
        assert_eq!(store.resolve(&conn, &a).unwrap().dims["店铺"].clone(), vec!["A".to_string()]);
        // 校验
        assert!(save_rule(&conn, &RuleInput { subject_id: " ".into(), subject_name: String::new(), dimension: None, mode: "all".into(), values: vec![], operator: String::new() }).is_err());
        assert!(save_rule(&conn, &RuleInput { subject_id: "a".into(), subject_name: String::new(), dimension: None, mode: "x".into(), values: vec![], operator: String::new() }).is_err());
        assert_eq!(list_rules(&conn).unwrap()[0].subject_id, "*");
    }

    /// 数据范围要能从请求任务传到阻塞线程（数据库操作）和 AI 的后台任务：AI 助手的查询靠这条链路受限
    #[tokio::test]
    async fn scope_reaches_blocking_threads_and_background_tasks() {
        let shops = |s: Option<Arc<Scope>>| run_with(s, || ambient().map(|a| a.dims["店铺"].clone()));
        let s = Arc::new(Scope::shops(vec!["A".into()]));
        let got = CURRENT
            .scope(s.clone(), async {
                // 与 routes::blocking 相同：在任务里取范围，带进阻塞线程
                let sc = current_task_scope();
                let direct = tokio::task::spawn_blocking(move || shops(sc)).await.unwrap();
                // 与 ai_chat 相同：后台任务重新带上范围
                let sc = current_task_scope();
                let background = tokio::spawn(async move {
                    match sc {
                        Some(sc) => CURRENT.scope(sc, async { tokio::task::spawn_blocking({ let c = current_task_scope(); move || shops(c) }).await.unwrap() }).await,
                        None => None,
                    }
                })
                .await
                .unwrap();
                // 不带上的话后台任务看不到范围（受限用户的查询会变成不受限）——所以必须像上面那样显式带上
                let lost = tokio::spawn(async { current_task_scope().is_none() }).await.unwrap();
                (direct, background, lost)
            })
            .await;
        assert_eq!(got.0, Some(vec!["A".to_string()]));
        assert_eq!(got.1, Some(vec!["A".to_string()]));
        assert!(got.2);
        // 请求之外没有范围；不受限的范围不会被带进去
        assert!(current_task_scope().is_none());
        assert!(CURRENT.scope(Arc::new(Scope::all()), async { current_task_scope() }).await.is_none());
    }

    #[test]
    fn scope_signature_and_notes() {
        assert_eq!(Scope::all().sig(), "");
        assert_ne!(Scope::shops(vec!["A".into()]).sig(), Scope::shops(vec!["B".into()]).sig());
        assert_eq!(Scope::shops(vec!["B".into(), "A".into()]).sig(), Scope::shops(vec!["A".into(), "B".into()]).sig());
        assert!(Scope::all().allows("任意"));
        assert!(!Scope::shops(vec![]).allows("A"));
        assert!(restriction_note(&Scope::all()).is_none());
        assert!(restriction_note(&Scope::shops(vec!["A".into(), "B".into()])).unwrap().contains("A、B"));
    }

    fn rule(conn: &Connection, subject: &str, dim: &str, mode: &str, values: &[&str]) {
        save_rule(conn, &RuleInput { subject_id: subject.into(), subject_name: subject.into(), dimension: Some(dim.into()), mode: mode.into(), values: values.iter().map(|s| s.to_string()).collect(), operator: "x".into() }).unwrap();
    }

    /// 数据集绑定到不同维度：同一个用户在不同数据集上受不同规则限制
    #[test]
    fn different_datasets_follow_different_dimensions() {
        let conn = setup();
        conn.execute_batch(
            r#"CREATE TABLE ds_region ("地区" VARCHAR, "金额" DOUBLE, search_text VARCHAR);
               INSERT INTO ds_region VALUES ('华东',1,'x'),('华北',2,'x'),('华南',3,'x');
               INSERT INTO ledger_dataset (id, name, columns, primary_key, kind) VALUES
                 ('ds_region', '区域销售', '[{"key":"地区","title":"地区","kind":"text","width":100},{"key":"金额","title":"金额","kind":"money","width":100}]', '[]', 'table');"#,
        )
        .unwrap();
        save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("店铺".into()), scope_dimension: None, open: false }).unwrap();
        save_binding(&conn, "ds_region", &BindingInput { scope_column: Some("地区".into()), scope_dimension: Some("地区".into()), open: false }).unwrap();
        save_binding(&conn, "ds_cost", &BindingInput { scope_column: None, scope_dimension: None, open: true }).unwrap();
        let store = ScopeStore::default();
        let u = user("u", false);

        // 只限制地区：订单（店铺维度）整个可见，区域销售按地区过滤
        rule(&conn, "u", "地区", "values", &["华东", "华南"]);
        store.invalidate();
        let scope = store.resolve(&conn, &u).unwrap();
        assert!(scope.restricted && scope.dims.len() == 1 && scope.dims.contains_key("地区"));
        let defs = run_with(Some(Arc::new(scope.clone())), || datasets(&conn)).unwrap();
        let by = |n: &str| defs.iter().find(|d| d.name == n).unwrap();
        assert_eq!(count(&conn, by("订单")), 4, "店铺维度不受限：订单全可见");
        assert_eq!(count(&conn, by("区域销售")), 2, "地区维度受限：只有华东、华南");
        assert_eq!(count(&conn, by("成本")), 2);
        assert!(by("订单").scope_sig.is_empty() && !by("区域销售").scope_sig.is_empty());

        // 再限制店铺：两个维度同时生效
        rule(&conn, "u", "店铺", "values", &["A"]);
        store.invalidate();
        let scope = store.resolve(&conn, &u).unwrap();
        assert_eq!(scope.dims.len(), 2);
        let defs = run_with(Some(Arc::new(scope.clone())), || datasets(&conn)).unwrap();
        assert_eq!(count(&conn, defs.iter().find(|d| d.name == "订单").unwrap()), 2);
        assert_eq!(count(&conn, defs.iter().find(|d| d.name == "区域销售").unwrap()), 2);
        // 范围签名区分维度（游标缓存不串）
        assert_ne!(scope.sig(), Scope::shops(vec!["A".into()]).sig());

        // 地区改为「不限」：只剩店铺受限
        rule(&conn, "u", "地区", "all", &[]);
        store.invalidate();
        let scope = store.resolve(&conn, &u).unwrap();
        assert_eq!(scope.dims.keys().collect::<Vec<_>>(), vec!["店铺"]);

        // 默认规则按维度生效：默认限制地区，用户自己没有地区规则的就受限；有自己的（哪怕是不限）以自己的为准
        rule(&conn, "*", "地区", "values", &["华北"]);
        store.invalidate();
        assert_eq!(store.resolve(&conn, &user("other", false)).unwrap().dims["地区"], vec!["华北".to_string()]);
        assert!(!store.resolve(&conn, &u).unwrap().dims.contains_key("地区"), "自己的「不限」优先于默认规则");

        // 只删某个维度的规则 / 删掉全部
        delete_rule(&conn, "u", Some("店铺")).unwrap();
        store.invalidate();
        assert!(!store.resolve(&conn, &u).unwrap().restricted, "店铺规则删了，地区是不限，默认规则的地区被自己的覆盖");
        delete_rule(&conn, "u", None).unwrap();
        store.invalidate();
        assert_eq!(store.resolve(&conn, &u).unwrap().dims["地区"], vec!["华北".to_string()], "全删后回到默认规则");
    }

    #[test]
    fn dimension_names_are_validated_and_defaults_apply() {
        let conn = setup();
        let bad = |d: &str| save_rule(&conn, &RuleInput { subject_id: "a".into(), subject_name: String::new(), dimension: Some(d.into()), mode: "all".into(), values: vec![], operator: String::new() });
        assert!(bad(&"x".repeat(21)).is_err());
        assert!(bad("a\nb").is_err());
        assert!(bad("  ").is_ok(), "空白按默认维度");
        let rules = list_rules(&conn).unwrap();
        assert_eq!(rules[0].dimension, "店铺");
        // 老写法：mode=shops 与 shops 字段名
        let old: RuleInput = serde_json::from_str(r#"{"subjectId":"z","mode":"shops","shops":["A"]}"#).unwrap();
        save_rule(&conn, &old).unwrap();
        let z = list_rules(&conn).unwrap().into_iter().find(|r| r.subject_id == "z").unwrap();
        assert_eq!((z.mode.as_str(), z.values.clone(), z.dimension.as_str()), ("values", vec!["A".to_string()], "店铺"));
    }

    #[test]
    fn writes_follow_the_dimension_the_dataset_is_bound_to() {
        let conn = setup();
        save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("店铺".into()), scope_dimension: Some("部门".into()), open: false }).unwrap();
        let in_dept = Arc::new(Scope::dimension("部门", vec!["销售".into()]));
        let in_shop = Arc::new(Scope::shops(vec!["A".into()]));
        assert_eq!(run_with(Some(in_dept), || write_limit(&conn, "ds_orders")).unwrap(), Some(("店铺".to_string(), vec!["销售".to_string()])));
        assert_eq!(run_with(Some(in_shop), || write_limit(&conn, "ds_orders")).unwrap(), None, "这个数据集绑定的维度上用户不受限");
        assert_eq!(write_limit(&conn, "ds_orders").unwrap(), None, "不受限的请求");
    }

    #[test]
    fn legacy_rules_are_migrated_once_and_old_table_is_emptied() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute("INSERT INTO ledger_data_scope (subject_id, subject_name, mode, shops, updated_by) VALUES ('u1', 'U', 'shops', '[\"A\",\"B\"]', 'root'), ('*', '默认', 'all', '[]', 'root')", []).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        let rules = list_rules(&conn).unwrap();
        let u1 = rules.iter().find(|r| r.subject_id == "u1").unwrap();
        assert_eq!((u1.dimension.as_str(), u1.mode.as_str(), u1.values.clone()), ("店铺", "values", vec!["A".to_string(), "B".to_string()]));
        assert_eq!(rules.iter().find(|r| r.subject_id == "*").unwrap().mode, "all");
        let old: i64 = conn.query_row("SELECT count(*) FROM ledger_data_scope", [], |r| r.get(0)).unwrap();
        assert_eq!(old, 0, "旧表清空，重启后不会把删掉的规则又迁移回来");
        // 再次升级（重启）不重复、不复活已删除的规则
        delete_rule(&conn, "u1", None).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        assert!(list_rules(&conn).unwrap().iter().all(|r| r.subject_id != "u1"));
    }
}
