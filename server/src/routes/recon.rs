//! 逐笔对账

use super::*;

/// 勾对人：登录用户的名字；没有登录体系时用请求里的 operator
fn who(user: &CurrentUser, fallback: &str) -> String {
    if user.enforced && !user.name.is_empty() { user.name.clone() } else { fallback.trim().to_string() }
}

#[derive(Deserialize)]
pub struct ReconBody {
    #[serde(default)]
    pub name: String,
    pub def: crate::recon::ReconDef,
    /// 修改时：换了数据集 / 金额列后确认清空已有勾对
    #[serde(default, rename = "resetMatches")]
    pub reset_matches: bool,
}

/// POST /api/ledger/recons/preview  { def }  新建前预览：两侧行数、日期范围、容易填错的地方
pub async fn recon_preview(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::recon::Preview>>> {
    let req: ReconBody = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| crate::recon::preview(conn, &req.def)).await?))
}

/// POST /api/ledger/recons  { name, def }  创建对账（同时得到一个同名的结果数据集），返回数据集 id
pub async fn create_recon(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let req: ReconBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let id = blocking(&db, move |conn| {
        let id = crate::recon::create(conn, &req.name, &req.def)?;
        dataset::set_owner(conn, &id, &user)?;
        Ok(id)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryBody {
    pub name: String,
    pub def: crate::summary::SummaryDef,
}

/// POST /api/ledger/datasets/summaries  { name, def: { source, rows, values, filters?, keyword? } }  透视表转为数据表：新建汇总数据集，返回数据集 id
pub async fn create_summary(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let req: SummaryBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let id = blocking(&db, move |conn| {
        let id = crate::summary::create(conn, &req.name, &req.def)?;
        dataset::set_owner(conn, &id, &user)?;
        Ok(id)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(id))
}

/// GET /api/ledger/recons/{id}  对账的定义
pub async fn get_recon(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::recon::Info>>> {
    Ok(ok(blocking_read(&db, move |conn| crate::recon::info(conn, &id)).await?))
}

/// PUT /api/ledger/recons/{id}  { name, def, resetMatches? }  修改（所有者或管理员）
pub async fn update_recon(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: ReconBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| crate::recon::update(conn, &id, &req.name, &req.def, req.reset_matches)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconSummaryQuery {
    #[serde(default)]
    pub as_of: Option<String>,
}

/// GET /api/ledger/recons/{id}/summary?asOf=  汇总：两侧笔数与金额、截至日的未达账项、勾对组统计
pub async fn recon_summary(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<ReconSummaryQuery>) -> AppResult<Json<Envelope<crate::recon::Summary>>> {
    Ok(ok(blocking_read(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::summary(conn, &ctx, q.as_of.as_deref())
    })
    .await?))
}

/// GET /api/ledger/recons/{id}/items?side=left|right&status=open|matched|all&q=&amount=&from=&to=&offset=&limit=  行列表
pub async fn recon_items(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<crate::recon::ItemQuery>) -> AppResult<Json<Envelope<crate::recon::ItemPage>>> {
    Ok(ok(blocking_read(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::items(conn, &ctx, &q)
    })
    .await?))
}

/// GET /api/ledger/recons/{id}/groups?method=&withDiff=&broken=&q=&offset=&limit=  勾对组（含组内的行）
pub async fn recon_groups(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<crate::recon::GroupQuery>) -> AppResult<Json<Envelope<crate::recon::GroupPage>>> {
    Ok(ok(blocking_read(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::groups(conn, &ctx, &q)
    })
    .await?))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReconAutoBody {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/recons/{id}/auto  { dryRun? }  自动勾对（dryRun 只试算不保存）
pub async fn recon_auto(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::recon::AutoOutcome>>> {
    let req: ReconAutoBody = if body.is_empty() { ReconAutoBody::default() } else { parse_json(&body)? };
    let _guard = db.import_lock.lock().await;
    let operator = who(&user, &req.operator);
    let dry = req.dry_run;
    let r = blocking(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::auto_match(conn, &ctx, &operator, dry)
    })
    .await?;
    if !dry {
        db.cursors.invalidate();
        db.checkpoint();
    }
    Ok(ok(r))
}

#[derive(Deserialize)]
pub struct ReconManualBody {
    #[serde(flatten)]
    pub req: crate::recon::ManualRequest,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/recons/{id}/match  { left: [行标识], right: [行标识], note?, force? }  手工勾对（任意多对多）
pub async fn recon_match(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::recon::ManualOutcome>>> {
    let req: ReconManualBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let operator = who(&user, &req.operator);
    let r = blocking(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::manual_match(conn, &ctx, &req.req, &operator)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}

/// POST /api/ledger/recons/{id}/unmatch  { groups: [勾对组] } 或 { mode: auto | all }  取消勾对
pub async fn recon_unmatch(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::recon::UnmatchOutcome>>> {
    let req: crate::recon::UnmatchRequest = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| {
        let ctx = crate::recon::context(conn, &id)?;
        crate::recon::unmatch(conn, &ctx, &req)
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}
