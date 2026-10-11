//! 数据集引擎：大数据表（千万行）的服务端部分
//!
//! - 数据集：登记表名与列元数据（列名、类型、宽度），前端据此渲染表格
//! - 查询游标：一次筛选 + 排序物化成「位置表」cursors.c_xxx(rid[, c])（行的位置 = 位置表自己的 rowid）（cursors 是单独挂载的临时库），之后按位置取任意一段只需几毫秒
//!   （千万行实测：每次重新排序取深页 2 s；物化一次 0.5 s，之后 2~10 ms）
//! - 游标按「数据版本 + 查询条件」缓存，数据有写入（导入、删除、改名）时整体失效
//!
//! 数据集当前为内置登记（资金流水）；通用导入（任意 CSV / XLSX 建数据集）在第二阶段接入

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use duckdb::types::Value;
use duckdb::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::detail::{RuleTerm, contribution_sql, to_json};
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::schema::{DETAIL_COLUMNS, ID_COLUMNS, ident, is_numeric};

/// 同时保留的游标数（每个约占 数据行数 × 16 字节，千万行约 160 MB）
const MAX_CURSORS: usize = 24;
const MAX_WINDOW: i64 = 2000;
const MAX_DISTINCT: i64 = 500;
/// 规则贡献列在结果中的列名
pub const CONTRIBUTION_KEY: &str = "__contribution";

// ── 数据集登记 ────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ColumnDef {
    /// 列名（SQL 列名，也是筛选 / 排序时引用的键）
    pub key: String,
    pub title: String,
    /// text / id / money / number / datetime
    pub kind: &'static str,
    pub width: u32,
    /// 计算列的 SQL 表达式（如资金净额 = 入账为正、出账为负）；None 为表中的实际列
    #[serde(skip)]
    pub expr: Option<String>,
    /// 是否为计算列（前端可区分显示）
    pub computed: bool,
    /// 用户计算列的公式（如 =[实付金额]-[成本价]）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// 关联视图继承自左侧数据集的计算列：定义它的数据集名称（只能在那个数据集上修改 / 删除）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// 列在 SQL 中的表达式：计算列返回表达式，实际列返回带引号的列名
pub fn column_sql(col: &ColumnDef) -> String {
    col.expr.clone().unwrap_or_else(|| ident(&col.key))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DatasetDef {
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub table: String,
    pub columns: Vec<ColumnDef>,
    /// 是否支持关键字搜索（表中有 search_text 列）
    pub searchable: bool,
    /// 是否支持「汇总项规则」贡献列（与汇总表下钻同源）
    pub supports_rule: bool,
    /// 内置数据集（由专门的导入流程维护，不能在数据集管理中改名 / 删除）
    pub builtin: bool,
    /// 主键列（导入时去重依据）
    pub primary_key: Vec<String>,
    /// 行标识表达式：实体表为 rowid，关联视图为左表行号 __rid（游标、取数、导出用它定位行）
    #[serde(skip)]
    pub rid: String,
    /// builtin / table / view
    pub kind: String,
    /// 数据范围（行级权限）的签名：不为空表示 table 已被换成带过滤条件的子查询；游标缓存按它区分
    #[serde(skip)]
    pub scope_sig: String,
    /// 关联视图的定义（前端展示「由哪两个数据集关联而来」）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<crate::views::ViewDef>,
}

/// 用户导入的数据集在登记表中保存的列定义
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StoredColumn {
    pub key: String,
    pub title: String,
    pub kind: String,
    pub width: u32,
    /// 文件中的原始列名（映射模板：同一来源的文件追加时按它对齐）
    #[serde(default)]
    pub source: String,
}

fn kind_static(k: &str) -> &'static str {
    match k {
        "id" => "id",
        "number" => "number",
        "money" => "money",
        "datetime" => "datetime",
        _ => "text",
    }
}

/// 用户导入的数据集：(id, 名称, 列)
pub fn stored(conn: &Connection) -> AppResult<Vec<(String, String, Vec<StoredColumn>)>> {
    let mut stmt = conn.prepare("SELECT id, name, columns FROM ledger_dataset WHERE (kind IS NULL OR kind = 'table') AND deleted_at IS NULL ORDER BY created_at")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, name, cols)| Ok((id, name, serde_json::from_str(&cols).map_err(|e| AppError::bad(format!("数据集列定义损坏：{e}")))?)))
        .collect()
}

pub fn register(conn: &Connection, id: &str, name: &str, columns: &[StoredColumn], primary_key: &[String]) -> AppResult<()> {
    let now = crate::db::now_local();
    conn.execute(
        "INSERT INTO ledger_dataset (id, name, columns, primary_key, created_at, updated_at) VALUES (?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        duckdb::params![id, name, serde_json::to_string(columns)?, serde_json::to_string(primary_key)?, now, now],
    )?;
    Ok(())
}

/// 改名（公式里引用的是名称，改名后需同步修改公式）
pub fn rename(conn: &Connection, id: &str, name: &str) -> AppResult<()> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::bad("数据集名称不能为空，且不超过 40 个字符"));
    }
    if datasets(conn)?.iter().any(|d| d.name == name && d.id != id) {
        return Err(AppError::bad(format!("已存在名为「{name}」的数据集")));
    }
    if conn.execute("UPDATE ledger_dataset SET name = ? WHERE id = ? AND deleted_at IS NULL", duckdb::params![name, id])? == 0 {
        return Err(AppError::bad("数据集不存在或为内置数据集"));
    }
    Ok(())
}

/// 删除数据集：移到回收站（数据保留，可恢复，见 dataset_trash）；被关联视图引用时不能删除
pub fn delete(conn: &Connection, id: &str, operator: &str) -> AppResult<i64> {
    let def = datasets(conn)?.into_iter().find(|d| d.id == id && !d.builtin).ok_or_else(|| AppError::bad("数据集不存在或为内置数据集"))?;
    let users = crate::views::dependents(conn, id)?;
    if !users.is_empty() {
        return Err(AppError::bad(format!("关联视图「{}」引用了这个数据集，请先删除这些视图", users.join("」「"))));
    }
    let recons = crate::recon::dependents(conn, id)?;
    if !recons.is_empty() {
        return Err(AppError::bad(format!("对账「{}」引用了这个数据集，请先删除这些对账", recons.join("」「"))));
    }
    let rows: i64 = conn.query_row(&format!("SELECT count(*) FROM {}", def.table), [], |r| r.get(0)).unwrap_or(0);
    conn.execute(
        "UPDATE ledger_dataset SET deleted_at = CAST(? AS TIMESTAMP), deleted_by = ? WHERE id = ? AND deleted_at IS NULL",
        duckdb::params![crate::db::now_local(), operator, def.id],
    )?;
    Ok(rows)
}

fn ledger_detail_def() -> DatasetDef {
    let mut columns = vec![
        ColumnDef { key: "shop".into(), title: "店铺".into(), kind: "text", width: 140, expr: None, computed: false, formula: None, origin: None },
        ColumnDef { key: "period".into(), title: "月份".into(), kind: "text", width: 80, expr: None, computed: false, formula: None, origin: None },
    ];
    for c in DETAIL_COLUMNS {
        let (kind, width) = if is_numeric(c) {
            ("money", 110)
        } else if *c == "动账时间" || *c == "下单时间" {
            ("datetime", 150)
        } else if ID_COLUMNS.contains(c) {
            ("id", 190)
        } else if matches!(*c, "商品名称" | "是否免佣" | "商户主体名称" | "备注") {
            ("text", 220)
        } else {
            ("text", 100)
        };
        columns.push(ColumnDef { key: c.to_string(), title: c.to_string(), kind, width, expr: None, computed: false, formula: None, origin: None });
        if *c == "动账金额" {
            // 资金净额：入账为正、出账为负，公式与筛选可直接对其求和
            columns.push(ColumnDef {
                key: "资金净额".into(),
                title: "资金净额".into(),
                kind: "money",
                width: 110,
                expr: Some(format!(
                    "CASE {dir} WHEN '入账' THEN {amt} WHEN '出账' THEN -{amt} ELSE 0 END",
                    dir = ident("动账方向"),
                    amt = ident("动账金额")
                )),
                computed: true,
                formula: None,
                origin: None,
            });
        }
    }
    DatasetDef {
        id: "ledger_detail".into(),
        name: "资金流水".into(),
        table: "ledger_detail".into(),
        columns,
        searchable: true,
        supports_rule: true,
        builtin: true,
        primary_key: vec!["动帐流水号".into()],
        rid: "rowid".into(),
        kind: "builtin".into(),
        scope_sig: String::new(),
        view: None,
    }
}

/// 数据集定义。当前请求的数据范围受限时（见 scope.rs），数据来源已换成带过滤条件的子查询，没有绑定的数据集不返回
pub fn datasets(conn: &Connection) -> AppResult<Vec<DatasetDef>> {
    crate::scope::restrict(conn, load(conn)?.0)
}

type CalcRow = (String, String, String, Option<String>);

/// 加载全部数据集，同时返回失效的计算列（「数据集」的「列」：原因）
///
/// 顺序：内置 / 导入的表 → 各自的计算列 → 关联视图（依赖先就绪，最多 4 层；每个视图建好后立刻挂上自己的计算列，
/// 这样视图之上的视图、以及右侧带入的计算列都能拿到正确的列类型）
pub fn load(conn: &Connection) -> AppResult<(Vec<DatasetDef>, Vec<String>)> {
    let mut out = vec![ledger_detail_def()];
    let mut problems = Vec::new();
    let calcs: Vec<CalcRow> = {
        let mut stmt = conn.prepare("SELECT dataset_id, name, formula, kind FROM ledger_calc_column ORDER BY created_at, name")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<Vec<_>, _>>()?
    };
    let mut stmt = conn.prepare("SELECT id, name, columns, primary_key, kind, definition FROM ledger_dataset WHERE deleted_at IS NULL ORDER BY created_at")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut views: Vec<(String, String, crate::views::ViewDef)> = Vec::new();
    for (id, name, cols, pk, kind, definition) in rows {
        if kind.as_deref() == Some("recon") {
            out.push(crate::recon::dataset_def(id, name));
            continue;
        }
        if kind.as_deref() == Some("view") {
            match definition.as_deref().and_then(|d| serde_json::from_str(d).ok()) {
                Some(v) => views.push((id, name, v)),
                None => problems.push(format!("关联视图「{name}」的定义损坏")),
            }
            continue;
        }
        let cols: Vec<StoredColumn> = serde_json::from_str(&cols).unwrap_or_default();
        out.push(DatasetDef {
            table: id.clone(),
            id,
            name,
            searchable: cols.iter().any(|c| matches!(c.kind.as_str(), "text" | "id")),
            columns: cols
                .into_iter()
                .map(|c| ColumnDef { key: c.key, title: c.title, kind: kind_static(&c.kind), width: c.width, expr: None, computed: false, formula: None, origin: None })
                .collect(),
            supports_rule: false,
            builtin: false,
            primary_key: serde_json::from_str(&pk).unwrap_or_default(),
            rid: "rowid".into(),
            kind: "table".into(),
            scope_sig: String::new(),
            view: None,
        });
    }
    for def in out.iter_mut() {
        apply_calcs(def, &calcs, &mut problems);
    }
    for _ in 0..4 {
        let mut pending = Vec::new();
        for (id, name, v) in views {
            let left = out.iter().find(|d| d.id == v.left).cloned();
            let right = out.iter().find(|d| d.id == v.right).cloned();
            let (Some(left), Some(right)) = (left, right) else {
                pending.push((id, name, v));
                continue;
            };
            // 左侧的列（含计算列：视图里有同名的列，表达式照样成立）
            let mut columns: Vec<ColumnDef> = left
                .columns
                .iter()
                .cloned()
                .map(|mut c| {
                    if c.formula.is_some() && c.origin.is_none() {
                        c.origin = Some(left.name.clone());
                    }
                    c
                })
                .collect();
            for c in &v.columns {
                let rc = right.columns.iter().find(|x| x.title == c.source || x.key == c.source);
                columns.push(ColumnDef {
                    key: c.title.clone(),
                    title: c.title.clone(),
                    kind: rc.map(|x| x.kind).unwrap_or("text"),
                    width: rc.map(|x| x.width).unwrap_or(120),
                    expr: None,
                    computed: false,
                    formula: None,
                    origin: None,
                });
            }
            let mut def = DatasetDef {
                table: id.clone(),
                id,
                name,
                columns,
                searchable: left.searchable,
                supports_rule: false,
                builtin: false,
                primary_key: vec![],
                rid: "__rid".into(),
                kind: "view".into(),
                scope_sig: String::new(),
                view: Some(v),
            };
            apply_calcs(&mut def, &calcs, &mut problems);
            out.push(def);
        }
        views = pending;
        if views.is_empty() {
            break;
        }
    }
    for (_, name, _) in &views {
        problems.push(format!("关联视图「{name}」引用的数据集不存在"));
    }
    Ok((out, problems))
}

/// 计算列：按添加顺序编译（后面的计算列可以引用前面的）
fn apply_calcs(def: &mut DatasetDef, calcs: &[CalcRow], problems: &mut Vec<String>) {
    for (dataset_id, name, formula, kind) in calcs {
        if *dataset_id != def.id {
            continue;
        }
        if def.columns.iter().any(|c| c.title == *name || c.key == *name) {
            problems.push(format!("「{}」的计算列「{name}」与已有列重名", def.name));
            continue;
        }
        match compile_calc(def, formula, kind.as_deref()) {
            Ok(col) => def.columns.push(ColumnDef { key: name.clone(), title: name.clone(), ..col }),
            Err(e) => {
                let msg = e.message();
                tracing::warn!(dataset = %def.name, column = %name, "计算列失效：{msg}");
                problems.push(format!("「{}」的计算列「{name}」：{msg}", def.name));
            }
        }
    }
}

/// 在事务中执行一次结构变更（改计算列、改关联视图），之后重建所有视图并检查：
/// 有新出现的失效计算列 / 视图时整体回滚，并把受影响的列告诉用户
pub fn guarded_change(conn: &Connection, change: impl FnOnce(&Connection) -> AppResult<()>) -> AppResult<()> {
    let (_, before) = load(conn)?;
    conn.execute_batch("BEGIN TRANSACTION")?;
    let r = (|| -> AppResult<()> {
        change(conn)?;
        crate::views::rebuild_all(conn)?;
        let (_, after) = load(conn)?;
        let broken: Vec<&String> = after.iter().filter(|p| !before.contains(p)).collect();
        if !broken.is_empty() {
            return Err(AppError::bad(format!(
                "修改后以下内容会失效，已取消：{}",
                broken.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("；")
            )));
        }
        Ok(())
    })();
    match r {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 由某数据集派生的关联视图（直接或间接，按依赖顺序）
pub fn derived_views<'a>(defs: &'a [DatasetDef], id: &str) -> Vec<&'a DatasetDef> {
    let mut ids = vec![id.to_string()];
    let mut out = Vec::new();
    for d in defs {
        if let Some(v) = &d.view {
            if ids.contains(&v.left) || ids.contains(&v.right) {
                ids.push(d.id.clone());
                out.push(d);
            }
        }
    }
    out
}

struct DefResolver<'a>(&'a DatasetDef);
impl crate::calc::Resolver for DefResolver<'_> {
    fn column(&self, name: &str) -> Option<(String, bool)> {
        self.0.columns.iter().find(|c| c.title == name || c.key == name).map(|c| (column_sql(c), matches!(c.kind, "money" | "number")))
    }
}

/// 编译计算列；kind 为空时自动推断（数值：引用了金额列 → money，否则 number）
pub fn compile_calc(def: &DatasetDef, formula: &str, kind: Option<&str>) -> AppResult<ColumnDef> {
    let compiled = crate::calc::compile(formula, &DefResolver(def))?;
    let (sql, inferred) = match compiled.ty {
        crate::calc::Ty::Num => {
            let money = !compiled.ratio && compiled.columns.iter().any(|n| def.columns.iter().any(|c| (c.title == *n || c.key == *n) && c.kind == "money"));
            (compiled.sql, if money { "money" } else { "number" })
        }
        crate::calc::Ty::Text => (compiled.sql, "text"),
        crate::calc::Ty::Bool => (format!("(CASE WHEN {s} THEN '是' WHEN NOT {s} THEN '否' END)", s = compiled.sql), "text"),
    };
    let kind = match kind {
        Some("money") if inferred != "text" => "money",
        Some("number") if inferred != "text" => "number",
        _ => inferred,
    };
    Ok(ColumnDef {
        key: String::new(),
        title: String::new(),
        kind,
        width: if kind == "text" { 140 } else { 110 },
        expr: Some(sql),
        computed: true,
        formula: Some(formula.trim().to_string()),
        origin: None,
    })
}

/// 按 ID 或名称查找数据集（公式里可以写名称，如 DSUMIFS("资金流水", ...)）
pub fn dataset(conn: &Connection, id: &str) -> AppResult<DatasetDef> {
    datasets(conn)?.into_iter().find(|d| d.id == id || d.name == id).ok_or_else(|| {
        let trashed: Option<String> = conn
            .query_row("SELECT name FROM ledger_dataset WHERE (id = ? OR name = ?) AND deleted_at IS NOT NULL LIMIT 1", duckdb::params![id, id], |r| r.get(0))
            .ok();
        match trashed {
            Some(name) => AppError::bad(format!("数据集「{name}」在回收站，恢复后才能使用")),
            None => AppError::bad(format!("数据集不存在：{id}")),
        }
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetInfo {
    #[serde(flatten)]
    pub def: DatasetDef,
    pub row_count: i64,
    /// 所有者（登录体系启用前创建的、内置的没有所有者，只有管理员能改名 / 删除 / 改结构）
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
}

pub fn list(conn: &Connection) -> AppResult<Vec<DatasetInfo>> {
    let owners: HashMap<String, (Option<String>, Option<String>)> = {
        let mut stmt = conn.prepare("SELECT id, owner_id, owner_name FROM ledger_dataset")?;
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?))))?.collect::<Result<_, _>>()?
    };
    datasets(conn)?
        .into_iter()
        .map(|def| {
            let row_count = conn.query_row(&format!("SELECT count(*) FROM {}", def.table), [], |r| r.get(0))?;
            let (owner_id, owner_name) = owners.get(&def.id).cloned().unwrap_or((None, None));
            Ok(DatasetInfo { def, row_count, owner_id, owner_name })
        })
        .collect()
}

/// 改名 / 删除 / 改结构：只有所有者与管理员（没有所有者的老数据集、内置数据集只有管理员）。返回数据集名称（审计用）
pub fn require_manage(conn: &Connection, id: &str, user: &CurrentUser) -> AppResult<String> {
    if !user.enforced || user.admin {
        return Ok(conn.query_row("SELECT name FROM ledger_dataset WHERE id = ?", [id], |r| r.get(0)).unwrap_or_default());
    }
    let row: Option<(String, Option<String>, Option<String>)> = duckdb::OptionalExt::optional(conn.query_row(
        "SELECT name, owner_id, owner_name FROM ledger_dataset WHERE id = ?",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ))?;
    let Some((name, owner_id, owner_name)) = row else {
        return Err(AppError::Forbidden("内置数据集只有管理员可以修改".into()));
    };
    if owner_id.as_deref().is_some_and(|o| o == user.id) {
        return Ok(name);
    }
    let who = owner_name.filter(|n| !n.is_empty()).map(|n| format!("所有者：{n}")).unwrap_or_else(|| "没有所有者".into());
    Err(AppError::Forbidden(format!("只有所有者或管理员可以修改数据集「{name}」（{who}）")))
}

/// 登录用户创建的数据集归自己（未启用认证时不改动）
pub fn set_owner(conn: &Connection, id: &str, user: &CurrentUser) -> AppResult<()> {
    if user.enforced {
        conn.execute("UPDATE ledger_dataset SET owner_id = ?, owner_name = ? WHERE id = ? AND owner_id IS NULL", duckdb::params![user.id, user.name, id])?;
    }
    Ok(())
}

// ── 查询条件 ──────────────────────────────────────────

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "op")]
pub enum Filter {
    /// 等于其中之一；"" 表示空值
    In { column: String, values: Vec<String> },
    /// 区间（含两端）；数值列用 number，文本 / 日期列用字符串比较
    Between {
        column: String,
        #[serde(default)]
        min: Option<Json>,
        #[serde(default)]
        max: Option<Json>,
    },
    /// 包含（不区分大小写）
    Contains { column: String, value: String },
    /// 以某前缀开头（如下单时间以 2026-08 开头）；"" 表示空值
    Prefix { column: String, value: String },
    /// 以其中某个前缀开头（图表点选了多个月份）；"" 表示空值
    Prefixes { column: String, values: Vec<String> },
}

impl Filter {
    pub fn column(&self) -> &str {
        match self {
            Self::In { column, .. } | Self::Between { column, .. } | Self::Contains { column, .. } | Self::Prefix { column, .. } | Self::Prefixes { column, .. } => column,
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QuerySpec {
    pub filters: Vec<Filter>,
    pub keyword: Option<String>,
    pub sort: Vec<SortSpec>,
    /// 汇总项规则：只保留贡献不为 0 的行，并提供贡献列（可按 __contribution 排序）
    pub rule: Option<Vec<RuleTerm>>,
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn check_column<'a>(def: &'a DatasetDef, key: &str) -> AppResult<&'a ColumnDef> {
    def.columns.iter().find(|c| c.key == key || c.title == key).ok_or_else(|| AppError::bad(format!("列不存在：{key}")))
}

/// 构造 WHERE（exclude：分面统计时去掉该列自身的筛选）
fn where_clause(def: &DatasetDef, spec: &QuerySpec, contribution: Option<&str>, exclude: Option<&str>) -> AppResult<(String, Vec<Value>)> {
    let mut conds: Vec<String> = vec!["TRUE".into()];
    let mut args: Vec<Value> = Vec::new();
    for f in &spec.filters {
        if exclude == Some(f.column()) {
            continue;
        }
        let col = check_column(def, f.column())?;
        let c = format!("({})", column_sql(col));
        match f {
            Filter::In { values, .. } => {
                if values.is_empty() {
                    continue;
                }
                let named: Vec<&String> = values.iter().filter(|v| !v.is_empty()).collect();
                let mut ors = Vec::new();
                if !named.is_empty() {
                    ors.push(format!("CAST({c} AS VARCHAR) IN ({})", vec!["?"; named.len()].join(", ")));
                    args.extend(named.into_iter().map(|v| Value::Text(v.clone())));
                }
                if values.iter().any(|v| v.is_empty()) {
                    ors.push(format!("{c} IS NULL"));
                }
                conds.push(format!("({})", ors.join(" OR ")));
            }
            Filter::Between { min, max, .. } => {
                let numeric = matches!(col.kind, "money" | "number");
                for (bound, op) in [(min, ">="), (max, "<=")] {
                    let Some(v) = bound.as_ref().filter(|v| !v.is_null()) else { continue };
                    if numeric {
                        let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).ok_or_else(|| AppError::bad(format!("「{}」的区间必须是数字", col.title)))?;
                        conds.push(format!("{c} {op} ?"));
                        args.push(Value::Double(n));
                    } else {
                        let s = v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
                        conds.push(format!("CAST({c} AS VARCHAR) {op} ?"));
                        args.push(Value::Text(s));
                    }
                }
            }
            Filter::Contains { value, .. } => {
                if value.trim().is_empty() {
                    continue;
                }
                conds.push(format!("CAST({c} AS VARCHAR) ILIKE ? ESCAPE '\\'"));
                args.push(Value::Text(format!("%{}%", escape_like(value.trim()))));
            }
            Filter::Prefix { value, .. } => {
                if value.is_empty() {
                    conds.push(format!("{c} IS NULL"));
                } else {
                    conds.push(format!("starts_with(CAST({c} AS VARCHAR), ?)"));
                    args.push(Value::Text(value.clone()));
                }
            }
            Filter::Prefixes { values, .. } => {
                if values.is_empty() {
                    continue;
                }
                let mut ors = Vec::new();
                for v in values {
                    if v.is_empty() {
                        ors.push(format!("{c} IS NULL"));
                    } else {
                        ors.push(format!("starts_with(CAST({c} AS VARCHAR), ?)"));
                        args.push(Value::Text(v.clone()));
                    }
                }
                conds.push(format!("({})", ors.join(" OR ")));
            }
        }
    }
    if let Some(kw) = spec.keyword.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if !def.searchable {
            return Err(AppError::bad("该数据集不支持关键字搜索"));
        }
        conds.push("search_text LIKE ? ESCAPE '\\'".into());
        args.push(Value::Text(format!("%{}%", escape_like(&kw.to_lowercase()))));
    }
    if let Some(expr) = contribution {
        conds.push(format!("({expr}) <> 0"));
    }
    Ok((conds.join(" AND "), args))
}

/// 只含筛选与关键字的 WHERE（透视表等聚合查询使用）
pub fn filter_sql(_conn: &Connection, def: &DatasetDef, spec: &QuerySpec) -> AppResult<(String, Vec<Value>)> {
    where_clause(def, spec, None, None)
}

/// 排序（含行号兜底），与游标的行顺序一致
pub fn order_sql(def: &DatasetDef, spec: &QuerySpec) -> AppResult<String> {
    order_clause(def, spec, None)
}

fn contribution_of(def: &DatasetDef, spec: &QuerySpec) -> AppResult<Option<String>> {
    match &spec.rule {
        Some(rule) if !rule.is_empty() => {
            if !def.supports_rule {
                return Err(AppError::bad("该数据集不支持汇总项规则"));
            }
            Ok(Some(contribution_sql(rule)?))
        }
        _ => Ok(None),
    }
}

fn order_clause(def: &DatasetDef, spec: &QuerySpec, contribution: Option<&str>) -> AppResult<String> {
    let mut parts = Vec::new();
    for s in &spec.sort {
        let expr = if s.column == CONTRIBUTION_KEY {
            format!("({})", contribution.ok_or_else(|| AppError::bad("未指定汇总项规则，不能按贡献金额排序"))?)
        } else {
            format!("({})", column_sql(check_column(def, &s.column)?))
        };
        parts.push(format!("{expr} {}", if s.desc { "DESC NULLS LAST" } else { "ASC NULLS LAST" }));
    }
    // 行号兜底：排序结果稳定（同值按入库顺序）
    parts.push(def.rid.clone());
    Ok(parts.join(", "))
}

/// 预览：符合条件的行数与前 limit 行（只取 columns 这几列，按 spec 排序）。不建游标，
/// 给 AI 工具生成"视图建议"时核对用——命中多少行、前几行长什么样
pub fn preview_rows(conn: &Connection, def: &DatasetDef, spec: &QuerySpec, columns: &[&ColumnDef], limit: i64) -> AppResult<(i64, Vec<Vec<Json>>)> {
    let (where_sql, args) = where_clause(def, spec, None, None)?;
    let matched: i64 = conn.query_row(&format!("SELECT count(*) FROM {} WHERE {where_sql}", def.table), params_from_iter(args.iter()), |r| r.get(0))?;
    if matched == 0 || columns.is_empty() {
        return Ok((matched, vec![]));
    }
    let cols: Vec<String> = columns.iter().map(|c| format!("({})", column_sql(c))).collect();
    let sql = format!("SELECT {} FROM {} WHERE {where_sql} ORDER BY {} LIMIT {}", cols.join(", "), def.table, order_clause(def, spec, None)?, limit.clamp(1, 50));
    let mut stmt = conn.prepare(&sql)?;
    let n = cols.len();
    let rows = stmt
        .query_map(params_from_iter(args.iter()), |r| (0..n).map(|i| Ok(to_json(r.get_ref(i)?))).collect::<duckdb::Result<Vec<_>>>())?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((matched, rows))
}

/// 数据集的总行数
pub fn total_rows(conn: &Connection, def: &DatasetDef) -> AppResult<i64> {
    Ok(conn.query_row(&format!("SELECT count(*) FROM {}", def.table), [], |r| r.get(0))?)
}

// ── 游标缓存 ──────────────────────────────────────────

struct CursorEntry {
    /// 位置表；None 为「直通游标」：无筛选 / 搜索 / 排序 / 规则、行号连续（0..N-1）时，位置就是 rowid，不需要物化
    table: Option<String>,
    total: i64,
    contribution: Option<f64>,
    has_contribution: bool,
    last_used: Instant,
}

/// 游标注册表：数据版本号 + 查询条件 → 位置表
pub struct CursorCache {
    version: AtomicU64,
    entries: Mutex<HashMap<String, CursorEntry>>,
    seq: AtomicU64,
    /// 串行化建表，避免同一查询被并发重复物化
    pub build_lock: tokio::sync::Mutex<()>,
}

impl CursorCache {
    /// 位置表所在的 cursors 库由 Db::open 在启动时重新创建
    pub fn new() -> Self {
        Self { version: AtomicU64::new(1), entries: Mutex::new(HashMap::new()), seq: AtomicU64::new(0), build_lock: tokio::sync::Mutex::new(()) }
    }

    /// 数据有写入：作废全部游标（位置表在下次建游标时清理）
    pub fn invalidate(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorInfo {
    pub cursor_id: String,
    pub total: i64,
    /// 规则贡献合计（有 rule 时）
    pub contribution: Option<f64>,
    pub has_contribution: bool,
    /// 本次是否命中缓存（便于观察性能）
    pub cached: bool,
    /// 直通游标：位置就是行号，没有物化位置表（整表按入库顺序浏览时）
    pub direct: bool,
    pub elapsed_ms: i64,
}

/// 建游标（或命中缓存）
pub fn open_cursor(conn: &Connection, cache: &CursorCache, dataset_id: &str, spec: &QuerySpec) -> AppResult<CursorInfo> {
    let started = Instant::now();
    let def = dataset(conn, dataset_id)?;
    let version = cache.version();
    // 数据范围不同的用户不能共用游标（位置表是按各自能看到的行建的）
    let key = format!("{version}|{dataset_id}|{}|{}", def.scope_sig, serde_json::to_string(spec)?);
    let cursor_id = format!("{:x}", md5_like(&key));

    {
        let mut entries = cache.entries.lock().map_err(|_| anyhow::anyhow!("游标缓存锁已损坏"))?;
        // 清理旧版本的游标
        let stale: Vec<String> = entries.keys().filter(|k| !k.starts_with(&format!("{version}|"))).cloned().collect();
        for k in stale {
            if let Some(e) = entries.remove(&k) {
                drop_cursor_table(conn, &e);
            }
        }
        if let Some(e) = entries.get_mut(&key) {
            e.last_used = Instant::now();
            return Ok(CursorInfo {
                cursor_id,
                total: e.total,
                contribution: e.contribution,
                has_contribution: e.has_contribution,
                cached: true,
                direct: e.table.is_none(),
                elapsed_ms: started.elapsed().as_millis() as i64,
            });
        }
    }

    let contribution = contribution_of(&def, spec)?;
    // 直通游标：整张表按行号顺序展示（打开大数据表的默认方式）。位置 = rowid，不建位置表：
    // 1 亿行时省掉 1.6 GB 内存与 5 ~ 6 秒；行号有空洞（删过行）或有范围限制时退回物化
    if let Some(total) = plain_total(conn, &def, spec, contribution.as_deref())? {
        let mut entries = cache.entries.lock().map_err(|_| anyhow::anyhow!("游标缓存锁已损坏"))?;
        entries.insert(key, CursorEntry { table: None, total, contribution: None, has_contribution: false, last_used: Instant::now() });
        return Ok(CursorInfo { cursor_id, total, contribution: None, has_contribution: false, cached: false, direct: true, elapsed_ms: started.elapsed().as_millis() as i64 });
    }
    let (where_sql, args) = where_clause(&def, spec, contribution.as_deref(), None)?;
    // 位置表只存行号（和规则的贡献金额），行的位置就是位置表自己的 rowid：INSERT ... SELECT 保持查询结果的顺序，
    // 所以不用再算 row_number() 窗口（它要把整个排序结果再缓冲一遍，1 亿行时多占 1 GB+ 内存和一轮扫描）。
    // 没有排序条件时，表扫描本来就按 rowid 顺序产出，连 ORDER BY 也省掉
    let order_by = if spec.sort.is_empty() && def.rid == "rowid" { String::new() } else { format!(" ORDER BY {}", order_clause(&def, spec, contribution.as_deref())?) };
    let table = format!("c_{}_{}", cache.seq.fetch_add(1, Ordering::SeqCst), cursor_id.get(0..8).unwrap_or("x"));
    let extra = contribution.as_deref().map(|e| format!(", ({e})::DOUBLE AS c")).unwrap_or_default();
    conn.execute(
        &format!(
            "CREATE TABLE cursors.{table} AS
             SELECT {rid} AS rid{extra}
             FROM {src} WHERE {where_sql}{order_by}",
            src = def.table,
            rid = def.rid
        ),
        params_from_iter(args.iter()),
    )?;
    let (total, sum): (i64, Option<f64>) = conn.query_row(
        &format!("SELECT count(*), {} FROM cursors.{table}", if contribution.is_some() { "sum(c)" } else { "NULL::DOUBLE" }),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let contribution_total = sum.map(|v| (v * 100.0).round() / 100.0 + 0.0).or(if contribution.is_some() { Some(0.0) } else { None });

    let mut entries = cache.entries.lock().map_err(|_| anyhow::anyhow!("游标缓存锁已损坏"))?;
    while entries.len() >= MAX_CURSORS {
        let Some(oldest) = entries.iter().min_by_key(|(_, e)| e.last_used).map(|(k, _)| k.clone()) else { break };
        if let Some(e) = entries.remove(&oldest) {
            drop_cursor_table(conn, &e);
        }
    }
    // 同一查询被并发建了两次时，换下来的旧位置表不再有人引用，及时清掉
    if let Some(old) = entries.insert(
        key,
        CursorEntry { table: Some(table), total, contribution: contribution_total, has_contribution: contribution.is_some(), last_used: Instant::now() },
    ) {
        drop_cursor_table(conn, &old);
    }
    Ok(CursorInfo {
        cursor_id,
        total,
        contribution: contribution_total,
        has_contribution: contribution.is_some(),
        cached: false,
        direct: false,
        elapsed_ms: started.elapsed().as_millis() as i64,
    })
}

fn drop_cursor_table(conn: &Connection, e: &CursorEntry) {
    if let Some(t) = &e.table {
        let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS cursors.{t}"));
    }
}

/// 能用直通游标时返回总行数：无筛选 / 搜索 / 排序 / 规则、没有数据范围限制、按 rowid 取行，并且 rowid 恰好是 0..N-1
/// （min / max / count 都是秒内的元数据级查询；删过行留下空洞时返回 None，由调用方物化）
fn plain_total(conn: &Connection, def: &DatasetDef, spec: &QuerySpec, contribution: Option<&str>) -> AppResult<Option<i64>> {
    let no_keyword = spec.keyword.as_deref().is_none_or(|k| k.trim().is_empty());
    if !(spec.filters.is_empty() && no_keyword && spec.sort.is_empty() && contribution.is_none() && def.scope_sig.is_empty() && def.rid == "rowid") {
        return Ok(None);
    }
    contiguous_rows(conn, &def.table)
}

/// 表的 rowid 恰好是 0..N-1 时返回 N（已删除但还没被整理掉的行会留下空洞，此时返回 None）
fn contiguous_rows(conn: &Connection, table: &str) -> AppResult<Option<i64>> {
    let (n, min, max): (i64, Option<i64>, Option<i64>) = conn.query_row(&format!("SELECT count(*), min(rowid), max(rowid) FROM {table}"), [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok((n == 0 || (min == Some(0) && max == Some(n - 1))).then_some(n))
}

/// 游标的取数来源：位置表（JOIN 回数据表）或直通（直接按 rowid 取）
struct CursorRef {
    table: Option<String>,
    has_c: bool,
    total: i64,
}

impl CursorRef {
    /// FROM 子句：数据表别名 t；位置表时别名 c 带 rid / c（位置是 c.rowid）
    fn from_sql(&self, def: &DatasetDef) -> String {
        match &self.table {
            Some(t) => format!("cursors.{t} c JOIN {src} t ON t.{rid} = c.rid", src = def.table, rid = def.rid),
            None => format!("{} t", def.table),
        }
    }

    /// 行位置表达式
    fn pos(&self, def: &DatasetDef) -> String {
        match &self.table {
            Some(_) => "c.rowid".into(),
            None => format!("t.{}", def.rid),
        }
    }
}

/// FNV-1a 64 位：游标 ID 只需在进程内唯一
fn md5_like(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn find_cursor(cache: &CursorCache, cursor_id: &str) -> AppResult<Option<CursorRef>> {
    let mut entries = cache.entries.lock().map_err(|_| anyhow::anyhow!("游标缓存锁已损坏"))?;
    let version = format!("{}|", cache.version());
    for (k, e) in entries.iter_mut() {
        if k.starts_with(&version) && format!("{:x}", md5_like(k)) == cursor_id {
            e.last_used = Instant::now();
            return Ok(Some(CursorRef { table: e.table.clone(), has_c: e.has_contribution, total: e.total }));
        }
    }
    Ok(None)
}

// ── 取数 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsWindow {
    /// 游标已失效（数据变更或被淘汰）：前端需重新建游标
    pub expired: bool,
    pub offset: i64,
    /// 每行按数据集列顺序；有规则时第一列为贡献金额
    pub rows: Vec<Vec<Json>>,
}

/// columns：只取这些列（工作表里的数据表只显示部分列）；有规则时贡献列仍在最前
pub fn rows(conn: &Connection, cache: &CursorCache, dataset_id: &str, cursor_id: &str, offset: i64, limit: i64, columns: Option<&[String]>) -> AppResult<RowsWindow> {
    let def = dataset(conn, dataset_id)?;
    let picked: Vec<&ColumnDef> = match columns {
        Some(names) => names.iter().map(|n| check_column(&def, n)).collect::<AppResult<_>>()?,
        None => def.columns.iter().collect(),
    };
    let Some(cur) = find_cursor(cache, cursor_id)? else {
        return Ok(RowsWindow { expired: true, offset, rows: vec![] });
    };
    let limit = limit.clamp(1, MAX_WINDOW);
    let offset = offset.max(0);
    let mut cols: Vec<String> = picked.iter().map(|c| format!("({})", column_sql(c))).collect();
    if cur.has_c {
        cols.insert(0, "c.c".into());
    }
    let pos = cur.pos(&def);
    let sql = format!("SELECT {} FROM {} WHERE {pos} >= ? AND {pos} < ? ORDER BY {pos}", cols.join(", "), cur.from_sql(&def));
    let mut stmt = conn.prepare(&sql)?;
    let n = cols.len();
    let rows = stmt
        .query_map([offset, offset + limit], |r| (0..n).map(|i| Ok(to_json(r.get_ref(i)?))).collect::<duckdb::Result<Vec<_>>>())?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RowsWindow { expired: false, offset, rows })
}

// ── 选区统计（状态栏）────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsRequest {
    pub cursor_id: String,
    /// 行位置区间 [from, to]（含两端）
    pub from: i64,
    pub to: i64,
    /// 选中的列；可含 __contribution
    pub columns: Vec<String>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SelectionStats {
    pub expired: bool,
    /// 非空单元格数
    pub count: i64,
    /// 数值单元格数
    pub numeric_count: i64,
    pub sum: Option<f64>,
    pub avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

pub fn stats(conn: &Connection, cache: &CursorCache, dataset_id: &str, req: &StatsRequest) -> AppResult<SelectionStats> {
    let def = dataset(conn, dataset_id)?;
    let Some(cur) = find_cursor(cache, &req.cursor_id)? else {
        return Ok(SelectionStats { expired: true, ..Default::default() });
    };
    let has_c = cur.has_c;
    let mut counts = Vec::new();
    let mut numeric = Vec::new();
    for key in &req.columns {
        if key == CONTRIBUTION_KEY {
            if has_c {
                counts.push("count(c.c)".to_string());
                numeric.push("c.c".to_string());
            }
            continue;
        }
        let col = check_column(&def, key)?;
        let c = format!("({})", column_sql(col));
        counts.push(format!("count({c})"));
        if matches!(col.kind, "money" | "number") {
            numeric.push(format!("{c}::DOUBLE"));
        }
    }
    if counts.is_empty() {
        return Ok(SelectionStats::default());
    }
    // 多列数值合并为一列再聚合（与 Excel 状态栏一致：所有选中数值单元格一起求和 / 平均）
    let numeric_sql = if numeric.is_empty() {
        "NULL::DOUBLE, 0::BIGINT, NULL::DOUBLE, NULL::DOUBLE".to_string()
    } else {
        let list = format!("[{}]", numeric.join(", "));
        format!(
            "sum(list_sum({list})), sum(list_count({list}))::BIGINT, min(list_min({list})), max(list_max({list}))"
        )
    };
    let sql = format!(
        "SELECT ({counts})::BIGINT, {numeric_sql}
         FROM {from} WHERE {pos} BETWEEN ? AND ?",
        counts = counts.iter().map(|c| format!("COALESCE({c}, 0)")).collect::<Vec<_>>().join(" + "),
        from = cur.from_sql(&def),
        pos = cur.pos(&def)
    );
    let (count, sum, numeric_count, min, max): (i64, Option<f64>, i64, Option<f64>, Option<f64>) =
        conn.query_row(&sql, [req.from.max(0), req.to], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
    let round = |v: f64| (v * 100.0).round() / 100.0 + 0.0;
    Ok(SelectionStats {
        expired: false,
        count,
        numeric_count,
        sum: sum.map(round),
        avg: sum.filter(|_| numeric_count > 0).map(|s| round(s / numeric_count as f64)),
        min: min.map(round),
        max: max.map(round),
    })
}

// ── 列值分面（筛选下拉）──────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistinctRequest {
    #[serde(flatten)]
    pub spec: QuerySpec,
    pub column: String,
    /// 在候选值中搜索
    pub search: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistinctValue {
    /// null 以 "" 表示
    pub value: String,
    pub count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistinctResult {
    pub values: Vec<DistinctValue>,
    /// 候选值总数（超过返回上限时用于提示）
    pub total_distinct: i64,
    pub min: Option<Json>,
    pub max: Option<Json>,
}

/// 某列在「其他列筛选条件」下的取值与计数（Excel 筛选下拉的候选项）
pub fn distinct(conn: &Connection, dataset_id: &str, req: &DistinctRequest) -> AppResult<DistinctResult> {
    let def = dataset(conn, dataset_id)?;
    let col = check_column(&def, &req.column)?.clone();
    let contribution = contribution_of(&def, &req.spec)?;
    let (mut where_sql, mut args) = where_clause(&def, &req.spec, contribution.as_deref(), Some(&col.key))?;
    let c = format!("({})", column_sql(&col));
    if let Some(s) = req.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        where_sql.push_str(&format!(" AND CAST({c} AS VARCHAR) ILIKE ? ESCAPE '\\'"));
        args.push(Value::Text(format!("%{}%", escape_like(s))));
    }
    let src = &def.table;
    let mut stmt = conn.prepare(&format!(
        "SELECT COALESCE(CAST({c} AS VARCHAR), '') AS v, count(*) AS n FROM {src} WHERE {where_sql}
         GROUP BY v ORDER BY n DESC, v LIMIT {MAX_DISTINCT}"
    ))?;
    let values = stmt
        .query_map(params_from_iter(args.iter()), |r| Ok(DistinctValue { value: r.get(0)?, count: r.get(1)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    let (total_distinct, min, max) = conn.query_row(
        &format!("SELECT count(DISTINCT {c}) + max(CASE WHEN {c} IS NULL THEN 1 ELSE 0 END), min({c}), max({c}) FROM {src} WHERE {where_sql}"),
        params_from_iter(args.iter()),
        |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), to_json(r.get_ref(1)?), to_json(r.get_ref(2)?))),
    )?;
    Ok(DistinctResult { values, total_distinct, min: Some(min).filter(|v| !v.is_null()), max: Some(max).filter(|v| !v.is_null()) })
}

// ── 导出 ──────────────────────────────────────────────

/// 单次导出上限：CSV 约 400 字节 / 行，百万行约 400 MB，再多应缩小筛选范围
pub const MAX_EXPORT_ROWS: i64 = 1_000_000;

/// 按游标顺序导出 CSV 到临时文件（DuckDB COPY，不经过内存拼接）；返回 (文件路径, 行数)
/// columns 为空时导出全部列；ID 类列加制表符前缀，防止 Excel 显示为科学计数法
pub fn export_csv(conn: &Connection, cache: &CursorCache, dataset_id: &str, cursor_id: &str, columns: &[String], dir: &std::path::Path) -> AppResult<(std::path::PathBuf, i64)> {
    let def = dataset(conn, dataset_id)?;
    let Some(cur) = find_cursor(cache, cursor_id)? else {
        return Err(AppError::bad("数据已更新，请刷新后重新导出"));
    };
    let (has_c, total) = (cur.has_c, cur.total);
    if total > MAX_EXPORT_ROWS {
        return Err(AppError::bad(format!(
            "当前结果 {total} 行，单次最多导出 {MAX_EXPORT_ROWS} 行，请先筛选缩小范围"
        )));
    }
    let keys: Vec<String> = if columns.is_empty() {
        let mut all: Vec<String> = def.columns.iter().map(|c| c.key.clone()).collect();
        if has_c {
            all.insert(0, CONTRIBUTION_KEY.into());
        }
        all
    } else {
        columns.to_vec()
    };
    let mut selects = Vec::new();
    for key in &keys {
        if key == CONTRIBUTION_KEY {
            if has_c {
                selects.push(format!("round(c.c, 2) AS {}", ident("贡献金额")));
            }
            continue;
        }
        let col = check_column(&def, key)?;
        let c = format!("({})", column_sql(col));
        let expr = match col.kind {
            "money" | "number" => format!("{c}::DOUBLE"),
            "id" => format!("CASE WHEN {c} IS NULL THEN NULL ELSE chr(9) || {c} END"),
            _ => c,
        };
        selects.push(format!("{expr} AS {}", ident(&col.title)));
    }
    let path = dir.join(format!("export-{}-{}.csv", cursor_id, std::process::id()));
    let path_sql = path.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!(
        "COPY (SELECT {} FROM {} ORDER BY {}) TO '{path_sql}' (HEADER, DELIMITER ',')",
        selects.join(", "),
        cur.from_sql(&def),
        cur.pos(&def)
    ))?;
    Ok((path, total))
}

/// Excel 单张工作表的数据行上限（1,048,576 行减去表头）
pub const MAX_XLSX_ROWS: i64 = 1_048_575;

fn excel_col(i: usize) -> String {
    let mut n = i + 1;
    let mut s = String::new();
    while n > 0 {
        let m = (n - 1) % 26;
        s.insert(0, (b'A' + m as u8) as char);
        n = (n - 1) / 26;
    }
    s
}

/// 按游标导出 .xlsx：「汇总」页签（数值列用 SUBTOTAL 公式引用数据，Excel 里筛选后自动按可见行重算）+「数据」页签（常量内存流式写入）
pub fn export_xlsx(
    conn: &Connection,
    cache: &CursorCache,
    dataset_id: &str,
    cursor_id: &str,
    columns: &[String],
    desc: &str,
    dir: &std::path::Path,
) -> AppResult<(std::path::PathBuf, i64)> {
    use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Formula, Workbook};

    let def = dataset(conn, dataset_id)?;
    let Some(cur) = find_cursor(cache, cursor_id)? else {
        return Err(AppError::bad("数据已更新，请刷新后重新导出"));
    };
    let (has_c, total) = (cur.has_c, cur.total);
    if total > MAX_XLSX_ROWS {
        return Err(AppError::bad(format!("当前结果 {total} 行，超过 Excel 单表上限 {MAX_XLSX_ROWS} 行，请先筛选或改用 CSV 导出")));
    }

    // 导出列（含贡献列）
    struct Col {
        title: String,
        kind: &'static str,
        width: u32,
        expr: String,
    }
    let keys: Vec<String> = if columns.is_empty() {
        let mut all: Vec<String> = def.columns.iter().map(|c| c.key.clone()).collect();
        if has_c {
            all.insert(0, CONTRIBUTION_KEY.into());
        }
        all
    } else {
        columns.to_vec()
    };
    let mut cols: Vec<Col> = Vec::new();
    for key in &keys {
        if key == CONTRIBUTION_KEY {
            if has_c {
                cols.push(Col { title: "贡献金额".into(), kind: "money", width: 120, expr: "c.c".into() });
            }
            continue;
        }
        let col = check_column(&def, key)?;
        let base = format!("({})", column_sql(col));
        let expr = if matches!(col.kind, "money" | "number") { format!("{base}::DOUBLE") } else { format!("CAST({base} AS VARCHAR)") };
        cols.push(Col { title: col.title.clone(), kind: col.kind, width: col.width, expr });
    }
    if cols.is_empty() {
        return Err(AppError::bad("没有可导出的列"));
    }

    let header = Format::new().set_bold().set_background_color(Color::RGB(0x107C41)).set_font_color(Color::White).set_align(FormatAlign::Center).set_border_bottom(FormatBorder::Thin);
    let money = Format::new().set_num_format("#,##0.00;-#,##0.00;\"-\"");
    let integer = Format::new().set_num_format("#,##0");
    let label = Format::new().set_font_color(Color::RGB(0x6B7280));
    let title_fmt = Format::new().set_bold().set_font_size(14);

    let mut book = Workbook::new();
    book.set_tempdir(dir).map_err(|e| anyhow::anyhow!("{e}"))?;

    // ── 汇总页签 ──
    let numeric: Vec<(usize, &Col)> = cols.iter().enumerate().filter(|(_, c)| matches!(c.kind, "money" | "number")).collect();
    let agg_sql = if numeric.is_empty() {
        String::new()
    } else {
        numeric
            .iter()
            .map(|(_, c)| format!("COALESCE(sum({e}), 0), avg({e}), min({e}), max({e})", e = c.expr))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let aggs: Vec<Option<f64>> = if numeric.is_empty() {
        vec![]
    } else {
        conn.query_row(
            &format!("SELECT {agg_sql} FROM {}", cur.from_sql(&def)),
            [],
            |r| (0..numeric.len() * 4).map(|i| r.get::<_, Option<f64>>(i)).collect(),
        )?
    };
    {
        let ws = book.add_worksheet();
        ws.set_name("汇总").map_err(|e| anyhow::anyhow!("{e}"))?;
        ws.set_column_width(0, 22).map_err(|e| anyhow::anyhow!("{e}"))?;
        for c in 1..=4u16 {
            ws.set_column_width(c, 18).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let w = |r: anyhow::Result<()>| r;
        let e = |x: rust_xlsxwriter::XlsxError| anyhow::anyhow!("{x}");
        w((|| {
            ws.write_string_with_format(0, 0, format!("{} 导出", def.name), &title_fmt).map_err(e)?;
            ws.write_string_with_format(1, 0, "导出时间", &label).map_err(e)?;
            ws.write_string(1, 1, crate::db::now_local()).map_err(e)?;
            ws.write_string_with_format(2, 0, "筛选条件", &label).map_err(e)?;
            ws.write_string(2, 1, if desc.trim().is_empty() { "全部数据" } else { desc.trim() }).map_err(e)?;
            ws.write_string_with_format(3, 0, "行数", &label).map_err(e)?;
            ws.write_number_with_format(3, 1, total as f64, &integer).map_err(e)?;
            ws.write_string_with_format(4, 0, "说明", &label).map_err(e)?;
            ws.write_string(4, 1, "下表用 SUBTOTAL 公式引用「数据」页签：在「数据」页签中筛选后，合计 / 平均 / 最小 / 最大按可见行自动重算").map_err(e)?;
            for (i, h) in ["列", "合计", "平均", "最小", "最大"].iter().enumerate() {
                ws.write_string_with_format(6, i as u16, *h, &header).map_err(e)?;
            }
            let last = total + 1;
            for (k, (ci, c)) in numeric.iter().enumerate() {
                let row = 7 + k as u32;
                ws.write_string(row, 0, &c.title).map_err(e)?;
                let rng = format!("'数据'!{col}2:{col}{last}", col = excel_col(*ci));
                // 109 / 101 / 105 / 104：忽略隐藏行的 SUM / AVERAGE / MIN / MAX
                for (j, code) in [109, 101, 105, 104].iter().enumerate() {
                    let cached = aggs.get(k * 4 + j).copied().flatten();
                    let mut f = Formula::new(format!("=SUBTOTAL({code},{rng})"));
                    if let Some(v) = cached {
                        f = f.set_result(format!("{v}"));
                    }
                    ws.write_formula_with_format(row, 1 + j as u16, f, &money).map_err(e)?;
                }
            }
            Ok(())
        })())?;
    }

    // ── 数据页签（常量内存：逐行顺序写入）──
    {
        let ws = book.add_worksheet_with_constant_memory();
        ws.set_name("数据").map_err(|e| anyhow::anyhow!("{e}"))?;
        for (i, c) in cols.iter().enumerate() {
            ws.set_column_width(i as u16, (c.width as f64 / 7.0).clamp(6.0, 60.0)).map_err(|e| anyhow::anyhow!("{e}"))?;
            ws.write_string_with_format(0, i as u16, &c.title, &header).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        ws.set_freeze_panes(1, 0).map_err(|e| anyhow::anyhow!("{e}"))?;
        ws.autofilter(0, 0, total.max(1) as u32, (cols.len() - 1) as u16).map_err(|e| anyhow::anyhow!("{e}"))?;
        let select = cols.iter().map(|c| c.expr.clone()).collect::<Vec<_>>().join(", ");
        let mut stmt = conn.prepare(&format!(
            "SELECT {select} FROM {} ORDER BY {}",
            cur.from_sql(&def),
            cur.pos(&def)
        ))?;
        let mut rows = stmt.query([])?;
        let mut r: u32 = 1;
        while let Some(row) = rows.next()? {
            for (i, c) in cols.iter().enumerate() {
                let i16 = i as u16;
                if matches!(c.kind, "money" | "number") {
                    if let Some(v) = row.get::<_, Option<f64>>(i)? {
                        let fmt = if c.kind == "money" { &money } else { &integer };
                        ws.write_number_with_format(r, i16, v, fmt).map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                } else if let Some(v) = row.get::<_, Option<String>>(i)? {
                    // 编号列按文本写入，Excel 不会转成科学计数法
                    ws.write_string(r, i16, v).map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
            r += 1;
        }
    }

    let path = dir.join(format!("export-{}-{}.xlsx", cursor_id, std::process::id()));
    book.save(&path).map_err(|e| anyhow::anyhow!("生成 Excel 失败：{e}"))?;
    Ok((path, total))
}

// ── 计算列管理 ────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalcPreview {
    pub kind: &'static str,
    /// 前 10 行的计算结果（含引用列的值，便于核对）
    pub samples: Vec<serde_json::Value>,
    /// 全表中结果为空的行数（如关联不到成本价）
    pub null_rows: i64,
    pub total_rows: i64,
}

fn check_calc_name(conn: &Connection, def: &DatasetDef, name: &str) -> AppResult<String> {
    let n = name.trim().replace(['"', '[', ']'], "");
    if n.is_empty() || n.chars().count() > 40 {
        return Err(AppError::bad("列名不能为空，且不超过 40 个字符"));
    }
    if def.columns.iter().any(|c| c.title == n || c.key == n) || matches!(n.to_lowercase().as_str(), "search_text" | "rowid" | "__rid") {
        return Err(AppError::bad(format!("已存在列「{n}」")));
    }
    // 计算列会被以它为左表的关联视图继承，不能和视图里的列重名
    let defs = datasets(conn)?;
    if let Some(v) = derived_views(&defs, &def.id).into_iter().find(|v| v.columns.iter().any(|c| c.title == n || c.key == n)) {
        return Err(AppError::bad(format!("关联视图「{}」中已有列「{n}」，请换个名字", v.name)));
    }
    Ok(n)
}

pub fn preview_calc(conn: &Connection, dataset_id: &str, formula: &str) -> AppResult<CalcPreview> {
    let def = dataset(conn, dataset_id)?;
    let col = compile_calc(&def, formula, None)?;
    let compiled = crate::calc::compile(formula, &DefResolver(&def))?;
    let expr = col.expr.clone().unwrap_or_default();
    let refs: Vec<String> = compiled.columns.iter().filter_map(|n| def.columns.iter().find(|c| c.title == *n || c.key == *n)).map(|c| format!("({}) AS {}", column_sql(c), ident(&c.title))).collect();
    let sql = format!("SELECT {}, ({expr}) AS __result FROM {} LIMIT 10", refs.join(", "), def.table);
    let mut stmt = conn.prepare(&sql).map_err(|e| AppError::bad(format!("公式无法计算：{e}")))?;
    let names: Vec<String> = compiled.columns.clone();
    let samples = stmt
        .query_map([], |r| {
            let mut m = serde_json::Map::new();
            for (i, n) in names.iter().enumerate() {
                m.insert(n.clone(), to_json(r.get_ref(i)?));
            }
            m.insert("结果".into(), to_json(r.get_ref(names.len())?));
            Ok(serde_json::Value::Object(m))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let (total_rows, null_rows): (i64, i64) = conn.query_row(
        &format!("SELECT count(*), count(*) FILTER (WHERE ({expr}) IS NULL) FROM {}", def.table),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(CalcPreview { kind: col.kind, samples, null_rows, total_rows })
}

pub fn add_calc(conn: &Connection, dataset_id: &str, name: &str, formula: &str, kind: Option<&str>) -> AppResult<()> {
    let def = dataset(conn, dataset_id)?;
    let name = check_calc_name(conn, &def, name)?;
    let col = compile_calc(&def, formula, kind)?;
    try_run(conn, &def, &col)?;
    let formula = formula.trim().to_string();
    let kind = kind.map(str::to_string);
    guarded_change(conn, |conn| {
        conn.execute(
            "INSERT INTO ledger_calc_column (dataset_id, name, formula, kind, created_at) VALUES (?, ?, ?, ?, CAST(? AS TIMESTAMP))",
            duckdb::params![def.id, name, formula, kind, crate::db::now_local()],
        )?;
        Ok(())
    })
}

/// 实际执行一次，确保 SQL 可运行（类型不匹配等编译期看不出的问题）
fn try_run(conn: &Connection, def: &DatasetDef, col: &ColumnDef) -> AppResult<()> {
    conn.query_row(&format!("SELECT count(({})) FROM (SELECT * FROM {} LIMIT 1000)", col.expr.clone().unwrap_or_default(), def.table), [], |r| r.get::<_, i64>(0))
        .map_err(|e| AppError::bad(format!("公式无法计算：{e}")))?;
    Ok(())
}

/// 找到本数据集自己定义的计算列（继承自左表的不能在视图上改）
fn own_calc<'a>(def: &'a DatasetDef, name: &str) -> AppResult<&'a ColumnDef> {
    let col = def.columns.iter().find(|c| c.title == name && c.formula.is_some()).ok_or_else(|| AppError::bad(format!("计算列「{name}」不存在")))?;
    if let Some(origin) = &col.origin {
        return Err(AppError::bad(format!("「{name}」是「{origin}」的计算列，请到「{origin}」中修改")));
    }
    Ok(col)
}

/// 修改计算列公式：引用它的计算列、关联视图随之更新；会导致它们失效时整体取消
pub fn update_calc(conn: &Connection, dataset_id: &str, name: &str, formula: &str, kind: Option<&str>) -> AppResult<()> {
    let def = dataset(conn, dataset_id)?;
    own_calc(&def, name)?;
    // 只能引用在它之前添加的列，避免循环引用
    let idx = def.columns.iter().position(|c| c.title == name).unwrap_or(def.columns.len());
    let before = DatasetDef { columns: def.columns[..idx].to_vec(), ..def.clone() };
    let col = compile_calc(&before, formula, kind).map_err(|e| match e {
        AppError::BadRequest(m) if def.columns[idx..].iter().any(|c| formula.contains(&format!("[{}]", c.title))) => {
            AppError::bad(format!("{m}（计算列只能引用在它之前添加的列，不能引用自身或之后的计算列）"))
        }
        e => e,
    })?;
    try_run(conn, &before, &col)?;
    // 数值 ↔ 文本 切换时，引用它的计算列会在运行时得到空值（编译不报错），这里直接拦下
    let old = &def.columns[idx];
    if (old.kind == "text") != (col.kind == "text") {
        let defs = datasets(conn)?;
        let pat = format!("[{name}]");
        let mut users: Vec<String> = def.columns.iter().filter(|c| c.title != name && c.formula.as_deref().is_some_and(|f| f.contains(&pat))).map(|c| format!("「{}」", c.title)).collect();
        for v in derived_views(&defs, &def.id) {
            users.extend(v.columns.iter().filter(|c| c.origin.is_none() && c.formula.as_deref().is_some_and(|f| f.contains(&pat))).map(|c| format!("「{}」的「{}」", v.name, c.title)));
        }
        if !users.is_empty() {
            return Err(AppError::bad(format!(
                "计算列{}引用了「{name}」，结果类型从{}改为{}会使它们的结果变为空，请先调整这些计算列",
                users.join("、"),
                if old.kind == "text" { "文本" } else { "数值" },
                if col.kind == "text" { "文本" } else { "数值" }
            )));
        }
    }
    let formula = formula.trim().to_string();
    let kind = kind.map(str::to_string);
    guarded_change(conn, |conn| {
        conn.execute("UPDATE ledger_calc_column SET formula = ?, kind = ? WHERE dataset_id = ? AND name = ?", duckdb::params![formula, kind, def.id, name])?;
        Ok(())
    })
}

/// 删除计算列：被其他计算列或关联视图引用时拒绝（提示受影响的列）
pub fn remove_calc(conn: &Connection, dataset_id: &str, name: &str) -> AppResult<()> {
    let def = dataset(conn, dataset_id)?;
    own_calc(&def, name)?;
    guarded_change(conn, |conn| {
        conn.execute("DELETE FROM ledger_calc_column WHERE dataset_id = ? AND name = ?", duckdb::params![def.id, name])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use duckdb::Connection;

    #[test]
    fn rowid_contiguity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INT); INSERT INTO t SELECT * FROM range(10)").unwrap();
        assert_eq!(super::contiguous_rows(&conn, "t").unwrap(), Some(10));
        conn.execute_batch("DELETE FROM t WHERE a IN (3, 4)").unwrap();
        assert_eq!(super::contiguous_rows(&conn, "t").unwrap(), None, "删除后行号有空洞，不能直通");
        conn.execute_batch("CREATE TABLE e (a INT)").unwrap();
        assert_eq!(super::contiguous_rows(&conn, "e").unwrap(), Some(0));
    }

    /// 位置表的约定：INSERT ... SELECT 保持查询顺序，行的位置就是位置表自己的 rowid。
    /// 数据量要跨多个行组、并行扫描（默认线程数），才能验证到「并行时顺序也不乱」
    #[test]
    fn cursor_position_is_rowid_of_position_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t AS SELECT i AS a, (i * 7919) % 10007 AS v, CASE WHEN i % 97 = 0 THEN NULL ELSE (i * 31) % 1000 END AS w FROM range(1500000) r(i);
             -- 有排序：与 row_number() 窗口的结果逐位置对照（含 NULL 排最后、同值按行号）
             CREATE TABLE sorted_c AS SELECT rowid AS rid FROM t WHERE v % 3 <> 0 ORDER BY w DESC NULLS LAST, v ASC NULLS LAST, rowid;
             CREATE TABLE sorted_ref AS SELECT (row_number() OVER (ORDER BY w DESC NULLS LAST, v ASC NULLS LAST, rowid) - 1)::BIGINT AS p, rowid AS rid FROM t WHERE v % 3 <> 0;
             -- 无排序：不写 ORDER BY，表扫描的产出顺序就是行号顺序
             CREATE TABLE plain_c AS SELECT rowid AS rid FROM t WHERE v % 5 = 1;",
        )
        .unwrap();
        let bad: i64 = conn.query_row("SELECT count(*) FROM sorted_c c JOIN sorted_ref r ON r.p = c.rowid WHERE r.rid <> c.rid", [], |r| r.get(0)).unwrap();
        let (n, nref): (i64, i64) = conn.query_row("SELECT (SELECT count(*) FROM sorted_c), (SELECT count(*) FROM sorted_ref)", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((bad, n), (0, nref), "排序位置表的 rowid 必须等于排序名次");
        let (n, contiguous, ascending): (i64, i64, i64) = conn
            .query_row(
                "SELECT count(*), count(*) FILTER (WHERE rowid = p), count(*) FILTER (WHERE rid > prev) FROM (SELECT rowid, rid, (row_number() OVER (ORDER BY rowid) - 1) AS p, lag(rid) OVER (ORDER BY rowid) AS prev FROM plain_c)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(n > 100_000);
        assert_eq!(contiguous, n, "位置表的 rowid 必须是 0..N-1");
        assert_eq!(ascending, n - 1, "无排序时行号必须递增");
    }
}
