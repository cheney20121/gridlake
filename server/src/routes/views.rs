//! 关联视图与计算列

use super::*;

#[derive(Deserialize)]
pub struct CreateViewBody {
    pub name: String,
    #[serde(flatten)]
    pub def: crate::views::ViewDef,
}

/// POST /api/ledger/datasets/views/preview  { left, right, on: [{ left, right }], columns: [{ source, title }] }  关联预览（匹配率、关联不上的键）
pub async fn view_preview(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::views::JoinPreview>>> {
    let v: crate::views::ViewDef = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::views::preview(conn, &v)).await?))
}

/// POST /api/ledger/datasets/views  { name, left, right, on, columns }  创建关联视图
pub async fn create_view(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let req: CreateViewBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let id = blocking(&db, move |conn| {
        let id = crate::views::create(conn, &req.name, &req.def)?;
        crate::dataset::set_owner(conn, &id, &user)?;
        Ok(id)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(id))
}

/// PUT /api/ledger/datasets/views/{id}  { name, left, right, on, columns }  修改关联视图
pub async fn update_view(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: CreateViewBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| crate::views::update(conn, &id, &req.name, &req.def)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

#[derive(Deserialize)]
pub struct CalcBody {
    #[serde(default)]
    pub name: String,
    pub formula: String,
    pub kind: Option<String>,
}

/// POST /api/ledger/datasets/{id}/calc/preview  { formula }  计算列预览（类型、样例、空值行数）
pub async fn calc_preview(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<dataset::CalcPreview>>> {
    let req: CalcBody = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| dataset::preview_calc(conn, &id, &req.formula)).await?))
}

/// POST /api/ledger/datasets/{id}/calc  { name, formula, kind? }  新增计算列
pub async fn add_calc(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: CalcBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| dataset::add_calc(conn, &id, &req.name, &req.formula, req.kind.as_deref())).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

/// DELETE /api/ledger/datasets/{id}/calc/{name}
pub async fn remove_calc(State(db): State<AppState>, Path((id, name)): Path<(String, String)>) -> AppResult<Json<Envelope<bool>>> {
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| dataset::remove_calc(conn, &id, &name)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

/// PUT /api/ledger/datasets/{id}/calc/{name}  { formula, kind? }  修改计算列
pub async fn update_calc(State(db): State<AppState>, Path((id, name)): Path<(String, String)>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: CalcBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| dataset::update_calc(conn, &id, &name, &req.formula, req.kind.as_deref())).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}
