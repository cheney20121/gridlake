//! MCP 服务：让外部 AI 助手（Claude Desktop、Cursor、Cherry Studio、Dify…）通过 Model Context Protocol 查询数据集与仪表盘
//!
//! - 传输：Streamable HTTP（POST /api/mcp，JSON-RPC 2.0，直接返回 JSON）；只支持本地 stdio 的客户端用
//!   `ledger-server mcp-bridge` 或 `npx mcp-remote` 转接
//! - 访问控制：必须带令牌（Authorization: Bearer）；没有任何有效令牌时整个接口不可用。令牌只保存 SHA-256 摘要，
//!   可限定能访问的数据集、是否允许 SQL；每次调用记审计日志
//! - 工具全部只读：数据集列表 / 结构 / 取值、汇总查询、两期对比、SQL、仪表盘列表与当前指标。
//!   新建透视表、写公式等会改工作簿的能力不对外开放（外部客户端没有人工确认环节）

pub mod bridge;

use std::collections::HashSet;
use std::io::Read;
use std::time::Instant;

use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

use crate::db::now_local;
use crate::error::{AppError, AppResult};

/// 支持的协议版本（新的在前）
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const KEEP_LOGS: i64 = 5000;

// ── 令牌 ──────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TokenView {
    pub id: String,
    pub name: String,
    /// 令牌开头几位（便于辨认，不能用于访问）
    pub prefix: String,
    pub allow_sql: bool,
    /// 可访问的数据集 ID；None 为全部
    pub datasets: Option<Vec<String>>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub calls: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInput {
    pub name: String,
    #[serde(default = "yes")]
    pub allow_sql: bool,
    /// None 或空：全部数据集
    #[serde(default)]
    pub datasets: Option<Vec<String>>,
    #[serde(default)]
    pub operator: Option<String>,
}

fn yes() -> bool {
    true
}

/// 当前请求的令牌（验证通过后）
#[derive(Clone)]
pub struct Caller {
    pub id: String,
    pub name: String,
    pub allow_sql: bool,
    pub scope: Option<HashSet<String>>,
}

fn digest(token: &str) -> String {
    let h = Sha256::digest(token.as_bytes());
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn new_token() -> String {
    let mut buf = [0u8; 24];
    OsRng.fill_bytes(&mut buf);
    format!("wxmcp_{}", buf.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn normalize_scope(conn: &Connection, ds: Option<Vec<String>>) -> AppResult<Option<Vec<String>>> {
    let Some(list) = ds.filter(|l| !l.is_empty()) else { return Ok(None) };
    let known: HashSet<String> = crate::dataset::datasets(conn)?.into_iter().map(|d| d.id).collect();
    if let Some(bad) = list.iter().find(|id| !known.contains(*id)) {
        return Err(AppError::bad(format!("数据集不存在：{bad}")));
    }
    Ok(Some(list))
}

pub fn list_tokens(conn: &Connection) -> AppResult<Vec<TokenView>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, t.prefix, t.allow_sql, t.datasets, t.created_by, strftime(t.created_at, '%Y-%m-%d %H:%M:%S'), strftime(t.last_used_at, '%Y-%m-%d %H:%M:%S'),
                (SELECT count(*) FROM ledger_mcp_log l WHERE l.token_id = t.id)
         FROM ledger_mcp_token t WHERE t.revoked_at IS NULL ORDER BY t.created_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TokenView {
            id: r.get(0)?,
            name: r.get(1)?,
            prefix: r.get(2)?,
            allow_sql: r.get(3)?,
            datasets: r.get::<_, Option<String>>(4)?.and_then(|s| serde_json::from_str(&s).ok()),
            created_by: r.get(5)?,
            created_at: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            last_used_at: r.get(7)?,
            calls: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 新建令牌；返回（令牌信息，令牌明文——只在此时出现一次）
pub fn create_token(conn: &Connection, input: TokenInput) -> AppResult<(TokenView, String)> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::bad("请填写令牌名称（不超过 40 个字符），如「财务部 Cursor」"));
    }
    let datasets = normalize_scope(conn, input.datasets)?;
    let token = new_token();
    let id = format!("mt_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    conn.execute(
        "INSERT INTO ledger_mcp_token (id, name, prefix, token_hash, allow_sql, datasets, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params![id, name, &token[..12], digest(&token), input.allow_sql, datasets.as_ref().map(serde_json::to_string).transpose()?, input.operator, now_local()],
    )?;
    let view = list_tokens(conn)?.into_iter().find(|t| t.id == id).ok_or_else(|| AppError::bad("令牌保存失败"))?;
    Ok((view, token))
}

pub fn update_token(conn: &Connection, id: &str, input: TokenInput) -> AppResult<()> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::bad("请填写令牌名称"));
    }
    let datasets = normalize_scope(conn, input.datasets)?;
    let n = conn.execute(
        "UPDATE ledger_mcp_token SET name = ?, allow_sql = ?, datasets = ? WHERE id = ? AND revoked_at IS NULL",
        params![name, input.allow_sql, datasets.as_ref().map(serde_json::to_string).transpose()?, id],
    )?;
    if n == 0 {
        return Err(AppError::bad("令牌不存在"));
    }
    Ok(())
}

/// 吊销（立即失效；审计日志保留）
pub fn revoke_token(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute("UPDATE ledger_mcp_token SET revoked_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now_local(), id])?;
    Ok(())
}

/// 验证 Authorization 头
pub fn authenticate(conn: &Connection, header: Option<&str>) -> AppResult<Caller> {
    let token = header.and_then(|h| h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))).map(str::trim).filter(|t| !t.is_empty());
    let Some(token) = token else {
        return Err(AppError::Forbidden("缺少访问令牌：请在请求头加 Authorization: Bearer <令牌>（在平台「MCP 接入」中创建）".into()));
    };
    let row: Option<(String, String, bool, Option<String>)> = conn
        .query_row(
            "SELECT id, name, allow_sql, datasets FROM ledger_mcp_token WHERE token_hash = ? AND revoked_at IS NULL",
            params![digest(token)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    let Some((id, name, allow_sql, datasets)) = row else {
        return Err(AppError::Forbidden("访问令牌无效或已吊销".into()));
    };
    let _ = conn.execute("UPDATE ledger_mcp_token SET last_used_at = CAST(? AS TIMESTAMP) WHERE id = ?", params![now_local(), id]);
    let scope = datasets.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).map(|v| v.into_iter().collect());
    Ok(Caller { id, name, allow_sql, scope })
}

// ── 审计日志 ──────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogView {
    pub token_name: String,
    pub method: String,
    pub tool: Option<String>,
    pub arguments: Option<String>,
    pub ok: bool,
    pub message: Option<String>,
    pub elapsed_ms: i64,
    pub client: Option<String>,
    pub created_at: String,
}

fn log(conn: &Connection, caller: &Caller, method: &str, tool: Option<&str>, args: Option<&Json>, ok: bool, message: Option<&str>, elapsed_ms: i64, client: Option<&str>) {
    let args = args.map(|a| a.to_string().chars().take(2000).collect::<String>());
    let r = conn.execute(
        "INSERT INTO ledger_mcp_log (token_id, token_name, method, tool, arguments, ok, message, elapsed_ms, client, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params![caller.id, caller.name, method, tool, args, ok, message.map(|m| m.chars().take(500).collect::<String>()), elapsed_ms, client, now_local()],
    );
    if let Err(e) = r {
        tracing::warn!("写入 MCP 审计日志失败：{e}");
        return;
    }
    let _ = conn.execute(
        "DELETE FROM ledger_mcp_log WHERE created_at < (SELECT min(created_at) FROM (SELECT created_at FROM ledger_mcp_log ORDER BY created_at DESC LIMIT ?))",
        params![KEEP_LOGS],
    );
}

pub fn logs(conn: &Connection, limit: i64) -> AppResult<Vec<LogView>> {
    let mut stmt = conn.prepare(
        "SELECT token_name, method, tool, arguments, ok, message, elapsed_ms, client, strftime(created_at, '%Y-%m-%d %H:%M:%S') FROM ledger_mcp_log ORDER BY created_at DESC, rowid DESC LIMIT ?",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok(LogView {
            token_name: r.get(0)?,
            method: r.get(1)?,
            tool: r.get(2)?,
            arguments: r.get(3)?,
            ok: r.get(4)?,
            message: r.get(5)?,
            elapsed_ms: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
            client: r.get(7)?,
            created_at: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ── 工具 ──────────────────────────────────────────────

/// 复用 AI 助手的只读工具
const SHARED_TOOLS: [&str; 6] = ["describe_dataset", "column_values", "query_data", "compare_periods", "find_anomalies", "run_sql"];

fn tool_list(caller: &Caller) -> Vec<Json> {
    let mut out = vec![json!({
        "name": "list_datasets",
        "title": "数据集列表",
        "description": "列出可以查询的数据集（名称、行数、列及类型）。先调用它了解有哪些数据，再用 query_data / run_sql 查询",
        "inputSchema": { "type": "object", "properties": {} },
        "annotations": { "readOnlyHint": true }
    })];
    for t in crate::ai::tools::definitions().as_array().cloned().unwrap_or_default() {
        let f = &t["function"];
        let name = f["name"].as_str().unwrap_or("");
        if !SHARED_TOOLS.contains(&name) || (name == "run_sql" && !caller.allow_sql) {
            continue;
        }
        let title = match name {
            "describe_dataset" => "数据集结构",
            "column_values" => "列取值",
            "query_data" => "汇总查询",
            "compare_periods" => "两期对比归因",
            "find_anomalies" => "异常发现",
            _ => "SQL 查询",
        };
        out.push(json!({ "name": name, "title": title, "description": f["description"], "inputSchema": f["parameters"], "annotations": { "readOnlyHint": true } }));
    }
    out.push(json!({
        "name": "list_dashboards",
        "title": "仪表盘列表",
        "description": "列出工作簿中的仪表盘页（含指标卡、明细卡、图表的工作表）",
        "inputSchema": { "type": "object", "properties": {} },
        "annotations": { "readOnlyHint": true }
    }));
    out.push(json!({
        "name": "read_dashboard",
        "title": "读取仪表盘",
        "description": "读取某个仪表盘页的当前指标：指标卡（数值与同比 / 环比）、明细卡排行、图表数据，按仪表盘上保存的切片器选择计算。用于回答「今天的经营日报 / 老板看板现在怎么样」",
        "inputSchema": {
            "type": "object",
            "properties": {
                "workbook": { "type": "string", "description": "工作簿名称或 ID" },
                "sheet": { "type": "string", "description": "仪表盘页名称或 ID（不填时取第一个仪表盘页）" }
            },
            "required": ["workbook"]
        },
        "annotations": { "readOnlyHint": true }
    }));
    out
}

fn in_scope(caller: &Caller, id: &str) -> bool {
    caller.scope.as_ref().is_none_or(|s| s.contains(id))
}

fn list_datasets(conn: &Connection, caller: &Caller) -> AppResult<String> {
    let mut out = String::new();
    for d in crate::dataset::list(conn)?.into_iter().filter(|d| in_scope(caller, &d.def.id)) {
        let cols: Vec<String> = d
            .def
            .columns
            .iter()
            .map(|c| format!("{}({})", c.title, match c.kind { "money" => "金额", "number" => "数值", "datetime" => "日期时间", "id" => "编号", _ => "文本" }))
            .collect();
        out.push_str(&format!("- 「{}」{} 行：{}\n", d.def.name, d.row_count, cols.join("、")));
    }
    if out.is_empty() {
        out = "（没有可访问的数据集）".into();
    }
    Ok(format!("可查询的数据集：\n{out}\n日期时间列以文本保存（如 2026-09-15 10:00:00），按月可用 left(列,7)。"))
}

fn gunzip(bytes: &[u8]) -> AppResult<Json> {
    let mut s = String::new();
    flate2::read::GzDecoder::new(bytes).read_to_string(&mut s)?;
    Ok(serde_json::from_str(&s)?)
}

/// 工作簿引用的数据集都在授权范围内才可读取
fn workbook_allowed(conn: &Connection, caller: &Caller, workbook_id: &str) -> AppResult<bool> {
    if caller.scope.is_none() {
        return Ok(true);
    }
    let (_, bytes) = crate::workbook::content(conn, workbook_id)?;
    let wb = gunzip(&bytes)?;
    let names: std::collections::HashMap<String, String> = crate::dataset::datasets(conn)?.into_iter().map(|d| (d.name, d.id)).collect();
    let refs = crate::template::refs::collect(&wb);
    Ok(refs.by_key.keys().all(|k| match k.strip_prefix("name:") {
        Some(n) => names.get(n).is_some_and(|id| in_scope(caller, id)),
        None => in_scope(caller, k),
    }))
}

fn list_dashboards(conn: &Connection, caller: &Caller) -> AppResult<String> {
    let mut out = String::new();
    for w in crate::workbook::list(conn)? {
        let sheets: Vec<(String, String, usize)> = crate::report::content::sheets_of(conn, &w.id)?.into_iter().filter(|s| s.2 > 0).collect();
        if sheets.is_empty() || !workbook_allowed(conn, caller, &w.id)? {
            continue;
        }
        for (_, name, parts) in sheets {
            out.push_str(&format!("- 工作簿「{}」/ 仪表盘页「{name}」（{parts} 个部件，更新于 {}）\n", w.name, w.updated_at.get(..16).unwrap_or(&w.updated_at)));
        }
    }
    Ok(if out.is_empty() { "（没有可访问的仪表盘）".into() } else { format!("仪表盘：\n{out}") })
}

#[derive(Deserialize)]
struct DashboardArg {
    workbook: String,
    #[serde(default)]
    sheet: Option<String>,
}

fn read_dashboard(conn: &Connection, caller: &Caller, args: &Json) -> AppResult<String> {
    let a: DashboardArg = serde_json::from_value(args.clone()).map_err(|e| AppError::bad(format!("参数格式不正确：{e}")))?;
    let wb = crate::workbook::list(conn)?
        .into_iter()
        .find(|w| w.id == a.workbook || w.name == a.workbook)
        .ok_or_else(|| AppError::bad(format!("没有工作簿「{}」，可先调用 list_dashboards", a.workbook)))?;
    if !workbook_allowed(conn, caller, &wb.id)? {
        return Err(AppError::bad("这个工作簿用到了令牌无权访问的数据集"));
    }
    let sheets: Vec<(String, String, usize)> = crate::report::content::sheets_of(conn, &wb.id)?.into_iter().filter(|s| s.2 > 0).collect();
    let sheet = match &a.sheet {
        Some(s) => sheets.iter().find(|x| &x.0 == s || &x.1 == s).ok_or_else(|| AppError::bad(format!("工作簿「{}」没有仪表盘页「{s}」", wb.name)))?,
        None => sheets.first().ok_or_else(|| AppError::bad(format!("工作簿「{}」没有仪表盘页", wb.name)))?,
    };
    let c = crate::report::content::build(conn, &wb.id, &sheet.0, crate::db::today_local())?;
    Ok(render_dashboard(&wb.name, &c))
}

/// 仪表盘内容 → Markdown（指标、表格）
fn render_dashboard(workbook: &str, c: &crate::report::content::ReportContent) -> String {
    use crate::report::content::{Block, compact, percent};
    let mut s = format!("# {workbook} / {}\n计算时间 {}", c.sheet_name, c.generated_at);
    if !c.filters.is_empty() {
        s.push_str(&format!("；筛选：{}", c.filters.join("；")));
    }
    s.push_str("\n\n");
    for b in &c.blocks {
        match b {
            Block::Kpi(k) => {
                let delta = k.delta.map(|d| format!("，{} {}", k.delta_label, percent(Some(d)))).unwrap_or_default();
                let period = k.period.as_ref().map(|p| format!("（{p}）")).unwrap_or_default();
                let exact = k.value.map(|v| format!("（{v:.2}）")).unwrap_or_default();
                s.push_str(&format!("- **{}**{period}：{}{exact}{delta}\n", k.title, compact(k.value, k.money)));
            }
            Block::Table(t) => {
                s.push_str(&format!("\n**{}**\n\n| {} |\n|{}|\n", t.title, t.header.join(" | "), t.header.iter().map(|_| "---").collect::<Vec<_>>().join("|")));
                for r in t.rows.iter().chain(t.total.iter()) {
                    s.push_str(&format!("| {} |\n", r.join(" | ")));
                }
            }
            Block::Error { title, message } => s.push_str(&format!("- **{title}**：无法计算（{message}）\n")),
        }
    }
    s
}

/// 执行工具：返回（文本，是否出错）
fn call_tool(conn: &Connection, caller: &Caller, name: &str, args: &Json) -> (String, bool) {
    let r: AppResult<String> = match name {
        "list_datasets" => list_datasets(conn, caller),
        "list_dashboards" => list_dashboards(conn, caller),
        "read_dashboard" => read_dashboard(conn, caller, args),
        "run_sql" if !caller.allow_sql => Err(AppError::bad("这个令牌不允许执行 SQL")),
        n if SHARED_TOOLS.contains(&n) => {
            let o = crate::usage::with_source(crate::usage::MCP, || crate::ai::tools::execute_scoped(conn, n, &args.to_string(), caller.scope.as_ref()));
            if o.ok { Ok(o.content) } else { Err(AppError::bad(o.content.trim_start_matches("错误：").to_string())) }
        }
        other => Err(AppError::bad(format!("没有工具 {other}"))),
    };
    match r {
        Ok(t) => (t, false),
        Err(e) => (e.message(), true),
    }
}

// ── JSON-RPC ──────────────────────────────────────────

fn rpc_error(id: Json, code: i64, message: &str) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn rpc_result(id: Json, result: Json) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

const INSTRUCTIONS: &str = "WebExcel 电商财务数据平台（只读）。先用 list_datasets 了解有哪些数据集与列，再用 query_data（分组汇总）、compare_periods（两期对比归因）或 run_sql（复杂计算）查询；用 list_dashboards / read_dashboard 读取老板看板等仪表盘的当前指标。所有数字都应来自工具结果。";

/// 处理一条 JSON-RPC 消息；通知（没有 id）返回 None
pub fn handle(conn: &Connection, caller: &Caller, msg: &Json, client: Option<&str>) -> Option<Json> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
    if msg.get("jsonrpc").and_then(Json::as_str) != Some("2.0") || method.is_empty() {
        // 客户端发来的响应（如对服务端请求的回复）不需要处理
        if msg.get("result").is_some() || msg.get("error").is_some() {
            return None;
        }
        return Some(rpc_error(id.unwrap_or(Json::Null), -32600, "无效的 JSON-RPC 请求"));
    }
    let id = id?; // 通知
    let params = msg.get("params").cloned().unwrap_or(Json::Null);
    let start = Instant::now();
    let resp = match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Json::as_str).unwrap_or("");
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == requested).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
            log(conn, caller, method, None, params.get("clientInfo"), true, None, 0, client);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "webexcel", "title": "WebExcel 数据平台", "version": crate::admin::VERSION },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": tool_list(caller) })),
        "tools/call" => {
            let name = params.get("name").and_then(Json::as_str).unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let (text, is_error) = call_tool(conn, caller, &name, &args);
            let ms = start.elapsed().as_millis() as i64;
            log(conn, caller, method, Some(&name), Some(&args), !is_error, if is_error { Some(&text) } else { None }, ms, client);
            rpc_result(id, json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        // 不支持的可选能力返回空列表，避免部分客户端报错
        "resources/list" => rpc_result(id, json!({ "resources": [] })),
        "resources/templates/list" => rpc_result(id, json!({ "resourceTemplates": [] })),
        "prompts/list" => rpc_result(id, json!({ "prompts": [] })),
        "logging/setLevel" => rpc_result(id, json!({})),
        other => rpc_error(id, -32601, &format!("不支持的方法：{other}")),
    };
    Some(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(sql: bool) -> Caller {
        Caller { id: "t".into(), name: "测试".into(), allow_sql: sql, scope: None }
    }

    #[test]
    fn tokens_and_tools() {
        let t = new_token();
        assert!(t.starts_with("wxmcp_") && t.len() == 54);
        assert_ne!(new_token(), t);
        assert_eq!(digest("abc").len(), 64);
        let names = |c: &Caller| tool_list(c).iter().map(|t| t["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let all = names(&caller(true));
        assert!(all.contains(&"run_sql".to_string()) && all.contains(&"read_dashboard".to_string()) && all.contains(&"list_datasets".to_string()));
        assert!(!all.iter().any(|n| n.starts_with("create_") || n == "write_formula" || n == "add_calc_column"));
        assert!(!names(&caller(false)).contains(&"run_sql".to_string()));
        for t in tool_list(&caller(true)) {
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }
}
