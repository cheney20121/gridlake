//! 运行指标与慢请求日志
//!
//! - `GET /api/admin/metrics`：Prometheus 文本格式（访问控制同运维接口：`x-admin-token` 或 `Authorization: Bearer <令牌>`，
//!   未配置令牌时只允许本机）。指标：请求数（按状态码类别）、请求耗时直方图、进行中的请求、数据库操作与「重计算」排队、
//!   数据文件大小、备份结果
//! - 慢请求日志：处理时间（到响应头返回为止，流式响应的后续传输不计）超过 `LEDGER_SLOW_REQUEST_MS`（默认 3000，0 关闭）的
//!   请求记一条 warn（方法、路径、状态码、耗时；不含查询参数）
//! - 日志格式：`LEDGER_LOG_FORMAT=json` 输出每行一个 JSON，便于日志系统采集（默认可读文本）

use std::fmt::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// 耗时直方图的上界（毫秒）；再加一个 +Inf
const BUCKETS_MS: [u64; 10] = [5, 10, 25, 50, 100, 250, 500, 1000, 5000, 30000];

pub struct Metrics {
    /// 1xx ~ 5xx 各自的请求数
    status: [AtomicU64; 5],
    /// 每个桶内（非累计）的请求数，最后一个是 +Inf
    buckets: [AtomicU64; BUCKETS_MS.len() + 1],
    sum_ms: AtomicU64,
    slow: AtomicU64,
    in_flight: AtomicI64,
}

pub static METRICS: Metrics = Metrics::new();

impl Metrics {
    const fn new() -> Self {
        Self {
            status: [const { AtomicU64::new(0) }; 5],
            buckets: [const { AtomicU64::new(0) }; BUCKETS_MS.len() + 1],
            sum_ms: AtomicU64::new(0),
            slow: AtomicU64::new(0),
            in_flight: AtomicI64::new(0),
        }
    }

    /// 记录一个处理完的请求
    pub fn record(&self, status: u16, ms: u64, slow: bool) {
        let class = (usize::from(status) / 100).clamp(1, 5) - 1;
        self.status[class].fetch_add(1, Relaxed);
        let bucket = BUCKETS_MS.iter().position(|b| ms <= *b).unwrap_or(BUCKETS_MS.len());
        self.buckets[bucket].fetch_add(1, Relaxed);
        self.sum_ms.fetch_add(ms, Relaxed);
        if slow {
            self.slow.fetch_add(1, Relaxed);
        }
    }
}

/// 慢请求阈值（毫秒）；0 为关闭
fn slow_threshold_ms() -> u64 {
    static T: OnceLock<u64> = OnceLock::new();
    *T.get_or_init(|| std::env::var("LEDGER_SLOW_REQUEST_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(3000))
}

fn is_api(path: &str) -> bool {
    path.starts_with("/api/") || path.starts_with("/ledger-api/api/")
}

/// 中间件：统计 /api 请求并记录慢请求（静态页面不统计）
pub async fn track(req: Request, next: Next) -> Response {
    if !is_api(req.uri().path()) {
        return next.run(req).await;
    }
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let started = Instant::now();
    METRICS.in_flight.fetch_add(1, Relaxed);
    let resp = next.run(req).await;
    METRICS.in_flight.fetch_sub(1, Relaxed);
    let ms = started.elapsed().as_millis() as u64;
    let limit = slow_threshold_ms();
    let slow = limit > 0 && ms >= limit;
    let status = resp.status().as_u16();
    METRICS.record(status, ms, slow);
    if slow {
        tracing::warn!(target: "gridlake::slow", %method, %path, status, ms, "慢请求");
    }
    resp
}

/// 渲染时需要的运行状态（和全局计数分开，便于测试）
pub struct Snapshot {
    pub version: &'static str,
    pub uptime_secs: u64,
    pub active_queries: usize,
    pub heavy_limit: usize,
    pub heavy_running: usize,
    pub heavy_waiting: usize,
    /// 数据文件、WAL 文件大小（字节）
    pub db_bytes: u64,
    pub wal_bytes: u64,
    /// 最近一次备份：是否成功、距今秒数、大小；从未备份过为 None
    pub backup: Option<(bool, i64, u64)>,
}

fn metric(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
}

/// Prometheus 文本格式（0.0.4）
pub fn render(m: &Metrics, s: &Snapshot) -> String {
    let mut o = String::new();
    metric(&mut o, "gridlake_build_info", "gauge", "Build information (value is always 1).");
    let _ = writeln!(o, "gridlake_build_info{{version=\"{}\"}} 1", s.version);
    metric(&mut o, "gridlake_uptime_seconds", "gauge", "Seconds since the server started.");
    let _ = writeln!(o, "gridlake_uptime_seconds {}", s.uptime_secs);

    metric(&mut o, "gridlake_http_requests_total", "counter", "API requests by status class.");
    for (i, c) in m.status.iter().enumerate() {
        let _ = writeln!(o, "gridlake_http_requests_total{{code=\"{}xx\"}} {}", i + 1, c.load(Relaxed));
    }
    metric(&mut o, "gridlake_http_requests_in_flight", "gauge", "API requests currently being handled.");
    let _ = writeln!(o, "gridlake_http_requests_in_flight {}", m.in_flight.load(Relaxed).max(0));
    metric(&mut o, "gridlake_http_slow_requests_total", "counter", "Requests slower than LEDGER_SLOW_REQUEST_MS.");
    let _ = writeln!(o, "gridlake_http_slow_requests_total {}", m.slow.load(Relaxed));

    metric(&mut o, "gridlake_http_request_duration_seconds", "histogram", "API request time until the response headers are sent.");
    let mut cumulative = 0;
    for (i, b) in BUCKETS_MS.iter().enumerate() {
        cumulative += m.buckets[i].load(Relaxed);
        let _ = writeln!(o, "gridlake_http_request_duration_seconds_bucket{{le=\"{}\"}} {cumulative}", *b as f64 / 1000.0);
    }
    cumulative += m.buckets[BUCKETS_MS.len()].load(Relaxed);
    let _ = writeln!(o, "gridlake_http_request_duration_seconds_bucket{{le=\"+Inf\"}} {cumulative}");
    let _ = writeln!(o, "gridlake_http_request_duration_seconds_sum {}", m.sum_ms.load(Relaxed) as f64 / 1000.0);
    let _ = writeln!(o, "gridlake_http_request_duration_seconds_count {cumulative}");

    metric(&mut o, "gridlake_active_queries", "gauge", "Database operations currently running.");
    let _ = writeln!(o, "gridlake_active_queries {}", s.active_queries);
    metric(&mut o, "gridlake_heavy_queries", "gauge", "Heavy computations (pivot, SQL, ...) by state.");
    let _ = writeln!(o, "gridlake_heavy_queries{{state=\"running\"}} {}", s.heavy_running);
    let _ = writeln!(o, "gridlake_heavy_queries{{state=\"waiting\"}} {}", s.heavy_waiting);
    metric(&mut o, "gridlake_heavy_queries_limit", "gauge", "Maximum concurrent heavy computations.");
    let _ = writeln!(o, "gridlake_heavy_queries_limit {}", s.heavy_limit);

    metric(&mut o, "gridlake_db_size_bytes", "gauge", "Size of the DuckDB data file and its WAL.");
    let _ = writeln!(o, "gridlake_db_size_bytes{{file=\"main\"}} {}", s.db_bytes);
    let _ = writeln!(o, "gridlake_db_size_bytes{{file=\"wal\"}} {}", s.wal_bytes);

    if let Some((ok, age, size)) = s.backup {
        metric(&mut o, "gridlake_last_backup_success", "gauge", "1 if the most recent backup succeeded.");
        let _ = writeln!(o, "gridlake_last_backup_success {}", u8::from(ok));
        metric(&mut o, "gridlake_last_backup_age_seconds", "gauge", "Seconds since the most recent backup attempt.");
        let _ = writeln!(o, "gridlake_last_backup_age_seconds {}", age.max(0));
        metric(&mut o, "gridlake_last_backup_size_bytes", "gauge", "Size of the most recent backup file.");
        let _ = writeln!(o, "gridlake_last_backup_size_bytes {size}");
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        Snapshot { version: "9.9.9", uptime_secs: 12, active_queries: 1, heavy_limit: 2, heavy_running: 1, heavy_waiting: 3, db_bytes: 4096, wal_bytes: 0, backup: Some((true, 60, 2048)) }
    }

    #[test]
    fn counts_by_status_class_and_cumulative_buckets() {
        let m = Metrics::new();
        m.record(200, 3, false); // ≤5ms
        m.record(204, 80, false); // ≤100ms
        m.record(404, 80, false);
        m.record(500, 6000, true); // ≤30000ms
        m.record(200, 90_000, true); // +Inf
        let text = render(&m, &snap());
        assert!(text.contains("gridlake_http_requests_total{code=\"2xx\"} 3"), "{text}");
        assert!(text.contains("gridlake_http_requests_total{code=\"4xx\"} 1"));
        assert!(text.contains("gridlake_http_requests_total{code=\"5xx\"} 1"));
        // 直方图是累计的：le 越大数字越大，+Inf 等于总数
        assert!(text.contains("duration_seconds_bucket{le=\"0.005\"} 1"));
        assert!(text.contains("duration_seconds_bucket{le=\"0.1\"} 3"));
        assert!(text.contains("duration_seconds_bucket{le=\"5\"} 3"));
        assert!(text.contains("duration_seconds_bucket{le=\"30\"} 4"));
        assert!(text.contains("duration_seconds_bucket{le=\"+Inf\"} 5"));
        assert!(text.contains("duration_seconds_count 5"));
        assert!(text.contains("duration_seconds_sum 96.163"), "{text}");
        assert!(text.contains("gridlake_http_slow_requests_total 2"));
    }

    #[test]
    fn renders_runtime_state() {
        let text = render(&Metrics::new(), &snap());
        for line in [
            "gridlake_build_info{version=\"9.9.9\"} 1",
            "gridlake_uptime_seconds 12",
            "gridlake_active_queries 1",
            "gridlake_heavy_queries{state=\"waiting\"} 3",
            "gridlake_heavy_queries_limit 2",
            "gridlake_db_size_bytes{file=\"main\"} 4096",
            "gridlake_last_backup_success 1",
            "gridlake_last_backup_age_seconds 60",
        ] {
            assert!(text.lines().any(|l| l == line), "缺少 {line}\n{text}");
        }
        // 每个指标都有 HELP / TYPE，且没有备份记录时不输出备份指标
        assert!(text.contains("# TYPE gridlake_http_request_duration_seconds histogram"));
        let mut s = snap();
        s.backup = None;
        assert!(!render(&Metrics::new(), &s).contains("last_backup"));
    }

    #[test]
    fn only_api_paths_are_tracked() {
        assert!(is_api("/api/ledger/me") && is_api("/ledger-api/api/health"));
        assert!(!is_api("/excel/home") && !is_api("/assets/index.js") && !is_api("/api"));
    }
}
