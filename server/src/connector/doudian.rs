//! 抖店开放平台（自用型应用）：签名与令牌，供 HTTP 接口来源使用
//!
//! - 接口地址写成 `https://openapi-fxg.jinritemai.com/order/searchList`，路径换成点号就是方法名（`order.searchList`）
//! - 请求：POST，查询串带 app_key / method / v=2 / timestamp / sign / sign_method / access_token，请求体是业务参数（param_json）
//! - 签名：`HMAC-SHA256(app_secret, app_secret + "app_key" + 值 + "method" + 值 + "param_json" + 值 + "timestamp" + 值 + "v" + "2" + app_secret)`，
//!   十六进制小写；param_json 是键按字典序递归排序的紧凑 JSON，发送的请求体与参与签名的必须逐字一致
//! - 令牌：自用型应用用 `token.create`（grant_type = authorization_self + 店铺 ID）换取，约 7 天有效；按「地址 + app_key + 店铺」缓存在内存，
//!   到期前 5 分钟重新换取；不支持工具型 / 服务商应用（它们的刷新令牌每次刷新都会换新，需要另行保存）
//! - 响应：`{ code: 10000, msg, sub_code, sub_msg, data, log_id }`，code 不是 10000 即失败，错误原因原样带出

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{FixedOffset, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Map, Value as Json};
use sha2::Sha256;

use crate::error::{AppError, AppResult};

const SUCCESS: i64 = 10000;
/// 令牌到期前提前换新的时间
const EARLY: Duration = Duration::from_secs(300);

struct Token {
    value: String,
    expires: Instant,
}

static TOKENS: LazyLock<Mutex<HashMap<String, Token>>> = LazyLock::new(Default::default);

/// 接口地址 → 方法名：`/order/searchList` → `order.searchList`
pub fn method_of(url: &reqwest::Url) -> Option<String> {
    let path = url.path().trim_matches('/');
    (!path.is_empty()).then(|| path.replace('/', "."))
}

/// 保存前的校验（地址、应用与店铺）
pub fn check(url: &str, app_key: &str, shop_id: &str) -> AppResult<()> {
    let u = reqwest::Url::parse(url.trim()).map_err(|_| AppError::bad("接口地址格式不正确"))?;
    if method_of(&u).is_none() {
        return Err(AppError::bad("抖店接口地址要带方法路径，如 https://openapi-fxg.jinritemai.com/order/searchList"));
    }
    if app_key.trim().is_empty() {
        return Err(AppError::bad("请填写抖店应用的 App Key"));
    }
    if shop_id.trim().is_empty() || !shop_id.trim().chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::bad("请填写店铺 ID（数字）"));
    }
    Ok(())
}

/// 键按字典序递归排序的紧凑 JSON（参与签名的 param_json）
pub fn canonical(v: &Json) -> String {
    fn sorted(v: &Json) -> Json {
        match v {
            Json::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                Json::Object(keys.into_iter().map(|k| (k.clone(), sorted(&m[k]))).collect::<Map<_, _>>())
            }
            Json::Array(a) => Json::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(v).to_string()
}

pub fn sign(app_secret: &str, app_key: &str, method: &str, param_json: &str, timestamp: &str) -> String {
    let text = format!("{app_secret}app_key{app_key}method{method}param_json{param_json}timestamp{timestamp}v2{app_secret}");
    let mut mac = Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()).expect("hmac accepts any key");
    mac.update(text.as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// 北京时间 `yyyy-MM-dd HH:mm:ss`
fn timestamp() -> String {
    let cn = FixedOffset::east_opt(8 * 3600).expect("valid offset");
    Utc::now().with_timezone(&cn).format("%Y-%m-%d %H:%M:%S").to_string()
}

fn describe(resp: &Json) -> String {
    let text = |k: &str| resp.get(k).and_then(|v| v.as_str().map(str::to_string).or_else(|| (!v.is_null()).then(|| v.to_string()))).unwrap_or_default();
    let code = resp.get("code").map(|c| c.to_string()).unwrap_or_default();
    let mut parts = vec![format!("code {code}")];
    for k in ["msg", "sub_code", "sub_msg"] {
        let t = text(k);
        if !t.is_empty() {
            parts.push(t);
        }
    }
    parts.join("，")
}

/// 一次签名请求；param_json 必须已是规范化（排序、紧凑）的文本
async fn call(client: &reqwest::Client, base: &reqwest::Url, method: &str, param_json: &str, app_key: &str, app_secret: &str, token: Option<&str>) -> AppResult<Json> {
    let ts = timestamp();
    let mut url = base.clone();
    url.set_path(&format!("/{}", method.replace('.', "/")));
    url.set_query(None);
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("app_key", app_key).append_pair("method", method).append_pair("v", "2").append_pair("timestamp", &ts).append_pair("sign_method", "hmac-sha256").append_pair("sign", &sign(app_secret, app_key, method, param_json, &ts));
        if let Some(t) = token {
            q.append_pair("access_token", t);
        }
    }
    let resp = client.post(url).header("Content-Type", "application/json").header("Accept", "application/json").body(param_json.to_string()).send().await.map_err(|e| {
        if e.is_connect() { AppError::bad(format!("无法连接抖店开放平台：{e}（检查网络与防火墙）")) } else { AppError::bad(format!("请求抖店开放平台失败：{e}")) }
    })?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| AppError::bad(format!("读取抖店响应失败：{e}")))?;
    let json: Json = serde_json::from_str(&text).map_err(|_| {
        let brief: String = text.chars().take(200).collect();
        AppError::bad(if status.is_success() { format!("抖店响应不是 JSON：{brief}") } else { format!("抖店接口返回错误（{status}）：{brief}") })
    })?;
    if json.get("code").and_then(Json::as_i64) != Some(SUCCESS) {
        return Err(AppError::bad(format!("抖店接口返回错误：{}", describe(&json))));
    }
    Ok(json)
}

fn cache_key(base: &reqwest::Url, app_key: &str, shop_id: &str) -> String {
    format!("{}|{app_key}|{shop_id}", base.origin().ascii_serialization())
}

async fn access_token(client: &reqwest::Client, base: &reqwest::Url, app_key: &str, app_secret: &str, shop_id: &str) -> AppResult<String> {
    let key = cache_key(base, app_key, shop_id);
    if let Some(t) = TOKENS.lock().unwrap_or_else(|e| e.into_inner()).get(&key).filter(|t| t.expires > Instant::now()) {
        return Ok(t.value.clone());
    }
    let param = canonical(&serde_json::json!({ "code": "", "grant_type": "authorization_self", "shop_id": shop_id }));
    let resp = call(client, base, "token.create", &param, app_key, app_secret, None).await.map_err(|e| AppError::bad(format!("获取抖店访问令牌失败：{}（检查 App Key、App Secret 与店铺 ID，应用需为自用型并已授权该店铺）", e.message())))?;
    let data = resp.get("data");
    let value = data.and_then(|d| d.get("access_token")).and_then(Json::as_str).filter(|t| !t.is_empty()).ok_or_else(|| AppError::bad("获取抖店访问令牌失败：响应里没有 access_token"))?.to_string();
    let secs = data.and_then(|d| d.get("expires_in")).and_then(Json::as_u64).unwrap_or(3600);
    let expires = Instant::now() + Duration::from_secs(secs).saturating_sub(EARLY);
    TOKENS.lock().unwrap_or_else(|e| e.into_inner()).insert(key, Token { value: value.clone(), expires });
    Ok(value)
}

/// 调用业务接口：自动换令牌；令牌失效时换一次新的重试
pub async fn fetch(client: &reqwest::Client, url: &str, body: &str, app_key: &str, shop_id: &str, app_secret: &str) -> AppResult<Json> {
    let base = reqwest::Url::parse(url.trim()).map_err(|_| AppError::bad("接口地址格式不正确"))?;
    let method = method_of(&base).ok_or_else(|| AppError::bad("抖店接口地址要带方法路径"))?;
    let params: Json = if body.trim().is_empty() { Json::Object(Map::new()) } else { serde_json::from_str(body).map_err(|e| AppError::bad(format!("请求体不是合法的 JSON：{e}")))? };
    let param_json = canonical(&params);
    let (app_key, shop_id) = (app_key.trim(), shop_id.trim());
    let token = access_token(client, &base, app_key, app_secret, shop_id).await?;
    match call(client, &base, &method, &param_json, app_key, app_secret, Some(&token)).await {
        Err(e) if e.message().to_lowercase().contains("token") => {
            TOKENS.lock().unwrap_or_else(|e| e.into_inner()).remove(&cache_key(&base, app_key, shop_id));
            let token = access_token(client, &base, app_key, app_secret, shop_id).await?;
            call(client, &base, &method, &param_json, app_key, app_secret, Some(&token)).await
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn method_from_path() {
        let u = |s: &str| reqwest::Url::parse(s).unwrap();
        assert_eq!(method_of(&u("https://openapi-fxg.jinritemai.com/order/searchList")).as_deref(), Some("order.searchList"));
        assert_eq!(method_of(&u("https://openapi-fxg.jinritemai.com/order/getSettleBillDetailV3?x=1")).as_deref(), Some("order.getSettleBillDetailV3"));
        assert_eq!(method_of(&u("https://openapi-fxg.jinritemai.com/")), None);
    }

    #[test]
    fn canonical_sorts_keys_recursively() {
        let v = json!({ "size": 100, "page": 0, "filter": { "z": [ { "b": 1, "a": "买家" } ], "a": null } });
        assert_eq!(canonical(&v), r#"{"filter":{"a":null,"z":[{"a":"买家","b":1}]},"page":0,"size":100}"#);
    }

    #[test]
    fn sign_matches_independent_implementation() {
        // 期望值由 Python hmac 独立计算：
        //   s = "sec" + "app_key" + "k1" + "method" + "order.searchList" + "param_json" + '{"page":0,"size":100}' + "timestamp" + "2026-10-07 10:00:00" + "v" + "2" + "sec"
        //   hmac.new(b"sec", s.encode(), hashlib.sha256).hexdigest()
        assert_eq!(sign("sec", "k1", "order.searchList", r#"{"page":0,"size":100}"#, "2026-10-07 10:00:00"), "229fda9c51870532d7f515615d205ed84287515ca4684414c720c92b03459abc");
    }

    #[test]
    fn checks() {
        let url = "https://openapi-fxg.jinritemai.com/order/searchList";
        assert!(check(url, "k", "123").is_ok());
        assert!(check("https://openapi-fxg.jinritemai.com/", "k", "123").is_err());
        assert!(check(url, " ", "123").is_err());
        assert!(check(url, "k", "abc").is_err());
    }
}
