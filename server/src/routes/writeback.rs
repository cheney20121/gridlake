//! 填报回写

use super::*;

/// POST /api/ledger/datasets/{id}/writeback  { mode: upsert|append, columns, rows, keys?, operator?, source? }
/// 整批校验，有错误时不写入并返回出错的格子（errors）
pub async fn dataset_writeback(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::writeback::WritebackOutcome>>> {
    let req: crate::writeback::WritebackRequest = parse_json(&body)?;
    let (dataset_id, actor) = (id.clone(), user.clone());
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| {
        // 数据集属于收集任务时：先校验期次和单位，写入成功后记下谁提交了哪个单位；不属于任何任务时这两步什么都不做
        crate::collect::before_submit(conn, &id, &req)?;
        let r = crate::writeback::submit(conn, &id, &req)?;
        crate::collect::after_submit(conn, &id, &req, &r, &user);
        Ok(r)
    })
    .await?;
    if r.batch_id.is_some() {
        tracing::info!(dataset = %r.dataset_name, inserted = r.inserted, updated = r.updated, "writeback");
        db.cursors.invalidate();
        db.checkpoint();
        crate::webhook::emit(
            &db,
            "writeback.submitted",
            Some(&actor),
            serde_json::json!({
                "datasetId": dataset_id, "name": r.dataset_name, "batchId": r.batch_id,
                "inserted": r.inserted, "updated": r.updated, "deleted": r.deleted, "unchanged": r.unchanged,
            }),
        );
    }
    Ok(ok(r))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WritebackListQuery {
    #[serde(default)]
    pub dataset_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// GET /api/ledger/writebacks?datasetId=&limit=  提交记录（新的在前），附带能否撤销
pub async fn writebacks(State(db): State<AppState>, Query(q): Query<WritebackListQuery>) -> AppResult<Json<Envelope<Vec<crate::writeback::WritebackRecord>>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    Ok(ok(blocking(&db, move |conn| crate::writeback::list(conn, q.dataset_id.as_deref().filter(|s| !s.is_empty()), limit)).await?))
}

/// POST /api/ledger/writebacks/{id}/undo?operator=  撤销一次提交（数据集之后没有别的改动时）
pub async fn undo_writeback(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<OperatorQuery>) -> AppResult<Json<Envelope<crate::writeback::UndoOutcome>>> {
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| crate::writeback::undo(conn, &id, &q.operator)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}

/// POST /api/ledger/datasets/input  { name, columns: [{ title, kind }], primaryKey }  新建空的填报数据集，返回 id
pub async fn create_input_dataset(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::writeback::NewInputDataset = parse_json(&body)?;
    let actor = user.clone();
    let _guard = db.import_lock.lock().await;
    let (id, name) = blocking(&db, move |conn| {
        let id = crate::writeback::create_dataset(conn, &input)?;
        crate::dataset::set_owner(conn, &id, &user)?;
        let name = crate::dataset::datasets(conn)?.into_iter().find(|d| d.id == id).map(|d| d.name);
        Ok((id, name))
    })
    .await?;
    db.checkpoint();
    crate::webhook::emit(&db, "dataset.created", Some(&actor), serde_json::json!({ "datasetId": id, "name": name, "source": "input" }));
    Ok(ok(id))
}

/// GET /api/ledger/datasets/{id}/anomalies  异常发现：最近 7 天相对此前 28 天明显偏离的走势与分组（受限用户只扫描自己范围内的数据）
pub async fn dataset_anomalies(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::anomaly::ScanResult>>> {
    Ok(ok(blocking_heavy(&db, move |conn| crate::anomaly::scan_dataset(conn, &id)).await?))
}

/// GET /api/ledger/datasets/{id}/usage  使用情况：引用它的工作簿 / 模板 / 视图 / SQL / 连接 / 推送，近 30 天访问次数
pub async fn dataset_usage(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::usage::DatasetUsage>>> {
    Ok(ok(blocking(&db, move |conn| crate::usage::usage(conn, &id)).await?))
}
