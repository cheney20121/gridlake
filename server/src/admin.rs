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
            Ok(BackupRecord { at, file: Some(target.display().to_string()), size, tables, elapsed_ms: started.elapsed().as_millis(), error: None })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
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

/// `gridlake restore <备份文件> [--yes]`：从备份恢复（服务需停止；当前数据文件改名保留）
pub fn cli_restore(config: &crate::config::Config, file: &Path, yes: bool) -> anyhow::Result<()> {
    if !file.is_file() {
        bail!("备份文件不存在：{}", file.display());
    }
    // 校验备份可以打开、包含本系统的表
    {
        let b = Connection::open_with_flags(file, duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?)
            .with_context(|| format!("备份文件无法打开：{}", file.display()))?;
        let n: i64 = b.query_row("SELECT count(*) FROM duckdb_tables() WHERE table_name IN ('ledger_dataset', 'ledger_workbook')", [], |r| r.get(0))?;
        if n < 2 {
            bail!("这不是 gridlake 的备份文件：{}", file.display());
        }
    }
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

/// 执行一次备份（与导入等写操作串行），记录结果并清理旧备份
pub async fn run_backup(state: &crate::routes::AppState) -> BackupRecord {
    let _guard = state.import_lock.lock().await;
    let st = state.clone();
    let r = tokio::task::spawn_blocking(move || -> anyhow::Result<BackupRecord> {
        st.checkpoint();
        let conn = st.connection()?;
        let r = backup(&conn, &st.config.backup_dir)?;
        copy_secret(&st.config.db_path, &st.config.backup_dir);
        prune(&st.config.backup_dir, st.config.backup_keep);
        Ok(r)
    })
    .await;
    let record = match r {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => BackupRecord { at: crate::db::now_local(), file: None, size: 0, tables: 0, elapsed_ms: 0, error: Some(format!("{e:#}")) },
        Err(e) => BackupRecord { at: crate::db::now_local(), file: None, size: 0, tables: 0, elapsed_ms: 0, error: Some(e.to_string()) },
    };
    if let Ok(mut last) = state.last_backup.lock() {
        *last = Some(record.clone());
    }
    record
}
