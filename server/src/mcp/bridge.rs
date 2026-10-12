//! `gridlake mcp-bridge`：stdio ↔ HTTP 转接，给只支持本地 stdio 的 MCP 客户端（如 Claude Desktop）使用
//!
//!   gridlake mcp-bridge --url http://服务器:8787/api/mcp --token wxmcp_xxx
//!   （或环境变量 LEDGER_MCP_URL / LEDGER_MCP_TOKEN）
//!
//! 每行一条 JSON-RPC 消息：转发到服务端，把响应写回 stdout（一行一条）；日志只写 stderr。
//! 连接失败时对带 id 的请求回一个 JSON-RPC 错误，避免客户端一直等待

use std::time::Duration;

use serde_json::{Value as Json, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let url = arg(args, "--url").or_else(|| std::env::var("LEDGER_MCP_URL").ok()).filter(|s| !s.trim().is_empty());
    let token = arg(args, "--token").or_else(|| std::env::var("LEDGER_MCP_TOKEN").ok()).filter(|s| !s.trim().is_empty());
    let (Some(url), Some(token)) = (url, token) else {
        anyhow::bail!("用法：gridlake mcp-bridge --url http://服务器:8787/api/mcp --token <令牌>（或设置 LEDGER_MCP_URL / LEDGER_MCP_TOKEN）");
    };
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(bridge(url, token))
}

async fn bridge(url: String, token: String) -> anyhow::Result<()> {
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(600)).build()?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    eprintln!("[gridlake mcp-bridge] 转接到 {url}");
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Json>(line).ok().and_then(|v| v.get("id").cloned());
        let replies = match forward(&client, &url, &token, line).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[gridlake mcp-bridge] {e}");
                match &id {
                    Some(id) => vec![json!({ "jsonrpc": "2.0", "id": id.clone(), "error": { "code": -32603, "message": format!("无法连接 Gridlake 服务：{e}") } }).to_string()],
                    None => vec![],
                }
            }
        };
        // 鉴权失败等错误的 id 为 null：补上请求的 id，客户端才能对应到自己的请求
        let replies: Vec<String> = replies
            .into_iter()
            .map(|r| match (serde_json::from_str::<Json>(&r), &id) {
                (Ok(mut v), Some(req_id)) if v.get("id").is_some_and(Json::is_null) => {
                    v["id"] = req_id.clone();
                    v.to_string()
                }
                _ => r,
            })
            .collect();
        for r in replies {
            out.write_all(r.as_bytes()).await?;
            out.write_all(b"\n").await?;
        }
        out.flush().await?;
    }
    Ok(())
}

/// 转发一条消息；返回要写回的响应（一行一条）
async fn forward(client: &reqwest::Client, url: &str, token: &str, body: &str) -> anyhow::Result<Vec<String>> {
    let resp = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
        .bearer_auth(token)
        .body(body.to_string())
        .send()
        .await?;
    let status = resp.status();
    if status == reqwest::StatusCode::ACCEPTED {
        return Ok(vec![]);
    }
    let sse = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("event-stream"));
    let text = resp.text().await?;
    if sse {
        return Ok(text.lines().filter_map(|l| l.strip_prefix("data:")).map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect());
    }
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    // 服务端返回的 JSON（含 401 时的 JSON-RPC 错误）原样转交；压成一行
    match serde_json::from_str::<Json>(&text) {
        Ok(v) => Ok(vec![v.to_string()]),
        Err(_) => anyhow::bail!("服务端返回 {status}：{}", text.chars().take(200).collect::<String>()),
    }
}
