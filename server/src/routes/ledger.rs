//! 电商资金台账：导入、店铺、月结、经营分析（项目最早的使用场景）

use super::*;

/// 把 multipart 中的文件流式写入指定路径，返回（文件名, 其他文本字段）
async fn receive_upload(mut multipart: Multipart, path: &std::path::Path) -> AppResult<(String, std::collections::HashMap<String, String>)> {
    let mut fields = std::collections::HashMap::new();
    let mut file_name = None;
    while let Some(mut field) = multipart.next_field().await.map_err(|e| AppError::bad(format!("上传数据不完整：{e}")))? {
        let Some(name) = field.name().map(str::to_string) else { continue };
        if name == "file" {
            file_name = Some(field.file_name().unwrap_or("upload.csv").to_string());
            let mut out = tokio::fs::File::create(path).await?;
            while let Some(chunk) = field.chunk().await.map_err(|e| AppError::bad(format!("文件上传中断：{e}")))? {
                out.write_all(&chunk).await?;
            }
            out.flush().await?;
        } else {
            fields.insert(name, field.text().await.map_err(|e| AppError::bad(e.to_string()))?);
        }
    }
    let file_name = file_name.ok_or_else(|| AppError::bad("请选择要导入的 CSV 文件"))?;
    Ok((file_name, fields))
}

fn parse_mode(v: Option<&String>) -> AppResult<ImportMode> {
    match v.map(String::as_str) {
        None | Some("") | Some("merge") => Ok(ImportMode::Merge),
        Some("replace") => Ok(ImportMode::Replace),
        Some(other) => Err(AppError::bad(format!("未知的导入模式：{other}"))),
    }
}

/// POST /api/ledger/imports/preview  multipart: file —— 暂存文件，识别店铺并查重，返回 token
pub async fn preview_import(State(db): State<AppState>, multipart: Multipart) -> AppResult<Json<Envelope<PreviewResult>>> {
    import::cleanup_previews(&db.tmp_dir);
    let token = import::new_preview_token();
    let path = db.tmp_dir.join(format!("preview-{token}.csv"));
    let (file_name, _) = match receive_upload(multipart, &path).await {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    };
    let result = blocking(&db, move |conn| {
        let r = import::preview(conn, &path, &token, &file_name);
        if r.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        r
    })
    .await?;
    Ok(ok(result))
}

#[derive(Deserialize)]
pub struct CommitRequest {
    pub token: String,
    pub shop: String,
    #[serde(default)]
    pub mode: ImportMode,
    #[serde(default, rename = "fileName")]
    pub file_name: String,
}

/// POST /api/ledger/imports/commit  { token, shop, mode: merge | replace }
pub async fn commit_import(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<ImportResult>>> {
    let req: CommitRequest = parse_json(&body)?;
    let path = import::preview_path(&db.tmp_dir, &req.token)?;
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| {
        let r = import::commit(conn, &path, &req.shop, &req.file_name, req.mode);
        if r.is_ok() {
            let _ = std::fs::remove_file(&path);
        }
        r
    })
    .await?;
    log_import(&result);
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(result))
}

/// POST /api/ledger/imports  multipart: shop + file (+ mode) —— 一步导入（脚本/命令行使用），默认按流水号合并
pub async fn import_csv(State(db): State<AppState>, multipart: Multipart) -> AppResult<Json<Envelope<ImportResult>>> {
    let tmp = tempfile::Builder::new().prefix("upload-").suffix(".csv").tempfile_in(&db.tmp_dir)?;
    let (file_name, fields) = receive_upload(multipart, tmp.path()).await?;
    let shop = fields.get("shop").cloned().ok_or_else(|| AppError::bad("请填写店铺名称"))?;
    let mode = parse_mode(fields.get("mode"))?;
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| {
        let r = import::commit(conn, tmp.path(), &shop, &file_name, mode);
        drop(tmp);
        r
    })
    .await?;
    log_import(&result);
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(result))
}

fn log_import(r: &ImportResult) {
    tracing::info!(
        shop = %r.shop, mode = %r.mode, rows = r.row_count, inserted = r.inserted_rows,
        skipped = r.skipped_rows, changed = r.changed_rows, replaced = r.replaced_rows, ms = r.elapsed_ms,
        "import finished"
    );
}

/// GET /api/ledger/shops
pub async fn list_shops(State(db): State<AppState>, Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>) -> AppResult<Json<Envelope<Vec<ShopInfo>>>> {
    let mut shops = blocking(&db, shop::list).await?;
    shops.retain(|s| scope.allows(&s.name));
    Ok(ok(shops))
}

/// POST /api/ledger/shops  { name, douyinShopId?, remark? }
pub async fn create_shop(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<Vec<ShopInfo>>>> {
    let input: ShopInput = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| shop::create(conn, input)).await?))
}

/// PUT /api/ledger/shops  { name, newName?, douyinShopId?, status?, remark? }（改名同步更新历史数据）
pub async fn update_shop(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<Vec<ShopInfo>>>> {
    let input: ShopUpdate = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| shop::update(conn, input)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(result))
}

/// GET /api/ledger/coverage  店铺 × 月份完整性矩阵
pub async fn coverage(State(db): State<AppState>) -> AppResult<Json<Envelope<Coverage>>> {
    Ok(ok(blocking(&db, coverage::coverage).await?))
}

#[derive(Deserialize)]
pub struct PeriodQuery {
    pub period: String,
}

/// GET /api/ledger/pivot?period=2026-09
pub async fn pivot(State(db): State<AppState>, Query(q): Query<PeriodQuery>) -> AppResult<Json<Envelope<PivotResult>>> {
    Ok(ok(blocking_read(&db, move |conn| query::pivot(conn, &q.period)).await?))
}

/// 自行解析 JSON 请求体：格式错误也返回统一信封（axum 的 Json 提取器返回纯文本）
pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> AppResult<T> {
    serde_json::from_slice(body).map_err(|e| AppError::bad(format!("请求格式错误：{e}")))
}

/// POST /api/ledger/details/rows  按偏移量取一段明细（前端虚拟滚动按可视区域分段加载）
pub async fn detail_rows(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<RowsResult>>> {
    let req: RowsRequest = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| detail::rows(conn, &req)).await?))
}

/// POST /api/ledger/details/summary  当前筛选的笔数、金额合计、规则贡献合计与分面统计
pub async fn detail_summary(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<DetailSummary>>> {
    let filter: DetailFilter = parse_json(&body)?;
    Ok(ok(blocking_read(&db, move |conn| detail::summary(conn, &filter)).await?))
}

/// POST /api/ledger/details/export  当前筛选结果导出为 CSV
pub async fn detail_export(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;
    let req: ExportRequest = parse_json(&body)?;
    let file_name = format!("资金流水明细_{}.csv", req.filter.period);
    let bytes = blocking(&db, move |conn| detail::export_csv(conn, &req)).await?;
    let disposition = format!("attachment;filename={}", percent_encode(&file_name));
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, disposition)], bytes)
        .into_response())
}

/// GET /api/ledger/periods
pub async fn periods(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<PeriodInfo>>>> {
    Ok(ok(blocking(&db, query::periods).await?))
}

/// GET /api/ledger/imports
pub async fn imports(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<ImportRecord>>>> {
    Ok(ok(blocking(&db, query::imports).await?))
}

/// GET /api/ledger/entity-aliases
pub async fn entity_aliases(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<EntityAlias>>>> {
    Ok(ok(blocking(&db, query::entity_aliases).await?))
}

/// PUT /api/ledger/entity-aliases  body: [{ entity, alias }]，alias 为空表示删除
pub async fn save_entity_aliases(
    State(db): State<AppState>,
    body: axum::body::Bytes,
) -> AppResult<Json<Envelope<Vec<EntityAlias>>>> {
    let items: Vec<EntityAliasInput> = parse_json(&body)?;
    Ok(ok(blocking(&db, move |conn| query::save_entity_aliases(conn, &items)).await?))
}

#[derive(Deserialize)]
pub struct DeleteQuery {
    pub shop: String,
    pub period: String,
}

/// DELETE /api/ledger/data?shop=&period=
pub async fn delete_data(State(db): State<AppState>, Query(q): Query<DeleteQuery>) -> AppResult<Json<Envelope<i64>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| query::delete_period(conn, &q.shop, &q.period)).await?;
    db.cursors.invalidate();
    db.checkpoint();
    Ok(ok(n))
}

// ── 第二阶段：月结闭环 ────────────────────────────────

#[derive(Deserialize)]
pub struct PeriodShopQuery {
    pub period: String,
    pub shop: Option<String>,
}

/// GET /api/ledger/balances?period=&shop=  余额勾稽
pub async fn balances(State(db): State<AppState>, Query(q): Query<PeriodShopQuery>) -> AppResult<Json<Envelope<Vec<BalanceRow>>>> {
    Ok(ok(blocking(&db, move |conn| balance::list(conn, &q.period, q.shop.as_deref().filter(|s| !s.is_empty()))).await?))
}

#[derive(Deserialize)]
pub struct SaveBalances {
    #[serde(default)]
    pub operator: String,
    pub items: Vec<BalanceInput>,
}

/// PUT /api/ledger/balances  { operator, items: [{ shop, entity, account, period, opening, closing, remark }] }
pub async fn save_balances(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: SaveBalances = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| balance::save(conn, &req.items, &req.operator)).await?;
    db.checkpoint();
    Ok(ok(true))
}

/// POST /api/ledger/bank/imports  multipart: file + entity (+ accountNo)  银行流水导入
pub async fn import_bank(State(db): State<AppState>, multipart: Multipart) -> AppResult<Json<Envelope<BankImportResult>>> {
    let tmp = tempfile::Builder::new().prefix("bank-").tempfile_in(&db.tmp_dir)?;
    let (file_name, fields) = receive_upload(multipart, tmp.path()).await?;
    let entity = fields.get("entity").cloned().unwrap_or_default();
    let account_no = fields.get("accountNo").cloned();
    let _guard = db.import_lock.lock().await;
    let result = blocking(&db, move |conn| {
        let r = bank::import(conn, tmp.path(), &file_name, &entity, account_no.as_deref());
        drop(tmp);
        r
    })
    .await?;
    db.checkpoint();
    Ok(ok(result))
}

/// GET /api/ledger/bank/imports  已导入的银行流水批次
pub async fn bank_batches(State(db): State<AppState>) -> AppResult<Json<Envelope<Vec<BankBatch>>>> {
    Ok(ok(blocking(&db, bank::batches).await?))
}

#[derive(Deserialize)]
pub struct BatchQuery {
    #[serde(rename = "batchId")]
    pub batch_id: String,
}

/// DELETE /api/ledger/bank/imports?batchId=
pub async fn delete_bank_batch(State(db): State<AppState>, Query(q): Query<BatchQuery>) -> AppResult<Json<Envelope<i64>>> {
    let _guard = db.import_lock.lock().await;
    let n = blocking(&db, move |conn| bank::delete_batch(conn, &q.batch_id)).await?;
    db.checkpoint();
    Ok(ok(n))
}

/// GET /api/ledger/withdrawals?period=&shop=  提现到账情况
pub async fn withdrawals(State(db): State<AppState>, Query(q): Query<PeriodShopQuery>) -> AppResult<Json<Envelope<WithdrawView>>> {
    Ok(ok(blocking(&db, move |conn| bank::view(conn, &q.period, q.shop.as_deref().filter(|s| !s.is_empty()))).await?))
}

#[derive(Deserialize)]
pub struct AutoMatchRequest {
    pub period: String,
    #[serde(default)]
    pub operator: String,
}

/// POST /api/ledger/withdrawals/auto-match  { period, operator }
pub async fn auto_match(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<AutoMatchResult>>> {
    let req: AutoMatchRequest = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    let r = blocking(&db, move |conn| bank::auto_match(conn, &req.period, &req.operator)).await?;
    db.checkpoint();
    Ok(ok(r))
}

#[derive(Deserialize)]
pub struct CandidateQuery {
    pub shop: String,
    pub period: String,
    #[serde(rename = "serialNo")]
    pub serial_no: String,
}

/// GET /api/ledger/withdrawals/candidates?shop=&period=&serialNo=  手工配对候选
pub async fn match_candidates(State(db): State<AppState>, Query(q): Query<CandidateQuery>) -> AppResult<Json<Envelope<Vec<BankTxn>>>> {
    Ok(ok(blocking(&db, move |conn| bank::candidates(conn, &q.shop, &q.period, &q.serial_no)).await?))
}

/// PUT /api/ledger/withdrawals/match  { shop, period, serialNo, txnId, operator }（txnId 为空 = 取消配对）
pub async fn manual_match(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let req: MatchInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| bank::manual_match(conn, &req)).await?;
    db.checkpoint();
    Ok(ok(true))
}

/// GET /api/ledger/close?period=  各店铺月结状态与体检
pub async fn close_overview(
    State(db): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Envelope<CloseOverview>>> {
    let allow = db.config.allow_self_approve;
    let mut o = blocking(&db, move |conn| close::overview(conn, &q.period, &user, allow)).await?;
    // 受限用户只看到自己范围内的店铺
    o.shops.retain(|s| scope.allows(&s.shop));
    Ok(ok(o))
}

/// POST /api/ledger/close  { shop, period, action: submit | approve | reject | cancel | reopen, operator, approver?, comment, acceptRisk? }
pub async fn close_action(
    State(db): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>,
    body: axum::body::Bytes,
) -> AppResult<Json<Envelope<bool>>> {
    let req: CloseAction = parse_json(&body)?;
    if !scope.allows(&req.shop) {
        return Err(AppError::Forbidden(format!("店铺「{}」不在你的数据范围内", req.shop)));
    }
    let allow = db.config.allow_self_approve;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| close::act(conn, &req, &user, allow)).await?;
    db.checkpoint();
    Ok(ok(true))
}

#[derive(Deserialize)]
pub struct OptionalPeriodQuery {
    pub period: Option<String>,
}

/// GET /api/ledger/close/logs?period=
pub async fn close_logs(
    State(db): State<AppState>,
    Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>,
    Query(q): Query<OptionalPeriodQuery>,
) -> AppResult<Json<Envelope<Vec<CloseLog>>>> {
    let mut logs = blocking(&db, move |conn| close::logs(conn, q.period.as_deref().filter(|s| !s.is_empty()))).await?;
    logs.retain(|l| scope.allows(&l.shop));
    Ok(ok(logs))
}

/// GET /api/ledger/close/locks  已冻结（审批中 / 已锁账）的店铺月份
pub async fn close_locks(State(db): State<AppState>, Extension(crate::scope::ScopeExt(scope)): Extension<crate::scope::ScopeExt>) -> AppResult<Json<Envelope<Vec<PeriodLock>>>> {
    let mut locks = blocking(&db, close::locks).await?;
    locks.retain(|l| scope.allows(&l.shop));
    Ok(ok(locks))
}

// ── 第三阶段：经营分析 ────────────────────────────────

#[derive(Deserialize)]
pub struct BreakdownQuery {
    pub period: String,
    pub shop: Option<String>,
    pub by: Dimension,
}

/// GET /api/ledger/analysis/breakdown?period=&shop=&by=channel|talent|product|order_month|shop|entity
pub async fn analysis_breakdown(State(db): State<AppState>, Query(q): Query<BreakdownQuery>) -> AppResult<Json<Envelope<Breakdown>>> {
    Ok(ok(blocking_read(&db, move |conn| analysis::breakdown(conn, &q.period, q.shop.as_deref(), q.by)).await?))
}

#[derive(Deserialize)]
pub struct ShopQuery {
    pub shop: Option<String>,
}

/// GET /api/ledger/analysis/cross-period?shop=  下单月份 × 结算月份
pub async fn analysis_cross_period(State(db): State<AppState>, Query(q): Query<ShopQuery>) -> AppResult<Json<Envelope<CrossPeriod>>> {
    Ok(ok(blocking_read(&db, move |conn| analysis::cross_period(conn, q.shop.as_deref())).await?))
}

/// GET /api/ledger/analysis/qianchuan?period=  千川充值 / 消耗 / 千川渠道成交
pub async fn analysis_qianchuan(State(db): State<AppState>, Query(q): Query<PeriodQuery>) -> AppResult<Json<Envelope<Vec<QianchuanRow>>>> {
    Ok(ok(blocking_read(&db, move |conn| analysis::qianchuan(conn, &q.period)).await?))
}

/// PUT /api/ledger/analysis/qianchuan  { shop, period, spend, remark, operator }  录入千川消耗
pub async fn save_ad_spend(State(db): State<AppState>, body: axum::body::Bytes) -> AppResult<Json<Envelope<bool>>> {
    let input: AdSpendInput = parse_json(&body)?;
    let _guard = db.import_lock.lock().await;
    blocking(&db, move |conn| analysis::save_ad_spend(conn, &input)).await?;
    db.checkpoint();
    Ok(ok(true))
}
