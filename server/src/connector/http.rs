//! HTTP 接口来源：按计划调用返回 JSON 的接口（ERP、自建系统、各类开放平台），把记录同步进数据集
//!
//! - GET / POST；请求头可自定义；鉴权：Bearer 令牌、Basic 账号密码、自定义请求头（密钥加密存储，接口永不返回）
//! - 记录位置：JSON 路径（如 `data.list`，为空时响应本身就是数组）；嵌套对象展开成「a.b」列，数组保留为 JSON 文本
//! - 分页：页码（{{page}}、{{size}}）、偏移（{{offset}}、{{size}}）、游标（{{cursor}}，从响应的指定路径取下一页游标），
//!   取到不满一页 / 没有记录 / 没有下一页游标为止
//! - 增量：URL 或请求体里写 {{watermark}}，平台记住上次取到的增量字段最大值，下次代入（首次用「初始值」）
//! - {{now}}：当前时间的 Unix 秒数（时间范围接口的结束时间）
//! - 抖店开放平台：鉴权选「抖店」，地址写方法路径、请求体写业务参数，签名与令牌自动处理
//! - 只允许 http(s)；单次最多 10000 页；记录先逐页写成 JSONL，结束后按出现过的全部字段写 CSV

use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

use super::database::{Extracted, greater};
use super::doudian;
use crate::error::{AppError, AppResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_PAGES: u32 = 10_000;

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    #[default]
    Get,
    Post,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Auth {
    #[default]
    None,
    /// Authorization: Bearer <密钥>
    Bearer,
    /// Authorization: Basic base64(账号:密钥)
    Basic { user: String },
    /// 自定义请求头：<name>: <密钥>
    Header { name: String },
    /// 抖店开放平台自用型应用：密钥填 App Secret，签名与访问令牌由平台处理（见 doudian.rs）
    #[serde(rename_all = "camelCase")]
    Doudian { app_key: String, shop_id: String },
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Pagination {
    #[default]
    None,
    /// {{page}} 从 start 开始，每页 {{size}} 条
    Page {
        #[serde(default = "one")]
        start: u32,
        size: u32,
    },
    /// {{offset}} 从 0 开始，每页 {{size}} 条
    Offset { size: u32 },
    /// {{cursor}} 首页为空，之后取响应中 cursorPath 的值
    #[serde(rename_all = "camelCase")]
    Cursor { cursor_path: String },
}

fn one() -> u32 {
    1
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HttpSource {
    pub url: String,
    #[serde(default)]
    pub method: Method,
    #[serde(default)]
    pub headers: Vec<Header>,
    /// POST 的 JSON 请求体模板（可含占位符）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub auth: Auth,
    /// 只在请求中出现（明文），保存时加密为 secret_enc，响应中永不返回
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_enc: Option<String>,
    /// 记录数组在响应中的路径（如 data.list），为空时响应本身是数组
    #[serde(default)]
    pub records_path: String,
    #[serde(default)]
    pub pagination: Pagination,
    /// 增量字段（记录中的字段名，展开后的写法，如 update_time）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental_field: Option<String>,
    /// 首次同步时 {{watermark}} 的值
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_watermark: Option<String>,
}

impl HttpSource {
    /// 比较来源是否变化（不含密钥）：变化后增量位置需要重置
    pub fn same_query(&self, other: &HttpSource) -> bool {
        self.url.trim() == other.url.trim()
            && self.method == other.method
            && self.body == other.body
            && self.records_path == other.records_path
            && self.pagination == other.pagination
            && self.incremental_field == other.incremental_field
            // 换了抖店应用或店铺，增量位置不再适用
            && (!matches!(self.auth, Auth::Doudian { .. }) && !matches!(other.auth, Auth::Doudian { .. }) || self.auth == other.auth)
    }

    fn incremental(&self) -> Option<&str> {
        self.incremental_field.as_deref().map(str::trim).filter(|f| !f.is_empty())
    }
}

/// 保存前的校验
pub fn check(src: &HttpSource) -> AppResult<()> {
    let url = reqwest::Url::parse(src.url.trim()).map_err(|_| AppError::bad("接口地址格式不正确，应以 http:// 或 https:// 开头"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::bad("接口地址只支持 http:// 或 https://"));
    }
    if let Some(b) = src.body.as_deref().filter(|b| !b.trim().is_empty()) {
        // 占位符代入示例值后应为合法 JSON
        serde_json::from_str::<Json>(&fill(b, &Vars { page: Some(1), size: Some(100), offset: Some(0), cursor: Some(String::new()), watermark: Some("0".into()) }, true)).map_err(|e| AppError::bad(format!("请求体不是合法的 JSON：{e}")))?;
    }
    match &src.pagination {
        Pagination::Page { size, .. } | Pagination::Offset { size } if *size == 0 || *size > 10_000 => return Err(AppError::bad("每页条数应在 1 到 10000 之间")),
        Pagination::Cursor { cursor_path } if cursor_path.trim().is_empty() => return Err(AppError::bad("游标分页需要填写下一页游标在响应中的路径")),
        _ => {}
    }
    let template = format!("{}{}", src.url, src.body.as_deref().unwrap_or(""));
    let need = |p: &str| template.contains(p);
    match &src.pagination {
        Pagination::Page { .. } if !need("{{page}}") => return Err(AppError::bad("页码分页需要在地址或请求体里写 {{page}}")),
        Pagination::Offset { .. } if !need("{{offset}}") => return Err(AppError::bad("偏移分页需要在地址或请求体里写 {{offset}}")),
        Pagination::Cursor { .. } if !need("{{cursor}}") => return Err(AppError::bad("游标分页需要在地址或请求体里写 {{cursor}}")),
        _ => {}
    }
    if src.incremental().is_some() && !need("{{watermark}}") {
        return Err(AppError::bad("增量同步需要在地址或请求体里写 {{watermark}}（代入上次同步到的值）"));
    }
    if let Auth::Header { name } = &src.auth
        && name.trim().is_empty()
    {
        return Err(AppError::bad("请填写放密钥的请求头名称"));
    }
    if let Auth::Doudian { app_key, shop_id } = &src.auth {
        doudian::check(&src.url, app_key, shop_id)?;
        if src.method != Method::Post {
            return Err(AppError::bad("抖店接口使用 POST，请求体填业务参数"));
        }
    }
    Ok(())
}

#[derive(Default)]
struct Vars {
    page: Option<u32>,
    size: Option<u32>,
    offset: Option<u64>,
    cursor: Option<String>,
    watermark: Option<String>,
}

/// 代入占位符；in_json 时值按 JSON 字符串内容转义（模板里占位符一般写在引号内），否则按 URL 编码
fn fill(template: &str, v: &Vars, in_json: bool) -> String {
    let enc = |s: &str| -> String {
        if in_json {
            let q = serde_json::to_string(s).unwrap_or_default();
            q[1..q.len() - 1].to_string()
        } else {
            url_encode(s)
        }
    };
    let mut out = template.to_string();
    for (key, val) in [
        ("{{page}}", v.page.map(|x| x.to_string())),
        ("{{size}}", v.size.map(|x| x.to_string())),
        ("{{offset}}", v.offset.map(|x| x.to_string())),
        ("{{cursor}}", v.cursor.clone()),
        ("{{watermark}}", v.watermark.clone()),
    ] {
        out = out.replace(key, &enc(val.as_deref().unwrap_or("")));
    }
    out.replace("{{now}}", &chrono::Utc::now().timestamp().to_string())
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// 按「a.b.0.c」路径取值（数字段为数组下标）
fn pick<'a>(v: &'a Json, path: &str) -> Option<&'a Json> {
    let path = path.trim().trim_start_matches("$.").trim_start_matches('$');
    if path.is_empty() {
        return Some(v);
    }
    path.split('.').try_fold(v, |cur, seg| match cur {
        Json::Object(m) => m.get(seg),
        Json::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    })
}

/// 记录展开成「字段 → 文本」：嵌套对象用「.」连接，数组保留为 JSON 文本
fn flatten(prefix: &str, v: &Json, out: &mut Vec<(String, String)>) {
    match v {
        Json::Object(m) => {
            for (k, x) in m {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, x, out);
            }
        }
        Json::Null => out.push((prefix.to_string(), String::new())),
        Json::String(s) => out.push((prefix.to_string(), s.clone())),
        Json::Array(_) => out.push((prefix.to_string(), v.to_string())),
        other => out.push((prefix.to_string(), other.to_string())),
    }
}

fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| AppError::bad(format!("无法创建网络客户端：{e}")))
}

/// 请求一页，返回响应 JSON
async fn fetch(client: &reqwest::Client, src: &HttpSource, secret: &str, vars: &Vars) -> AppResult<Json> {
    let url = fill(src.url.trim(), vars, false);
    if let Auth::Doudian { app_key, shop_id } = &src.auth {
        if secret.is_empty() {
            return Err(AppError::bad("请填写抖店应用的 App Secret"));
        }
        let body = src.body.as_deref().map(|b| fill(b, vars, true)).unwrap_or_default();
        return doudian::fetch(client, &url, &body, app_key, shop_id, secret).await;
    }
    let mut req = match src.method {
        Method::Get => client.get(&url),
        Method::Post => {
            let body = src.body.as_deref().filter(|b| !b.trim().is_empty()).map(|b| fill(b, vars, true)).unwrap_or_else(|| "{}".into());
            client.post(&url).header("Content-Type", "application/json").body(body)
        }
    };
    req = req.header("Accept", "application/json");
    for h in &src.headers {
        if !h.name.trim().is_empty() {
            req = req.header(h.name.trim(), h.value.trim());
        }
    }
    req = match &src.auth {
        Auth::None => req,
        Auth::Bearer => req.bearer_auth(secret),
        Auth::Basic { user } => req.basic_auth(user.trim(), Some(secret)),
        Auth::Header { name } => req.header(name.trim(), secret),
        Auth::Doudian { .. } => unreachable!("抖店请求在上面单独处理"),
    };
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() {
            AppError::bad(format!("接口响应超时（{}）", REQUEST_TIMEOUT.as_secs()))
        } else if e.is_connect() {
            AppError::bad(format!("无法连接接口：{e}（检查地址、网络与防火墙）"))
        } else {
            AppError::bad(format!("请求接口失败：{e}"))
        }
    })?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| AppError::bad(format!("读取接口响应失败：{e}")))?;
    if !status.is_success() {
        let brief: String = text.chars().take(300).collect();
        return Err(AppError::bad(match status.as_u16() {
            401 | 403 => format!("接口拒绝访问（{status}），请检查鉴权方式与密钥：{brief}"),
            404 => format!("接口地址不存在（{status}）：{brief}"),
            429 => format!("接口限流（{status}），请把同步间隔调大：{brief}"),
            _ => format!("接口返回错误（{status}）：{brief}"),
        }));
    }
    serde_json::from_str(&text).map_err(|_| AppError::bad(format!("接口返回的不是 JSON：{}", text.chars().take(200).collect::<String>())))
}

fn records<'a>(src: &HttpSource, resp: &'a Json) -> AppResult<&'a Vec<Json>> {
    let path = src.records_path.trim();
    match pick(resp, path) {
        Some(Json::Array(a)) => Ok(a),
        Some(Json::Null) => Err(AppError::bad(format!("响应中「{path}」为空"))),
        Some(_) => Err(AppError::bad(format!("响应中「{}」不是数组，请检查记录路径", if path.is_empty() { "（根）" } else { path }))),
        None => {
            let keys = resp.as_object().map(|m| m.keys().take(10).cloned().collect::<Vec<_>>().join("、")).unwrap_or_default();
            Err(AppError::bad(format!("响应中找不到「{path}」（顶层字段：{keys}）")))
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpTestResult {
    /// 第一页的记录数
    pub records: usize,
    /// 第一条记录展开后的字段
    pub fields: Vec<String>,
    /// 响应的开头（便于填写记录路径）
    pub sample: String,
}

/// 测试：只取第一页
pub async fn test(src: &HttpSource, secret: &str) -> AppResult<HttpTestResult> {
    check(src)?;
    let resp = fetch(&client()?, src, secret, &first_vars(src, None)).await?;
    let sample: String = serde_json::to_string_pretty(&resp).unwrap_or_default().chars().take(1500).collect();
    let list = match records(src, &resp) {
        Ok(l) => l,
        Err(e) => return Err(AppError::bad(format!("{}。响应开头：{}", e.message(), sample.chars().take(300).collect::<String>()))),
    };
    let mut fields = Vec::new();
    if let Some(first) = list.first() {
        let mut kv = Vec::new();
        flatten("", first, &mut kv);
        fields = kv.into_iter().map(|(k, _)| k).collect();
    }
    if let Some(f) = src.incremental()
        && !fields.is_empty()
        && !fields.iter().any(|x| x == f)
    {
        return Err(AppError::bad(format!("记录中没有增量字段「{f}」（可用的字段：{}）", fields.join("、"))));
    }
    Ok(HttpTestResult { records: list.len(), fields, sample })
}

fn first_vars(src: &HttpSource, watermark: Option<&str>) -> Vars {
    let w = watermark.map(str::to_string).or_else(|| src.initial_watermark.clone()).unwrap_or_default();
    match &src.pagination {
        Pagination::None => Vars { watermark: Some(w), ..Default::default() },
        Pagination::Page { start, size } => Vars { page: Some(*start), size: Some(*size), watermark: Some(w), ..Default::default() },
        Pagination::Offset { size } => Vars { offset: Some(0), size: Some(*size), watermark: Some(w), ..Default::default() },
        Pagination::Cursor { .. } => Vars { cursor: Some(String::new()), watermark: Some(w), ..Default::default() },
    }
}

/// 逐页取数写入 CSV（第一行为字段名）；limit 为预览的行数上限
pub async fn extract(src: &HttpSource, secret: &str, watermark: Option<&str>, limit: Option<u32>, max_rows: i64, out: &Path) -> AppResult<Extracted> {
    check(src)?;
    let client = client()?;
    let jsonl = out.with_extension("jsonl");
    let mut w = std::io::BufWriter::new(std::fs::File::create(&jsonl)?);
    let mut fields: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rows: i64 = 0;
    let mut max: Option<String> = None;
    let inc = src.incremental();
    let mut vars = first_vars(src, watermark);
    let result: AppResult<()> = async {
        for page in 0..MAX_PAGES {
            let resp = fetch(&client, src, secret, &vars).await?;
            let list = records(src, &resp)?;
            for rec in list {
                let mut kv = Vec::new();
                flatten("", rec, &mut kv);
                for (k, _) in &kv {
                    if seen.insert(k.clone()) {
                        fields.push(k.clone());
                    }
                }
                if let Some(f) = inc
                    && let Some((_, v)) = kv.iter().find(|(k, _)| k == f)
                    && !v.is_empty()
                    && max.as_deref().is_none_or(|m| greater(v, m))
                {
                    max = Some(v.clone());
                }
                let obj: Map<String, Json> = kv.into_iter().map(|(k, v)| (k, Json::String(v))).collect();
                serde_json::to_writer(&mut w, &obj)?;
                w.write_all(b"\n")?;
                rows += 1;
                if rows > max_rows {
                    return Err(AppError::bad(format!("单次同步超过 {max_rows} 行上限，请缩小范围或改为增量同步")));
                }
                if limit.is_some_and(|l| rows >= l as i64) {
                    return Ok(());
                }
            }
            // 下一页
            match &src.pagination {
                Pagination::None => return Ok(()),
                Pagination::Page { size, .. } => {
                    if list.len() < *size as usize {
                        return Ok(());
                    }
                    vars.page = vars.page.map(|p| p + 1);
                }
                Pagination::Offset { size } => {
                    if list.len() < *size as usize {
                        return Ok(());
                    }
                    vars.offset = vars.offset.map(|o| o + list.len() as u64);
                }
                Pagination::Cursor { cursor_path } => {
                    let next = pick(&resp, cursor_path).and_then(|c| match c {
                        Json::String(s) if !s.is_empty() => Some(s.clone()),
                        Json::Number(n) => Some(n.to_string()),
                        _ => None,
                    });
                    match next {
                        Some(n) if !list.is_empty() && Some(&n) != vars.cursor.as_ref() => vars.cursor = Some(n),
                        _ => return Ok(()),
                    }
                }
            }
            if page + 1 == MAX_PAGES {
                return Err(AppError::bad(format!("翻页超过 {MAX_PAGES} 页仍未结束，请检查分页设置")));
            }
        }
        Ok(())
    }
    .await;
    w.flush()?;
    drop(w);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&jsonl);
        return Err(e);
    }
    // JSONL → CSV（按出现过的全部字段）
    let (jsonl2, out2) = (jsonl.clone(), out.to_path_buf());
    let fields2 = fields.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let mut csv = csv::Writer::from_path(&out2)?;
        csv.write_record(if fields2.is_empty() { vec!["（无数据）".to_string()] } else { fields2.clone() })?;
        for line in std::io::BufReader::new(std::fs::File::open(&jsonl2)?).lines() {
            let obj: Map<String, Json> = serde_json::from_str(&line?)?;
            csv.write_record(fields2.iter().map(|f| obj.get(f).and_then(Json::as_str).unwrap_or("")))?;
        }
        csv.flush()?;
        Ok(())
    })
    .await??;
    let _ = std::fs::remove_file(&jsonl);
    Ok(Extracted { rows, watermark: max.or_else(|| watermark.map(str::to_string)) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_and_flatten() {
        let v = json!({ "data": { "list": [{ "id": 1, "shop": { "name": "A店" }, "tags": ["x"], "amt": null }], "next": "c2" } });
        assert_eq!(pick(&v, "data.list.0.id"), Some(&json!(1)));
        assert_eq!(pick(&v, "$.data.next"), Some(&json!("c2")));
        let mut kv = Vec::new();
        flatten("", &v["data"]["list"][0], &mut kv);
        // 保留接口返回的字段顺序
        assert_eq!(kv, vec![("id".into(), "1".into()), ("shop.name".into(), "A店".into()), ("tags".into(), "[\"x\"]".into()), ("amt".into(), String::new())]);
    }

    #[test]
    fn placeholders() {
        let vars = Vars { page: Some(2), size: Some(50), watermark: Some("2026-10-01 08:00:00".into()), ..Default::default() };
        assert_eq!(fill("https://x/api?p={{page}}&s={{size}}&since={{watermark}}", &vars, false), "https://x/api?p=2&s=50&since=2026-10-01%2008%3A00%3A00");
        let now = fill(r#"{"to":{{now}}}"#, &Vars::default(), true);
        assert!(serde_json::from_str::<Json>(&now).unwrap()["to"].as_i64().unwrap() > 1_790_000_000);
        assert_eq!(fill(r#"{"since":"{{watermark}}","page":{{page}}}"#, &Vars { page: Some(1), watermark: Some("a\"b".into()), ..Default::default() }, true), r#"{"since":"a\"b","page":1}"#);
    }

    #[test]
    fn pagination_json() {
        let p: Pagination = serde_json::from_str(r#"{"kind":"cursor","cursorPath":"data.next"}"#).unwrap();
        assert_eq!(p, Pagination::Cursor { cursor_path: "data.next".into() });
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"kind":"cursor","cursorPath":"data.next"}"#);
    }

    #[test]
    fn checks() {
        let base = HttpSource { url: "https://x/api?page={{page}}".into(), method: Method::Get, headers: vec![], body: None, auth: Auth::None, secret: None, secret_enc: None, records_path: "data".into(), pagination: Pagination::Page { start: 1, size: 100 }, incremental_field: None, initial_watermark: None };
        assert!(check(&base).is_ok());
        assert!(check(&HttpSource { url: "ftp://x".into(), ..base.clone() }).is_err());
        assert!(check(&HttpSource { url: "https://x/api".into(), ..base.clone() }).is_err());
        assert!(check(&HttpSource { incremental_field: Some("ts".into()), ..base.clone() }).is_err());
        assert!(check(&HttpSource { method: Method::Post, body: Some("{bad".into()), ..base }).is_err());
    }
}
