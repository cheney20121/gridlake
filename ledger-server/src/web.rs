//! 前端页面：设置 LEDGER_WEB_DIR 后由本服务直接提供前端构建产物，私有化部署只需一个进程
//!
//! - 单页应用：找不到的路径（如 /excel）回退到 index.html，由前端路由处理
//! - 缓存：/assets/ 下的文件名带内容哈希，长期缓存；index.html、config/index.js 等每次校验，升级后立即生效
//! - 打包时预先生成的 .gz 直接返回（大体积的表格引擎脚本不必每次压缩）

use std::convert::Infallible;
use std::path::Path;

use axum::body::Body;
use axum::http::{HeaderValue, Request, Response, header};
use tower::{Service, ServiceExt};
use tower_http::services::{ServeDir, ServeFile};

pub fn service(dir: &Path) -> impl Service<Request<Body>, Response = Response<Body>, Error = Infallible, Future: Send> + Clone + Send + Sync + 'static {
    let serve = ServeDir::new(dir).precompressed_gzip().fallback(ServeFile::new(dir.join("index.html")).precompressed_gzip());
    tower::service_fn(move |req: Request<Body>| {
        let serve = serve.clone();
        async move {
            let hashed = req.uri().path().starts_with("/assets/");
            let res = match serve.oneshot(req).await {
                Ok(r) => r,
                Err(e) => match e {},
            };
            let mut res = res.map(Body::new);
            let cache = if hashed && res.status().is_success() { "public, max-age=31536000, immutable" } else { "no-cache" };
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            Ok(res)
        }
    })
}
