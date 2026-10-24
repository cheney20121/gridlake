//! 前端页面：设置 LEDGER_WEB_DIR 后由本服务直接提供前端构建产物，私有化部署只需一个进程
//!
//! - 单页应用：找不到的路径（如 /excel）回退到 index.html，由前端路由处理
//! - 缓存：/assets/ 下的文件名带内容哈希，长期缓存；index.html、config/index.js 等每次校验，升级后立即生效
//! - 打包时预先生成的 .gz 直接返回（大体积的表格引擎脚本不必每次压缩）
//! - 页面（HTML）的安全响应头：防点击劫持——默认只允许同源页面用 iframe 嵌入；分享页（/share/…）可用 `LEDGER_EMBED_ORIGINS` 放开给指定网站；
//!   分享页还带 `Referrer-Policy: no-referrer`，链接里的令牌不会通过 Referer 泄露给别的网站

use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, Response, header};
use tower::{Service, ServiceExt};
use tower_http::services::{ServeDir, ServeFile};

pub fn service(dir: &Path, embed_origins: Vec<String>) -> impl Service<Request<Body>, Response = Response<Body>, Error = Infallible, Future: Send> + Clone + Send + Sync + 'static {
    let serve = ServeDir::new(dir).precompressed_gzip().fallback(ServeFile::new(dir.join("index.html")).precompressed_gzip());
    let embed = Arc::new(embed_origins);
    tower::service_fn(move |req: Request<Body>| {
        let serve = serve.clone();
        let embed = embed.clone();
        async move {
            let path = req.uri().path().to_string();
            let hashed = path.starts_with("/assets/");
            let res = match serve.oneshot(req).await {
                Ok(r) => r,
                Err(e) => match e {},
            };
            let mut res = res.map(Body::new);
            let cache = if hashed && res.status().is_success() { "public, max-age=31536000, immutable" } else { "no-cache" };
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            if res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/html")) {
                for (k, v) in page_headers(&path, &embed) {
                    res.headers_mut().insert(k, v);
                }
            }
            Ok(res)
        }
    })
}

/// 页面（HTML）响应的安全头。`embed` 是允许嵌入分享页的来源（见 config 的 `embed_origins`）
pub fn page_headers(path: &str, embed: &[String]) -> Vec<(HeaderName, HeaderValue)> {
    let h = |k: &'static str, v: &str| (HeaderName::from_static(k), HeaderValue::from_str(v).expect("响应头的值只含可见字符"));
    let mut out = vec![h("x-content-type-options", "nosniff")];
    let share = path.starts_with("/share/");
    if share {
        out.push(h("referrer-policy", "no-referrer"));
    }
    if share && !embed.is_empty() {
        // X-Frame-Options 表达不了「这几个网站」，嵌入开放时只用 CSP
        out.push(h("content-security-policy", &format!("frame-ancestors {}", embed.join(" "))));
    }
    else {
        out.push(h("content-security-policy", "frame-ancestors 'self'"));
        out.push(h("x-frame-options", "SAMEORIGIN"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(h: &[(HeaderName, HeaderValue)], k: &str) -> Option<String> {
        h.iter().find(|(n, _)| n.as_str() == k).map(|(_, v)| v.to_str().unwrap().to_string())
    }

    #[test]
    fn pages_cannot_be_framed_by_other_sites_by_default() {
        for path in ["/", "/excel", "/share/gls_abc", "/login"] {
            let h = page_headers(path, &[]);
            assert_eq!(get(&h, "content-security-policy").as_deref(), Some("frame-ancestors 'self'"), "{path}");
            assert_eq!(get(&h, "x-frame-options").as_deref(), Some("SAMEORIGIN"), "{path}");
            assert_eq!(get(&h, "x-content-type-options").as_deref(), Some("nosniff"));
        }
    }

    #[test]
    fn only_share_pages_open_up_to_the_configured_sites() {
        let embed = vec!["https://a.example.com".to_string(), "https://*.b.example.com".to_string()];
        let share = page_headers("/share/gls_abc", &embed);
        assert_eq!(get(&share, "content-security-policy").as_deref(), Some("frame-ancestors https://a.example.com https://*.b.example.com"));
        assert!(get(&share, "x-frame-options").is_none(), "开放嵌入时不能再带 X-Frame-Options，否则浏览器仍会拒绝");
        // 其他页面（编辑器、登录页、首页）即使配置了也不开放
        for path in ["/", "/excel", "/login", "/excel/home", "/sharing", "/api/share/x"] {
            let h = page_headers(path, &embed);
            assert_eq!(get(&h, "content-security-policy").as_deref(), Some("frame-ancestors 'self'"), "{path}");
            assert_eq!(get(&h, "x-frame-options").as_deref(), Some("SAMEORIGIN"), "{path}");
        }
        assert_eq!(get(&page_headers("/share/gls_abc", &["*".to_string()]), "content-security-policy").as_deref(), Some("frame-ancestors *"));
    }

    #[test]
    fn share_pages_never_leak_the_token_through_referer() {
        for embed in [vec![], vec!["*".to_string()]] {
            assert_eq!(get(&page_headers("/share/gls_abc", &embed), "referrer-policy").as_deref(), Some("no-referrer"));
        }
        assert!(get(&page_headers("/excel", &[]), "referrer-policy").is_none());
    }
}
