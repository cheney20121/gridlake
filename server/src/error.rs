//! 统一响应信封：与前端 `src/utils/request.ts` 的约定一致 —— `{ code, data, msg }`，code 为 200 表示成功

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Serialize)]
pub struct Envelope<T: Serialize> {
    pub code: u16,
    pub data: Option<T>,
    pub msg: String,
}

pub fn ok<T: Serialize>(data: T) -> Json<Envelope<T>> {
    Json(Envelope { code: 200, data: Some(data), msg: "success".into() })
}

#[derive(Debug)]
pub enum AppError {
    /// 请求参数或上传文件不合法（400），msg 会直接展示给用户
    BadRequest(String),
    /// 无权访问（403），如运维接口未带令牌
    Forbidden(String),
    /// 未登录或登录已失效（401）
    Unauthorized(String),
    /// 依赖的服务暂不可用（503），如身份服务连不上
    Unavailable(String),
    Internal(anyhow::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn bad(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    /// 对应的 HTTP 状态码
    pub fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Forbidden(_) => 403,
            Self::Unauthorized(_) => 401,
            Self::Unavailable(_) => 503,
            Self::Internal(_) => 500,
        }
    }

    /// 给用户看的错误说明
    pub fn message(&self) -> String {
        match self {
            Self::BadRequest(m) | Self::Forbidden(m) | Self::Unauthorized(m) | Self::Unavailable(m) => m.clone(),
            Self::Internal(e) => e.to_string(),
        }
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        let err: anyhow::Error = err.into();
        // DuckDB 内存不足：给用户说清楚怎么办（503，稍后可重试），而不是「服务内部错误」
        if err.to_string().contains("Out of Memory Error") {
            tracing::warn!("内存不足：{err:#}");
            return Self::Unavailable("服务器内存不足，这次计算放不下。请缩小筛选范围、减少汇总维度或排序列，或稍后重试；管理员可调大 LEDGER_MEMORY_LIMIT 或给机器加内存".into());
        }
        Self::Internal(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            Self::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            Self::Unavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
            Self::Internal(err) => {
                tracing::error!("internal error: {err:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("服务内部错误：{err}"))
            }
        };
        let body = Envelope::<()> { code: status.as_u16(), data: None, msg: crate::msg_i18n::localize(msg) };
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_memory_is_a_friendly_503() {
        let e: AppError = anyhow::anyhow!("Out of Memory Error: could not allocate block of size 16.0 KiB (953.6 MiB/953.6 MiB used)").into();
        assert_eq!(e.status(), 503);
        assert!(e.message().contains("内存不足"));
        let other: AppError = anyhow::anyhow!("别的错误").into();
        assert_eq!(other.status(), 500);
    }
}
