//! 数据范围（行级权限）：按店铺限制用户能看到的数据
//!
//! - **规则**：管理员给用户（或「默认规则」`*`，用于没有单独规则的用户）指定可见的店铺；没有任何规则的用户、管理员不受限制，
//!   所以开启后不会把所有人锁在外面
//! - **数据集绑定**：每个数据集由管理员指定「按哪一列过滤」或「不限范围（所有人可见）」；内置的资金流水固定按店铺列；
//!   关联视图没有单独设置时沿用左表的过滤列；**没有绑定的数据集对受限用户不可见**（默认安全）
//! - **怎么生效**：受限用户的请求在「加载数据集定义」这个统一出口把数据来源换成带过滤条件的子查询，
//!   透视、公式下推、SQL、AI、导出、游标等所有走数据集定义的读取自动受限；游标缓存按范围区分
//! - **绕过过滤的写操作**（导入、建视图、定时推送、保存 SQL…）对受限用户一律关闭：`auth.rs` 的 `RESTRICTED_OK` 是白名单，未登记的接口默认关闭
//! - 不覆盖：已经写进工作簿单元格的数值（共享工作簿由所有者负责）；MCP 令牌与定时推送按服务账号执行，不受创建人范围限制

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Extension, Path, State};
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::auth::CurrentUser;
use crate::dataset::{DatasetDef, column_sql};
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::AppState;
use crate::schema::literal;

/// 没有单独规则的用户适用的默认规则
pub const DEFAULT_SUBJECT: &str = "*";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    /// 是否受限
    pub restricted: bool,
    /// 受限时可见的店铺
    pub shops: Vec<String>,
}

impl Scope {
    pub fn all() -> Self {
        Self { restricted: false, shops: Vec::new() }
    }

    pub fn shops(mut shops: Vec<String>) -> Self {
        shops.retain(|s| !s.trim().is_empty());
        shops.sort();
        shops.dedup();
        Self { restricted: true, shops }
    }

    /// 这个店铺在范围内吗
    pub fn allows(&self, shop: &str) -> bool {
        !self.restricted || self.shops.iter().any(|s| s == shop)
    }

    /// 范围的签名：游标缓存按它区分（不受限为空）
    pub fn sig(&self) -> String {
        if self.restricted { format!("r:{}", self.shops.join("\u{1f}")) } else { String::new() }
    }

    /// 给用户看的描述
    pub fn describe(&self) -> String {
        if self.shops.is_empty() { "没有任何店铺".into() } else { self.shops.join("、") }
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

// ── 规则 ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleRow {
    pub subject_id: String,
    pub subject_name: String,
    /// all 不限 / shops 指定店铺
    pub mode: String,
    pub shops: Vec<String>,
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Default)]
pub struct ScopeStore {
    cache: Mutex<Option<Arc<HashMap<String, RuleRow>>>>,
}

impl ScopeStore {
    pub fn invalidate(&self) {
        if let Ok(mut c) = self.cache.lock() {
            *c = None;
        }
    }

    fn rules(&self, conn: &Connection) -> AppResult<Arc<HashMap<String, RuleRow>>> {
        if let Some(r) = self.cache.lock().ok().and_then(|c| c.clone()) {
            return Ok(r);
        }
        let map: HashMap<String, RuleRow> = list_rules(conn)?.into_iter().map(|r| (r.subject_id.clone(), r)).collect();
        let arc = Arc::new(map);
        if let Ok(mut c) = self.cache.lock() {
            *c = Some(arc.clone());
        }
        Ok(arc)
    }

    /// 用户的数据范围：管理员 / 未开启登录校验不受限；否则取用户自己的规则，没有就取默认规则，都没有则不受限
    pub fn resolve(&self, conn: &Connection, user: &CurrentUser) -> AppResult<Scope> {
        if !user.enforced || user.admin {
            return Ok(Scope::all());
        }
        let rules = self.rules(conn)?;
        Ok(rules.get(&user.id).or_else(|| rules.get(DEFAULT_SUBJECT)).map_or_else(Scope::all, rule_scope))
    }
}

fn rule_scope(r: &RuleRow) -> Scope {
    if r.mode == "shops" { Scope::shops(r.shops.clone()) } else { Scope::all() }
}

fn list_rules(conn: &Connection) -> AppResult<Vec<RuleRow>> {
    let mut stmt = conn.prepare(
        "SELECT subject_id, COALESCE(subject_name, ''), mode, COALESCE(shops, '[]'), updated_by, strftime(updated_at, '%Y-%m-%d %H:%M:%S') FROM ledger_data_scope ORDER BY subject_id = '*' DESC, subject_name",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let shops: String = r.get(3)?;
            Ok(RuleRow { subject_id: r.get(0)?, subject_name: r.get(1)?, mode: r.get(2)?, shops: serde_json::from_str(&shops).unwrap_or_default(), updated_by: r.get(4)?, updated_at: r.get(5)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ── 数据集绑定 ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Binding {
    /// 所有人可见
    Open,
    /// 按这一列（列的 key）过滤
    Column(String),
    /// 没有绑定：受限用户看不到
    Hidden,
}

fn stored_bindings(conn: &Connection) -> AppResult<HashMap<String, (Option<String>, bool)>> {
    let mut stmt = conn.prepare("SELECT id, scope_column, COALESCE(scope_open, false) FROM ledger_dataset")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, Option<String>>(1)?, r.get::<_, bool>(2)?))))?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// 数据集的有效绑定（defs 需按依赖顺序：被引用的在前）
fn effective(defs: &[DatasetDef], stored: &HashMap<String, (Option<String>, bool)>) -> HashMap<String, Binding> {
    let mut out: HashMap<String, Binding> = HashMap::new();
    for d in defs {
        // 汇总数据集沿用源数据集的范围（数据来源在 apply 里按受限后的源重建）
        if let Some(src) = d.summary.as_ref().and_then(|s| out.get(&s.source)).cloned() {
            out.insert(d.id.clone(), src);
            continue;
        }
        let (col, open) = if d.builtin { (Some("shop".to_string()), false) } else { stored.get(&d.id).cloned().unwrap_or((None, false)) };
        let b = if let Some(c) = col.filter(|c| d.columns.iter().any(|x| &x.key == c)) {
            Binding::Column(c)
        } else if open {
            Binding::Open
        } else if let Some(Binding::Column(c)) = d.view.as_ref().and_then(|v| out.get(&v.left)).cloned().filter(|b| matches!(b, Binding::Column(_))) {
            // 关联视图没有单独设置：沿用左表的过滤列（视图里保留了左表的列）
            if d.columns.iter().any(|x| x.key == c) { Binding::Column(c) } else { Binding::Hidden }
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
    let list = scope.shops.iter().map(|s| literal(s)).collect::<Vec<_>>().join(", ");
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
            Some(Binding::Column(key)) => {
                let Some(col) = d.columns.iter().find(|c| &c.key == key) else { continue };
                let pred = if scope.shops.is_empty() { "false".to_string() } else { format!("CAST(({}) AS VARCHAR) IN ({list})", column_sql(col)) };
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

/// 填报时的范围限制：None = 不受限；Some((范围列的 key, 可以填的店铺))。
/// 受限用户只能填报「按某一列划分范围」的数据集，而且只能填自己范围内的行（所有人可见的共享数据集、没有绑定的都不行）
pub fn write_limit(conn: &Connection, dataset_id: &str) -> AppResult<Option<(String, Vec<String>)>> {
    let Some(s) = ambient() else { return Ok(None) };
    let (defs, _) = crate::dataset::load(conn)?;
    let stored = stored_bindings(conn)?;
    match effective(&defs, &stored).get(dataset_id) {
        Some(Binding::Column(c)) => Ok(Some((c.clone(), s.shops.clone()))),
        _ => Err(AppError::Forbidden(format!("你的数据范围受限，只能填报按店铺划分范围的数据集。{}", restriction_note(&s).unwrap_or_default()))),
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
    /// 设置为所有人可见
    pub open: bool,
    /// 实际生效：column 按列过滤 / open 所有人可见 / inherit 沿用左表 / hidden 受限用户看不到
    pub effective: String,
    pub effective_column: Option<String>,
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
    /// 店铺档案里的店铺名（选店铺时的候选）
    pub shops: Vec<String>,
}

pub fn overview(conn: &Connection) -> AppResult<ScopeOverview> {
    let defs = crate::dataset::datasets(conn)?;
    let stored = stored_bindings(conn)?;
    let eff = effective(&defs, &stored);
    let datasets = defs
        .iter()
        .map(|d| {
            let (col, open) = if d.builtin { (Some("shop".to_string()), false) } else { stored.get(&d.id).cloned().unwrap_or((None, false)) };
            let (effective, effective_column) = match eff.get(&d.id) {
                Some(Binding::Open) => ("open", None),
                Some(Binding::Column(c)) if col.as_deref() == Some(c) => ("column", Some(c.clone())),
                Some(Binding::Column(c)) => ("inherit", Some(c.clone())),
                _ => ("hidden", None),
            };
            DatasetBinding {
                id: d.id.clone(),
                name: d.name.clone(),
                kind: d.kind.clone(),
                scope_column: col,
                open,
                effective: effective.into(),
                effective_column,
                columns: d.columns.iter().map(|c| ColumnOption { key: c.key.clone(), title: c.title.clone() }).collect(),
            }
        })
        .collect();
    let mut stmt = conn.prepare("SELECT name FROM ledger_shop ORDER BY name")?;
    let shops = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(ScopeOverview { rules: list_rules(conn)?, datasets, shops })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleInput {
    pub subject_id: String,
    #[serde(default)]
    pub subject_name: String,
    /// all / shops
    pub mode: String,
    #[serde(default)]
    pub shops: Vec<String>,
    #[serde(default)]
    pub operator: String,
}

pub fn save_rule(conn: &Connection, input: &RuleInput) -> AppResult<()> {
    let id = input.subject_id.trim();
    if id.is_empty() {
        return Err(AppError::bad("请选择用户"));
    }
    if !matches!(input.mode.as_str(), "all" | "shops") {
        return Err(AppError::bad("范围应为 all（不限）或 shops（指定店铺）"));
    }
    let shops = Scope::shops(input.shops.clone()).shops;
    let name = if id == DEFAULT_SUBJECT {
        "默认规则（没有单独规则的用户）".to_string()
    } else {
        crate::approval::user_name(conn, id)?.unwrap_or_else(|| input.subject_name.trim().to_string())
    };
    conn.execute(
        "INSERT INTO ledger_data_scope (subject_id, subject_name, mode, shops, updated_by, updated_at) VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))
         ON CONFLICT (subject_id) DO UPDATE SET subject_name = excluded.subject_name, mode = excluded.mode, shops = excluded.shops, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![id, name, input.mode, serde_json::to_string(&shops)?, input.operator, now_local()],
    )?;
    Ok(())
}

pub fn delete_rule(conn: &Connection, subject_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM ledger_data_scope WHERE subject_id = ?", params![subject_id])?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingInput {
    /// 按这一列过滤（列 key）；为空表示不按列
    #[serde(default)]
    pub scope_column: Option<String>,
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
    if let Some(c) = col {
        if input.open {
            return Err(AppError::bad("「按列过滤」与「所有人可见」只能选一种"));
        }
        if !def.columns.iter().any(|x| x.key == c) {
            return Err(AppError::bad("数据集里没有这一列"));
        }
    }
    let changed = conn.execute("UPDATE ledger_dataset SET scope_column = ?, scope_open = ? WHERE id = ?", params![col, input.open, dataset_id])?;
    if changed == 0 {
        return Err(AppError::bad("数据集不存在"));
    }
    Ok(())
}

/// 受限用户的说明（界面提示用）；不受限返回 None
pub fn restriction_note(scope: &Scope) -> Option<String> {
    scope.restricted.then(|| format!("你的数据范围被限制在店铺：{}", scope.describe()))
}

async fn run<T: Send + 'static>(db: &AppState, f: impl FnOnce(&Connection) -> AppResult<T> + Send + 'static) -> AppResult<T> {
    crate::auth::run(db, f).await
}

/// GET /api/ledger/data-scope  规则、数据集绑定、店铺候选
pub async fn get_overview(State(db): State<AppState>) -> AppResult<Json<Envelope<ScopeOverview>>> {
    Ok(ok(run(&db, overview).await?))
}

/// PUT /api/ledger/data-scope/rules  { subjectId, subjectName?, mode: all | shops, shops }  subjectId 为 * 是默认规则
pub async fn put_rule(State(db): State<AppState>, Extension(_): Extension<CurrentUser>, Json(input): Json<RuleInput>) -> AppResult<Json<Envelope<bool>>> {
    run(&db, move |c| save_rule(c, &input)).await?;
    db.scope.invalidate();
    Ok(ok(true))
}

/// DELETE /api/ledger/data-scope/rules/{id}  删除规则（该用户回到默认规则）
pub async fn remove_rule(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    run(&db, move |c| delete_rule(c, &id)).await?;
    db.scope.invalidate();
    Ok(ok(true))
}

/// PUT /api/ledger/data-scope/datasets/{id}  { scopeColumn?, open }
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
        assert_eq!(scope.shops, vec!["A", "B"]);
        // 没有绑定：受限用户一个数据集都看不到（流水固定按店铺列，在）
        let names = |defs: &[DatasetDef]| defs.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
        let r = run_with(Some(Arc::new(scope.clone())), || datasets(&conn)).unwrap();
        assert_eq!(names(&r), vec!["资金流水"]);
        // 不受限：全部可见
        assert_eq!(datasets(&conn).unwrap().len(), 3);

        // 订单按店铺列过滤，成本所有人可见
        save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("店铺".into()), open: false }).unwrap();
        save_binding(&conn, "ds_cost", &BindingInput { scope_column: None, open: true }).unwrap();
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
        assert!(save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("不存在".into()), open: false }).unwrap_err().message().contains("没有这一列"));
        assert!(save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("店铺".into()), open: true }).unwrap_err().message().contains("只能选一种"));
        assert!(save_binding(&conn, "ledger_detail", &BindingInput { scope_column: None, open: true }).is_err());
        assert!(save_binding(&conn, "nope", &BindingInput { scope_column: None, open: true }).is_err());
        let ov = overview(&conn).unwrap();
        let find = |n: &str| ov.datasets.iter().find(|d| d.name == n).unwrap();
        assert_eq!(find("资金流水").effective, "column");
        assert_eq!(find("订单").effective, "hidden");
        save_binding(&conn, "ds_orders", &BindingInput { scope_column: Some("店铺".into()), open: false }).unwrap();
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
        save_rule(&conn, &RuleInput { subject_id: "a".into(), subject_name: String::new(), mode: "shops".into(), shops: vec!["B".into(), "A".into()], operator: "x".into() }).unwrap();
        store.invalidate();
        let sa = store.resolve(&conn, &a).unwrap();
        assert_eq!((sa.restricted, sa.shops.clone()), (true, vec!["A".to_string(), "B".to_string()]));
        assert!(sa.allows("A") && !sa.allows("C"));
        assert!(!store.resolve(&conn, &b).unwrap().restricted);
        // 默认规则：没有单独规则的人都受限；有单独规则的以自己的为准（可以是「不限」）
        save_rule(&conn, &RuleInput { subject_id: "*".into(), subject_name: String::new(), mode: "shops".into(), shops: vec!["A".into()], operator: "x".into() }).unwrap();
        save_rule(&conn, &RuleInput { subject_id: "c".into(), subject_name: "小陈".into(), mode: "all".into(), shops: vec![], operator: "x".into() }).unwrap();
        store.invalidate();
        assert_eq!(store.resolve(&conn, &b).unwrap().shops, vec!["A".to_string()]);
        assert!(!store.resolve(&conn, &user("c", false)).unwrap().restricted);
        // 管理员与未开启登录校验不受任何规则影响
        assert!(!store.resolve(&conn, &root).unwrap().restricted);
        assert!(!store.resolve(&conn, &CurrentUser::open()).unwrap().restricted);
        // 删除后回到默认规则
        delete_rule(&conn, "a").unwrap();
        store.invalidate();
        assert_eq!(store.resolve(&conn, &a).unwrap().shops, vec!["A".to_string()]);
        // 校验
        assert!(save_rule(&conn, &RuleInput { subject_id: " ".into(), subject_name: String::new(), mode: "all".into(), shops: vec![], operator: String::new() }).is_err());
        assert!(save_rule(&conn, &RuleInput { subject_id: "a".into(), subject_name: String::new(), mode: "x".into(), shops: vec![], operator: String::new() }).is_err());
        assert_eq!(list_rules(&conn).unwrap()[0].subject_id, "*");
    }

    /// 数据范围要能从请求任务传到阻塞线程（数据库操作）和 AI 的后台任务：AI 助手的查询靠这条链路受限
    #[tokio::test]
    async fn scope_reaches_blocking_threads_and_background_tasks() {
        let shops = |s: Option<Arc<Scope>>| run_with(s, || ambient().map(|a| a.shops.clone()));
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
}
