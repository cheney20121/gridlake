//! ClickHouse 来源：走 ClickHouse 自带的 HTTP 接口（默认端口 8123，HTTPS 8443），不需要额外驱动
//!
//! - 每个请求带 readonly=1（服务端拒绝任何写操作）与 max_execution_time，结果以 CSVWithNames 返回
//! - 结果流式写入临时 CSV，再读一遍统计行数与增量列最大值
//! - SQL 校验、增量条件（`增量列 >= 上次最大值`）与 MySQL / PostgreSQL 一致

use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use super::database::{DbSource, Extracted, SslMode, TestResult, build_query, greater};
use crate::error::{AppError, AppResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STATEMENT_TIMEOUT_SECS: u64 = 600;

fn base_url(src: &DbSource) -> String {
    let scheme = if src.ssl == SslMode::Require { "https" } else { "http" };
    format!("{scheme}://{}:{}/", src.host.trim(), src.port)
}

fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(Duration::from_secs(STATEMENT_TIMEOUT_SECS + 60))
        .build()
        .map_err(|e| AppError::bad(format!("无法创建网络客户端：{e}")))
}

fn friendly(src: &DbSource, e: reqwest::Error) -> AppError {
    let target = format!("{}:{}", src.host, src.port);
    if e.is_timeout() {
        AppError::bad(format!("连接 ClickHouse（{target}）超时，请检查地址、端口与网络"))
    } else if e.is_connect() {
        AppError::bad(format!("无法连接 ClickHouse（{target}）：服务未启动、端口不对（HTTP 接口默认 8123，HTTPS 8443），或防火墙不允许访问"))
    } else {
        AppError::bad(format!("ClickHouse 请求失败（{target}）：{e}"))
    }
}

/// 执行一条查询，返回响应（错误时读出 ClickHouse 的错误信息）
async fn query(src: &DbSource, password: &str, sql: &str) -> AppResult<reqwest::Response> {
    let resp = client()?
        .post(base_url(src))
        .query(&[
            ("database", src.database.trim()),
            ("readonly", "1"),
            ("max_execution_time", &STATEMENT_TIMEOUT_SECS.to_string()),
            ("output_format_csv_crlf_end_of_line", "0"),
        ])
        .header("X-ClickHouse-User", src.user.trim())
        .header("X-ClickHouse-Key", password)
        .body(sql.to_string())
        .send()
        .await
        .map_err(|e| friendly(src, e))?;
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let msg: String = text.lines().next().unwrap_or("").chars().take(300).collect();
    Err(AppError::bad(match status.as_u16() {
        401 | 403 | 516 => format!("ClickHouse 拒绝登录，请检查账号与密码：{msg}"),
        _ if msg.contains("UNKNOWN_DATABASE") => format!("ClickHouse 中没有这个库：{msg}"),
        _ => format!("ClickHouse 返回错误：{msg}"),
    }))
}

/// 测试连接；SQL 不为空时取结果的列名（不取数据）
pub async fn test(src: &DbSource, password: &str) -> AppResult<TestResult> {
    let version = query(src, password, "SELECT version() FORMAT TabSeparated").await?.text().await.map_err(|e| friendly(src, e))?;
    let columns = if src.sql.trim().is_empty() {
        vec![]
    } else {
        let header = query(src, password, &format!("{} LIMIT 0 FORMAT CSVWithNames", build_query(src, None, None)?)).await?.text().await.map_err(|e| friendly(src, e))?;
        let mut r = csv::ReaderBuilder::new().has_headers(true).from_reader(header.as_bytes());
        r.headers().map(|h| h.iter().map(String::from).collect()).unwrap_or_default()
    };
    if let Some(col) = src.incremental_column.as_deref().filter(|c| !c.trim().is_empty())
        && !columns.is_empty()
        && !columns.iter().any(|c| c == col.trim())
    {
        return Err(AppError::bad(format!("查询结果中没有增量列「{}」（可用的列：{}）", col.trim(), columns.join("、"))));
    }
    Ok(TestResult { server_version: format!("ClickHouse {}", version.trim()), columns })
}

/// 取数写入 CSV（第一行为列名）
pub async fn extract(src: &DbSource, password: &str, watermark: Option<&str>, limit: Option<u32>, max_rows: i64, out: &Path) -> AppResult<Extracted> {
    let sql = format!("{} FORMAT CSVWithNames", build_query(src, watermark, limit)?);
    let resp = query(src, password, &sql).await?;
    let mut file = tokio::io::BufWriter::with_capacity(1 << 20, tokio::fs::File::create(out).await?);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        file.write_all(&chunk.map_err(|e| AppError::bad(format!("读取 ClickHouse 结果中断：{e}")))?).await?;
    }
    file.flush().await?;
    drop(file);
    // 统计行数与增量列最大值
    let inc = src.incremental_column.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(String::from);
    let path = out.to_path_buf();
    let (rows, max) = tokio::task::spawn_blocking(move || -> AppResult<(i64, Option<String>)> {
        let mut r = csv::ReaderBuilder::new().has_headers(true).flexible(true).from_path(&path)?;
        let idx = match &inc {
            Some(n) => Some(r.headers()?.iter().position(|h| h == n).ok_or_else(|| AppError::bad(format!("查询结果中没有增量列「{n}」")))?),
            None => None,
        };
        let mut rows = 0i64;
        let mut max: Option<String> = None;
        for rec in r.records() {
            let rec = rec?;
            if let Some(v) = idx.and_then(|i| rec.get(i)).filter(|v| !v.is_empty())
                && max.as_deref().is_none_or(|m| greater(v, m))
            {
                max = Some(v.to_string());
            }
            rows += 1;
            if rows > max_rows {
                return Err(AppError::bad(format!("单次同步超过 {max_rows} 行上限，请缩小查询范围或改为增量同步")));
            }
        }
        Ok((rows, max))
    })
    .await??;
    Ok(Extracted { rows, watermark: max.or_else(|| watermark.map(str::to_string)) })
}
