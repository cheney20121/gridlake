//! 运维：备份 / 恢复 / 状态检查（私有化部署由客户自己的运维值守，需要不依赖开发人员就能完成）
//!
//! - 自动备份：每天 LEDGER_BACKUP_AT（默认 02:30）把数据库完整复制到备份目录，保留最近 LEDGER_BACKUP_KEEP 份
//!   用 DuckDB 的 COPY FROM DATABASE 生成独立的新文件（事务内一致快照、自动压实），复制后逐表核对行数
//! - 手动备份：POST /api/admin/backup（只允许本机或带运维令牌）
//! - 恢复：停服务后执行 `gridlake restore <备份文件>`，原文件改名保留，不会直接覆盖

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, bail};
use duckdb::Connection;
use serde::Serialize;

use crate::db::{open_file, sql_path};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecord {
    pub at: String,
    pub file: Option<String>,
    pub size: u64,
    pub tables: usize,
    pub elapsed_ms: u128,
    /// 失败原因（成功为 None）
    pub error: Option<String>,
    /// 已上传到对象存储时的地址（配置了 LEDGER_BACKUP_S3_* 才有）
    pub remote: Option<String>,
    /// 上传到对象存储失败的原因。本地备份已经成功，所以不算 error，但监控指标会标为失败
    pub remote_error: Option<String>,
    /// 备份生成后立即做的验证（用与恢复相同的方式重新打开这个文件）
    pub verify: Option<VerifyReport>,
}

/// 恢复演练（验证备份能用）的结果
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    /// 验证的是哪个文件（本地路径，或 s3://桶/键）
    pub file: String,
    /// 备份文件的数据结构版本
    pub schema_version: i64,
    pub tables: usize,
    /// 导入的数据集个数（含视图）
    pub datasets: usize,
    /// 各数据表的总行数
    pub rows: i64,
    /// 是否做了全量扫描（读出每张表每一列，校验所有数据块）
    pub deep: bool,
    pub elapsed_ms: u128,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub file: String,
    pub size: u64,
    pub modified: String,
}

/// 生成一份完整备份；返回备份文件路径、表数
pub fn backup(conn: &Connection, dir: &Path) -> anyhow::Result<BackupRecord> {
    let started = Instant::now();
    let at = crate::db::now_local();
    std::fs::create_dir_all(dir).with_context(|| format!("创建备份目录失败：{}", dir.display()))?;
    let name = format!("ledger-{}.duckdb", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let target = dir.join(&name);
    // 先写 .part，核对无误后再改名：中途失败不会留下看似完整的备份
    let part = dir.join(format!("{name}.part"));
    for f in [&part, &part.with_extension("part.wal")] {
        let _ = std::fs::remove_file(f);
    }
    let main: String = conn.query_row("SELECT current_database()", [], |r| r.get(0))?;
    conn.execute_batch(&format!("ATTACH '{}' AS __backup", sql_path(&part)))?;
    let result = (|| -> anyhow::Result<usize> {
        conn.execute_batch(&format!("COPY FROM DATABASE \"{}\" TO __backup", main.replace('"', "\"\"")))?;
        // 逐表核对行数
        let mut stmt = conn.prepare("SELECT schema_name, table_name FROM duckdb_tables() WHERE database_name = ? AND NOT temporary")?;
        let tables = stmt.query_map([&main], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        for (schema, table) in &tables {
            let q = |db: &str| format!("SELECT count(*) FROM \"{db}\".\"{schema}\".\"{table}\"");
            let a: i64 = conn.query_row(&q(&main), [], |r| r.get(0))?;
            let b: i64 = conn.query_row(&q("__backup"), [], |r| r.get(0))?;
            if a != b {
                bail!("备份核对失败：{schema}.{table} 原有 {a} 行，备份中 {b} 行");
            }
        }
        Ok(tables.len())
    })();
    let _ = conn.execute_batch("DETACH __backup");
    match result {
        Ok(tables) => {
            std::fs::rename(&part, &target)?;
            let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
            Ok(BackupRecord { at, file: Some(target.display().to_string()), size, tables, elapsed_ms: started.elapsed().as_millis(), error: None, remote: None, remote_error: None, verify: None })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// 恢复演练：用恢复时相同的方式（只读打开这个文件）验证备份真的能用——能打开、是 gridlake 的数据文件、结构版本不比当前程序新、
/// 数据集定义没有损坏、每张数据表都读得出行数。`deep` 为真时还把每张表的每一列整个读一遍（校验全部数据块，亿级数据要几分钟）
pub fn verify_backup(file: &Path, deep: bool) -> anyhow::Result<VerifyReport> {
    let started = Instant::now();
    if !file.is_file() {
        bail!("备份文件不存在：{}", file.display());
    }
    let conn = Connection::open_with_flags(file, duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?)
        .with_context(|| format!("备份文件无法打开（可能已损坏）：{}", file.display()))?;
    let tables: Vec<String> = {
        let mut stmt = conn.prepare("SELECT table_name FROM duckdb_tables() WHERE NOT temporary ORDER BY table_name")?;
        stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?
    };
    if !["ledger_dataset", "ledger_workbook"].iter().all(|t| tables.iter().any(|x| x == t)) {
        bail!("这不是 gridlake 的备份文件：{}", file.display());
    }
    let schema_version: i64 = if tables.iter().any(|t| t == "ledger_meta") {
        conn.query_row("SELECT value FROM ledger_meta WHERE key = 'schema_version'", [], |r| r.get::<_, String>(0)).ok().and_then(|v| v.parse().ok()).unwrap_or(0)
    } else {
        0
    };
    if schema_version > crate::schema::SCHEMA_VERSION {
        bail!("备份的数据结构版本（{schema_version}）比当前程序（{}）更新，需要升级程序后才能恢复", crate::schema::SCHEMA_VERSION);
    }
    let (defs, problems) = crate::dataset::load(&conn).map_err(|e| anyhow::anyhow!("读取数据集定义失败：{}", e.message()))?;
    if !problems.is_empty() {
        bail!("备份里有失效的数据集定义：{}", problems.join("；"));
    }
    let mut rows = 0i64;
    for d in defs.iter().filter(|d| d.kind == "table" || d.builtin) {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {}", d.table), [], |r| r.get(0)).with_context(|| format!("数据集「{}」读取失败", d.name))?;
        rows += n;
    }
    if deep {
        for t in &tables {
            let cols: Vec<String> = {
                let mut stmt = conn.prepare("SELECT column_name FROM duckdb_columns() WHERE table_name = ? AND NOT internal ORDER BY column_index")?;
                stmt.query_map([t], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?
            };
            if cols.is_empty() {
                continue;
            }
            let args = cols.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect::<Vec<_>>().join(", ");
            conn.query_row(&format!("SELECT count(*) FROM (SELECT hash({args}) AS h FROM \"{}\") WHERE h IS NULL", t.replace('"', "\"\"")), [], |r| r.get::<_, i64>(0))
                .with_context(|| format!("全量扫描表 {t} 失败，备份文件可能已损坏"))?;
        }
    }
    Ok(VerifyReport { file: file.display().to_string(), schema_version, tables: tables.len(), datasets: defs.len(), rows, deep, elapsed_ms: started.elapsed().as_millis() })
}

/// 备份列表（新的在前）
pub fn list_backups(dir: &Path) -> Vec<BackupFile> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<BackupFile> = rd
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with("ledger-") && n.ends_with(".duckdb")
        })
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let modified: chrono::DateTime<chrono::Local> = m.modified().ok()?.into();
            Some(BackupFile { file: e.path().display().to_string(), size: m.len(), modified: modified.format("%Y-%m-%d %H:%M:%S").to_string() })
        })
        .collect();
    out.sort_by(|a, b| b.file.cmp(&a.file));
    out
}

/// 连接器密钥（secret.key）一并放进备份目录：换服务器恢复时没有它，已保存的数据库密码无法解密
/// （通过 LEDGER_SECRET_KEY 环境变量提供密钥时不存在该文件，由运维自行保管）
pub fn copy_secret(db_path: &Path, dir: &Path) {
    let key = db_path.parent().unwrap_or(Path::new(".")).join("secret.key");
    if key.is_file() {
        let to = dir.join("secret.key");
        match std::fs::copy(&key, &to) {
            Ok(_) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o600));
                }
            }
            Err(e) => tracing::warn!("复制连接器密钥到备份目录失败：{e}"),
        }
    }
}

/// 只保留最近 keep 份；顺带清理中断留下的 .part
pub fn prune(dir: &Path, keep: usize) -> usize {
    let mut removed = 0;
    for b in list_backups(dir).into_iter().skip(keep) {
        if std::fs::remove_file(&b.file).is_ok() {
            removed += 1;
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            let n = e.file_name().to_string_lossy().to_string();
            if n.contains(".duckdb.part") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    removed
}

// ── 状态 ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetStatus {
    pub name: String,
    pub kind: String,
    pub rows: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: &'static str,
    /// 数据文件的结构版本（见 schema::SCHEMA_VERSION）
    pub schema_version: i64,
    pub db_file: String,
    pub db_size: u64,
    pub wal_size: u64,
    pub memory_limit: String,
    pub threads: String,
    pub datasets: Vec<DatasetStatus>,
    pub workbooks: i64,
    /// 失效的计算列 / 关联视图（升级或误删数据后需要处理）
    pub problems: Vec<String>,
    pub backups: Vec<BackupFile>,
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

pub fn status(conn: &Connection, db_path: &Path, backup_dir: &Path) -> anyhow::Result<Status> {
    let setting = |name: &str| -> String { conn.query_row("SELECT current_setting(?)::VARCHAR", [name], |r| r.get(0)).unwrap_or_default() };
    let (defs, problems) = crate::dataset::load(conn).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let datasets = defs
        .iter()
        .map(|d| DatasetStatus {
            name: d.name.clone(),
            kind: d.kind.clone(),
            rows: conn.query_row(&format!("SELECT count(*) FROM {}", d.table), [], |r| r.get(0)).ok(),
        })
        .collect();
    let workbooks = conn.query_row("SELECT count(*) FROM ledger_workbook WHERE deleted_at IS NULL", [], |r| r.get(0)).unwrap_or(0);
    Ok(Status {
        version: VERSION,
        schema_version: conn.query_row("SELECT value FROM ledger_meta WHERE key = 'schema_version'", [], |r| r.get::<_, String>(0)).ok().and_then(|v| v.parse().ok()).unwrap_or(0),
        db_file: db_path.display().to_string(),
        db_size: file_size(db_path),
        wal_size: file_size(&PathBuf::from(format!("{}.wal", db_path.display()))),
        memory_limit: setting("memory_limit"),
        threads: setting("threads"),
        datasets,
        workbooks,
        problems,
        backups: list_backups(backup_dir),
    })
}

fn human(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.2} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} MB", n / 1e6)
    } else {
        format!("{:.0} KB", n / 1e3)
    }
}

// ── 命令行 ────────────────────────────────────────────

/// `gridlake check`：离线检查数据文件（服务需停止；运行中请用 GET /api/admin/status）
pub fn cli_check(config: &crate::config::Config) -> anyhow::Result<()> {
    let conn = open_file(&config.db_path)?;
    let s = status(&conn, &config.db_path, &config.backup_dir)?;
    println!("gridlake {}", s.version);
    println!("数据文件  {}（{}，WAL {}）", s.db_file, human(s.db_size), human(s.wal_size));
    println!("数据集    {} 个，工作簿 {} 个", s.datasets.len(), s.workbooks);
    for d in &s.datasets {
        println!("  - {:<24} {:<8} {}", d.name, d.kind, d.rows.map(|r| format!("{r} 行")).unwrap_or_else(|| "无法读取".into()));
    }
    println!("备份      {} 份（{}）", s.backups.len(), config.backup_dir.display());
    if let Some(b) = s.backups.first() {
        println!("  最近：{}（{}，{}）", b.file, human(b.size), b.modified);
    }
    if s.problems.is_empty() {
        println!("检查结果  正常");
        Ok(())
    } else {
        println!("检查结果  发现 {} 个问题：", s.problems.len());
        for p in &s.problems {
            println!("  ! {p}");
        }
        bail!("数据检查未通过")
    }
}

/// `gridlake backup [目录]`：离线备份（服务运行中请用 POST /api/admin/backup）
pub fn cli_backup(config: &crate::config::Config, dir: Option<PathBuf>) -> anyhow::Result<()> {
    let conn = open_file(&config.db_path)?;
    let dir = dir.unwrap_or_else(|| config.backup_dir.clone());
    let r = backup(&conn, &dir)?;
    copy_secret(&config.db_path, &dir);
    println!("已备份：{}（{}，{} 张表，{} ms）", r.file.unwrap_or_default(), human(r.size), r.tables, r.elapsed_ms);
    Ok(())
}

/// `gridlake verify <备份文件> [--full]`：验证备份文件能用（只读打开，不改动任何文件）
pub fn cli_verify(file: &Path, deep: bool) -> anyhow::Result<()> {
    let v = verify_backup(file, deep)?;
    println!(
        "备份可以使用：{} 张表，{} 个数据集，共 {} 行，数据结构版本 {}（{}，{} ms）",
        v.tables,
        v.datasets,
        v.rows,
        v.schema_version,
        if v.deep { "已全量扫描" } else { "快速检查" },
        v.elapsed_ms
    );
    Ok(())
}

/// `gridlake restore <备份文件> [--yes]`：从备份恢复（服务需停止；当前数据文件改名保留）
pub fn cli_restore(config: &crate::config::Config, file: &Path, yes: bool) -> anyhow::Result<()> {
    if !file.is_file() {
        bail!("备份文件不存在：{}", file.display());
    }
    // 恢复前先验证：能打开、是 gridlake 的数据文件、结构版本能用、数据集读得出来
    let v = verify_backup(file, false)?;
    println!("备份校验通过：{} 张表，{} 个数据集，共 {} 行（数据结构版本 {}）", v.tables, v.datasets, v.rows, v.schema_version);
    let db = &config.db_path;
    // 能打开说明服务没有在运行（运行中的服务持有文件锁）
    if db.exists() {
        drop(open_file(db).context("请先停止服务再恢复")?);
    }
    println!("将用备份 {} 恢复数据文件 {}", file.display(), db.display());
    if db.exists() {
        println!("当前数据文件会改名保留为 *.before-restore-<时间>，确认无误后可自行删除");
    }
    if !yes {
        print!("输入 yes 继续：");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != "yes" {
            println!("已取消");
            return Ok(());
        }
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    if db.exists() {
        let keep = PathBuf::from(format!("{}.before-restore-{stamp}", db.display()));
        std::fs::rename(db, &keep)?;
        let wal = PathBuf::from(format!("{}.wal", db.display()));
        if wal.exists() {
            std::fs::rename(&wal, PathBuf::from(format!("{}.wal", keep.display())))?;
        }
        println!("原数据文件已保留：{}", keep.display());
    }
    if let Some(dir) = db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::copy(file, db)?;
    println!("恢复完成，启动服务即可");
    Ok(())
}

/// 每天定时备份
pub async fn schedule_backups(state: crate::routes::AppState) {
    let Some(at) = state.config.backup_at else { return };
    loop {
        let now = chrono::Local::now().naive_local();
        let mut next = now.date().and_time(at);
        if next <= now {
            next += chrono::Duration::days(1);
        }
        let wait = (next - now).to_std().unwrap_or_default();
        tracing::info!("下次自动备份：{}", next.format("%Y-%m-%d %H:%M"));
        tokio::time::sleep(wait).await;
        let r = run_backup(&state).await;
        match &r.error {
            None => tracing::info!("自动备份完成：{} ({} ms)", r.file.as_deref().unwrap_or(""), r.elapsed_ms),
            Some(e) => tracing::error!("自动备份失败：{e}"),
        }
    }
}

/// 执行一次备份（生成备份文件的这一段与导入等写操作串行），然后上传到对象存储（不占用导入锁），记录结果并清理旧备份
pub async fn run_backup(state: &crate::routes::AppState) -> BackupRecord {
    let r = {
        let _guard = state.import_lock.lock().await;
        let st = state.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<BackupRecord> {
            st.checkpoint();
            let conn = st.connection()?;
            let mut r = backup(&conn, &st.config.backup_dir)?;
            // 立刻用恢复时相同的方式重新打开这份备份：打不开 / 读不出来的备份不能留着当作可用的
            if let Some(file) = r.file.clone() {
                match verify_backup(Path::new(&file), false) {
                    Ok(v) => r.verify = Some(v),
                    Err(e) => {
                        let bad = format!("{file}.failed");
                        let _ = std::fs::rename(&file, &bad);
                        bail!("备份文件已生成，但验证未通过，已改名为 {bad}：{e:#}");
                    }
                }
            }
            copy_secret(&st.config.db_path, &st.config.backup_dir);
            prune(&st.config.backup_dir, st.config.backup_keep);
            Ok(r)
        })
        .await
    };
    let mut record = match r {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => BackupRecord { at: crate::db::now_local(), file: None, size: 0, tables: 0, elapsed_ms: 0, error: Some(format!("{e:#}")), remote: None, remote_error: None, verify: None },
        Err(e) => BackupRecord { at: crate::db::now_local(), file: None, size: 0, tables: 0, elapsed_ms: 0, error: Some(e.to_string()), remote: None, remote_error: None, verify: None },
    };
    if record.error.is_none()
        && let (Some(src), Some(file)) = (state.config.backup_s3.as_ref(), record.file.clone())
    {
        match upload_remote(state, src, Path::new(&file)).await {
            Ok(url) => record.remote = Some(url),
            Err(e) => {
                tracing::error!("备份已生成，但上传到对象存储失败：{e}");
                record.remote_error = Some(e);
            }
        }
    }
    if let Ok(mut last) = state.last_backup.lock() {
        *last = Some(record.clone());
    }
    record
}

/// 把备份文件传到对象存储，核对大小，再清理对象存储上较早的备份。不上传 secret.key：它和备份放在一起就等于没加密，请单独保管
async fn upload_remote(state: &crate::routes::AppState, src: &crate::connector::s3::S3Source, file: &Path) -> Result<String, String> {
    use crate::connector::s3;
    let rt = &state.connectors;
    let secret = src.secret_key.clone().unwrap_or_default();
    let name = file.file_name().and_then(|n| n.to_str()).ok_or("备份文件名无效")?;
    let key = format!("{}{name}", src.prefix);
    let url = format!("s3://{}/{key}", src.bucket);
    let local = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
    let started = Instant::now();
    s3::upload_file(src, &secret, rt.s3_client(), &key, file, state.config.backup_s3_part_bytes).await.map_err(|e| e.message())?;
    let remote = s3::head_size(src, &secret, rt.s3_client(), &key).await.map_err(|e| e.message())?;
    if remote != local {
        return Err(format!("上传后核对失败：本地 {local} 字节，对象存储上 {remote} 字节（{url}）"));
    }
    tracing::info!("备份已上传到 {url}（{} ，{} ms）", human(local), started.elapsed().as_millis());
    // 可选：把传上去的备份下载回来，用恢复时相同的方式验证（要多一次下载流量，默认只核对大小）
    if state.config.backup_s3_verify {
        let v = verify_remote(state, src, &secret, &key, false).await.map_err(|e| format!("上传的备份下载回来验证未通过：{e}"))?;
        tracing::info!("已验证对象存储上的备份可以恢复：{} 个数据集，{} 行", v.datasets, v.rows);
    }
    // 保留最近 N 份；清理失败只记日志，不影响这次备份的结果
    match s3::list_objects(src, &secret, rt.s3_client()).await {
        Ok(objects) => {
            let mut keys: Vec<String> = objects.into_iter().map(|(k, _, _)| k).filter(|k| remote_backup_name(k)).collect();
            keys.sort_by(|a, b| b.cmp(a));
            for old in keys.into_iter().skip(state.config.backup_s3_keep) {
                match s3::delete_object(src, &secret, rt.s3_client(), &old).await {
                    Ok(()) => tracing::info!("已清理对象存储上较早的备份：s3://{}/{old}", src.bucket),
                    Err(e) => tracing::warn!("清理对象存储上的旧备份失败（{old}）：{}", e.message()),
                }
            }
        }
        Err(e) => tracing::warn!("列出对象存储上的备份失败，跳过清理：{}", e.message()),
    }
    Ok(url)
}

/// 把对象存储上的一份备份下载到临时目录，做恢复演练，再删掉临时文件
async fn verify_remote(state: &crate::routes::AppState, src: &crate::connector::s3::S3Source, secret: &str, key: &str, deep: bool) -> Result<VerifyReport, String> {
    use crate::connector::{folder::FileEntry, s3};
    let rt = &state.connectors;
    let name = key.rsplit('/').next().unwrap_or(key).to_string();
    let url = format!("s3://{}/{key}", src.bucket);
    let size = s3::head_size(src, secret, rt.s3_client(), key).await.map_err(|e| e.message())?;
    let dest = state.tmp_dir.join(format!("verify-{name}"));
    let entry = FileEntry { path: url.clone(), name, size, mtime: String::new(), imported: false, settling: false };
    let downloaded = s3::download(src, secret, rt.s3_client(), &entry, &dest, u64::MAX).await.map_err(|e| e.message());
    let result = match downloaded {
        Ok(()) => {
            let d = dest.clone();
            tokio::task::spawn_blocking(move || verify_backup(&d, deep)).await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| format!("{e:#}")))
        }
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&dest);
    result.map(|mut v| {
        v.file = url;
        v
    })
}

/// 恢复演练：source 为 local 时验证本地最新的备份，remote 时把对象存储上最新的备份下载回来验证；deep 为真时全量扫描
pub async fn run_verify(state: &crate::routes::AppState, remote: bool, deep: bool) -> Result<VerifyReport, String> {
    if remote {
        let src = state.config.backup_s3.as_ref().ok_or("没有配置对象存储备份（LEDGER_BACKUP_S3_BUCKET）")?;
        let secret = src.secret_key.clone().unwrap_or_default();
        let mut keys: Vec<String> = crate::connector::s3::list_objects(src, &secret, state.connectors.s3_client()).await.map_err(|e| e.message())?.into_iter().map(|(k, _, _)| k).filter(|k| remote_backup_name(k)).collect();
        keys.sort_by(|a, b| b.cmp(a));
        let key = keys.first().ok_or("对象存储上还没有备份")?;
        verify_remote(state, src, &secret, key, deep).await
    } else {
        let file = list_backups(&state.config.backup_dir).into_iter().next().ok_or("本地还没有备份")?.file;
        tokio::task::spawn_blocking(move || verify_backup(Path::new(&file), deep)).await.map_err(|e| e.to_string())?.map_err(|e| format!("{e:#}"))
    }
}

/// 对象键的文件名是不是备份文件（只清理它们，不碰同一前缀下别的东西）
fn remote_backup_name(key: &str) -> bool {
    let n = key.rsplit('/').next().unwrap_or(key);
    n.starts_with("ledger-") && n.ends_with(".duckdb")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个带一张导入数据表的数据文件
    fn sample_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        crate::schema::stamp_version(&conn, "test").unwrap();
        conn.execute_batch(
            "CREATE TABLE ds_1 (a VARCHAR, b BIGINT);
             INSERT INTO ds_1 SELECT 'x' || i, i FROM range(1000) t(i);
             INSERT INTO ledger_dataset (id, name, columns, kind) VALUES ('ds_1', '测试表', '[{\"key\":\"a\",\"title\":\"a\",\"kind\":\"text\",\"width\":100},{\"key\":\"b\",\"title\":\"b\",\"kind\":\"number\",\"width\":100}]', 'table');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_fresh_backup_verifies_and_reports_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let conn = sample_db(&dir.path().join("live.duckdb"));
        let r = backup(&conn, &dir.path().join("backups")).unwrap();
        let file = PathBuf::from(r.file.unwrap());
        let v = verify_backup(&file, false).unwrap();
        assert_eq!((v.rows, v.deep, v.schema_version), (1000, false, crate::schema::SCHEMA_VERSION));
        assert!(v.datasets >= 1);
        assert!(v.tables >= 10);
        // 全量扫描同样通过
        assert!(verify_backup(&file, true).unwrap().deep);
    }

    #[test]
    fn broken_backups_are_rejected_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        // 不存在
        assert!(verify_backup(&dir.path().join("nope.duckdb"), false).unwrap_err().to_string().contains("不存在"));
        // 不是数据库
        let junk = dir.path().join("junk.duckdb");
        std::fs::write(&junk, vec![7u8; 4096]).unwrap();
        assert!(verify_backup(&junk, false).unwrap_err().to_string().contains("无法打开"));
        // 是 DuckDB 文件但不是 gridlake 的
        let other = dir.path().join("other.duckdb");
        Connection::open(&other).unwrap().execute_batch("CREATE TABLE t (a INT)").unwrap();
        assert!(verify_backup(&other, false).unwrap_err().to_string().contains("不是 gridlake"));
        // 数据结构版本比当前程序新
        let newer = dir.path().join("newer.duckdb");
        let conn = sample_db(&newer);
        conn.execute("UPDATE ledger_meta SET value = ? WHERE key = 'schema_version'", [(crate::schema::SCHEMA_VERSION + 1).to_string()]).unwrap();
        drop(conn);
        assert!(verify_backup(&newer, false).unwrap_err().to_string().contains("更新"));
        // 数据集登记了，但数据表丢了
        let lost = dir.path().join("lost.duckdb");
        let conn = sample_db(&lost);
        conn.execute_batch("DROP TABLE ds_1").unwrap();
        drop(conn);
        assert!(verify_backup(&lost, false).unwrap_err().to_string().contains("测试表"));
    }

    #[test]
    fn remote_backup_names_are_recognised() {
        assert!(remote_backup_name("gl-backups/ledger-20261004-023000.duckdb"));
        assert!(!remote_backup_name("gl-backups/notes.txt"));
        assert!(!remote_backup_name("gl-backups/ledger-20261004-023000.duckdb.failed"));
    }
}
