//! CSV 入库：预览（识别店铺、查重）→ 提交（按流水号去重合并 / 替换时间段）
//!
//! 一个 CSV 不含店铺标识，多店铺场景下最大的风险是串店、重复导入、分批导出相互覆盖：
//! - 预览：计算文件与各店铺的「商品 ID 重合度」「已存在的流水号数量」，识别文件属于哪个店铺；按文件 MD5 发现重复上传
//! - 提交（合并，默认）：以「店铺 + 动帐流水号」去重，只插入新流水；同流水号但金额/方向/场景不同的计为「变更」并提示
//! - 提交（替换）：删除该店铺在文件时间范围内的旧数据后写入，用于平台修正历史数据后整段重导

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::close;
use crate::error::{AppError, AppResult};
use crate::schema::{DETAIL_COLUMNS, ID_COLUMNS, REQUIRED_COLUMNS, ident, is_numeric, literal, search_text_sql};

const STAGING: &str = "ledger_staging";
/// 汇总表的合计列使用的店铺名，不能用作真实店铺
pub const RESERVED_SHOP_NAMES: &[&str] = &["主体合计", "全部店铺"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityCount {
    pub name: String,
    pub row_count: i64,
    /// 已保存的主体简称（仅聚合接口返回）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidNumber {
    pub column: String,
    pub count: i64,
    pub sample: String,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ImportMode {
    /// 按「店铺 + 动帐流水号」去重，只插入新流水（分批导出、重复导出都安全）
    #[default]
    Merge,
    /// 删除该店铺在文件时间范围内的旧数据后写入
    Replace,
}

impl ImportMode {
    fn as_str(self) -> &'static str {
        match self {
            ImportMode::Merge => "merge",
            ImportMode::Replace => "replace",
        }
    }
}

/// 暂存（临时表）后的文件概况
struct Staged {
    row_count: i64,
    /// 文件内重复的流水（同一流水号出现多次），只保留一条
    duplicate_in_file: i64,
    periods: Vec<String>,
    entities: Vec<EntityCount>,
    time_from: String,
    time_to: String,
    product_count: i64,
    file_hash: String,
    missing_columns: Vec<String>,
    extra_columns: Vec<String>,
    invalid_numbers: Vec<InvalidNumber>,
}

// ── 公共校验 ──────────────────────────────────────────

/// 店铺名同时用作汇总表的列标题与公式条件，限制与 Excel 工作表名一致的非法字符
pub fn validate_shop(shop: &str) -> AppResult<String> {
    let shop = shop.trim();
    if shop.is_empty() {
        return Err(AppError::bad("请填写店铺名称"));
    }
    if shop.chars().count() > 31 {
        return Err(AppError::bad("店铺名称不能超过 31 个字符"));
    }
    if shop.contains(['[', ']', ':', '*', '?', '/', '\\']) {
        return Err(AppError::bad("店铺名称不能包含 [ ] : * ? / \\"));
    }
    if RESERVED_SHOP_NAMES.contains(&shop) {
        return Err(AppError::bad(format!("「{shop}」是汇总表保留名称，请换一个店铺名")));
    }
    Ok(shop.to_string())
}

fn new_id(prefix: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
    format!("{prefix}_{millis}_{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 非 UTF-8 文件按 GBK 解码后重写（部分导出工具/Excel 另存为生成 GBK 编码的 CSV）
pub(crate) fn ensure_utf8(path: &Path) -> AppResult<()> {
    let bytes = std::fs::read(path)?;
    if std::str::from_utf8(&bytes).is_ok() {
        return Ok(());
    }
    let (text, _, had_errors) = encoding_rs::GBK.decode(&bytes);
    if had_errors {
        return Err(AppError::bad("文件编码无法识别，请导出为 UTF-8 或 GBK 编码的 CSV"));
    }
    std::fs::write(path, text.as_bytes())?;
    Ok(())
}

fn csv_source(path: &Path) -> String {
    format!(
        "read_csv({}, header = true, all_varchar = true, delim = ',', quote = '\"', escape = '\"', null_padding = true, strict_mode = false)",
        literal(&path.to_string_lossy())
    )
}

/// 去重主键：动帐流水号；极少数没有流水号的行用关键字段哈希代替
fn dedupe_key(alias: &str) -> String {
    let c = |name: &str| format!("{alias}.{}", ident(name));
    format!(
        "COALESCE({id}, md5(concat_ws('|', {t}, {d}, CAST({a} AS VARCHAR), {s}, {o}, {r}, {m})))",
        id = c("动帐流水号"),
        t = c("动账时间"),
        d = c("动账方向"),
        a = c("动账金额"),
        s = c("动账场景"),
        o = c("订单号"),
        r = c("售后编号"),
        m = c("备注"),
    )
}

// ── 暂存 ──────────────────────────────────────────────

/// 读取表头：返回 (规范化列名, 文件中的原始列名)；原始列名可能带 BOM
fn read_header(conn: &Connection, source: &str) -> AppResult<Vec<(String, String)>> {
    let mut stmt = conn.prepare(&format!("DESCRIBE SELECT * FROM {source}"))?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| AppError::bad(format!("CSV 解析失败：{e}")))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names.into_iter().map(|raw| (raw.trim_start_matches('\u{feff}').trim().to_string(), raw)).collect())
}

/// 解析 CSV 到临时表 ledger_staging（文件内同一流水号只保留一条），并统计概况
fn stage(conn: &Connection, path: &Path, shop: &str, import_id: &str) -> AppResult<Staged> {
    ensure_utf8(path)?;
    let source = csv_source(path);
    let header = read_header(conn, &source)?;
    let raw_of: HashMap<&str, &str> = header.iter().map(|(n, r)| (n.as_str(), r.as_str())).collect();

    let missing_required: Vec<&str> = REQUIRED_COLUMNS.iter().copied().filter(|c| !raw_of.contains_key(c)).collect();
    if !missing_required.is_empty() {
        return Err(AppError::bad(format!("不是抖店资金流水明细：缺少列 {}", missing_required.join("、"))));
    }
    let missing_columns: Vec<String> =
        DETAIL_COLUMNS.iter().filter(|c| !raw_of.contains_key(*c)).map(|c| c.to_string()).collect();
    let extra_columns: Vec<String> =
        header.iter().map(|(n, _)| n.clone()).filter(|n| !n.is_empty() && !DETAIL_COLUMNS.contains(&n.as_str())).collect();

    // 标准列表达式：缺失列补 NULL，保证临时表与 ledger_detail 列一致
    let select_columns = DETAIL_COLUMNS
        .iter()
        .map(|col| {
            let target = ident(col);
            match raw_of.get(col) {
                Some(raw) if is_numeric(col) => format!("TRY_CAST(NULLIF(trim({}), '') AS DECIMAL(18,2)) AS {target}", ident(raw)),
                Some(raw) if ID_COLUMNS.contains(col) => {
                    format!("NULLIF(regexp_replace(trim({}), '^''', ''), '') AS {target}", ident(raw))
                }
                Some(raw) => format!("NULLIF({}, '') AS {target}", ident(raw)),
                None if is_numeric(col) => format!("CAST(NULL AS DECIMAL(18,2)) AS {target}"),
                None => format!("CAST(NULL AS VARCHAR) AS {target}"),
            }
        })
        .collect::<Vec<_>>()
        .join(",\n  ");

    let period_expr = format!("substr(trim({}), 1, 7)", ident(raw_of["动账时间"]));
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {STAGING}_raw AS
         SELECT {shop} AS shop, {period_expr} AS period, {import_id} AS import_id,
                row_number() OVER () AS row_no,
                {select_columns}
         FROM {source};
         CREATE OR REPLACE TEMP TABLE {STAGING} AS
         SELECT * FROM {STAGING}_raw s QUALIFY row_number() OVER (PARTITION BY {key} ORDER BY row_no) = 1;",
        shop = literal(shop),
        import_id = literal(import_id),
        key = dedupe_key("s"),
    ))
    .map_err(|e| AppError::bad(format!("CSV 解析失败：{e}")))?;

    let (raw_count, row_count): (i64, i64) = conn.query_row(
        &format!("SELECT (SELECT count(*) FROM {STAGING}_raw), (SELECT count(*) FROM {STAGING})"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    conn.execute_batch(&format!("DROP TABLE IF EXISTS {STAGING}_raw"))?;
    if row_count == 0 {
        return Err(AppError::bad("CSV 中没有数据行"));
    }

    let bad_period: i64 = conn.query_row(
        &format!("SELECT count(*) FROM {STAGING} WHERE period IS NULL OR NOT regexp_matches(period, '^\\d{{4}}-\\d{{2}}$')"),
        [],
        |r| r.get(0),
    )?;
    if bad_period > 0 {
        return Err(AppError::bad(format!("有 {bad_period} 行「动账时间」无法识别月份（应为 YYYY-MM-DD hh:mm:ss）")));
    }

    let periods = query_strings(conn, &format!("SELECT DISTINCT period FROM {STAGING} ORDER BY 1"))?;
    let entities = {
        let mut stmt = conn.prepare(&format!(
            "SELECT COALESCE({e}, ''), count(*) FROM {STAGING} GROUP BY 1 ORDER BY 2 DESC",
            e = ident("商户主体名称")
        ))?;
        stmt.query_map([], |r| Ok(EntityCount { name: r.get(0)?, row_count: r.get(1)?, alias: None }))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let time = ident("动账时间");
    let (time_from, time_to, product_count): (String, String, i64) = conn.query_row(
        &format!("SELECT min({time}), max({time}), count(DISTINCT {p}) FROM {STAGING}", p = ident("商品ID")),
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let file_hash: String = conn.query_row(
        &format!("SELECT md5(content) FROM read_blob({})", literal(&path.to_string_lossy())),
        [],
        |r| r.get(0),
    )?;
    let invalid_numbers = count_invalid_numbers(conn, &source, &raw_of)?;

    Ok(Staged {
        row_count,
        duplicate_in_file: raw_count - row_count,
        periods,
        entities,
        time_from,
        time_to,
        product_count,
        file_hash,
        missing_columns,
        extra_columns,
        invalid_numbers,
    })
}

fn drop_staging(conn: &Connection) {
    let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS {STAGING}; DROP TABLE IF EXISTS {STAGING}_raw;"));
}

// ── 预览 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopCandidate {
    pub shop: String,
    /// 文件中有多少个商品 ID 在该店铺出现过
    pub matched_products: i64,
    /// matched_products / 文件商品数
    pub product_ratio: f64,
    /// 文件中有多少条流水已在该店铺（导入该店铺时会被跳过）
    pub duplicate_rows: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SameFile {
    pub shop: String,
    pub imported_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResult {
    /// 提交时回传，对应服务器上暂存的文件
    pub token: String,
    pub file_name: String,
    pub file_size: u64,
    pub file_hash: String,
    pub row_count: i64,
    pub duplicate_in_file: i64,
    pub periods: Vec<String>,
    pub time_from: String,
    pub time_to: String,
    pub entities: Vec<EntityCount>,
    pub product_count: i64,
    /// 按匹配程度降序；只包含有重合的店铺
    pub candidates: Vec<ShopCandidate>,
    /// 系统建议的店铺（商品重合度或流水重合度 ≥ 50%）
    pub suggested_shop: Option<String>,
    /// 同一文件（内容 MD5 相同）以前导入过
    pub same_file: Option<SameFile>,
    pub missing_columns: Vec<String>,
    pub extra_columns: Vec<String>,
    pub invalid_numbers: Vec<InvalidNumber>,
    /// 文件涉及月份中已冻结（审批中 / 已锁账）的店铺月份：导入到这些店铺会被拒绝
    pub locks: Vec<close::PeriodLock>,
}

pub fn preview(conn: &Connection, path: &Path, token: &str, file_name: &str) -> AppResult<PreviewResult> {
    let result = (|| {
        let staged = stage(conn, path, "", token)?;
        let product = ident("商品ID");
        let id = ident("动帐流水号");

        let mut overlap: HashMap<String, (i64, i64)> = HashMap::new();
        let mut stmt = conn.prepare(&format!(
            "SELECT d.shop, count(DISTINCT d.{product}) FROM ledger_detail d
             WHERE d.{product} IN (SELECT DISTINCT {product} FROM {STAGING} WHERE {product} IS NOT NULL)
             GROUP BY 1"
        ))?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (shop, n) = row?;
            overlap.entry(shop).or_default().0 = n;
        }
        let mut stmt = conn.prepare(&format!(
            // 统计文件中有多少个流水号已存在于该店（不是库中命中行数，避免同一流水号多次出现时重复计数）
            "SELECT d.shop, count(DISTINCT s.id) FROM ledger_detail d
             JOIN (SELECT DISTINCT {id} AS id FROM {STAGING} WHERE {id} IS NOT NULL) s ON d.{id} = s.id
             GROUP BY 1"
        ))?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (shop, n) = row?;
            overlap.entry(shop).or_default().1 = n;
        }

        let products = staged.product_count.max(1) as f64;
        let rows = staged.row_count.max(1) as f64;
        let mut candidates: Vec<ShopCandidate> = overlap
            .into_iter()
            .map(|(shop, (matched, dup))| ShopCandidate {
                shop,
                matched_products: matched,
                product_ratio: (matched as f64 / products * 1000.0).round() / 1000.0,
                duplicate_rows: dup,
            })
            .collect();
        // 流水号已存在于某店是最强证据（同品牌多店可能卖相同商品，商品重合度会接近），其次看商品重合度
        let dup_ratio = |c: &ShopCandidate| c.duplicate_rows as f64 / rows;
        candidates.sort_by(|a, b| dup_ratio(b).total_cmp(&dup_ratio(a)).then(b.product_ratio.total_cmp(&a.product_ratio)));
        let suggested_shop =
            candidates.first().filter(|c| dup_ratio(c) >= 0.5 || c.product_ratio >= 0.5).map(|c| c.shop.clone());

        let same_file = conn
            .query_row(
                "SELECT shop, strftime(imported_at, '%Y-%m-%d %H:%M:%S') FROM ledger_import
                 WHERE file_hash = ? ORDER BY imported_at DESC LIMIT 1",
                params![staged.file_hash],
                |r| Ok(SameFile { shop: r.get(0)?, imported_at: r.get(1)? }),
            )
            .ok();

        let locks = close::locks(conn)?.into_iter().filter(|l| staged.periods.contains(&l.period)).collect();
        Ok(PreviewResult {
            token: token.to_string(),
            file_name: file_name.to_string(),
            file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or_default(),
            file_hash: staged.file_hash,
            row_count: staged.row_count,
            duplicate_in_file: staged.duplicate_in_file,
            periods: staged.periods,
            time_from: staged.time_from,
            time_to: staged.time_to,
            entities: staged.entities,
            product_count: staged.product_count,
            candidates,
            suggested_shop,
            same_file,
            missing_columns: staged.missing_columns,
            extra_columns: staged.extra_columns,
            invalid_numbers: staged.invalid_numbers,
            locks,
        })
    })();
    drop_staging(conn);
    result
}

// ── 提交 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedRow {
    pub id: String,
    pub time: String,
    pub before: String,
    pub after: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub import_id: String,
    pub shop: String,
    pub file_name: String,
    pub mode: String,
    /// 文件有效行数（已去除文件内重复）
    pub row_count: i64,
    /// 实际新增入库的行数
    pub inserted_rows: i64,
    /// 合并模式下因已存在而跳过的行数（含其中的变更行）
    pub skipped_rows: i64,
    /// 合并模式下流水号已存在、但金额/方向/场景与库中不同的行数（未覆盖，需人工确认）
    pub changed_rows: i64,
    pub changed_samples: Vec<ChangedRow>,
    /// 替换模式下被删除的旧数据行数
    pub replaced_rows: i64,
    pub duplicate_in_file: i64,
    pub periods: Vec<String>,
    pub time_from: String,
    pub time_to: String,
    pub entities: Vec<EntityCount>,
    pub missing_columns: Vec<String>,
    pub extra_columns: Vec<String>,
    pub invalid_numbers: Vec<InvalidNumber>,
    pub elapsed_ms: i64,
}

pub fn commit(conn: &Connection, path: &Path, shop: &str, file_name: &str, mode: ImportMode) -> AppResult<ImportResult> {
    let started = Instant::now();
    let shop = validate_shop(shop)?;
    let import_id = new_id("imp");
    let result = (|| {
        let staged = stage(conn, path, &shop, &import_id)?;
        close::ensure_periods_editable(conn, &shop, &staged.periods)?;
        let s_key = dedupe_key("s");
        let d_key = dedupe_key("d");
        let (amt, dir, scene, time) = (ident("动账金额"), ident("动账方向"), ident("动账场景"), ident("动账时间"));

        // 流水号已存在但内容不同（平台修正过数据），合并模式下不覆盖，只提示
        let changed_sql = format!(
            "FROM {STAGING} s JOIN ledger_detail d ON d.shop = s.shop AND {d_key} = {s_key}
             WHERE d.{amt} IS DISTINCT FROM s.{amt} OR d.{dir} IS DISTINCT FROM s.{dir} OR d.{scene} IS DISTINCT FROM s.{scene}"
        );
        let (changed_rows, changed_samples) = if mode == ImportMode::Merge {
            let n: i64 = conn.query_row(&format!("SELECT count(*) {changed_sql}"), [], |r| r.get(0))?;
            let mut stmt = conn.prepare(&format!(
                "SELECT {s_key}, s.{time},
                        concat_ws(' / ', d.{dir}, CAST(d.{amt} AS VARCHAR), d.{scene}),
                        concat_ws(' / ', s.{dir}, CAST(s.{amt} AS VARCHAR), s.{scene})
                 {changed_sql} LIMIT 5"
            ))?;
            let samples = stmt
                .query_map([], |r| Ok(ChangedRow { id: r.get(0)?, time: r.get(1)?, before: r.get(2)?, after: r.get(3)? }))?
                .collect::<Result<Vec<_>, _>>()?;
            (n, samples)
        } else {
            (0, vec![])
        };

        conn.execute_batch("BEGIN TRANSACTION")?;
        let written = (|| -> anyhow::Result<(i64, i64, i64)> {
            let replaced = if mode == ImportMode::Replace {
                conn.execute(
                    &format!("DELETE FROM ledger_detail WHERE shop = ? AND {time} BETWEEN ? AND ?"),
                    params![shop, staged.time_from, staged.time_to],
                )? as i64
            } else {
                0
            };
            let inserted = conn.execute(
                &format!(
                    "INSERT INTO ledger_detail BY NAME
                     SELECT s.*, {search} AS search_text FROM {STAGING} s
                     WHERE NOT EXISTS (SELECT 1 FROM ledger_detail d WHERE d.shop = s.shop AND {d_key} = {s_key})",
                    search = search_text_sql(),
                ),
                [],
            )? as i64;
            let skipped = staged.row_count - inserted;
            conn.execute(
                "INSERT INTO ledger_shop (name, created_at) SELECT ?, CAST(? AS TIMESTAMP) WHERE NOT EXISTS (SELECT 1 FROM ledger_shop WHERE name = ?)",
                params![shop, crate::db::now_local(), shop],
            )?;
            conn.execute(
                "INSERT INTO ledger_import (import_id, shop, file_name, periods, entities, row_count, replaced_rows,
                    missing_columns, extra_columns, elapsed_ms, file_hash, mode, inserted_rows, skipped_rows, changed_rows, imported_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
                params![
                    import_id,
                    shop,
                    file_name,
                    serde_json::to_string(&staged.periods)?,
                    serde_json::to_string(&staged.entities.iter().map(|e| &e.name).collect::<Vec<_>>())?,
                    staged.row_count,
                    replaced,
                    serde_json::to_string(&staged.missing_columns)?,
                    serde_json::to_string(&staged.extra_columns)?,
                    started.elapsed().as_millis() as i64,
                    staged.file_hash,
                    mode.as_str(),
                    inserted,
                    skipped,
                    changed_rows,
                    crate::db::now_local(),
                ],
            )?;
            Ok((replaced, inserted, skipped))
        })();
        let (replaced_rows, inserted_rows, skipped_rows) = match written {
            Ok(v) => {
                conn.execute_batch("COMMIT")?;
                v
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e.into());
            }
        };

        Ok(ImportResult {
            import_id: import_id.clone(),
            shop: shop.clone(),
            file_name: file_name.to_string(),
            mode: mode.as_str().into(),
            row_count: staged.row_count,
            inserted_rows,
            skipped_rows,
            changed_rows,
            changed_samples,
            replaced_rows,
            duplicate_in_file: staged.duplicate_in_file,
            periods: staged.periods,
            time_from: staged.time_from,
            time_to: staged.time_to,
            entities: staged.entities,
            missing_columns: staged.missing_columns,
            extra_columns: staged.extra_columns,
            invalid_numbers: staged.invalid_numbers,
            elapsed_ms: started.elapsed().as_millis() as i64,
        })
    })();
    drop_staging(conn);
    result
}

// ── 预览暂存文件 ──────────────────────────────────────

/// 预览上传的文件暂存在 tmp/preview-<token>.csv，提交时按 token 取回
pub fn preview_path(tmp_dir: &Path, token: &str) -> AppResult<PathBuf> {
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(AppError::bad("无效的预览凭证"));
    }
    let path = tmp_dir.join(format!("preview-{token}.csv"));
    if !path.exists() {
        return Err(AppError::bad("预览已过期，请重新选择文件"));
    }
    Ok(path)
}

pub fn new_preview_token() -> String {
    new_id("pv")
}

/// 清理超过 2 小时未提交的预览文件
pub fn cleanup_previews(tmp_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(tmp_dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let expired = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|e| e.as_secs() > 2 * 3600).unwrap_or(false))
            .unwrap_or(false);
        if name.to_string_lossy().starts_with("preview-") && expired {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

// ── 工具 ──────────────────────────────────────────────

/// 一次扫描统计每个金额列中「非空但不是数字」的值
fn count_invalid_numbers(conn: &Connection, source: &str, raw_of: &HashMap<&str, &str>) -> AppResult<Vec<InvalidNumber>> {
    let present: Vec<(&str, &str)> =
        DETAIL_COLUMNS.iter().filter(|c| is_numeric(c)).filter_map(|c| raw_of.get(c).map(|raw| (*c, *raw))).collect();
    if present.is_empty() {
        return Ok(vec![]);
    }
    let exprs = present
        .iter()
        .map(|(_, raw)| {
            let q = ident(raw);
            let bad = format!("trim({q}) <> '' AND TRY_CAST(trim({q}) AS DECIMAL(18,2)) IS NULL");
            format!("count(*) FILTER (WHERE {bad}), any_value({q}) FILTER (WHERE {bad})")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let values = conn.query_row(&format!("SELECT {exprs} FROM {source}"), [], |row| {
        let mut out = Vec::new();
        for (i, (col, _)) in present.iter().enumerate() {
            let count: i64 = row.get(i * 2)?;
            if count > 0 {
                let sample: Option<String> = row.get(i * 2 + 1)?;
                out.push(InvalidNumber { column: col.to_string(), count, sample: sample.unwrap_or_default() });
            }
        }
        Ok(out)
    })?;
    Ok(values)
}

fn query_strings(conn: &Connection, sql: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
