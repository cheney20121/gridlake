//! 接口错误信息的多语言
//!
//! 服务端的错误信息是中文模板（`AppError::bad(format!("找不到「{x}」", ..))`）。请求带了英文语言设置时，
//! 在生成错误响应的统一位置（`AppError::into_response`）把中文信息按模板匹配成英文：
//!
//! - `msg_en.json` 是「中文模板 → 英文模板」，占位符统一写成 `{0}` `{1}`…（按在中文模板里出现的顺序），
//!   由 `scripts/extract-messages.py` 从源码提取，缺英文的由 `cargo test` 里的覆盖检查报告；
//! - 运行时把消息按模板里的字面片段切开，取出被替换进去的值（值本身如果也是中文信息，递归翻译），再套进英文模板；
//! - 语言来自请求头 `x-lang` 或 cookie `gridlake-lang`（前端每个页面都会写）；都没有时是中文。脚本调用不受影响。
//!
//! 匹配不上的消息（动态拼出来、没收录）原样返回中文，不会出错。

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;

tokio::task_local! {
    /// 当前请求是否要英文
    static ENGLISH: bool;
}

const CATALOG_JSON: &str = include_str!("msg_en.json");

struct Pattern {
    /// 模板按占位符切开后的字面片段，长度 = 占位符个数 + 1
    parts: Vec<String>,
    english: String,
    /// 字面字数：越长越具体，越先尝试
    weight: usize,
}

struct Catalog {
    exact: HashMap<String, String>,
    patterns: Vec<Pattern>,
}

enum Tok {
    Lit(String),
    /// `{0}` `{12}`：第几个被替换进去的值
    Ph(usize),
}

/// 把模板切成字面片段和占位符；`{` `}` 只有包着纯数字时才是占位符，其余（如 `{page}`）都是字面量
fn tokenize(tpl: &str) -> Vec<Tok> {
    let chars: Vec<char> = tpl.chars().collect();
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{'
            && let Some(len) = chars[i + 1..].iter().position(|&c| c == '}')
            && len > 0
            && chars[i + 1..i + 1 + len].iter().all(char::is_ascii_digit)
            && let Ok(n) = chars[i + 1..i + 1 + len].iter().collect::<String>().parse()
        {
            if !lit.is_empty() {
                out.push(Tok::Lit(std::mem::take(&mut lit)));
            }
            out.push(Tok::Ph(n));
            i += len + 2;
            continue;
        }
        lit.push(chars[i]);
        i += 1;
    }
    if !lit.is_empty() {
        out.push(Tok::Lit(lit));
    }
    out
}

/// 占位符把模板切成的字面片段，长度 = 占位符个数 + 1（两头或相邻占位符之间为空串）
fn split_placeholders(tpl: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    for t in tokenize(tpl) {
        match t {
            Tok::Lit(l) => parts.last_mut().expect("parts 非空").push_str(&l),
            Tok::Ph(_) => parts.push(String::new()),
        }
    }
    parts
}

#[cfg(test)]
fn placeholder_ids(tpl: &str) -> Vec<usize> {
    let mut ids: Vec<usize> = tokenize(tpl).into_iter().filter_map(|t| if let Tok::Ph(n) = t { Some(n) } else { None }).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let raw: HashMap<String, String> = serde_json::from_str(CATALOG_JSON).unwrap_or_default();
        let mut exact = HashMap::new();
        let mut patterns = Vec::new();
        for (zh, en) in raw {
            let parts = split_placeholders(&zh);
            if parts.len() == 1 {
                exact.insert(zh, en);
            } else {
                let weight = parts.iter().map(|p| p.chars().count()).sum();
                patterns.push(Pattern { parts, english: en, weight });
            }
        }
        patterns.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.parts.cmp(&b.parts)));
        Catalog { exact, patterns }
    })
}

/// 按字面片段匹配，返回被替换进去的各个值
fn capture<'a>(parts: &[String], msg: &'a str) -> Option<Vec<&'a str>> {
    let first = &parts[0];
    let last = parts.last()?;
    if !msg.starts_with(first.as_str()) || msg.len() < first.len() + last.len() || !msg.ends_with(last.as_str()) {
        return None;
    }
    let end = msg.len() - last.len();
    let mut pos = first.len();
    let mut caps = Vec::with_capacity(parts.len() - 1);
    for lit in &parts[1..parts.len() - 1] {
        let rel = if lit.is_empty() { Some(0) } else { msg.get(pos..end)?.find(lit.as_str()) };
        let at = pos + rel?;
        caps.push(msg.get(pos..at)?);
        pos = at + lit.len();
    }
    caps.push(msg.get(pos..end)?);
    Some(caps)
}

fn fill(english: &str, caps: &[String]) -> String {
    let mut out = String::with_capacity(english.len() + 16);
    for t in tokenize(english) {
        match t {
            Tok::Lit(l) => out.push_str(&l),
            Tok::Ph(n) => match caps.get(n) {
                Some(v) => out.push_str(v),
                None => out.push_str(&format!("{{{n}}}")),
            },
        }
    }
    out
}

fn has_han(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

fn translate_depth(msg: &str, depth: u8) -> Option<String> {
    if !has_han(msg) {
        return None;
    }
    let cat = catalog();
    if let Some(en) = cat.exact.get(msg) {
        return Some(en.clone());
    }
    if depth == 0 {
        return None;
    }
    for p in &cat.patterns {
        if let Some(caps) = capture(&p.parts, msg) {
            // 被替换进去的值本身可能是另一条中文信息（如「导入失败：{原因}」），一并翻译
            let caps: Vec<String> = caps.iter().map(|c| translate_depth(c, depth - 1).unwrap_or_else(|| (*c).to_string())).collect();
            return Some(fill(&p.english, &caps));
        }
    }
    None
}

/// 把一条中文错误信息译成英文；没有收录的返回 None
pub fn to_english(msg: &str) -> Option<String> {
    translate_depth(msg, 3)
}

/// 当前请求要英文时翻译错误信息，否则原样返回
pub fn localize(msg: String) -> String {
    if ENGLISH.try_with(|e| *e).unwrap_or(false)
        && let Some(en) = to_english(&msg)
    {
        return en;
    }
    msg
}

fn wants_english(headers: &HeaderMap) -> bool {
    let from_header = headers.get("x-lang").and_then(|v| v.to_str().ok()).map(str::to_string);
    let from_cookie = || {
        headers
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|c| c.split(';'))
            .filter_map(|kv| kv.trim().strip_prefix("gridlake-lang="))
            .map(str::to_string)
            .next()
    };
    from_header.or_else(from_cookie).is_some_and(|v| v.to_ascii_lowercase().starts_with("en"))
}

/// 中间件：记下这个请求的语言，供生成错误响应时使用
pub async fn middleware(req: Request, next: Next) -> Response {
    let english = wants_english(req.headers());
    ENGLISH.scope(english, next.run(req)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_pattern_messages_translate() {
        assert_eq!(to_english("请先登录").as_deref(), Some("Please sign in first"));
        assert_eq!(to_english("找不到工作簿「月报」").as_deref(), Some("Workbook \"月报\" not found"));
        assert_eq!(
            to_english("有 3 个名为「月报」的工作簿，请改用工作簿 ID（如 wb_1）").as_deref(),
            Some("There are 3 workbooks named \"月报\"; use the workbook ID instead (e.g. wb_1)")
        );
    }

    #[test]
    fn nested_messages_translate_recursively() {
        assert_eq!(to_english("服务内部错误：请先登录").as_deref(), Some("Internal server error: Please sign in first"));
    }

    #[test]
    fn more_specific_pattern_wins() {
        assert_eq!(to_english("下载失败：服务器返回 404").as_deref(), Some("Download failed: the server returned 404"));
        assert_eq!(to_english("下载失败：连接被重置").as_deref(), Some("Download failed: 连接被重置"));
    }

    #[test]
    fn unknown_messages_and_literal_braces() {
        assert_eq!(to_english("完全没收录的一句话"), None);
        assert_eq!(to_english("hello"), None);
        // 字面量花括号不是占位符
        assert_eq!(to_english("页码分页需要在地址或请求体里写 {page}").as_deref(), Some("Page-number paging needs {page} in the URL or request body"));
    }

    #[test]
    fn language_comes_from_header_or_cookie() {
        let mut h = HeaderMap::new();
        assert!(!wants_english(&h));
        h.insert("cookie", "a=1; gridlake-lang=en-US; b=2".parse().unwrap());
        assert!(wants_english(&h));
        let mut h2 = HeaderMap::new();
        h2.insert("cookie", "gridlake-lang=zh-CN".parse().unwrap());
        assert!(!wants_english(&h2));
        h2.insert("x-lang", "en-US".parse().unwrap());
        assert!(wants_english(&h2), "请求头优先于 cookie");
    }

    #[test]
    fn every_catalog_entry_keeps_its_placeholders() {
        let raw: HashMap<String, String> = serde_json::from_str(CATALOG_JSON).unwrap();
        for (zh, en) in &raw {
            assert_eq!(placeholder_ids(zh), placeholder_ids(en), "占位符对不上：{zh} → {en}");
            assert!(!en.trim().is_empty(), "译文为空：{zh}");
        }
    }
}
