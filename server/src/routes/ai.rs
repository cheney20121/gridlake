//! AI 助手

use super::*;

/// GET /api/ledger/ai/status  是否已配置模型（模型名 @ 主机，不含密钥）
pub async fn ai_status() -> AppResult<Json<Envelope<crate::ai::Status>>> {
    Ok(ok(crate::ai::status()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiThreadsQuery {
    pub workbook_id: String,
}

/// GET /api/ledger/ai/threads?workbookId=  工作簿的会话（新的在前）
pub async fn ai_threads(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Query(q): Query<AiThreadsQuery>) -> AppResult<Json<Envelope<Vec<crate::ai::Thread>>>> {
    Ok(ok(blocking(&db, move |conn| crate::ai::threads(conn, &q.workbook_id, &user)).await?))
}

/// POST /api/ledger/ai/threads  { workbookId, operator? }  新会话
pub async fn ai_create_thread(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::ai::Thread>>> {
    let mut input: crate::ai::NewThread = parse_json(&body)?;
    input.owner_id = user.enforced.then_some(user.id);
    Ok(ok(blocking(&db, move |conn| crate::ai::create_thread(conn, input)).await?))
}

/// GET /api/ledger/ai/threads/{id}  会话内容：提问、回答、工具调用、建议及处理结果
pub async fn ai_thread(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::ai::ThreadDetail>>> {
    Ok(ok(blocking(&db, move |conn| crate::ai::detail(conn, &id)).await?))
}

#[derive(Deserialize)]
pub struct AiSummaryBody {
    #[serde(default)]
    pub summary: Option<String>,
}

/// PUT /api/ledger/ai/threads/{id}/summary  { summary }  手动编辑会话摘要；留空 = 清除，之后提问时重新整理
pub async fn ai_set_summary(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: AiSummaryBody = parse_json(&body)?;
    blocking(&db, move |conn| crate::ai::set_summary(conn, &id, input.summary.as_deref())).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/ai/threads/{id}
pub async fn ai_delete_thread(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    blocking(&db, move |conn| crate::ai::delete_thread(conn, &id)).await?;
    Ok(ok(true))
}

/// POST /api/ledger/ai/threads/{id}/chat  { message, context } 或 { resume: true }（接着回答被中断的问题）  提问；以 SSE 返回事件：
/// start / status{text}（如整理摘要）/ summary{summary} / text{delta} / break / tool{id,name,status,summary,actionId} / action{action} / done / error{message}
pub async fn ai_chat(
    State(db): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AppResult<axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>> {
    let input: crate::ai::ChatInput = parse_json(&body)?;
    let tid = id.clone();
    let input = blocking(&db, move |conn| crate::ai::begin(conn, &tid, &input).map(|()| input)).await?;
    let rx = crate::ai::subscribe(&id).ok_or_else(|| AppError::bad("回答启动失败"))?;
    // 回答在后台任务中进行，与这个连接无关：断开后继续，可经 /events 重新连上
    // 后台任务里不再有请求的任务上下文：带上数据范围，AI 的查询与建议同样受限
    let scope = crate::scope::current_task_scope();
    let (st, tid) = (db.clone(), id);
    tokio::spawn(async move {
        match scope {
            Some(s) => crate::scope::CURRENT.scope(s, crate::ai::chat(st, tid, input)).await,
            None => crate::ai::chat(st, tid, input).await,
        }
    });
    Ok(ai_sse(rx))
}

fn ai_sse(rx: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>) -> axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (Ok(axum::response::sse::Event::default().data(v.to_string())), rx)) });
    axum::response::sse::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

/// GET /api/ledger/ai/threads/{id}/events  重新连上正在进行的回答：先补发本次回答已产生的事件，再继续推送；
/// 没有在回答时只返回 { type: idle }
pub async fn ai_events(Path(id): Path<String>) -> axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let rx = crate::ai::subscribe(&id).unwrap_or_else(|| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = tx.send(serde_json::json!({ "type": "idle" }));
        rx
    });
    ai_sse(rx)
}

/// POST /api/ledger/ai/threads/{id}/stop  停止正在进行的回答（已产生的文字保留）
pub async fn ai_stop(Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    Ok(ok(crate::ai::stop(&id)))
}

/// POST /api/ledger/ai/actions/{id}  { status: applied | dismissed, result?, operator? }  记录建议的处理结果
pub async fn ai_decide(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: crate::ai::DecideInput = parse_json(&body)?;
    blocking(&db, move |conn| crate::ai::decide(conn, &id, input)).await?;
    Ok(ok(true))
}
