//! OpenAI 兼容的对话接口（/chat/completions + tools）：客户内网的 DeepSeek、Qwen（vLLM / Ollama / Xinference），或云端模型
//!
//! 配置（环境变量）：
//! - LEDGER_AI_BASE_URL：接口地址，如 `https://api.deepseek.com/v1`、`http://10.0.0.8:8000/v1`（不含 /chat/completions）
//! - LEDGER_AI_API_KEY：密钥（内网模型可不设）
//! - LEDGER_AI_MODEL：模型名，如 `deepseek-chat`、`qwen2.5-72b-instruct`
//! - LEDGER_AI_STREAM：默认流式返回；个别内网部署不支持「流式 + 工具调用」时设为 `0`
//! - LEDGER_AI_TIMEOUT_SECS：单次请求超时，默认 180
//! - LEDGER_AI_TEMPERATURE：默认 0.2（分析类任务要稳定）

use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct Config {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub stream: bool,
    pub timeout: Duration,
    pub temperature: f64,
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Option<Self> {
        Some(Self {
            base_url: env("LEDGER_AI_BASE_URL")?.trim_end_matches('/').trim_end_matches("/chat/completions").to_string(),
            model: env("LEDGER_AI_MODEL")?,
            api_key: env("LEDGER_AI_API_KEY"),
            stream: env("LEDGER_AI_STREAM").is_none_or(|v| v != "0" && v != "off"),
            timeout: Duration::from_secs(env("LEDGER_AI_TIMEOUT_SECS").and_then(|v| v.parse().ok()).unwrap_or(180)),
            temperature: env("LEDGER_AI_TEMPERATURE").and_then(|v| v.parse().ok()).unwrap_or(0.2),
        })
    }

    /// 给界面显示的模型说明（不含密钥）
    pub fn describe(&self) -> String {
        let host = reqwest::Url::parse(&self.base_url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default();
        format!("{} @ {host}", self.model)
    }
}

/// 一条对话消息（OpenAI 格式）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function_type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FunctionCall {
    pub name: String,
    /// JSON 字符串
    #[serde(default)]
    pub arguments: String,
}

fn function_type() -> String {
    "function".into()
}

impl Message {
    pub fn system(s: impl Into<String>) -> Self {
        Self { role: "system".into(), content: Some(s.into()), ..Default::default() }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self { role: "user".into(), content: Some(s.into()), ..Default::default() }
    }
    pub fn tool(id: &str, s: impl Into<String>) -> Self {
        Self { role: "tool".into(), content: Some(s.into()), tool_call_id: Some(id.into()), ..Default::default() }
    }
}

/// 模型的一轮回复
#[derive(Debug, Default)]
pub struct Turn {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

fn client(cfg: &Config) -> AppResult<reqwest::Client> {
    reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).timeout(cfg.timeout).build().map_err(|e| AppError::bad(format!("无法创建网络客户端：{e}")))
}

fn error_text(status: reqwest::StatusCode, body: &str) -> String {
    let msg = serde_json::from_str::<Json>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").or_else(|| v.get("message")).or_else(|| v.get("detail")).and_then(Json::as_str).map(String::from))
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status.as_u16() {
        401 | 403 => format!("模型接口拒绝访问（{status}），请检查 LEDGER_AI_API_KEY：{msg}"),
        404 => format!("模型接口地址或模型名不正确（{status}），请检查 LEDGER_AI_BASE_URL / LEDGER_AI_MODEL：{msg}"),
        429 => format!("模型接口限流（{status}），请稍后再试：{msg}"),
        _ => format!("模型接口返回错误（{status}）：{msg}"),
    }
}

/// 调用一轮；on_text 收到增量文本（流式时逐段，非流式时一次）
pub async fn chat(cfg: &Config, messages: &[Message], tools: &Json, on_text: &mut (dyn FnMut(&str) + Send)) -> AppResult<Turn> {
    let mut body = json!({
        "model": cfg.model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "temperature": cfg.temperature,
        "stream": cfg.stream,
    });
    if !cfg.stream {
        body.as_object_mut().expect("object").remove("stream");
    }
    // 不带工具的调用（如整理对话摘要）：部分接口不接受空的 tools
    if tools.as_array().is_some_and(|a| a.is_empty()) {
        let o = body.as_object_mut().expect("object");
        o.remove("tools");
        o.remove("tool_choice");
    }
    let mut req = client(cfg)?.post(format!("{}/chat/completions", cfg.base_url)).json(&body);
    if let Some(k) = &cfg.api_key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() {
            AppError::bad("模型接口响应超时")
        } else {
            AppError::bad(format!("无法连接模型接口（{}）：{e}", cfg.base_url))
        }
    })?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::bad(error_text(status, &text)));
    }
    let is_sse = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("event-stream"));
    if !is_sse {
        let v: Json = resp.json().await.map_err(|e| AppError::bad(format!("模型接口返回的不是 JSON：{e}")))?;
        let msg: Message = serde_json::from_value(v.pointer("/choices/0/message").cloned().unwrap_or(Json::Null)).map_err(|_| AppError::bad("模型接口返回格式不正确（缺少 choices[0].message）"))?;
        let content = msg.content.unwrap_or_default();
        if !content.is_empty() {
            on_text(&content);
        }
        return Ok(Turn { content, tool_calls: msg.tool_calls });
    }
    parse_stream(resp, on_text).await
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

async fn parse_stream(resp: reqwest::Response, on_text: &mut (dyn FnMut(&str) + Send)) -> AppResult<Turn> {
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut turn = Turn::default();
    let mut calls: Vec<PartialCall> = Vec::new();
    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AppError::bad(format!("读取模型回复中断：{e}")))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break 'outer;
            }
            let Ok(v) = serde_json::from_str::<Json>(data) else { continue };
            if let Some(err) = v.get("error") {
                return Err(AppError::bad(format!("模型接口返回错误：{}", err.get("message").and_then(Json::as_str).unwrap_or(&err.to_string()))));
            }
            let Some(delta) = v.pointer("/choices/0/delta") else { continue };
            if let Some(t) = delta.get("content").and_then(Json::as_str).filter(|t| !t.is_empty()) {
                turn.content.push_str(t);
                on_text(t);
            }
            if let Some(tcs) = delta.get("tool_calls").and_then(Json::as_array) {
                for tc in tcs {
                    let idx = tc.get("index").and_then(Json::as_u64).unwrap_or(calls.len() as u64) as usize;
                    while calls.len() <= idx {
                        calls.push(PartialCall::default());
                    }
                    let c = &mut calls[idx];
                    if let Some(id) = tc.get("id").and_then(Json::as_str).filter(|s| !s.is_empty()) {
                        c.id = id.to_string();
                    }
                    if let Some(n) = tc.pointer("/function/name").and_then(Json::as_str) {
                        c.name.push_str(n);
                    }
                    if let Some(a) = tc.pointer("/function/arguments").and_then(Json::as_str) {
                        c.arguments.push_str(a);
                    }
                }
            }
        }
    }
    turn.tool_calls = calls
        .into_iter()
        .enumerate()
        .filter(|(_, c)| !c.name.is_empty())
        .map(|(i, c)| ToolCall {
            id: if c.id.is_empty() { format!("call_{i}") } else { c.id },
            kind: "function".into(),
            function: FunctionCall { name: c.name, arguments: if c.arguments.trim().is_empty() { "{}".into() } else { c.arguments } },
        })
        .collect();
    Ok(turn)
}
