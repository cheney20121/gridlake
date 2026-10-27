//! DuckDB 连接管理
//!
//! DuckDB 支持同一进程内多连接并发读、单写。读请求各自 `try_clone` 一个连接在阻塞线程池里执行；
//! 导入（大批量写）通过 `import_lock` 串行，避免两次导入交错删除/插入同一店铺月份的数据

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use duckdb::Connection;

use crate::schema;

/// 本地时间（YYYY-MM-DD HH:MM:SS）。内置 DuckDB 未编译 ICU 时区扩展，current_timestamp / current_date
/// 按 UTC 计算（北京时间 0~8 点会差一天），所有写入的时间与「今天」统一由程序按本地时区给出
pub fn now_local() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn today_local() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

pub struct Db {
    base: Mutex<Connection>,
    pub config: crate::config::Config,
    pub started: std::time::Instant,
    /// 重计算（透视、SQL、异常发现、带筛选 / 排序的游标）的并发闸门：同时进行的个数有限，其余排队
    pub heavy: HeavyGate,
    /// 正在执行的数据库操作数（健康检查里显示）
    pub active_queries: std::sync::atomic::AtomicUsize,
    /// 最近一次备份的结果（成功或失败原因），运维状态页展示
    pub last_backup: Mutex<Option<crate::admin::BackupRecord>>,
    /// 数据连接器：密钥、收件目录、正在同步的连接器
    pub connectors: crate::connector::Runtime,
    /// 大数据表的查询游标（数据写入后需调用 cursors.invalidate()）
    pub cursors: crate::dataset::CursorCache,
    pub import_lock: tokio::sync::Mutex<()>,
    pub tmp_dir: PathBuf,
    /// 登录身份与操作审计
    pub auth: crate::auth::Auth,
    /// 数据范围（行级权限）规则缓存
    pub scope: crate::scope::ScopeStore,
    /// 事件 Webhook 的发送队列唤醒信号与订阅缓存
    pub webhooks: crate::webhook::Hub,
}

/// 重计算的并发闸门。DuckDB 每个重查询都会吃满全部 CPU 核并占用大块内存，几个一起跑时内存峰值叠加（亿级数据实测可到 6 GB 以上），
/// 所以限制同时进行的个数，其余排队；排队太久返回「服务繁忙」而不是无限挂着。排队中的请求若客户端断开，会自动退出队列
pub struct HeavyGate {
    sem: std::sync::Arc<tokio::sync::Semaphore>,
    limit: usize,
    waiting: std::sync::atomic::AtomicUsize,
    wait: std::time::Duration,
}

/// 闸门状态（健康检查里显示）
#[derive(serde::Serialize)]
pub struct HeavyStatus {
    pub limit: usize,
    pub running: usize,
    pub waiting: usize,
}

impl HeavyGate {
    pub fn new(limit: usize, wait: std::time::Duration) -> Self {
        let limit = limit.max(1);
        Self { sem: std::sync::Arc::new(tokio::sync::Semaphore::new(limit)), limit, waiting: std::sync::atomic::AtomicUsize::new(0), wait }
    }

    pub async fn acquire(&self) -> Result<tokio::sync::OwnedSemaphorePermit, crate::error::AppError> {
        use std::sync::atomic::Ordering::SeqCst;
        // 先试一下：有空位就不排队
        if let Ok(p) = self.sem.clone().try_acquire_owned() {
            return Ok(p);
        }
        self.waiting.fetch_add(1, SeqCst);
        let _waiting = scopeguard_waiting(&self.waiting);
        match tokio::time::timeout(self.wait, self.sem.clone().acquire_owned()).await {
            Ok(Ok(p)) => Ok(p),
            _ => Err(crate::error::AppError::Unavailable(format!(
                "服务繁忙：已有 {} 个大计算在进行，排队超过 {} 秒。请稍后重试，或缩小筛选范围 / 减少汇总维度",
                self.limit,
                self.wait.as_secs()
            ))),
        }
    }

    pub fn status(&self) -> HeavyStatus {
        HeavyStatus { limit: self.limit, running: self.limit - self.sem.available_permits().min(self.limit), waiting: self.waiting.load(std::sync::atomic::Ordering::SeqCst) }
    }
}

/// 离开作用域（拿到位置、超时或客户端断开）时把排队计数减一
struct Waiting<'a>(&'a std::sync::atomic::AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn scopeguard_waiting(c: &std::sync::atomic::AtomicUsize) -> Waiting<'_> {
    Waiting(c)
}

impl Db {
    pub fn open(config: &crate::config::Config) -> anyhow::Result<Self> {
        let path = config.db_path.as_path();
        let conn = open_file(path)?;
        let tmp_dir = path.parent().unwrap_or(Path::new(".")).join("tmp");
        std::fs::create_dir_all(&tmp_dir)?;

        // 资源上限：内存不够时排序 / 关联 / 聚合的中间结果落盘，而不是被系统 OOM 杀掉
        let spill = tmp_dir.join("spill");
        std::fs::create_dir_all(&spill)?;
        conn.execute_batch(&format!("SET GLOBAL temp_directory = '{}'", sql_path(&spill)))?;
        if let Some(m) = &config.memory_limit {
            conn.execute_batch(&format!("SET GLOBAL memory_limit = '{m}'")).with_context(|| format!("内存上限设置无效：{m}"))?;
        }
        if let Some(n) = config.threads {
            conn.execute_batch(&format!("SET GLOBAL threads = {n}"))?;
        }

        // 先看数据文件是不是比本程序更新的版本写的（是就拒绝，不做任何改动），再建表 / 升级 / 记录版本
        let from = schema::check_version(&conn)?;
        conn.execute_batch(&schema::create_tables_sql())?;
        conn.execute_batch(&schema::migrate_sql())?;
        schema::stamp_version(&conn, crate::admin::VERSION)?;
        // 场景包：要在加载数据集之前确定（内置的「资金流水」数据集只在电商场景包启用时出现）
        let ecommerce = crate::ecommerce::init(&config.packs, &conn);
        tracing::info!("场景包：电商台账{}", if ecommerce { "已启用" } else { "未启用" });
        if from < schema::SCHEMA_VERSION {
            tracing::info!("数据文件结构已升级：版本 {from} → {}", schema::SCHEMA_VERSION);
        }
        // 查询游标的位置表放在单独的临时库：不让主库文件膨胀、不进备份；进程重启后全部作废
        // （旧版本放在主库的 cursors schema 里，这里一并清掉）
        conn.execute_batch("DROP SCHEMA IF EXISTS cursors CASCADE")?;
        let cursor_db = tmp_dir.join("cursors.duckdb");
        for f in [cursor_db.clone(), cursor_db.with_extension("duckdb.wal")] {
            let _ = std::fs::remove_file(f);
        }
        conn.execute_batch(&format!("ATTACH '{}' AS cursors", sql_path(&cursor_db)))?;
        // 立即把迁移写入主文件：进程若在 WAL 合并前被强制结束，下次启动回放 WAL 可能触发 DuckDB 内部错误
        conn.execute_batch("CHECKPOINT")?;
        crate::connector::recover(&conn).map_err(|e| anyhow::anyhow!(e.message()))?;
        let cursors = crate::dataset::CursorCache::new();
        Ok(Self {
            base: Mutex::new(conn),
            config: config.clone(),
            started: std::time::Instant::now(),
            heavy: HeavyGate::new(config.heavy_queries, std::time::Duration::from_secs(config.heavy_wait_secs)),
            active_queries: std::sync::atomic::AtomicUsize::new(0),
            last_backup: Mutex::new(None),
            connectors: crate::connector::Runtime::new(config)?,
            cursors,
            import_lock: tokio::sync::Mutex::new(()),
            tmp_dir,
            auth: crate::auth::Auth::new(config)?,
            scope: crate::scope::ScopeStore::default(),
            webhooks: crate::webhook::Hub::default(),
        })
    }

    /// 将 WAL 合并进主文件（写操作完成后、进程退出前调用）
    pub fn checkpoint(&self) {
        match self.connection().and_then(|c| Ok(c.execute_batch("CHECKPOINT")?)) {
            Ok(()) => {}
            Err(e) => tracing::warn!("checkpoint failed: {e:#}"),
        }
    }

    pub fn connection(&self) -> anyhow::Result<Connection> {
        let base = self.base.lock().map_err(|_| anyhow::anyhow!("数据库连接锁已损坏"))?;
        Ok(base.try_clone()?)
    }
}

/// 打开数据文件（命令行工具与服务共用）；文件被正在运行的服务占用时给出明确提示
pub fn open_file(path: &Path) -> anyhow::Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("创建数据目录失败：{}", dir.display()))?;
    }
    Connection::open(path).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("lock") {
            anyhow::anyhow!("数据文件正被另一个进程使用（服务是否在运行？）：{}\n{msg}", path.display())
        } else {
            anyhow::anyhow!("打开 DuckDB 失败：{}\n{msg}", path.display())
        }
    })
}

/// 路径拼进 SQL 字符串字面量
pub fn sql_path(p: &Path) -> String {
    p.to_string_lossy().replace('\'', "''")
}
