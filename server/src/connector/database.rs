//! 数据库来源：MySQL / MariaDB、PostgreSQL、ClickHouse（ClickHouse 走 HTTP 接口，见 clickhouse.rs）
//!
//! - 只允许单条 SELECT / WITH，并在只读事务里执行，设置语句超时
//! - 用文本协议取数（MySQL COM_QUERY、PostgreSQL 简单查询），所有类型都以文本拿到，与 CSV 导入的清洗口径一致
//! - 流式写入临时 CSV，内存占用恒定
//! - 增量：`增量列 >= 上次最大值`，同一时刻后写入的行不会漏，重复的行由主键去重 / 覆盖

use std::path::Path;
use std::time::Duration;

use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{Column, Connection, Executor, Row};

use crate::error::{AppError, AppResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STATEMENT_TIMEOUT_SECS: u64 = 600;

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Mysql,
    Postgres,
    Clickhouse,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DbSource {
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    /// 只在请求中出现（明文），保存时加密为 password_enc，响应中永不返回
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_enc: Option<String>,
    #[serde(default)]
    pub ssl: SslMode,
    pub sql: String,
    /// 增量列（为空则每次全量取数）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental_column: Option<String>,
}

impl DbSource {
    /// 比较来源是否变化（不含密码）：变化后增量位置需要重置
    pub fn same_query(&self, other: &DbSource) -> bool {
        self.engine == other.engine
            && self.host == other.host
            && self.port == other.port
            && self.database == other.database
            && self.sql.trim() == other.sql.trim()
            && self.incremental_column == other.incremental_column
    }
}

/// 校验 SQL：单条 SELECT / WITH 语句
pub fn check_sql(sql: &str) -> AppResult<String> {
    let s = sql.trim().trim_end_matches(';').trim();
    let lower = s.to_lowercase();
    if !(lower.starts_with("select") || lower.starts_with("with")) {
        return Err(AppError::bad("只支持查询语句（SELECT / WITH 开头）"));
    }
    if s.contains(';') {
        return Err(AppError::bad("只能写一条查询语句（不能包含分号）"));
    }
    Ok(s.to_string())
}

fn quote_ident(engine: &Engine, name: &str) -> String {
    match engine {
        Engine::Mysql | Engine::Clickhouse => format!("`{}`", name.replace('`', "``")),
        Engine::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

fn quote_literal(engine: &Engine, v: &str) -> String {
    let v = v.replace('\'', "''");
    match engine {
        // MySQL、ClickHouse 默认把反斜杠当转义符
        Engine::Mysql | Engine::Clickhouse => format!("'{}'", v.replace('\\', "\\\\")),
        Engine::Postgres => format!("'{v}'"),
    }
}

/// 实际执行的查询：原 SQL 作为子查询，增量时加条件与排序，预览时加 LIMIT
pub(super) fn build_query(src: &DbSource, watermark: Option<&str>, limit: Option<u32>) -> AppResult<String> {
    let base = check_sql(&src.sql)?;
    let mut q = format!("SELECT * FROM ({base}) __src");
    if let Some(col) = src.incremental_column.as_deref().filter(|c| !c.trim().is_empty()) {
        let c = quote_ident(&src.engine, col.trim());
        if let Some(w) = watermark {
            q.push_str(&format!(" WHERE {c} >= {}", quote_literal(&src.engine, w)));
        }
        q.push_str(&format!(" ORDER BY {c}"));
    }
    if let Some(n) = limit {
        q.push_str(&format!(" LIMIT {n}"));
    }
    Ok(q)
}

enum Conn {
    My(sqlx::MySqlConnection),
    Pg(sqlx::PgConnection),
}

fn friendly(e: sqlx::Error) -> AppError {
    let msg = match &e {
        sqlx::Error::Database(d) => format!("数据库返回错误：{}", d.message()),
        sqlx::Error::Io(io) => match io.kind() {
            std::io::ErrorKind::ConnectionRefused => "无法连接数据库：连接被拒绝（数据库服务未启动、端口不对，或防火墙不允许本服务器访问）".into(),
            std::io::ErrorKind::TimedOut => "无法连接数据库：连接超时（检查地址、端口与网络）".into(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::UnexpectedEof => {
                format!("数据库连接被断开（{io}），可能是网络不稳定或数据库重启，下次会自动重试")
            }
            _ if io.to_string().contains("failed to lookup") || io.to_string().contains("nodename nor servname") => "无法连接数据库：找不到这个主机名（检查地址拼写或 DNS）".into(),
            _ => format!("无法连接数据库：{io}"),
        },
        sqlx::Error::Protocol(m) => format!("对方不是 MySQL / PostgreSQL 服务，或数据库类型、端口选错了（{m}）"),
        sqlx::Error::Decode(_) if e.to_string().contains("SSLRequest") || e.to_string().contains("unexpected") => {
            format!("对方不是 MySQL / PostgreSQL 服务，或数据库类型、端口选错了（{e}）")
        }
        sqlx::Error::Tls(t) => format!("加密连接失败：{t}（可把「加密连接」改为不加密或优先加密）"),
        other => format!("数据库错误：{other}"),
    };
    AppError::bad(msg)
}

async fn connect(src: &DbSource, password: &str) -> AppResult<Conn> {
    let fut = async {
        Ok::<_, sqlx::Error>(match src.engine {
            Engine::Mysql => {
                use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
                let opts = MySqlConnectOptions::new()
                    .host(&src.host)
                    .port(src.port)
                    .username(&src.user)
                    .password(password)
                    .database(&src.database)
                    .charset("utf8mb4")
                    .ssl_mode(match src.ssl {
                        SslMode::Disable => MySqlSslMode::Disabled,
                        SslMode::Prefer => MySqlSslMode::Preferred,
                        SslMode::Require => MySqlSslMode::Required,
                    });
                Conn::My(sqlx::MySqlConnection::connect_with(&opts).await?)
            }
            Engine::Clickhouse => unreachable!("ClickHouse 走 HTTP 接口"),
            Engine::Postgres => {
                use sqlx::postgres::{PgConnectOptions, PgSslMode};
                let opts = PgConnectOptions::new()
                    .host(&src.host)
                    .port(src.port)
                    .username(&src.user)
                    .password(password)
                    .database(&src.database)
                    .application_name("gridlake-connector")
                    .ssl_mode(match src.ssl {
                        SslMode::Disable => PgSslMode::Disable,
                        SslMode::Prefer => PgSslMode::Prefer,
                        SslMode::Require => PgSslMode::Require,
                    });
                Conn::Pg(sqlx::PgConnection::connect_with(&opts).await?)
            }
        })
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, fut).await {
        Ok(r) => r.map_err(friendly),
        Err(_) => Err(AppError::bad(format!("连接 {}:{} 超时（10 秒），请检查地址、端口与网络", src.host, src.port))),
    }
}

/// 会话设置：只读事务 + 语句超时（MariaDB / MySQL 的超时变量不同，不支持的忽略）
async fn begin_read_only(conn: &mut Conn) -> AppResult<()> {
    match conn {
        Conn::My(c) => {
            let _ = c.execute(sqlx::raw_sql(&format!("SET SESSION max_execution_time = {}", STATEMENT_TIMEOUT_SECS * 1000))).await;
            let _ = c.execute(sqlx::raw_sql(&format!("SET SESSION max_statement_time = {STATEMENT_TIMEOUT_SECS}"))).await;
            c.execute(sqlx::raw_sql("START TRANSACTION READ ONLY")).await.map_err(friendly)?;
        }
        Conn::Pg(c) => {
            c.execute(sqlx::raw_sql(&format!("SET statement_timeout = '{STATEMENT_TIMEOUT_SECS}s'"))).await.map_err(friendly)?;
            c.execute(sqlx::raw_sql("BEGIN READ ONLY")).await.map_err(friendly)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub server_version: String,
    /// 查询结果的列（SQL 不为空时）
    pub columns: Vec<String>,
}

fn columns_of(cols: &[impl Column]) -> Vec<String> {
    cols.iter().map(|c| c.name().to_string()).collect()
}

/// 测试连接；SQL 不为空时校验语句（只编译不执行）并返回列名
pub async fn test(src: &DbSource, password: &str) -> AppResult<TestResult> {
    if src.engine == Engine::Clickhouse {
        return super::clickhouse::test(src, password).await;
    }
    let mut conn = connect(src, password).await?;
    let version_sql = match src.engine {
        Engine::Postgres => "SHOW server_version",
        _ => "SELECT VERSION()",
    };
    let (server_version, columns) = match &mut conn {
        Conn::My(c) => {
            let v: String = sqlx::query_scalar(version_sql).fetch_one(&mut *c).await.map_err(friendly)?;
            let cols = if src.sql.trim().is_empty() { vec![] } else { columns_of(c.describe(&build_query(src, None, None)?).await.map_err(friendly)?.columns()) };
            (format!("MySQL {v}"), cols)
        }
        Conn::Pg(c) => {
            let v: String = sqlx::query_scalar(version_sql).fetch_one(&mut *c).await.map_err(friendly)?;
            let cols = if src.sql.trim().is_empty() { vec![] } else { columns_of(c.describe(&build_query(src, None, None)?).await.map_err(friendly)?.columns()) };
            (format!("PostgreSQL {v}"), cols)
        }
    };
    if let Some(col) = src.incremental_column.as_deref().filter(|c| !c.trim().is_empty())
        && !columns.is_empty() && !columns.iter().any(|c| c == col.trim()) {
            return Err(AppError::bad(format!("查询结果中没有增量列「{}」（可用的列：{}）", col.trim(), columns.join("、"))));
        }
    Ok(TestResult { server_version, columns })
}

#[derive(Debug)]
pub struct Extracted {
    pub rows: i64,
    /// 本次读到的增量列最大值（无增量列或没有数据时为 None）
    pub watermark: Option<String>,
}

/// 比较增量值：都能按数字解析时按数字比较，否则按文本（日期时间 YYYY-MM-DD HH:MM:SS 按文本比较即可）
pub(super) fn greater(a: &str, b: &str) -> bool {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x > y,
        _ => a > b,
    }
}

/// 取数写入 CSV（第一行为列名）
pub async fn extract(src: &DbSource, password: &str, watermark: Option<&str>, limit: Option<u32>, max_rows: i64, out: &Path) -> AppResult<Extracted> {
    if src.engine == Engine::Clickhouse {
        return super::clickhouse::extract(src, password, watermark, limit, max_rows, out).await;
    }
    let query = build_query(src, watermark, limit)?;
    let mut conn = connect(src, password).await?;
    begin_read_only(&mut conn).await?;
    let file = std::fs::File::create(out)?;
    let mut w = csv::Writer::from_writer(std::io::BufWriter::with_capacity(1 << 20, file));
    let inc = src.incremental_column.as_deref().map(str::trim).filter(|c| !c.is_empty());

    macro_rules! pump {
        ($c:expr) => {{
            // 先取列名（没有数据时也要写表头）
            let names = columns_of($c.describe(&query).await.map_err(friendly)?.columns());
            w.write_record(&names)?;
            let inc_idx = inc.and_then(|n| names.iter().position(|x| x == n));
            if inc.is_some() && inc_idx.is_none() {
                return Err(AppError::bad(format!("查询结果中没有增量列「{}」", inc.unwrap_or_default())));
            }
            let mut rows: i64 = 0;
            let mut max: Option<String> = None;
            let mut stream = sqlx::raw_sql(&query).fetch(&mut *$c);
            let mut record: Vec<String> = Vec::with_capacity(names.len());
            while let Some(row) = stream.try_next().await.map_err(friendly)? {
                record.clear();
                for i in 0..names.len() {
                    let v: Option<String> = row.try_get_unchecked(i).map_err(friendly)?;
                    record.push(v.unwrap_or_default());
                }
                if let Some(i) = inc_idx {
                    let v = &record[i];
                    if !v.is_empty() && max.as_deref().is_none_or(|m| greater(v, m)) {
                        max = Some(v.clone());
                    }
                }
                w.write_record(&record)?;
                rows += 1;
                if rows > max_rows {
                    return Err(AppError::bad(format!("单次同步超过 {max_rows} 行上限，请缩小查询范围或改为增量同步")));
                }
            }
            drop(stream);
            let _ = $c.execute(sqlx::raw_sql("ROLLBACK")).await;
            (rows, max)
        }};
    }
    let (rows, max) = match &mut conn {
        Conn::My(c) => pump!(c),
        Conn::Pg(c) => pump!(c),
    };
    w.flush()?;
    // 没有新数据时保持原来的增量位置
    Ok(Extracted { rows, watermark: max.or_else(|| watermark.map(str::to_string)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(sql: &str, inc: Option<&str>) -> DbSource {
        DbSource {
            engine: Engine::Mysql,
            host: "h".into(),
            port: 3306,
            database: "d".into(),
            user: "u".into(),
            password: None,
            password_enc: None,
            ssl: SslMode::Prefer,
            sql: sql.into(),
            incremental_column: inc.map(Into::into),
        }
    }

    #[test]
    fn sql_checks() {
        assert!(check_sql("select * from t;").is_ok());
        assert!(check_sql("  WITH a AS (select 1) select * from a").is_ok());
        assert!(check_sql("delete from t").is_err());
        assert!(check_sql("select 1; drop table t").is_err());
        assert!(check_sql("update t set a=1").is_err());
    }

    #[test]
    fn incremental_query() {
        let q = build_query(&src("select * from orders", Some("update_time")), Some("2026-10-01 00:00:00' OR '1'='1"), None).unwrap();
        assert_eq!(q, "SELECT * FROM (select * from orders) __src WHERE `update_time` >= '2026-10-01 00:00:00'' OR ''1''=''1' ORDER BY `update_time`");
        let q = build_query(&src("select * from orders", Some("id")), None, Some(200)).unwrap();
        assert_eq!(q, "SELECT * FROM (select * from orders) __src ORDER BY `id` LIMIT 200");
    }

    #[test]
    fn clickhouse_query() {
        let mut s = src("select * from events", Some("ts"));
        s.engine = Engine::Clickhouse;
        let q = build_query(&s, Some("2026-10-01\\' OR 1"), Some(10)).unwrap();
        assert_eq!(q, "SELECT * FROM (select * from events) __src WHERE `ts` >= '2026-10-01\\\\'' OR 1' ORDER BY `ts` LIMIT 10");
    }

    #[test]
    fn watermark_compare() {
        assert!(greater("10", "9"));
        assert!(!greater("9", "10"));
        assert!(greater("2026-10-02 00:00:00", "2026-10-01 23:59:59"));
    }
}
