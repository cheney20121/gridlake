//! 定时推送

use super::*;

/// GET /api/ledger/reports  推送任务、渠道（地址已打码）、邮件是否已配置、看板链接前缀
pub async fn list_reports(State(db): State<AppState>) -> AppResult<Json<Envelope<serde_json::Value>>> {
    let st = db.clone();
    let (items, channels) = blocking(&db, move |conn| Ok((crate::report::list(conn)?, crate::report::list_channels(conn, &st)?))).await?;
    Ok(ok(serde_json::json!({
        "items": items,
        "channels": channels,
        "smtpConfigured": crate::report::channels::smtp_configured(),
        "publicUrl": crate::report::public_url(),
    })))
}

/// POST /api/ledger/reports  新建推送任务
pub async fn create_report(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::report::ReportInput = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::report::create(conn, input)).await?))
}

/// PUT /api/ledger/reports/{id}
pub async fn update_report(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::report::ReportInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::report::update(conn, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/reports/{id}
pub async fn delete_report(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::report::delete(conn, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/reports/{id}/send  立即发送（等待完成，返回本次记录）
pub async fn send_report(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::report::RunRecord>>> {
    Ok(ok(crate::report::send(&db, &id, "manual").await?))
}

/// GET /api/ledger/reports/{id}/runs?limit=  推送记录（新的在前）
pub async fn report_runs(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<RunsQuery>) -> AppResult<Json<Envelope<Vec<crate::report::RunRecord>>>> {
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    Ok(ok(blocking(&db, move |conn| crate::report::runs(conn, &id, limit)).await?))
}

/// POST /api/ledger/reports/preview  { workbookId, sheetId, title? }  预览推送内容
pub async fn preview_report(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::report::PreviewOutcome>>> {
    let input: crate::report::PreviewInput = parse_json(&body)?;
    Ok(ok(crate::report::preview(&db, input).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetsQuery {
    pub workbook_id: String,
}

/// GET /api/ledger/reports/sheets?workbookId=  工作簿里含仪表盘部件的工作表
pub async fn report_sheets(State(db): State<AppState>, Query(q): Query<SheetsQuery>) -> AppResult<Json<Envelope<Vec<crate::report::SheetOption>>>> {
    Ok(ok(blocking(&db, move |conn| crate::report::sheets(conn, &q.workbook_id)).await?))
}

/// POST /api/ledger/report-channels  新建推送渠道
pub async fn create_report_channel(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<String>>> {
    let input: crate::report::ChannelInput = parse_json(&body)?;
    let st = db.clone();
    Ok(ok(blocking(&db, move |conn| crate::report::create_channel(conn, &st, input)).await?))
}

/// PUT /api/ledger/report-channels/{id}  修改（地址留空沿用已保存的）
pub async fn update_report_channel(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::report::ChannelInput = parse_json(&body)?;
    let st = db.clone();
    blocking(&db, move |conn| crate::report::update_channel(conn, &st, &id, input)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/report-channels/{id}
pub async fn delete_report_channel(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::report::delete_channel(conn, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/report-channels/{id}/test  发送测试消息
pub async fn test_report_channel(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    crate::report::test_channel(&db, &id).await?;
    Ok(ok(true))
}
