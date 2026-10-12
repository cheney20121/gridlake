//! 模板

use super::*;

/// GET /api/ledger/templates  内置与自建模板（需要的数据集与列、概要、使用次数）
pub async fn list_templates(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<crate::template::TemplateView>>>> {
    Ok(ok(blocking(&db, crate::template::list).await?))
}

/// POST /api/ledger/templates  { workbookId, name, description, category, replaceId?, operator? }  把已保存的工作簿发布为模板
pub async fn publish_template(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::template::TemplateView>>> {
    let input: crate::template::PublishInput = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::template::publish(conn, input)).await?))
}

/// PUT /api/ledger/templates/{id}  { name, description, category }
pub async fn update_template(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::template::MetaInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::template::update_meta(conn, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/templates/{id}  删除自建模板（已用它创建的工作簿不受影响）
pub async fn delete_template(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::template::delete(conn, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/templates/{id}/use  { name, bindings: { key: { datasetId, columns } }, clearSlicers?, operator? }  用模板创建工作簿
pub async fn use_template(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::template::UseOutcome>>> {
    let input: crate::template::UseInput = parse_json(&body)?;
    // 填参数会生成 / 重算保存的 SQL 查询（按服务账号运行）：与「生成实例」一样只对完整数据权限的用户开放
    let _guard = if input.params.is_empty() {
        None
    } else {
        if let Some(scope) = crate::scope::current_task_scope() {
            return Err(AppError::Forbidden(format!("按参数生成数据集需要完整的数据权限，请改为选择数据集。{}", crate::scope::restriction_note(&scope).unwrap_or_default())));
        }
        Some(db.import_lock.lock().await)
    };
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| {
        let mut out = crate::template::use_template(&st, conn, &id, input)?;
        workbook::set_owner(conn, &out.workbook.id, &user)?;
        out.workbook = workbook::meta(conn, &out.workbook.id)?;
        Ok(out)
    })
    .await?))
}

/// GET /api/ledger/templates/{id}/uses  最近 50 次使用（生成的工作簿）
pub async fn template_uses(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Vec<crate::template::UseRecord>>>> {
    Ok(ok(blocking(&db, move |conn| crate::template::uses(conn, &id)).await?))
}
