//! 多人填报：收集任务

use super::*;

/// GET /api/ledger/collects  收集任务列表（含最近一期的提交进度）
pub async fn list_collects(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<crate::collect::CollectView>>>> {
    Ok(ok(blocking(&db, crate::collect::list).await?))
}

/// POST /api/ledger/collects  新建任务：{ name, datasetId | newTable, unitColumn?, periodColumn?, dimension?, units: [{ unit, userId? }] }
/// 单位与负责人会同步成数据权限（负责人只能看 / 填自己负责的单位）。新建填报表时数据集建在这里，所以和导入一样串行
pub async fn create_collect(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::collect::CollectInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let id = blocking(&db, move |conn| crate::collect::create(conn, &user, input)).await?;
    db.scope.invalidate();
    db.checkpoint();
    Ok(ok(id))
}

/// GET /api/ledger/collects/mine  我负责的单位在最近几期里填了没有（受数据范围限制的用户也能看）
pub async fn my_collects(State(db): State<AppState>, Extension(user): Extension<CurrentUser>) -> AppResult<Json<Envelope<Vec<crate::collect::MyTask>>>> {
    Ok(ok(blocking(&db, move |conn| crate::collect::mine(conn, &user)).await?))
}

#[derive(Deserialize)]
pub struct PeriodQuery {
    #[serde(default)]
    pub period: Option<String>,
}

/// GET /api/ledger/collects/{id}?period=  看板：已开启的期次，以及某一期（默认最近一期）每个单位的提交状态
pub async fn collect_detail(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<PeriodQuery>) -> AppResult<Json<Envelope<crate::collect::Detail>>> {
    Ok(ok(blocking(&db, move |conn| crate::collect::detail(conn, &id, q.period.as_deref())).await?))
}

/// PUT /api/ledger/collects/{id}  { name, units }  改名、整体替换填报单位与负责人
pub async fn update_collect(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::collect::UpdateInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::collect::update(conn, &user, &id, input)).await?;
    db.scope.invalidate();
    Ok(ok(true))
}

/// DELETE /api/ledger/collects/{id}  删除任务及其期次、提交记录；数据集和里面的数据保留
pub async fn delete_collect(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::collect::delete(conn, &user, &id)).await?;
    db.scope.invalidate();
    Ok(ok(true))
}

#[derive(Deserialize)]
pub struct OpenPeriod {
    pub period: String,
}

/// POST /api/ledger/collects/{id}/periods  { period }  开启新一期（如 2026-10）
pub async fn open_collect_period(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, Json(b): Json<OpenPeriod>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::collect::open_period(conn, &user, &id, &b.period)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/collects/{id}/periods/{period}  删除还没有人提交过的一期
pub async fn delete_collect_period(State(db): State<AppState>, Path((id, period)): Path<(String, String)>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::collect::delete_period(conn, &id, &period)).await?;
    Ok(ok(true))
}
