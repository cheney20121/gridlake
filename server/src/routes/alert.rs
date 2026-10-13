//! 阈值告警

use super::*;

/// GET /api/ledger/alerts  告警规则、可选的推送渠道（地址已打码）、邮件是否已配置
pub async fn list_alerts(State(db): State<AppState>) -> AppResult<Json<Envelope<serde_json::Value>>> {
    let st = db.clone();
    let (items, channels) = blocking(&db, move |conn| Ok((crate::report::alert::list(conn)?, crate::report::list_channels(conn, &st)?))).await?;
    Ok(ok(serde_json::json!({
        "items": items,
        "channels": channels,
        "smtpConfigured": crate::report::channels::smtp_configured(),
    })))
}

/// POST /api/ledger/alerts  新建告警（受数据范围限制的用户不能建）
pub async fn create_alert(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::report::alert::AlertInput = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::report::alert::create(conn, &user, input)).await?))
}

/// PUT /api/ledger/alerts/{id}  只有创建人或管理员能改
pub async fn update_alert(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::report::alert::AlertInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::report::alert::update(conn, &user, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/alerts/{id}  删除告警及其评估记录（创建人或管理员）
pub async fn delete_alert(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::report::alert::delete(conn, &user, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/alerts/preview  试算：按当前数据算一遍会命中哪些分组，不保存、不发送
pub async fn preview_alert(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::report::alert::PreviewOutcome>>> {
    let input: crate::report::alert::AlertInput = parse_json(&body)?;
    Ok(ok(blocking_heavy(&db, move |conn| crate::report::alert::preview(conn, &input, crate::db::today_local())).await?))
}

/// POST /api/ledger/alerts/{id}/evaluate  立即评估一次并按规则通知（等待完成，返回本次记录）
pub async fn evaluate_alert(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::report::alert::AlertRun>>> {
    Ok(ok(crate::report::alert::run(&db, &id, "manual").await?))
}

/// GET /api/ledger/alerts/{id}/runs?limit=  评估记录（新的在前）
pub async fn alert_runs(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<RunsQuery>) -> AppResult<Json<Envelope<Vec<crate::report::alert::AlertRun>>>> {
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    Ok(ok(blocking(&db, move |conn| crate::report::alert::runs(conn, &id, limit)).await?))
}
