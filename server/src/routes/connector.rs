//! 数据连接器

use super::*;

/// GET /api/ledger/connectors  连接器列表（含最近一次同步、下次同步时间）
pub async fn list_connectors(State(db): State<AppState>) -> AppResult<Json<Envelope<serde_json::Value>>> {
    let st = db.clone();
    let (items, roots) = blocking(&db, move |conn| Ok((crate::connector::list(conn, &st.connectors)?, crate::connector::roots(&st.connectors)))).await?;
    Ok(ok(serde_json::json!({ "items": items, "inboxRoots": roots })))
}

/// POST /api/ledger/connectors  新建
pub async fn create_connector(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::connector::ConnectorInput = parse_json(&body)?;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| crate::connector::create(conn, &st.connectors, input)).await?))
}

/// PUT /api/ledger/connectors/{id}  修改（密码留空则沿用已保存的）
pub async fn update_connector(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::connector::ConnectorInput = parse_json(&body)?;
    let st = db.clone();
    blocking(&db, move |conn| crate::connector::update(conn, &st.connectors, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/connectors/{id}  删除连接器（数据集保留）
pub async fn delete_connector(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    let st = db.clone();
    blocking(&db, move |conn| crate::connector::delete(conn, &st.connectors, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/connectors/test  测试来源（数据库连通性 / 文件夹匹配的文件）
pub async fn test_connector(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::connector::TestOutcome>>> {
    let input: crate::connector::ProbeInput = parse_json(&body)?;
    Ok(ok(crate::connector::test(&db, input).await?))
}

/// POST /api/ledger/connectors/preview  预览来源数据（样例行、推断的列）
pub async fn preview_connector(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::connector::PreviewOutcome>>> {
    let input: crate::connector::ProbeInput = parse_json(&body)?;
    Ok(ok(crate::connector::preview(&db, input).await?))
}

/// POST /api/ledger/connectors/{id}/run  立即同步（后台执行，返回同步记录 ID）
pub async fn run_connector(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<String>>> {
    Ok(ok(crate::connector::start(&db, &id, "manual").await?))
}

#[derive(Deserialize)]
pub struct RunsQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

/// GET /api/ledger/connectors/{id}/runs?limit=  同步记录（新的在前）
pub async fn connector_runs(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<RunsQuery>) -> AppResult<Json<Envelope<Vec<crate::connector::RunRecord>>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    Ok(ok(blocking(&db, move |conn| crate::connector::runs(conn, &id, limit)).await?))
}
