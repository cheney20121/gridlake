//! 外部数据集：直接查询服务器上的 Parquet 文件，不导入

use super::*;

#[derive(Deserialize)]
pub struct CreateExternalBody {
    pub name: String,
    #[serde(flatten)]
    pub input: crate::external::ExternalInput,
}

/// POST /api/ledger/datasets/external/preview  { path, pattern, recursive, hive }  预览外部 Parquet 文件：匹配的文件、总行数、列与类型、前几行
pub async fn external_preview(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::external::ExternalPreview>>> {
    let input: crate::external::ExternalInput = parse_json(&body)?;
    let roots = db.config.inbox_roots.clone();
    Ok(ok(blocking(&db, move |conn| crate::external::preview(conn, &roots, &input)).await?))
}

/// POST /api/ledger/datasets/external  { name, path, pattern, recursive, hive }  新建外部数据集（只读，不导入）
pub async fn create_external(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let req: CreateExternalBody = parse_json(&body)?;
    let roots = db.config.inbox_roots.clone();
    let (name, actor) = (req.name.trim().to_string(), user.clone());
    let _guard = db.import_lock.lock().await;
    let id = blocking(&db, move |conn| {
        let id = crate::external::create(conn, &roots, &req.name, &req.input)?;
        crate::dataset::set_owner(conn, &id, &user)?;
        Ok(id)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    crate::webhook::emit(&db, "dataset.created", Some(&actor), serde_json::json!({ "datasetId": id, "name": name, "source": "external" }));
    Ok(ok(id))
}

/// POST /api/ledger/datasets/{id}/external/refresh  重新查找文件、读取结构（文件有增减或结构变化后）
pub async fn refresh_external(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::external::RefreshOutcome>>> {
    let roots = db.config.inbox_roots.clone();
    let _guard = db.import_lock.lock().await;
    let outcome = blocking(&db, move |conn| crate::external::refresh(conn, &roots, &id)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(outcome))
}
