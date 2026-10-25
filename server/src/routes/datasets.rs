//! 大数据表（数据集引擎）与通用数据集导入、管理

use super::*;

/// GET /api/ledger/datasets  数据集与列定义
pub async fn list_datasets(State(db): State<AppState>, filter: Option<axum::extract::Extension<crate::auth::DatasetFilter>>) -> AppResult<Json<Envelope<Vec<DatasetInfo>>>> {
    let mut all = blocking(&db, dataset::list).await?;
    // 限定了数据集的访问令牌只看得到它限定的那几个
    if let Some(axum::extract::Extension(crate::auth::DatasetFilter(allowed))) = filter {
        all.retain(|d| allowed.contains(&d.def.id));
    }
    Ok(ok(all))
}

/// POST /api/ledger/datasets/{id}/cursor  QuerySpec → 游标（筛选 + 排序物化，按条件缓存）
pub async fn open_cursor(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<CursorInfo>>> {
    let spec: QuerySpec = parse_json(&body)?;
    let db2 = db.clone();
    crate::usage::hit(&id, crate::usage::BROWSE);
    let work = move |conn: &duckdb::Connection| dataset::open_cursor(conn, &db2.cursors, &id, &spec);
    // 整表浏览（没有任何条件）基本是即时的直通游标，不排队、也不等别人的排序；
    // 带筛选 / 搜索 / 排序 / 规则的要物化位置表（可能很重）：先过闸门，再串行建表避免同一查询被并发重复物化
    let plain = spec_is_plain(&body);
    if plain {
        return Ok(ok(blocking_read(&db, work).await?));
    }
    let _permit = db.heavy.acquire().await?;
    let _guard = db.cursors.build_lock.lock().await;
    Ok(ok(blocking_read(&db, work).await?))
}

#[derive(Deserialize)]
pub struct RowsQuery {
    pub cursor: String,
    pub offset: i64,
    pub limit: i64,
    /// 只取这些列（逗号分隔，按此顺序）；省略为全部列
    #[serde(default)]
    pub columns: Option<String>,
}

/// GET /api/ledger/datasets/{id}/rows?cursor=&offset=&limit=&columns=  按位置取一段
pub async fn dataset_rows(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<RowsQuery>) -> AppResult<Json<Envelope<RowsWindow>>> {
    let db2 = db.clone();
    let columns: Option<Vec<String>> = q.columns.as_deref().filter(|s| !s.trim().is_empty()).map(|s| s.split(',').map(|c| c.trim().to_string()).collect());
    Ok(ok(blocking_read(&db, move |conn| dataset::rows(conn, &db2.cursors, &id, &q.cursor, q.offset, q.limit, columns.as_deref())).await?))
}

/// POST /api/ledger/datasets/{id}/stats  选区统计
pub async fn dataset_stats(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<SelectionStats>>> {
    let req: StatsRequest = parse_json(&body)?;
    let db2 = db.clone();
    Ok(ok(blocking_read(&db, move |conn| dataset::stats(conn, &db2.cursors, &id, &req)).await?))
}

/// POST /api/ledger/datasets/{id}/totals  { cursorId, items: [{ column, func }] }  汇总行：按当前筛选对整个结果集算每列的合计 / 平均 / 计数等
pub async fn dataset_totals(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<dataset::TotalsResult>>> {
    let req: dataset::TotalsRequest = parse_json(&body)?;
    let db2 = db.clone();
    Ok(ok(blocking_heavy(&db, move |conn| dataset::totals(conn, &db2.cursors, &id, &req)).await?))
}

/// POST /api/ledger/datasets/{id}/groups  { filters?, keyword?, rule?, violation?, column, desc?, items: [{ column, func }] }  分类汇总：按某列分组，每组的行数与汇总（顺序与按该列排序的数据表一致）
pub async fn dataset_groups(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<dataset::GroupsResult>>> {
    let req: dataset::GroupsRequest = parse_json(&body)?;
    Ok(ok(blocking_heavy(&db, move |conn| dataset::groups(conn, &id, &req)).await?))
}

/// POST /api/ledger/datasets/{id}/distinct  列值分面（筛选下拉）
pub async fn dataset_distinct(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<DistinctResult>>> {
    let req: DistinctRequest = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| dataset::distinct(conn, &id, &req)).await?))
}

/// POST /api/ledger/datasets/{id}/profile  { filters?, keyword?, rule?, columns? }  数据概况：按当前筛选，每列的空值、近似去重数、范围、数值分布（一次扫描）
pub async fn dataset_profile(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::profile::Profile>>> {
    let req: crate::profile::ProfileRequest = if body.is_empty() { Default::default() } else { parse_json(&body)? };
    Ok(ok(blocking_heavy(&db, move |conn| crate::profile::overview(conn, &id, &req)).await?))
}

/// POST /api/ledger/datasets/{id}/profile/column  { column, filters?, keyword?, rule? }  一列的分布：直方图（数值分箱 / 日期按日月年）与最常见的取值
pub async fn dataset_profile_column(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::profile::ColumnDetail>>> {
    let req: crate::profile::ColumnRequest = parse_json(&body)?;
    Ok(ok(blocking_heavy(&db, move |conn| crate::profile::column(conn, &id, &req)).await?))
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub cursor: String,
    /// 逗号分隔的列名（按表格当前列顺序）；为空导出全部列
    #[serde(default)]
    pub columns: String,
    #[serde(default)]
    pub name: String,
    /// csv（默认）/ xlsx
    #[serde(default)]
    pub format: String,
    /// 筛选条件描述（写入 xlsx 汇总页签）
    #[serde(default)]
    pub desc: String,
    /// 分类汇总（JSON：{ column, desc, items, collapsed, allCollapsed }，只用于 xlsx）
    #[serde(default)]
    pub group: String,
}

/// GET /api/ledger/datasets/{id}/export?cursor=&columns=&name=&format=csv|xlsx&desc=  按当前筛选 / 排序导出（流式下载）
pub async fn dataset_export(State(db): State<AppState>, Path(id): Path<String>, Query(q): Query<ExportQuery>) -> AppResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;
    let columns: Vec<String> = q.columns.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    crate::usage::hit(&id, crate::usage::EXPORT);
    let dir = db.tmp_dir.clone();
    let db2 = db.clone();
    let cursor = q.cursor.clone();
    let xlsx = q.format == "xlsx";
    let desc = q.desc.clone();
    let group: Option<dataset::GroupExport> = if q.group.trim().is_empty() { None } else { Some(serde_json::from_str(&q.group).map_err(|e| AppError::bad(format!("分类汇总参数不正确：{e}")))?) };
    let (path, rows) = blocking(&db, move |conn| {
        if xlsx {
            dataset::export_xlsx(conn, &db2.cursors, &id, &cursor, &columns, &desc, group.as_ref(), &dir)
        } else {
            dataset::export_csv(conn, &db2.cursors, &id, &cursor, &columns, &dir)
        }
    })
    .await?;
    let file = tokio::fs::File::open(&path).await?;
    // 打开后即删除目录项：句柄仍可读，传输完成或中断时文件自动释放
    let _ = tokio::fs::remove_file(&path).await;
    tracing::info!(rows, xlsx, "dataset export");
    if xlsx {
        let name = if q.name.trim().is_empty() { "数据导出".to_string() } else { q.name.trim().to_string() };
        let disposition = format!("attachment; filename*=UTF-8''{}", percent_encode(&format!("{name}.xlsx")));
        let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
        return Ok((
            [
                (header::CONTENT_TYPE, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()),
                (header::CONTENT_DISPOSITION, disposition),
            ],
            body,
        )
            .into_response());
    }
    // UTF-8 BOM：Excel 直接双击打开不乱码
    let bom = futures_util::stream::once(async { Ok::<_, std::io::Error>(axum::body::Bytes::from_static("\u{feff}".as_bytes())) });
    let body = axum::body::Body::from_stream(futures_util::StreamExt::chain(bom, tokio_util::io::ReaderStream::new(file)));
    let name = if q.name.trim().is_empty() { "数据导出".to_string() } else { q.name.trim().to_string() };
    let disposition = format!("attachment; filename*=UTF-8''{}", percent_encode(&format!("{name}.csv")));
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, disposition)], body).into_response())
}

#[derive(Deserialize)]
pub struct AggregateRequest {
    pub calls: Vec<crate::aggregate::AggregateCall>,
    /// 范围：工作表里数据表的筛选 / 关键字 / 排序（TSUM 等按数据表当前显示的行计算）
    #[serde(default)]
    pub query: Option<dataset::QuerySpec>,
}

/// POST /api/ledger/datasets/{id}/aggregate  { calls: [{ func, column, criteria: [{ column, value }], row? }], query? }  公式下推批量计算
pub async fn dataset_aggregate(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::aggregate::AggregateResult>>> {
    let req: AggregateRequest = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| {
        // 公式里按名称引用：按解析出的数据集计数
        if let Ok(d) = dataset::dataset(conn, &id) {
            crate::usage::hit(&d.id, crate::usage::FORMULA);
        }
        crate::aggregate::aggregate(conn, &id, &req.calls, req.query.as_ref())
    })
    .await?))
}

// ── 通用数据集导入与管理 ──────────────────────────────

/// POST /api/ledger/datasets/import/preview  multipart: file（CSV / XLS / XLSX）
pub async fn dataset_import_preview(State(db): State<AppState>, mut multipart: Multipart) -> AppResult<Json<Envelope<crate::dataset_import::ImportPreview>>> {
    import::cleanup_previews(&db.tmp_dir);
    let token = import::new_preview_token();
    // 可以一次上传多个文件（字段名都是 file），合并为一个数据集
    let mut uploads: Vec<crate::import_source::Upload> = Vec::new();
    let received = async {
        while let Some(mut field) = multipart.next_field().await.map_err(|e| AppError::bad(format!("上传数据不完整：{e}")))? {
            if field.name() != Some("file") {
                continue;
            }
            if uploads.len() >= crate::import_source::MAX_FILES {
                return Err(AppError::bad(format!("一次最多合并 {} 个文件", crate::import_source::MAX_FILES)));
            }
            let name = field.file_name().unwrap_or("upload.csv").to_string();
            let path = db.tmp_dir.join(format!("preview-{token}-{}.upload", uploads.len()));
            uploads.push(crate::import_source::Upload { name, path: path.clone() });
            let mut out = tokio::fs::File::create(&path).await?;
            while let Some(chunk) = field.chunk().await.map_err(|e| AppError::bad(format!("文件上传中断：{e}")))? {
                out.write_all(&chunk).await?;
            }
            out.flush().await?;
        }
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(e) = received {
        for u in &uploads {
            let _ = std::fs::remove_file(&u.path);
        }
        return Err(e);
    }
    Ok(ok(preview_uploads(&db, token, uploads).await?))
}

/// 规整上传 / 下载的文件并预览
async fn preview_uploads(db: &AppState, token: String, uploads: Vec<crate::import_source::Upload>) -> AppResult<crate::dataset_import::ImportPreview> {
    let dir = db.tmp_dir.clone();
    blocking(db, move |conn| {
        let (file_name, infos) = crate::import_source::prepare(conn, &dir, &token, uploads)?;
        let path = crate::dataset_import::staged_path(&dir, &token, &file_name)?;
        let r = crate::dataset_import::preview(conn, &path, &token, &file_name).map(|mut p| {
            p.source_files = infos;
            p
        });
        if r.is_err() {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(crate::import_source::hints_path(&path));
        }
        r
    })
    .await
}

#[derive(Deserialize)]
pub struct UrlImportBody {
    pub url: String,
}

/// POST /api/ledger/datasets/import/preview-url  { url }  从网址下载文件并预览（http / https，不访问本机地址）
pub async fn dataset_import_preview_url(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::dataset_import::ImportPreview>>> {
    let req: UrlImportBody = parse_json(&body)?;
    import::cleanup_previews(&db.tmp_dir);
    let token = import::new_preview_token();
    let max = db.config.max_upload_mb as u64 * 1024 * 1024;
    let upload = crate::import_source::download(&req.url, &db.tmp_dir, &token, max).await?;
    Ok(ok(preview_uploads(&db, token, vec![upload]).await?))
}

/// POST /api/ledger/datasets/import/commit  { token, mode: create | append | replace, datasetId?, name?, columns, primaryKey, fileName }
pub async fn dataset_import_commit(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::dataset_import::ImportOutcome>>> {
    let req: crate::dataset_import::CommitRequest = parse_json(&body)?;
    let path = crate::dataset_import::staged_path(&db.tmp_dir, &req.token, &req.file_name)?;
    if !path.exists() {
        return Err(AppError::bad("预览已过期，请重新选择文件"));
    }
    let dir = db.tmp_dir.clone();
    let (mode, file_name) = (req.mode, req.file_name.clone());
    let actor = user.clone();
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| {
        // 整体替换已有数据集：和改结构一样，只有所有者与管理员
        if let Some(id) = req.dataset_id.as_deref().filter(|_| matches!(req.mode, crate::dataset_import::Mode::Replace)) {
            crate::dataset::require_manage(conn, id, &user)?;
        }
        let r = crate::dataset_import::commit(conn, &path, &req, &dir);
        if let Ok(o) = &r {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(crate::import_source::hints_path(&path));
            crate::dataset::set_owner(conn, &o.dataset_id, &user)?;
        }
        r
    })
    .await?;
    tracing::info!(dataset = %result.dataset_id, rows = result.rows_read, inserted = result.inserted, ms = result.elapsed_ms, "dataset import");
    db.cursors.invalidate();
    db.checkpoint();
    let created = matches!(mode, crate::dataset_import::Mode::Create);
    crate::webhook::emit(
        &db,
        if created { "dataset.created" } else { "dataset.imported" },
        Some(&actor),
        serde_json::json!({
            "datasetId": result.dataset_id, "name": result.name, "source": "import", "mode": mode, "fileName": file_name,
            "rowsRead": result.rows_read, "inserted": result.inserted, "updated": result.updated, "replaced": result.replaced, "skipped": result.skipped,
        }),
    );
    if !created {
        crate::sqlq::on_data_changed(&db, &result.dataset_id);
    }
    Ok(ok(result))
}

#[derive(Deserialize)]
pub struct RenameDataset {
    pub name: String,
}

/// PUT /api/ledger/datasets/{id}  { name }  改名
pub async fn rename_dataset(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: RenameDataset = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| dataset::rename(conn, &id, &req.name)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(true))
}

/// POST /api/ledger/datasets/{id}/columns/kind/preview  { column, kind }  预览修改列类型：多少个值转不过来、举例、转换前后的样子
pub async fn column_kind_preview(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::dataset_alter::KindPreview>>> {
    let req: crate::dataset_alter::KindChange = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| crate::dataset_alter::preview(conn, &id, &req)).await?))
}

/// PUT /api/ledger/datasets/{id}/columns/kind  { column, kind, acceptLoss? }  修改列类型（就地转换；有值转不过来时要 acceptLoss）
pub async fn column_kind_apply(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::dataset_alter::KindPreview>>> {
    let req: crate::dataset_alter::KindChange = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| crate::dataset_alter::apply(conn, &id, &req)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}

/// POST /api/ledger/datasets/{id}/clean/preview  { op: dedupe|trim|replace|fill|delete_rows, … }  数据清洗预览：多少行受影响、改之前 / 之后的样例
pub async fn clean_preview(State(db): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::clean::CleanPreview>>> {
    let op: crate::clean::CleanOp = parse_json(&body)?;
    Ok(ok(blocking_heavy(&db, move |conn| crate::clean::preview(conn, &id, &op)).await?))
}

/// POST /api/ledger/datasets/{id}/clean  { op, … }  执行数据清洗（可撤销最近一次）
pub async fn clean_apply(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::clean::CleanPreview>>> {
    let op: crate::clean::CleanOp = parse_json(&body)?;
    let (did, operator) = (id.clone(), user.name.clone());
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| crate::clean::apply(conn, &did, &op, &operator)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    crate::sqlq::on_data_changed(&db, &id);
    Ok(ok(r))
}

/// GET /api/ledger/datasets/{id}/validation  数据校验规则与最近一次检查结果
pub async fn validation_get(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::validation::Validation>>> {
    Ok(ok(blocking(&db, move |conn| crate::validation::get(conn, &id)).await?))
}

#[derive(Deserialize)]
pub struct ValidationBody {
    pub rules: Vec<crate::validation::Rule>,
}

/// PUT /api/ledger/datasets/{id}/validation  { rules: [{ column, kind: required / unique / range / format / allowed, … }] }  保存校验规则并立即检查
pub async fn validation_save(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: axum::body::Bytes) -> AppResult<Json<Envelope<crate::validation::Validation>>> {
    let req: ValidationBody = parse_json(&body)?;
    Ok(ok(blocking_heavy(&db, move |conn| crate::validation::save(conn, &id, req.rules, &user)).await?))
}

/// POST /api/ledger/datasets/{id}/validation/check  按规则重新检查（数据范围受限的用户只检查自己能看到的行）
pub async fn validation_check(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::validation::Report>>> {
    Ok(ok(blocking_heavy(&db, move |conn| crate::validation::check(conn, &id)).await?))
}

/// GET /api/ledger/datasets/{id}/clean  最近一次清洗与能否撤销（没有时为 null）
pub async fn clean_last(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Option<crate::clean::LastClean>>>> {
    Ok(ok(blocking(&db, move |conn| crate::clean::last(conn, &id)).await?))
}

/// POST /api/ledger/datasets/{id}/clean/undo  撤销最近一次清洗（之后数据集没有别的改动时）
pub async fn clean_undo(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::clean::LastClean>>> {
    let (did, operator) = (id.clone(), user.name.clone());
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| crate::clean::undo(conn, &did, &operator)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    crate::sqlq::on_data_changed(&db, &id);
    Ok(ok(r))
}

/// DELETE /api/ledger/datasets/{id}?operator=  删除数据集：移到回收站（数据保留，可恢复）
pub async fn delete_dataset(State(db): State<AppState>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, Query(q): Query<OperatorQuery>) -> AppResult<Json<Envelope<i64>>> {
    let _guard = db.import_lock.lock().await;
    let (n, id, name) = blocking(&db, move |conn| {
        let name = dataset::datasets(conn)?.into_iter().find(|d| d.id == id).map(|d| d.name);
        Ok((dataset::delete(conn, &id, &q.operator)?, id, name))
    })
    .await?;
    db.cursors.invalidate();
    db.checkpoint();
    crate::webhook::emit(&db, "dataset.deleted", Some(&user), serde_json::json!({ "datasetId": id, "name": name }));
    Ok(ok(n))
}

/// GET /api/ledger/datasets/trash  数据集回收站 { days, items }
pub async fn dataset_trash(State(db): State<AppState>) -> AppResult<Json<Envelope<crate::dataset_trash::TrashList>>> {
    Ok(ok(blocking(&db, crate::dataset_trash::list).await?))
}

/// DELETE /api/ledger/datasets/trash  清空数据集回收站（不可恢复）
pub async fn empty_dataset_trash(State(db): State<AppState>) -> AppResult<Json<Envelope<usize>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, crate::dataset_trash::empty).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(n))
}

/// POST /api/ledger/datasets/{id}/untrash  从回收站恢复
pub async fn untrash_dataset(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<crate::dataset_trash::Restored>>> {
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| crate::dataset_trash::restore(conn, &id)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(r))
}

/// DELETE /api/ledger/datasets/{id}/purge  彻底删除（连同回收站里基于它的关联视图），返回删除的名称
pub async fn purge_dataset(State(db): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Envelope<Vec<String>>>> {
    let _guard = db.import_lock.lock().await;
    let names = blocking(&db, move |conn| crate::dataset_trash::purge(conn, &id)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(names))
}
