//! SQL 查询

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlRunBody {
    pub sql: String,
    #[serde(default)]
    pub limit: Option<usize>,
    /// 参数化查询：参数定义与取值
    #[serde(default)]
    pub params: Vec<crate::sqlq::params::ParamDef>,
    #[serde(default)]
    pub values: std::collections::HashMap<String, String>,
}

/// POST /api/ledger/sql/run  { sql, limit?, params?, values? }  执行只读查询（最多返回 1000 行）
pub async fn sql_run(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::RunResult>>> {
    let b: SqlRunBody = parse_json(&body)?;
    Ok(ok(blocking_heavy(&db, move |conn| crate::sqlq::run_with_params(conn, &b.sql, b.limit.unwrap_or(1000), &b.params, &b.values)).await?))
}

/// GET /api/ledger/sql/queries  已保存的查询（关联数据集、最近一次重算、下次重算时间）
pub async fn sql_queries(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<crate::sqlq::SavedQuery>>>> {
    Ok(ok(blocking(&db, crate::sqlq::list).await?))
}

/// POST /api/ledger/sql/queries  { name, sql, description?, saveAsDataset?, columnKinds?, schedule?, operator? }
pub async fn sql_create_query(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::SavedQuery>>> {
    let input: crate::sqlq::QueryInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let q = crate::sqlq::save(&st, conn, None, input)?;
        // 另存出来的数据集归创建人（与导入、对账、关联视图一致），否则连创建人自己也改不了、删不掉
        if let Some(d) = &q.dataset_id {
            dataset::set_owner(conn, d, &user)?;
        }
        Ok(q)
    })
    .await?))
}

/// PUT /api/ledger/sql/queries/{id}  修改（已关联数据集时立即按新 SQL 重算）
pub async fn sql_update_query(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::SavedQuery>>> {
    let input: crate::sqlq::QueryInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let q = crate::sqlq::save(&st, conn, Some(&id), input)?;
        // 修改时才第一次另存出数据集的，同样归当前用户（已有所有者的不动）
        if let Some(d) = &q.dataset_id {
            dataset::set_owner(conn, d, &user)?;
        }
        Ok(q)
    })
    .await?))
}

/// DELETE /api/ledger/sql/queries/{id}  删除查询（另存的数据集保留）
pub async fn sql_delete_query(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::sqlq::delete(conn, &id)).await?;
    Ok(ok(true))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SqlRefreshBody {
    /// 还没有关联数据集时：新数据集名称
    #[serde(default)]
    pub dataset_name: Option<String>,
}

/// POST /api/ledger/sql/queries/{id}/refresh  { datasetName? }  立即重算并写入数据集
pub async fn sql_refresh_query(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::Materialized>>> {
    let b: SqlRefreshBody = if body.is_empty() { SqlRefreshBody::default() } else { parse_json(&body)? };
    let _guard = db.import_lock.lock().await;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let m = crate::sqlq::refresh_now(&st, conn, &id, b.dataset_name.as_deref())?;
        dataset::set_owner(conn, &m.dataset_id, &user)?;
        Ok(m)
    })
    .await?))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SqlInstanceBody {
    #[serde(default)]
    pub values: std::collections::HashMap<String, String>,
    /// 实例（同时也是数据集）的名称；不填按「查询名（参数=值）」
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub operator: Option<String>,
}

/// POST /api/ledger/sql/queries/{id}/instances  { values, name?, operator? }  用一组参数值生成实例（独立的查询 + 新数据集）
pub async fn sql_instantiate_query(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::sqlq::SavedQuery>>> {
    let b: SqlInstanceBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let q = crate::sqlq::instantiate(&st, conn, &id, b.values, b.name.as_deref(), b.operator)?;
        if let Some(d) = &q.dataset_id {
            dataset::set_owner(conn, d, &user)?;
        }
        Ok(q)
    })
    .await?))
}
