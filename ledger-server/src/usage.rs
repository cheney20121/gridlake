//! 数据集使用情况
//!
//! - 静态引用：哪些工作簿（透视表、图表、切片器、指标卡、表格卡片、数据表、填报区、D 系列公式）、模板、
//!   关联视图、SQL 查询（来源 / 输出）、数据连接、定时推送在用这个数据集。删除、改名前据此提示影响范围
//! - 访问次数：按天、按来源（明细浏览、透视 / 图表、公式、导出、AI、MCP、推送）计数，保留 90 天。
//!   计数先累加在内存里，每分钟与退出前写入数据库，不给每次查询增加一次写库

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::sync::{Mutex, OnceLock};

use chrono::{Duration, Local, NaiveDate};
use duckdb::{Connection, params};
use serde::Serialize;
use serde_json::Value as Json;

use crate::error::{AppError, AppResult};

/// 访问来源
pub const BROWSE: &str = "browse";
pub const PIVOT: &str = "pivot";
pub const FORMULA: &str = "formula";
pub const EXPORT: &str = "export";
pub const AI: &str = "ai";
pub const MCP: &str = "mcp";
pub const REPORT: &str = "report";
pub const SQL: &str = "sql";

/// 访问记录保留天数
const KEEP_DAYS: i64 = 90;

type Key = (String, String, &'static str);

fn pending() -> &'static Mutex<HashMap<Key, i64>> {
    static P: OnceLock<Mutex<HashMap<Key, i64>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    /// 调用方指定的来源（MCP 复用 AI 的查询工具，借此区分）
    static SOURCE: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// 在 f 执行期间把本线程上的访问都记为 kind
pub fn with_source<T>(kind: &'static str, f: impl FnOnce() -> T) -> T {
    let prev = SOURCE.with(|s| s.replace(Some(kind)));
    let out = f();
    SOURCE.with(|s| s.set(prev));
    out
}

/// 记一次访问
pub fn hit(dataset_id: &str, kind: &'static str) {
    if dataset_id.is_empty() {
        return;
    }
    let kind = SOURCE.with(|s| s.get()).unwrap_or(kind);
    let day = Local::now().format("%Y-%m-%d").to_string();
    if let Ok(mut p) = pending().lock() {
        *p.entry((day, dataset_id.to_string(), kind)).or_default() += 1;
    }
}

/// 把内存里的计数写入数据库（并清理过期记录）
pub fn flush(conn: &Connection) -> AppResult<usize> {
    let batch: Vec<(Key, i64)> = match pending().lock() {
        Ok(mut p) => p.drain().collect(),
        Err(_) => return Ok(0),
    };
    if batch.is_empty() {
        return Ok(0);
    }
    let n = batch.len();
    let r = (|| -> AppResult<()> {
        let mut stmt = conn.prepare(
            "INSERT INTO ledger_dataset_access (day, dataset_id, kind, hits) VALUES (CAST(? AS DATE), ?, ?, ?)
             ON CONFLICT (day, dataset_id, kind) DO UPDATE SET hits = ledger_dataset_access.hits + excluded.hits",
        )?;
        for ((day, id, kind), hits) in &batch {
            stmt.execute(params![day, id, kind, hits])?;
        }
        let cutoff = (Local::now().date_naive() - Duration::days(KEEP_DAYS)).format("%Y-%m-%d").to_string();
        conn.execute("DELETE FROM ledger_dataset_access WHERE day < CAST(? AS DATE)", params![cutoff])?;
        Ok(())
    })();
    if let Err(e) = r {
        // 写入失败时放回，下次再写
        if let Ok(mut p) = pending().lock() {
            for (k, v) in batch {
                *p.entry(k).or_default() += v;
            }
        }
        return Err(e);
    }
    Ok(n)
}

// ── 使用情况 ──────────────────────────────────────────

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkbookRef {
    pub id: String,
    pub name: String,
    pub updated_at: String,
    pub updated_by: Option<String>,
    /// 部件类型 → 个数（pivot / webexcel-chart / webexcel-slicer / webexcel-kpi / webexcel-table / webexcel-datatable / input / formula）
    pub parts: BTreeMap<String, usize>,
    /// 引用的列
    pub columns: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedRef {
    pub id: String,
    pub name: String,
    /// 补充说明（视图：左表 / 右表；SQL：来源 / 输出；连接：同步方式；推送：所在工作簿）
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessDay {
    pub day: String,
    pub hits: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    /// 最近 30 天（含今天），没有访问的日子为 0
    pub days: Vec<AccessDay>,
    /// 来源 → 30 天合计
    pub by_kind: BTreeMap<String, i64>,
    pub total: i64,
    /// 最近一次有访问的日期（90 天内）
    pub last_day: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetUsage {
    pub dataset_id: String,
    pub name: String,
    pub workbooks: Vec<WorkbookRef>,
    pub templates: Vec<NamedRef>,
    pub views: Vec<NamedRef>,
    pub sql_queries: Vec<NamedRef>,
    pub connectors: Vec<NamedRef>,
    pub reports: Vec<NamedRef>,
    /// 填报提交次数（未撤销的）
    pub writebacks: i64,
    pub access: Access,
}

fn gunzip(bytes: &[u8]) -> AppResult<Json> {
    let mut s = String::new();
    flate2::read::GzDecoder::new(bytes).read_to_string(&mut s)?;
    Ok(serde_json::from_str(&s)?)
}

/// 工作簿对数据集的引用：按 id 的部件 + 按名称的公式
fn workbook_ref(wb: &Json, id: &str, name: &str) -> Option<(BTreeMap<String, usize>, BTreeSet<String>)> {
    let c = crate::template::refs::collect(wb);
    let mut parts: BTreeMap<String, usize> = BTreeMap::new();
    let mut columns: BTreeSet<String> = BTreeSet::new();
    for key in [id.to_string(), format!("name:{name}")] {
        if let Some(u) = c.by_key.get(&key) {
            for (k, n) in &u.parts {
                *parts.entry(k.clone()).or_default() += n;
            }
            columns.extend(u.columns.iter().cloned());
        }
    }
    (!parts.is_empty()).then_some((parts, columns))
}

fn access(conn: &Connection, id: &str) -> AppResult<Access> {
    let today = Local::now().date_naive();
    let from = today - Duration::days(29);
    let mut stmt = conn.prepare("SELECT strftime(day, '%Y-%m-%d'), kind, hits FROM ledger_dataset_access WHERE dataset_id = ? AND day >= CAST(? AS DATE)")?;
    let rows = stmt
        .query_map(params![id, from.format("%Y-%m-%d").to_string()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    // 还没写入数据库的计数也算上
    let mut all = rows;
    if let Ok(p) = pending().lock() {
        for ((day, did, kind), n) in p.iter() {
            if did == id {
                all.push((day.clone(), kind.to_string(), *n));
            }
        }
    }
    let mut per_day: BTreeMap<String, i64> = BTreeMap::new();
    let mut by_kind: BTreeMap<String, i64> = BTreeMap::new();
    for (day, kind, n) in &all {
        *per_day.entry(day.clone()).or_default() += n;
        *by_kind.entry(kind.clone()).or_default() += n;
    }
    let days: Vec<AccessDay> = (0..30)
        .map(|i| {
            let d: NaiveDate = from + Duration::days(i);
            let key = d.format("%Y-%m-%d").to_string();
            AccessDay { hits: per_day.get(&key).copied().unwrap_or(0), day: key }
        })
        .collect();
    let last_day: Option<String> = conn
        .query_row("SELECT strftime(max(day), '%Y-%m-%d') FROM ledger_dataset_access WHERE dataset_id = ?", params![id], |r| r.get::<_, Option<String>>(0))?
        .into_iter()
        .chain(per_day.keys().last().cloned())
        .max();
    Ok(Access { total: by_kind.values().sum(), days, by_kind, last_day })
}

pub fn usage(conn: &Connection, dataset_id: &str) -> AppResult<DatasetUsage> {
    let catalog = crate::dataset::datasets(conn)?;
    let def = catalog.iter().find(|d| d.id == dataset_id).ok_or_else(|| AppError::bad("数据集不存在或在回收站"))?;
    let name = def.name.clone();

    // 工作簿（不含回收站）
    let mut workbooks = Vec::new();
    for meta in crate::workbook::list(conn)? {
        let Ok((_, gz)) = crate::workbook::content(conn, &meta.id) else { continue };
        let Ok(wb) = gunzip(&gz) else { continue };
        if let Some((parts, columns)) = workbook_ref(&wb, dataset_id, &name) {
            workbooks.push(WorkbookRef { id: meta.id, name: meta.name, updated_at: meta.updated_at, updated_by: meta.updated_by, parts, columns: columns.into_iter().collect() });
        }
    }
    let used_wb: BTreeSet<String> = workbooks.iter().map(|w| w.id.clone()).collect();

    // 模板：发布时引用的数据集（requirements 里的 key 为原数据集 ID）
    let mut templates = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, name, requirements FROM ledger_template ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        for (id, tname, req) in rows {
            let list: Vec<crate::template::TemplateDataset> = serde_json::from_str(&req).unwrap_or_default();
            if list.iter().any(|t| t.key == dataset_id) {
                templates.push(NamedRef { id, name: tname, role: "模板引用".into(), enabled: None });
            }
        }
    }

    // 关联视图
    let views = catalog
        .iter()
        .filter_map(|d| {
            let v = d.view.as_ref()?;
            let role = if v.left == dataset_id { "左表" } else if v.right == dataset_id { "右表" } else { return None };
            Some(NamedRef { id: d.id.clone(), name: d.name.clone(), role: role.into(), enabled: None })
        })
        .collect();

    // SQL 查询：输出到这个数据集，或在 SQL 里读它
    let mut sql_queries = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, name, sql, dataset_id FROM ledger_sql_query ORDER BY updated_at DESC")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        for (id, qname, sql, target) in rows {
            let reads = crate::sqlq::guard::check(conn, &sql, &|_| true).map(|c| c.tables.iter().any(|t| t == &name)).unwrap_or(false);
            let writes = target.as_deref() == Some(dataset_id);
            if reads || writes {
                let role = match (reads, writes) {
                    (true, true) => "读取并输出到它",
                    (false, true) => "结果保存到它",
                    _ => "从它读取",
                };
                sql_queries.push(NamedRef { id, name: qname, role: role.into(), enabled: None });
            }
        }
    }

    let connectors = crate::connector::targeting(conn, dataset_id)?
        .into_iter()
        .map(|(id, cname, mode, enabled)| NamedRef { id, name: cname, role: mode, enabled: Some(enabled) })
        .collect();

    // 定时推送：所在工作簿引用了这个数据集
    let reports = crate::report::list(conn)?
        .into_iter()
        .filter(|r| used_wb.contains(&r.workbook_id))
        .map(|r| NamedRef { id: r.id, name: r.name, role: format!("工作簿「{}」", r.workbook_name.unwrap_or_default()), enabled: Some(r.enabled) })
        .collect();

    let writebacks: i64 = conn.query_row("SELECT count(*) FROM ledger_writeback WHERE dataset_id = ? AND undone_at IS NULL", params![dataset_id], |r| r.get(0))?;

    Ok(DatasetUsage { dataset_id: dataset_id.into(), name, workbooks, templates, views, sql_queries, connectors, reports, writebacks, access: access(conn, dataset_id)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_flush_and_merge() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        // 用不会与其他测试冲突的 ID
        let id = "ds_usage_test";
        hit(id, PIVOT);
        hit(id, PIVOT);
        hit(id, BROWSE);
        // 未写入前也计入
        let a = access(&conn, id).unwrap();
        assert_eq!(a.total, 3);
        flush(&conn).unwrap();
        hit(id, PIVOT);
        flush(&conn).unwrap();
        let a = access(&conn, id).unwrap();
        assert_eq!(a.by_kind.get(PIVOT), Some(&3));
        assert_eq!(a.by_kind.get(BROWSE), Some(&1));
        assert_eq!(a.days.len(), 30);
        assert_eq!(a.days.last().unwrap().hits, 4);
        assert_eq!(a.last_day.as_deref(), Some(Local::now().format("%Y-%m-%d").to_string().as_str()));
    }

    #[test]
    fn workbook_refs_by_id_and_name() {
        let wb: Json = serde_json::json!({
            "sheetOrder": ["s1"],
            "sheets": {
                "s1": {
                    "name": "看板",
                    "custom": { "pivot": { "version": 1, "datasetId": "ds_a", "datasetName": "订单", "request": { "rows": [{ "column": "店铺" }], "columns": [], "values": [{ "column": "金额", "agg": "sum" }] } } },
                    "cellData": { "0": { "0": { "f": "=DSUMIFS(\"订单\",\"金额\")" }, "1": { "f": "=DSUMIFS(\"别的\",\"金额\")" } } }
                }
            },
            "resources": []
        });
        let (parts, cols) = workbook_ref(&wb, "ds_a", "订单").unwrap();
        assert_eq!(parts.get("pivot"), Some(&1));
        assert_eq!(parts.get("formula"), Some(&1));
        assert!(cols.contains("店铺") && cols.contains("金额"));
        assert!(workbook_ref(&wb, "ds_b", "无关").is_none());
    }
}
