//! 服务端工作簿：保存、版本、共享、协作

use super::*;

#[derive(Deserialize)]
pub struct CreateWorkbookQuery {
    pub name: String,
    #[serde(default)]
    pub operator: String,
    /// 新建到这个文件夹（空为根目录）
    #[serde(default)]
    pub folder: Option<String>,
}

/// GET /api/ledger/workbooks
pub async fn list_workbooks(State(db): State<AppState>, Extension(user): Extension<CurrentUser>) -> AppResult<Json<Envelope<Vec<WorkbookMeta>>>> {
    Ok(ok(blocking(&db, move |conn| workbook::list_for(conn, &user)).await?))
}

/// POST /api/ledger/workbooks?name=&operator=  body: gzip 压缩的工作簿 JSON
pub async fn create_workbook(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Query(q): Query<CreateWorkbookQuery>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let _guard = db.import_lock.lock().await;
    let m = blocking(&db, move |conn| {
        let m = workbook::create_in(conn, &q.name, &body, &q.operator, q.folder.as_deref().filter(|s| !s.is_empty()))?;
        workbook::set_owner(conn, &m.id, &user)?;
        workbook::meta(conn, &m.id)
    })
    .await?;
    db.checkpoint();
    Ok(ok(m))
}

/// GET /api/ledger/workbooks/{id}  元数据
pub async fn workbook_meta(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    Ok(ok(blocking(&db, move |conn| workbook::meta(conn, &id)).await?))
}

fn gzip_json_response(data: Vec<u8>, version: i64) -> axum::response::Response {
    use axum::http::header;
    use axum::response::IntoResponse;
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8".to_string()),
            // 存储的就是 gzip：原样返回，由浏览器解压
            (header::CONTENT_ENCODING, "gzip".to_string()),
            (header::HeaderName::from_static("x-workbook-version"), version.to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        data,
    )
        .into_response()
}

/// GET /api/ledger/workbooks/{id}/content  工作簿 JSON（Content-Encoding: gzip，X-Workbook-Version 为版本号）
pub async fn workbook_content(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<axum::response::Response> {
    let (m, data) = blocking(&db, move |conn| workbook::content(conn, &id)).await?;
    Ok(gzip_json_response(data, m.version))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveWorkbookQuery {
    pub base_version: i64,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub operator: String,
}

/// PUT /api/ledger/workbooks/{id}/content?baseVersion=&force=&auto=&description=&operator=  body: gzip 压缩的 JSON
/// 冲突时返回 saved: false + conflict（不覆盖）
pub async fn save_workbook(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<SaveWorkbookQuery>, headers: axum::http::HeaderMap, body: axum::body::Bytes) -> AppResult<Json<Envelope<SaveResult>>> {
    let _guard = db.import_lock.lock().await;
    let wb = id.clone();
    let operator = q.operator.clone();
    let r = blocking(&db, move |conn| {
        workbook::save(conn, &id, &body, &workbook::SaveRequest { base_version: q.base_version, force: q.force, operator: &q.operator, description: &q.description, is_auto: q.auto })
    })
    .await?;
    if r.saved {
        db.checkpoint();
        // 协作：保存的快照已包含到哪个序号（没带说明不是协作客户端写的，房间里的人需要重新载入）
        let applied = headers.get("x-collab-seq").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        crate::collab::saved(&wb, r.version, applied, &operator);
    }
    Ok(ok(r))
}

#[derive(Deserialize)]
pub struct WorkbookNameBody {
    pub name: String,
    #[serde(default)]
    pub operator: String,
}

/// PUT /api/ledger/workbooks/{id}  { name }  改名
pub async fn rename_workbook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let req: WorkbookNameBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    Ok(ok(blocking(&db, move |conn| workbook::rename(conn, &id, &req.name)).await?))
}

#[derive(Deserialize, Default)]
pub struct OperatorQuery {
    #[serde(default)]
    pub operator: String,
}

/// DELETE /api/ledger/workbooks/{id}?operator=  移到回收站（可恢复）
pub async fn delete_workbook(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<OperatorQuery>) -> AppResult<Json<Envelope<bool>>> {
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| workbook::move_to_trash(conn, &id, &q.operator)).await?;
    db.checkpoint();
    Ok(ok(true))
}

/// GET /api/ledger/workbooks/trash  回收站
pub async fn workbook_trash(State(db): State<AppState>, Extension(user): Extension<CurrentUser>) -> AppResult<Json<Envelope<workbook::TrashList>>> {
    Ok(ok(blocking(&db, move |conn| workbook::trash_for(conn, &user)).await?))
}

/// DELETE /api/ledger/workbooks/trash  清空回收站（不可恢复）
pub async fn empty_workbook_trash(State(db): State<AppState>) -> AppResult<Json<Envelope<usize>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, workbook::empty_trash).await?;
    db.checkpoint();
    Ok(ok(n))
}

/// POST /api/ledger/workbooks/{id}/untrash  从回收站恢复
pub async fn untrash_workbook(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let _guard = db.import_lock.lock().await;
    let m = blocking(&db, move |conn| workbook::untrash(conn, &id)).await?;
    db.checkpoint();
    Ok(ok(m))
}

/// DELETE /api/ledger/workbooks/{id}/purge  彻底删除（只能删回收站里的，含全部版本）
pub async fn purge_workbook(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<bool>>> {
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| workbook::purge(conn, &id)).await?;
    db.checkpoint();
    Ok(ok(true))
}

#[derive(Deserialize)]
pub struct StarBody {
    pub starred: bool,
}

/// PUT /api/ledger/workbooks/{id}/star  { starred }
pub async fn star_workbook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: StarBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| workbook::set_starred(conn, &id, req.starred)).await?;
    Ok(ok(true))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveBody {
    pub ids: Vec<String>,
    #[serde(default)]
    pub folder_id: Option<String>,
}

/// POST /api/ledger/workbooks/move  { ids, folderId }  移到文件夹（folderId 为空表示根目录）
pub async fn move_workbooks(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<usize>>> {
    let req: MoveBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| {
        for id in &req.ids {
            workbook::require(conn, id, &user, Need::Edit, false)?;
        }
        workbook::move_to(conn, &req.ids, req.folder_id.as_deref().filter(|s| !s.is_empty()))
    })
    .await?;
    db.checkpoint();
    Ok(ok(n))
}

#[derive(Deserialize)]
pub struct RangesBody {
    pub items: Vec<crate::xref::RangeRequest>,
}

/// POST /api/ledger/workbooks/ranges  { items: [{ workbook, range }] }  跨工作簿引用：取其他工作簿已保存版本里的区域值
pub async fn workbook_ranges(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<Vec<crate::xref::RangeResult>>>> {
    let req: RangesBody = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::xref::ranges(conn, req.items, &user)).await?))
}

/// GET /api/ledger/folders
pub async fn list_folders(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<workbook::Folder>>>> {
    Ok(ok(blocking(&db, workbook::folders).await?))
}

/// POST /api/ledger/folders  { name, parentId, operator }
pub async fn create_folder(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<workbook::Folder>>> {
    let req: workbook::FolderInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    Ok(ok(blocking(&db, move |conn| workbook::create_folder(conn, &req)).await?))
}

/// PUT /api/ledger/folders/{id}  { name, parentId }  改名 / 移动
pub async fn update_folder(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: workbook::FolderInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| workbook::update_folder(conn, &id, &req)).await?;
    Ok(ok(true))
}

/// DELETE /api/ledger/folders/{id}?operator=  删除文件夹（含子文件夹），其中的工作簿移到回收站
pub async fn delete_folder(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<OperatorQuery>) -> AppResult<Json<Envelope<usize>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| workbook::delete_folder(conn, &id, &q.operator)).await?;
    db.checkpoint();
    Ok(ok(n))
}

/// POST /api/ledger/workbooks/{id}/duplicate  { name, operator }  复制为新工作簿
pub async fn duplicate_workbook(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let req: WorkbookNameBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let m = blocking(&db, move |conn| {
        let m = workbook::duplicate(conn, &id, &req.name, &req.operator)?;
        workbook::set_owner(conn, &m.id, &user)?;
        workbook::meta(conn, &m.id)
    })
    .await?;
    db.checkpoint();
    Ok(ok(m))
}

/// GET /api/ledger/workbooks/{id}/versions
pub async fn workbook_versions(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Vec<VersionInfo>>>> {
    Ok(ok(blocking(&db, move |conn| workbook::versions(conn, &id)).await?))
}

/// GET /api/ledger/workbooks/{id}/versions/{version}/content
pub async fn workbook_version_content(State(db): State<AppState>, Path((id, version)): Path<(String, i64)>) -> AppResult<axum::response::Response> {
    let data = blocking(&db, move |conn| workbook::version_content(conn, &id, version)).await?;
    Ok(gzip_json_response(data, version))
}

#[derive(Deserialize)]
pub struct ShareBody {
    pub visibility: String,
}

/// PUT /api/ledger/workbooks/{id}/share  { visibility: private | view | edit }  修改共享范围（所有者与管理员）
pub async fn share_workbook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let req: ShareBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let m = blocking(&db, move |conn| {
        workbook::set_visibility(conn, &id, &req.visibility)?;
        workbook::meta(conn, &id)
    })
    .await?;
    db.checkpoint();
    Ok(ok(m))
}

#[derive(Deserialize)]
pub struct RestoreBody {
    pub version: i64,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/workbooks/{id}/restore  { version, operator }  以历史版本内容生成新版本
pub async fn restore_workbook(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<WorkbookMeta>>> {
    let req: RestoreBody = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let wb = id.clone();
    let operator = req.operator.clone();
    let m = blocking(&db, move |conn| workbook::restore(conn, &id, req.version, &req.operator)).await?;
    db.checkpoint();
    crate::collab::saved(&wb, m.version, None, &operator);
    Ok(ok(m))
}

/// POST /api/ledger/datasets/{id}/pivot  数据透视表（rows / columns / values / filters / sort / top / subtotals / grandTotals）
pub async fn dataset_pivot(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::pivot::PivotResult>>> {
    let req: crate::pivot::PivotRequest = parse_json(&body)?;
    crate::usage::hit(&id, crate::usage::PIVOT);
    Ok(ok(blocking_heavy(&db, move |conn| crate::pivot::pivot(conn, &id, &req)).await?))
}
