//! MCP 服务

use super::*;

fn mcp_json(status: axum::http::StatusCode, body: serde_json::Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    (status, [(axum::http::header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

/// GET /api/mcp  405：本服务不提供服务端推送流（Streamable HTTP 的可选部分）
/// DELETE /api/mcp  405：本服务不维护会话
pub async fn mcp_get() -> axum::response::Response {
    use axum::response::IntoResponse;
    (axum::http::StatusCode::METHOD_NOT_ALLOWED, [(axum::http::header::ALLOW, "POST")], "").into_response()
}

/// POST /api/mcp  MCP Streamable HTTP：JSON-RPC 2.0（单条或批量），需 Authorization: Bearer <令牌>
pub async fn mcp_post(State(db): State<AppState>, headers: axum::http::HeaderMap, body: axum::body::Bytes) -> axum::response::Response {
    use axum::http::StatusCode;
    let auth = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(String::from);
    let client = headers.get(axum::http::header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.chars().take(120).collect::<String>());
    let msg: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return mcp_json(StatusCode::BAD_REQUEST, serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("JSON 解析失败：{e}") } })),
    };
    let r = blocking(&db, move |conn| {
        let caller = crate::mcp::authenticate(conn, auth.as_deref())?;
        let out = match &msg {
            serde_json::Value::Array(items) => {
                let rs: Vec<serde_json::Value> = items.iter().filter_map(|m| crate::mcp::handle(conn, &caller, m, client.as_deref())).collect();
                if rs.is_empty() { None } else { Some(serde_json::Value::Array(rs)) }
            }
            m => crate::mcp::handle(conn, &caller, m, client.as_deref()),
        };
        Ok(out)
    })
    .await;
    match r {
        Ok(Some(v)) => mcp_json(StatusCode::OK, v),
        Ok(None) => mcp_json(StatusCode::ACCEPTED, serde_json::Value::Null).map(|_| axum::body::Body::empty()),
        Err(AppError::Forbidden(m)) => {
            let mut resp = mcp_json(StatusCode::UNAUTHORIZED, serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32001, "message": m } }));
            resp.headers_mut().insert(axum::http::header::WWW_AUTHENTICATE, axum::http::HeaderValue::from_static("Bearer"));
            resp
        }
        Err(e) => mcp_json(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32603, "message": e.message() } })),
    }
}

/// GET /api/ledger/mcp/tokens  有效的 MCP 令牌（不含明文）
pub async fn mcp_tokens(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<crate::mcp::TokenView>>>> {
    Ok(ok(blocking(&db, crate::mcp::list_tokens).await?))
}

/// POST /api/ledger/mcp/tokens  { name, allowSql?, datasets?, operator? } → { token, secret }（明文只返回这一次）
pub async fn mcp_create_token(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<serde_json::Value>>> {
    let input: crate::mcp::TokenInput = parse_json(&body)?;
    let (view, secret) = blocking(&db, move |conn| crate::mcp::create_token(conn, input)).await?;
    Ok(ok(serde_json::json!({ "token": view, "secret": secret })))
}

/// PUT /api/ledger/mcp/tokens/{id}  修改名称 / 是否允许 SQL / 可访问的数据集
pub async fn mcp_update_token(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::mcp::TokenInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::mcp::update_token(conn, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/mcp/tokens/{id}  吊销（立即失效）
pub async fn mcp_revoke_token(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::mcp::revoke_token(conn, &id)).await?;
    Ok(ok(true))
}

/// GET /api/ledger/mcp/logs?limit=  调用审计日志（新的在前）
pub async fn mcp_logs(State(db): State<AppState>, Query(q): Query<RunsQuery>) -> AppResult<Json<Envelope<Vec<crate::mcp::LogView>>>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    Ok(ok(blocking(&db, move |conn| crate::mcp::logs(conn, limit)).await?))
}
