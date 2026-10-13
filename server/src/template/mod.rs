//! 模板：任意工作簿发布为模板；使用模板时把模板引用的数据集换成使用者自己的数据集，生成一份新工作簿
//!
//! - 发布：保存工作簿副本，并收集它引用的数据集与列（透视表、图表 / 仪表盘部件、D 系列公式）
//! - 使用：每个模板数据集对应到使用者的一个数据集、逐列对应列名 → 改写所有引用 → 新建工作簿。
//!   打开时透视表、图表、指标卡、公式都按使用者的最新数据计算
//! - 内置模板随程序发布（templates/*.json），引用内置数据集「资金流水」的自动对应
//! - 使用统计：每次使用记一条（模板、生成的工作簿、时间）
//! - 打开时填参数：模板数据集来自参数化 SQL 查询（查询本身或它的实例）时，使用者可以只填参数值，
//!   按这组值生成（或复用）一个实例作为数据集，不用先去 SQL 工作台生成再回来选

pub mod refs;

use std::collections::{BTreeMap, HashMap};
use std::io::Read;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::dataset::{self, DatasetDef};
use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::workbook::{self, WorkbookMeta};

/// 内置模板（编译进程序）
const BUILTIN: [&str; 7] = [
    include_str!("../../templates/monthly-ops.json"),
    include_str!("../../templates/refund-analysis.json"),
    include_str!("../../templates/qianchuan-roi.json"),
    include_str!("../../templates/platform-pnl.json"),
    include_str!("../../templates/ar-ap.json"),
    include_str!("../../templates/stock.json"),
    include_str!("../../templates/bank-rec.json"),
];

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TemplateColumn {
    pub title: String,
    /// text / id / money / number / datetime
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 模板需要的一个数据集
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TemplateDataset {
    /// 模板内的引用键：发布时为原数据集 ID（公式里只按名称引用、目录中找不到时为 "name:<名称>"）
    pub key: String,
    /// 发布时的数据集名称（公式按名称引用）
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 引用的是内置数据集（如资金流水）：使用时自动对应
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin_id: Option<String>,
    pub columns: Vec<TemplateColumn>,
    /// 部件类型 → 个数（透视表 / 图表 / 指标卡 / 明细卡 / 切片器 / 公式）
    #[serde(default)]
    pub parts: BTreeMap<String, usize>,
    /// 来自参数化 SQL 查询：使用时可以填参数值代替选择数据集
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_source: Option<ParamSource>,
}

/// 模板数据集的参数化来源
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ParamSource {
    /// 参数化查询（实例的话取它的来源查询）
    pub query_id: String,
    pub query_name: String,
    pub params: Vec<crate::sqlq::params::ParamDef>,
    /// 发布时用的参数值（使用时作为默认值）
    #[serde(default)]
    pub values: HashMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinFile {
    id: String,
    name: String,
    description: String,
    category: String,
    /// 属于哪个场景包（如 ecommerce）；没启用这个场景包时模板不出现。空表示通用模板
    #[serde(default)]
    pack: Option<String>,
    datasets: Vec<TemplateDataset>,
    workbook: Json,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub builtin: bool,
    pub datasets: Vec<TemplateDataset>,
    /// sheets / pivot / webexcel-chart / webexcel-kpi / … / formula → 个数
    pub summary: BTreeMap<String, usize>,
    /// 无法自动替换的引用说明
    pub warnings: Vec<String>,
    pub sheet_names: Vec<String>,
    pub use_count: i64,
    pub last_used_at: Option<String>,
    pub source_workbook_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

struct Stored {
    view: TemplateView,
    workbook: Json,
}

fn gunzip(bytes: &[u8]) -> AppResult<Json> {
    let mut s = String::new();
    flate2::read::GzDecoder::new(bytes).read_to_string(&mut s)?;
    Ok(serde_json::from_str(&s)?)
}

fn sheet_names(wb: &Json) -> Vec<String> {
    let order: Vec<String> = wb.get("sheetOrder").and_then(|o| serde_json::from_value(o.clone()).ok()).unwrap_or_default();
    order.iter().filter_map(|id| wb.pointer(&format!("/sheets/{id}/name")).and_then(Json::as_str).map(String::from)).collect()
}

fn usage(conn: &Connection) -> AppResult<HashMap<String, (i64, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT template_id, count(*), strftime(max(used_at), '%Y-%m-%d %H:%M:%S') FROM ledger_template_use GROUP BY template_id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))))?;
    Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
}

fn builtins() -> Vec<Stored> {
    BUILTIN
        .iter()
        .filter_map(|s| match serde_json::from_str::<BuiltinFile>(s) {
            Ok(b) => Some(b),
            Err(e) => {
                tracing::error!("内置模板格式错误：{e}");
                None
            }
        })
        .filter(|b| b.pack.as_deref() != Some("ecommerce") || crate::ecommerce::enabled())
        .map(|mut b| {
            // 内置模板文件只声明数据集与列，各数据集被哪些部件使用按工作簿统计
            let collected = refs::collect(&b.workbook);
            for d in &mut b.datasets {
                for (key, u) in &collected.by_key {
                    if key == &d.key || key.strip_prefix("name:") == Some(d.name.as_str()) {
                        for (k, n) in &u.parts {
                            *d.parts.entry(k.clone()).or_default() += n;
                        }
                    }
                }
            }
            (b, collected.warnings)
        })
        .map(|(b, warnings)| Stored {
            view: TemplateView {
                summary: refs::summary(&b.workbook),
                warnings,
                sheet_names: sheet_names(&b.workbook),
                id: b.id,
                name: b.name,
                description: b.description,
                category: b.category,
                builtin: true,
                datasets: b.datasets,
                use_count: 0,
                last_used_at: None,
                source_workbook_id: None,
                created_by: None,
                created_at: None,
                updated_at: None,
            },
            workbook: b.workbook,
        })
        .collect()
}

fn load_user(conn: &Connection, only: Option<&str>, with_workbook: bool) -> AppResult<Vec<Stored>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, category, requirements, summary, warnings, source_workbook_id, created_by,
                strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(updated_at, '%Y-%m-%d %H:%M:%S'), CASE WHEN ? THEN data END
         FROM ledger_template WHERE ? IS NULL OR id = ? ORDER BY updated_at DESC",
    )?;
    let rows = stmt
        .query_map(params![with_workbook, only, only], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?),
                (r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, Option<String>>(6)?),
                (r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?, r.get::<_, Option<String>>(9)?, r.get::<_, Option<String>>(10)?),
                r.get::<_, Option<Vec<u8>>>(11)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|((id, name, description, category), (req, summary, warnings), (source, by, created, updated), data)| {
            let workbook = match data {
                Some(d) => gunzip(&d)?,
                None => Json::Null,
            };
            let summary: BTreeMap<String, usize> = serde_json::from_str(&summary).unwrap_or_default();
            Ok(Stored {
                view: TemplateView {
                    datasets: serde_json::from_str(&req).unwrap_or_default(),
                    sheet_names: Vec::new(),
                    summary,
                    warnings: warnings.and_then(|w| serde_json::from_str(&w).ok()).unwrap_or_default(),
                    id,
                    name,
                    description: description.unwrap_or_default(),
                    category: category.unwrap_or_default(),
                    builtin: false,
                    use_count: 0,
                    last_used_at: None,
                    source_workbook_id: source,
                    created_by: by,
                    created_at: created,
                    updated_at: updated,
                },
                workbook,
            })
        })
        .collect()
}

pub fn list(conn: &Connection) -> AppResult<Vec<TemplateView>> {
    let stats = usage(conn)?;
    let names: HashMap<String, Vec<String>> = {
        // 自建模板的工作表名称存在 summary 之外，单独取（避免把整个工作簿读出来）
        let mut stmt = conn.prepare("SELECT id, sheet_names FROM ledger_template")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        rows.filter_map(Result::ok).map(|(id, s)| (id, s.and_then(|x| serde_json::from_str(&x).ok()).unwrap_or_default())).collect()
    };
    let mut out: Vec<TemplateView> = builtins().into_iter().map(|s| s.view).collect();
    out.extend(load_user(conn, None, false)?.into_iter().map(|mut s| {
        s.view.sheet_names = names.get(&s.view.id).cloned().unwrap_or_default();
        s.view
    }));
    for t in &mut out {
        if let Some((n, last)) = stats.get(&t.id) {
            t.use_count = *n;
            t.last_used_at = last.clone();
        }
    }
    Ok(out)
}

fn find(conn: &Connection, id: &str) -> AppResult<Stored> {
    if let Some(b) = builtins().into_iter().find(|b| b.view.id == id) {
        return Ok(b);
    }
    load_user(conn, Some(id), true)?.into_iter().next().ok_or_else(|| AppError::bad("模板不存在"))
}

// ── 发布 ──────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishInput {
    pub workbook_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    /// 覆盖已有的自建模板（重新发布）
    #[serde(default)]
    pub replace_id: Option<String>,
    #[serde(default)]
    pub operator: Option<String>,
}

/// 由引用收集结果与数据集目录得到模板需要的数据集
fn requirements(c: &refs::Collected, catalog: &[DatasetDef]) -> AppResult<(Vec<TemplateDataset>, Vec<String>)> {
    let mut warnings = c.warnings.clone();
    let mut merged: BTreeMap<String, refs::Usage> = BTreeMap::new();
    for (key, u) in &c.by_key {
        // 公式按名称引用：目录里有同名数据集时并到它的 ID 下
        let key = match key.strip_prefix("name:") {
            Some(name) => catalog.iter().find(|d| d.name == name).map(|d| d.id.clone()).unwrap_or_else(|| key.clone()),
            None => key.clone(),
        };
        let m = merged.entry(key).or_default();
        m.ids.extend(u.ids.iter().cloned());
        m.names.extend(u.names.iter().cloned());
        m.columns.extend(u.columns.iter().cloned());
        for (k, n) in &u.parts {
            *m.parts.entry(k.clone()).or_default() += n;
        }
    }
    let mut out = Vec::new();
    for (key, u) in merged {
        let def = catalog.iter().find(|d| d.id == key);
        if def.is_none() && !key.starts_with("name:") {
            return Err(AppError::bad(format!("工作簿引用了已删除的数据集（{key}），请先修改或删除引用它的部件后再发布")));
        }
        let name = def.map(|d| d.name.clone()).unwrap_or_else(|| key.trim_start_matches("name:").to_string());
        if def.is_none() {
            warnings.push(format!("公式引用的数据集「{name}」不存在，使用模板时需要对应到已有的数据集"));
        }
        let columns = u
            .columns
            .iter()
            .map(|t| {
                let kind = def.and_then(|d| d.columns.iter().find(|c| &c.title == t)).map(|c| c.kind.to_string());
                if def.is_some() && kind.is_none() {
                    warnings.push(format!("数据集「{name}」没有列「{t}」（可能已改名），使用模板时需要手动对应"));
                }
                TemplateColumn { title: t.clone(), kind: kind.unwrap_or_else(|| "text".into()), description: None }
            })
            .collect();
        out.push(TemplateDataset {
            builtin_id: def.filter(|d| d.builtin).map(|d| d.id.clone()),
            key,
            name,
            description: None,
            columns,
            parts: u.parts,
            param_source: None,
        });
    }
    Ok((out, warnings))
}

/// 模板数据集是参数化查询的结果（查询本身或实例）时记下来源：来源查询还在就用它，已删除就用实例自己（实例也带参数）
fn attach_param_sources(conn: &Connection, datasets: &mut [TemplateDataset]) -> AppResult<()> {
    let queries = crate::sqlq::list(conn)?;
    for d in datasets.iter_mut() {
        let Some(q) = queries.iter().find(|q| q.dataset_id.as_deref() == Some(d.key.as_str()) && !q.params.is_empty()) else { continue };
        let root = q.template_id.as_deref().and_then(|t| queries.iter().find(|x| x.id == t && !x.params.is_empty())).unwrap_or(q);
        d.param_source = Some(ParamSource { query_id: root.id.clone(), query_name: root.name.clone(), params: root.params.clone(), values: q.values.clone() });
    }
    Ok(())
}

pub fn publish(conn: &Connection, input: PublishInput) -> AppResult<TemplateView> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(AppError::bad("模板名称不能为空，且不超过 60 个字符"));
    }
    let (_, bytes) = workbook::content(conn, &input.workbook_id)?;
    let wb = gunzip(&bytes)?;
    let collected = refs::collect(&wb);
    let (mut datasets, warnings) = requirements(&collected, &dataset::datasets(conn)?)?;
    attach_param_sources(conn, &mut datasets)?;
    let summary = refs::summary(&wb);
    let now = now_local();
    let id = match input.replace_id.as_deref().filter(|s| !s.is_empty()) {
        Some(rid) => {
            if rid.starts_with("builtin_") {
                return Err(AppError::bad("内置模板不能覆盖，请另存为新模板"));
            }
            let n = conn.execute(
                "UPDATE ledger_template SET name = ?, description = ?, category = ?, data = ?, requirements = ?, summary = ?, warnings = ?, sheet_names = ?, source_workbook_id = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
                params![
                    name,
                    input.description.trim(),
                    input.category.trim(),
                    bytes,
                    serde_json::to_string(&datasets)?,
                    serde_json::to_string(&summary)?,
                    serde_json::to_string(&warnings)?,
                    serde_json::to_string(&sheet_names(&wb))?,
                    input.workbook_id,
                    now,
                    rid
                ],
            )?;
            if n == 0 {
                return Err(AppError::bad("要覆盖的模板不存在"));
            }
            rid.to_string()
        }
        None => {
            if conn.query_row("SELECT count(*) FROM ledger_template WHERE name = ?", params![name], |r| r.get::<_, i64>(0))? > 0 {
                return Err(AppError::bad(format!("已有名为「{name}」的模板，请换一个名称，或选择覆盖该模板")));
            }
            let id = format!("tpl_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
            conn.execute(
                "INSERT INTO ledger_template (id, name, description, category, data, requirements, summary, warnings, sheet_names, source_workbook_id, created_by, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
                params![
                    id,
                    name,
                    input.description.trim(),
                    input.category.trim(),
                    bytes,
                    serde_json::to_string(&datasets)?,
                    serde_json::to_string(&summary)?,
                    serde_json::to_string(&warnings)?,
                    serde_json::to_string(&sheet_names(&wb))?,
                    input.workbook_id,
                    input.operator.as_deref().unwrap_or(""),
                    now,
                    now
                ],
            )?;
            id
        }
    };
    list(conn)?.into_iter().find(|t| t.id == id).ok_or_else(|| AppError::bad("模板保存失败"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
}

pub fn update_meta(conn: &Connection, id: &str, input: MetaInput) -> AppResult<()> {
    if id.starts_with("builtin_") {
        return Err(AppError::bad("内置模板不能修改"));
    }
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(AppError::bad("模板名称不能为空，且不超过 60 个字符"));
    }
    if conn.query_row("SELECT count(*) FROM ledger_template WHERE name = ? AND id <> ?", params![name, id], |r| r.get::<_, i64>(0))? > 0 {
        return Err(AppError::bad(format!("已有名为「{name}」的模板")));
    }
    let n = conn.execute(
        "UPDATE ledger_template SET name = ?, description = ?, category = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
        params![name, input.description.trim(), input.category.trim(), now_local(), id],
    )?;
    if n == 0 {
        return Err(AppError::bad("模板不存在"));
    }
    Ok(())
}

pub fn delete(conn: &Connection, id: &str) -> AppResult<()> {
    if id.starts_with("builtin_") {
        return Err(AppError::bad("内置模板不能删除"));
    }
    conn.execute("DELETE FROM ledger_template WHERE id = ?", params![id])?;
    conn.execute("DELETE FROM ledger_template_use WHERE template_id = ?", params![id])?;
    Ok(())
}

// ── 使用 ──────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindInput {
    pub dataset_id: String,
    /// 模板列名 → 数据集列名（未列出的按同名对应）
    #[serde(default)]
    pub columns: HashMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UseInput {
    /// 新工作簿名称
    pub name: String,
    /// 模板数据集 key → 绑定
    #[serde(default)]
    pub bindings: HashMap<String, BindInput>,
    /// 清空切片器里模板作者选中的值（默认是）
    #[serde(default = "yes")]
    pub clear_slicers: bool,
    /// 模板数据集 key → 参数值（来自参数化查询的数据集，代替 bindings：按这组值生成或复用实例）
    #[serde(default)]
    pub params: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    pub operator: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UseOutcome {
    pub workbook: WorkbookMeta,
    /// 无法自动替换的引用（需要手动检查）
    pub warnings: Vec<String>,
    /// 按参数生成 / 复用的数据集说明
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn numeric(kind: &str) -> bool {
    matches!(kind, "money" | "number")
}

/// 所需列类型与数据集列类型是否相容：日期时间、数值需要同类；文本类可对应任意列
fn compatible(need: &str, have: &str) -> bool {
    match need {
        "datetime" => have == "datetime",
        k if numeric(k) => numeric(have),
        _ => true,
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "datetime" => "日期时间",
        "money" => "金额",
        "number" => "数值",
        "id" => "编号",
        _ => "文本",
    }
}

/// 校验绑定并生成改写规则
fn resolve(t: &Stored, input: &UseInput, catalog: &[DatasetDef]) -> AppResult<refs::Rebind> {
    let mut rb = refs::Rebind { clear_slicers: input.clear_slicers, ..Default::default() };
    for need in &t.view.datasets {
        let bind = input.bindings.get(&need.key);
        let target_id = match (bind, &need.builtin_id) {
            (Some(b), _) => b.dataset_id.clone(),
            (None, Some(builtin)) => builtin.clone(),
            (None, None) => return Err(AppError::bad(format!("请为模板数据集「{}」选择对应的数据集", need.name))),
        };
        let def = catalog.iter().find(|d| d.id == target_id).ok_or_else(|| AppError::bad(format!("「{}」对应的数据集不存在", need.name)))?;
        let mut columns = HashMap::new();
        for col in &need.columns {
            let to = bind.and_then(|b| b.columns.get(&col.title)).cloned().unwrap_or_else(|| col.title.clone());
            let have = def.columns.iter().find(|c| c.title == to).ok_or_else(|| {
                AppError::bad(format!("「{}」需要的列「{}」在数据集「{}」中没有对应列，请选择", need.name, col.title, def.name))
            })?;
            if !compatible(&col.kind, have.kind) {
                return Err(AppError::bad(format!(
                    "「{}」的列「{}」需要{}列，「{}」是{}列",
                    need.name,
                    col.title,
                    kind_label(&col.kind),
                    to,
                    kind_label(have.kind)
                )));
            }
            if to != col.title {
                columns.insert(col.title.clone(), to);
            }
        }
        let b = refs::Binding { dataset_id: def.id.clone(), dataset_name: def.name.clone(), columns };
        if !need.key.starts_with("name:") {
            rb.by_id.insert(need.key.clone(), b.clone());
        }
        rb.by_name.insert(need.name.clone(), b);
    }
    Ok(rb)
}

/// 按参数值取得数据集，写进绑定（列名与发布时相同：同一段 SQL）
fn bind_params(state: &crate::routes::AppState, conn: &Connection, t: &Stored, input: &mut UseInput) -> AppResult<Vec<String>> {
    let mut notes = Vec::new();
    let params = std::mem::take(&mut input.params);
    for (key, values) in params {
        let need = t.view.datasets.iter().find(|d| d.key == key).ok_or_else(|| AppError::bad(format!("模板里没有数据集 {key}")))?;
        let src = need.param_source.as_ref().ok_or_else(|| AppError::bad(format!("模板数据集「{}」不是参数化查询，请选择数据集", need.name)))?;
        if input.bindings.contains_key(&key) {
            return Err(AppError::bad(format!("模板数据集「{}」不能既填参数又选择数据集", need.name)));
        }
        let (q, created) = crate::sqlq::instance_for(state, conn, &src.query_id, values, input.operator.clone())?;
        let dataset_id = q.dataset_id.clone().ok_or_else(|| AppError::bad(format!("查询「{}」没有生成数据集", q.name)))?;
        let name = q.dataset_name.clone().unwrap_or_else(|| q.name.clone());
        notes.push(if created { format!("「{}」按参数生成了数据集「{name}」（{} 行）", need.name, q.last_rows.unwrap_or(0)) } else { format!("「{}」使用已有的数据集「{name}」（同样的参数，已按最新数据重算，{} 行）", need.name, q.last_rows.unwrap_or(0)) });
        input.bindings.insert(key, BindInput { dataset_id, columns: HashMap::new() });
    }
    Ok(notes)
}

pub fn use_template(state: &crate::routes::AppState, conn: &Connection, id: &str, mut input: UseInput) -> AppResult<UseOutcome> {
    let t = find(conn, id)?;
    let notes = bind_params(state, conn, &t, &mut input)?;
    let rb = resolve(&t, &input, &dataset::datasets(conn)?)?;
    let mut wb = t.workbook;
    let mut warnings = refs::rebind(&mut wb, &rb);
    let name = input.name.trim();
    if let Some(o) = wb.as_object_mut() {
        o.insert("name".into(), Json::String(name.to_string()));
    }
    let gz = workbook::gzip(wb.to_string().as_bytes());
    let operator = input.operator.as_deref().filter(|s| !s.is_empty()).unwrap_or("模板");
    let meta = workbook::create(conn, name, &gz, operator)?;
    conn.execute(
        "INSERT INTO ledger_template_use (template_id, workbook_id, used_by, used_at) VALUES (?, ?, ?, CAST(? AS TIMESTAMP))",
        params![id, meta.id, operator, now_local()],
    )?;
    warnings.dedup();
    Ok(UseOutcome { workbook: meta, warnings, notes })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UseRecord {
    pub workbook_id: String,
    /// 工作簿已删除时为空
    pub workbook_name: Option<String>,
    pub used_by: Option<String>,
    pub used_at: String,
}

pub fn uses(conn: &Connection, id: &str) -> AppResult<Vec<UseRecord>> {
    let mut stmt = conn.prepare(
        "SELECT u.workbook_id, w.name, u.used_by, strftime(u.used_at, '%Y-%m-%d %H:%M:%S')
         FROM ledger_template_use u LEFT JOIN ledger_workbook w ON w.id = u.workbook_id
         WHERE u.template_id = ? ORDER BY u.used_at DESC LIMIT 50",
    )?;
    let rows = stmt.query_map(params![id], |r| Ok(UseRecord { workbook_id: r.get(0)?, workbook_name: r.get(1)?, used_by: r.get(2)?, used_at: r.get::<_, Option<String>>(3)?.unwrap_or_default() }))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecommerce_templates_are_hidden_when_the_pack_is_off() {
        let ids = |on| crate::ecommerce::with_enabled(on, || builtins().into_iter().map(|s| s.view.id).collect::<Vec<_>>());
        let all = ids(true);
        let generic = ids(false);
        for id in ["builtin_refund_analysis", "builtin_qianchuan_roi", "builtin_platform_pnl"] {
            assert!(all.iter().any(|x| x == id) && !generic.iter().any(|x| x == id), "{id}");
        }
        for id in ["builtin_ar_ap", "builtin_bank_rec", "builtin_stock", "builtin_monthly_ops"] {
            assert!(generic.iter().any(|x| x == id), "通用模板应一直在：{id}");
        }
    }

    #[test]
    fn builtin_templates_are_valid() {
        let list = builtins();
        assert_eq!(list.len(), BUILTIN.len(), "内置模板解析失败");
        for t in list {
            let c = refs::collect(&t.workbook);
            // 模板声明的数据集覆盖了工作簿里的全部引用，声明的列也覆盖了用到的列
            for (key, u) in &c.by_key {
                let need = t
                    .view
                    .datasets
                    .iter()
                    .find(|d| &d.key == key || key.strip_prefix("name:") == Some(d.name.as_str()))
                    .unwrap_or_else(|| panic!("{}：引用 {key} 未声明", t.view.name));
                for col in &u.columns {
                    assert!(need.columns.iter().any(|x| &x.title == col), "{}：{key} 的列 {col} 未声明", t.view.name);
                }
            }
            assert!(t.view.warnings.is_empty(), "{}：{:?}", t.view.name, t.view.warnings);
            assert!(!t.view.sheet_names.is_empty());
        }
    }

    #[test]
    fn kinds() {
        assert!(compatible("money", "number"));
        assert!(!compatible("money", "text"));
        assert!(!compatible("datetime", "text"));
        assert!(compatible("text", "id"));
    }
}
