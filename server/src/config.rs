//! 运行配置：全部来自环境变量（私有化部署时写在 ledger.env / docker-compose 的 environment 里）

use std::path::PathBuf;

use anyhow::{Context, bail};
use chrono::NaiveTime;

#[derive(Debug, Clone)]
pub struct Config {
    /// 监听地址，默认 127.0.0.1:8787；对外提供服务时设为 0.0.0.0:8787
    pub addr: String,
    /// DuckDB 数据文件
    pub db_path: PathBuf,
    /// 单个上传文件上限（MB）
    pub max_upload_mb: usize,
    /// 前端构建产物目录（index.html 所在目录）；设置后由本服务直接提供页面，无需另配 nginx
    pub web_dir: Option<PathBuf>,
    /// DuckDB 内存上限，如 8GB；超出部分落盘到 数据目录/tmp/spill。
    /// 没配置时按机器（或容器）内存的 30% 自动设置：DuckDB 默认的 80% 在排序 / 聚合时会被中间结果冲破，亿级数据上实测峰值可达上限的 2~4 倍
    pub memory_limit: Option<String>,
    /// 内存上限是自动算出的（启动日志里注明）
    pub memory_limit_auto: bool,
    /// 同时进行的「重计算」（透视、SQL、异常发现、带筛选 / 排序的游标）个数，超出的排队
    pub heavy_queries: usize,
    /// 重计算排队的最长等待秒数，超过返回「服务繁忙」
    pub heavy_wait_secs: u64,
    /// DuckDB 计算线程数，默认等于 CPU 核数
    pub threads: Option<usize>,
    /// 备份目录，默认 数据目录/backups
    pub backup_dir: PathBuf,
    /// 每天自动备份的时间（本地时间），None 为关闭
    pub backup_at: Option<NaiveTime>,
    /// 保留最近几份备份
    pub backup_keep: usize,
    /// 允许跨域访问的来源：None 不允许（前端与本服务同源部署时不需要）；Some(["*"]) 允许任意来源
    pub cors: Option<Vec<String>>,
    /// 运维接口（/api/admin/*）的令牌；未设置时只允许本机直接访问
    pub admin_token: Option<String>,
    /// 文件夹连接器可以使用的目录（及其子目录），默认 数据目录/inbox
    pub inbox_roots: Vec<PathBuf>,
    /// 连接器单次同步的行数上限
    pub connector_max_rows: i64,
    /// 登录认证：off（默认，不校验，行为与旧版一致）/ gateway（用 x-user-token 到网关校验）/ proxy（信任反向代理注入的身份头）
    pub auth_mode: AuthMode,
    /// gateway 模式：校验令牌的网关接口（如 https://网关/api/current_user），带请求头 x-user-token 调用
    pub auth_url: Option<String>,
    /// 校验结果缓存秒数（令牌被网关吊销后最多这么久仍有效）
    pub auth_cache_secs: u64,
    /// gateway 模式：网关用内部 CA 签发 HTTPS 证书时，这个 PEM 文件里的根证书（可含多个）会被信任；
    /// 程序只内置公共根证书，不配置的话企业内网网关的证书校验会失败，所有人登录不了
    pub auth_ca_file: Option<PathBuf>,
    /// 管理员：用户 id 或登录名，逗号分隔；网关标记为超级管理员 / 租户管理员的用户同样是管理员
    pub admins: Vec<String>,
    /// proxy 模式：可信的反向代理地址（只有这些来源发来的身份头才被采信），默认只有本机
    pub trusted_proxies: Vec<std::net::IpAddr>,
    /// local 模式：首次启动（还没有任何账号）时创建的管理员用户名与密码；密码不设置时随机生成并打印在日志里
    pub admin_user: String,
    pub admin_password: Option<String>,
    /// local 模式：登录有效天数
    pub session_days: i64,
    /// 操作审计保留天数
    pub audit_days: i64,
    /// 明确允许「不登录 + 对外暴露」（监听非本机地址或开放跨域）。默认不允许：见 `Config::exposure_check`
    pub allow_no_auth: bool,
    /// 允许自己审批自己提交的单据（小团队一个人兼任提交与审批时使用）；默认不允许
    pub allow_self_approve: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Off,
    /// 内置账号：用户名 + 密码登录，账号由管理员在界面里管理（见 account.rs）
    Local,
    Gateway,
    Proxy,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// 校验内存上限的写法（会拼进 SET 语句，只允许 数字 + 单位）
fn parse_memory(v: &str) -> anyhow::Result<String> {
    let v = v.replace(' ', "").to_uppercase();
    let unit_at = v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len());
    let (num, unit) = v.split_at(unit_at);
    let ok_num = !num.is_empty() && num.parse::<f64>().is_ok_and(|n| n > 0.0);
    let ok_unit = matches!(unit, "KB" | "MB" | "GB" | "TB" | "KIB" | "MIB" | "GIB" | "TIB");
    if !ok_num || !ok_unit {
        bail!("LEDGER_MEMORY_LIMIT 格式应为 数字+单位，如 8GB、512MB（当前：{v}）");
    }
    Ok(v)
}

/// 机器 / 容器可用的内存（字节）：取物理内存与 cgroup 限制中较小的；探测不到返回 None
pub fn detect_total_memory() -> Option<u64> {
    let mut found: Vec<u64> = Vec::new();
    // 容器：cgroup v2 / v1 的内存上限（没限制时是 "max" 或一个极大的数）
    for f in ["/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory/memory.limit_in_bytes"] {
        found.extend(std::fs::read_to_string(f).ok().and_then(|t| t.trim().parse::<u64>().ok()).filter(|v| *v < (1 << 50)));
    }
    found.extend(parse_meminfo(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default()));
    if found.is_empty() {
        // macOS
        found.extend(std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|t| t.trim().parse::<u64>().ok()));
    }
    found.into_iter().filter(|v| *v >= 256 * 1024 * 1024).min()
}

/// /proc/meminfo 的 MemTotal（kB）→ 字节
fn parse_meminfo(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
}

/// 自动内存上限：总内存的 30%，不少于 1GB
fn auto_memory_limit(total: u64) -> String {
    let mb = (total as f64 * 0.30 / 1048576.0).floor().max(1024.0) as u64;
    format!("{mb}MB")
}

/// 「8GB」「512MB」→ 字节
fn memory_bytes(v: &str) -> Option<u64> {
    let unit_at = v.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = v.split_at(unit_at);
    let n: f64 = num.parse().ok()?;
    let mult: f64 = match unit {
        "KB" | "KIB" => 1024.0,
        "MB" | "MIB" => 1048576.0,
        "GB" | "GIB" => 1073741824.0,
        "TB" | "TIB" => 1099511627776.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// 默认的重计算并发数：每 2GB 内存上限一个，1 ~ 4 个；不知道上限时 2 个
fn default_heavy(limit_bytes: Option<u64>) -> usize {
    limit_bytes.map_or(2, |b| ((b / (2 << 30)) as usize).clamp(1, 4))
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let db_path = PathBuf::from(env("LEDGER_DB").unwrap_or_else(|| "data/ledger.duckdb".into()));
        let data_dir = db_path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let web_dir = match env("LEDGER_WEB_DIR") {
            Some(d) => {
                let d = PathBuf::from(d);
                if !d.join("index.html").is_file() {
                    bail!("LEDGER_WEB_DIR 下没有 index.html：{}", d.display());
                }
                Some(d)
            }
            None => None,
        };
        let backup_at = match env("LEDGER_BACKUP_AT").as_deref() {
            None => Some(NaiveTime::from_hms_opt(2, 30, 0).expect("valid time")),
            Some("off" | "OFF" | "none" | "0") => None,
            Some(t) => Some(NaiveTime::parse_from_str(t, "%H:%M").with_context(|| format!("LEDGER_BACKUP_AT 格式应为 HH:MM 或 off（当前：{t}）"))?),
        };
        let (memory_limit, memory_limit_auto) = match env("LEDGER_MEMORY_LIMIT").as_deref() {
            // off / default：不设置，用 DuckDB 自己的默认（内存的 80%）
            Some(v) if matches!(v.to_lowercase().as_str(), "off" | "default" | "none") => (None, false),
            Some(v) => (Some(parse_memory(v)?), false),
            None => (detect_total_memory().map(auto_memory_limit), true),
        };
        let limit_bytes = memory_limit.as_deref().and_then(memory_bytes);
        let heavy_queries = env("LEDGER_HEAVY_QUERIES").map(|v| v.parse().context("LEDGER_HEAVY_QUERIES 应为正整数")).transpose()?.filter(|n: &usize| *n > 0).unwrap_or_else(|| default_heavy(limit_bytes));
        Ok(Self {
            addr: env("LEDGER_ADDR").unwrap_or_else(|| "127.0.0.1:8787".into()),
            max_upload_mb: env("LEDGER_MAX_UPLOAD_MB").map(|v| v.parse().context("LEDGER_MAX_UPLOAD_MB 应为整数")).transpose()?.unwrap_or(1024),
            web_dir,
            memory_limit,
            memory_limit_auto,
            heavy_queries,
            heavy_wait_secs: env("LEDGER_HEAVY_WAIT_SECS").map(|v| v.parse().context("LEDGER_HEAVY_WAIT_SECS 应为整数")).transpose()?.filter(|n| *n >= 1).unwrap_or(120),
            threads: env("LEDGER_THREADS").map(|v| v.parse().context("LEDGER_THREADS 应为正整数")).transpose()?.filter(|n| *n > 0),
            backup_dir: env("LEDGER_BACKUP_DIR").map(PathBuf::from).unwrap_or_else(|| data_dir.join("backups")),
            backup_at,
            backup_keep: env("LEDGER_BACKUP_KEEP").map(|v| v.parse().context("LEDGER_BACKUP_KEEP 应为整数")).transpose()?.unwrap_or(7).max(1),
            cors: env("LEDGER_CORS").map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
            admin_token: env("LEDGER_ADMIN_TOKEN"),
            inbox_roots: match env("LEDGER_INBOX_ROOTS") {
                Some(v) => v.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).collect(),
                None => vec![data_dir.join("inbox")],
            },
            auth_mode: match env("LEDGER_AUTH_MODE").as_deref().map(str::to_lowercase).as_deref() {
                None | Some("off" | "none") => AuthMode::Off,
                Some("local") => AuthMode::Local,
                Some("gateway") => AuthMode::Gateway,
                Some("proxy") => AuthMode::Proxy,
                Some(other) => bail!("LEDGER_AUTH_MODE 应为 off / local / gateway / proxy（当前：{other}）"),
            },
            auth_url: env("LEDGER_AUTH_URL"),
            auth_ca_file: env("LEDGER_AUTH_CA_FILE").map(PathBuf::from),
            auth_cache_secs: env("LEDGER_AUTH_CACHE_SECS").map(|v| v.parse().context("LEDGER_AUTH_CACHE_SECS 应为整数")).transpose()?.unwrap_or(60),
            admins: env("LEDGER_ADMINS").map(|v| v.split([',', ';']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default(),
            trusted_proxies: match env("LEDGER_TRUSTED_PROXIES") {
                Some(v) => v.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.parse().with_context(|| format!("LEDGER_TRUSTED_PROXIES 含无效地址：{s}"))).collect::<anyhow::Result<_>>()?,
                None => vec!["127.0.0.1".parse()?, "::1".parse()?],
            },
            admin_user: env("LEDGER_ADMIN_USER").unwrap_or_else(|| "admin".into()),
            admin_password: env("LEDGER_ADMIN_PASSWORD"),
            session_days: env("LEDGER_SESSION_DAYS").map(|v| v.parse().context("LEDGER_SESSION_DAYS 应为整数")).transpose()?.unwrap_or(7).clamp(1, 365),
            allow_no_auth: matches!(env("LEDGER_ALLOW_NO_AUTH").as_deref().map(str::to_lowercase).as_deref(), Some("1" | "true" | "yes")),
            allow_self_approve: matches!(env("LEDGER_ALLOW_SELF_APPROVE").as_deref().map(str::to_lowercase).as_deref(), Some("1" | "true" | "yes")),
            audit_days: env("LEDGER_AUDIT_DAYS").map(|v| v.parse().context("LEDGER_AUDIT_DAYS 应为整数")).transpose()?.unwrap_or(365).max(1),
            connector_max_rows: env("LEDGER_CONNECTOR_MAX_ROWS").map(|v| v.parse().context("LEDGER_CONNECTOR_MAX_ROWS 应为整数")).transpose()?.unwrap_or(20_000_000),
            db_path,
        })
        .and_then(|c| {
            if c.auth_mode == AuthMode::Gateway && !c.auth_url.as_deref().is_some_and(|u| u.starts_with("http://") || u.starts_with("https://")) {
                bail!("LEDGER_AUTH_MODE=gateway 需要同时设置 LEDGER_AUTH_URL（以 http:// 或 https:// 开头的网关用户信息接口）");
            }
            Ok(c)
        })
    }

    /// 启动前的安全检查：返回要提醒的警告；配置本身不安全时返回错误，拒绝启动。
    ///
    /// - 不登录（`off`）却监听非本机地址：网络上任何能访问到的人都能读写全部数据 → 拒绝（`LEDGER_ALLOW_NO_AUTH=1` 可明确放行）
    /// - 不登录又开放任意来源跨域（`LEDGER_CORS=*`）：本机上打开的任何网页都能调用接口 → 同样拒绝
    /// - 登录开启但跨域为 `*`：不会泄露（令牌不在 cookie 里自动携带），但没有必要，给出警告
    pub fn exposure_check(&self) -> Result<Vec<String>, String> {
        use std::net::ToSocketAddrs;
        let mut warnings = Vec::new();
        let cors_any = self.cors.as_ref().is_some_and(|c| c.iter().any(|o| o == "*"));
        let exposed = match self.addr.to_socket_addrs() {
            Ok(mut it) => it.any(|a| !a.ip().is_loopback()),
            // 解析不了的地址让后面的 bind 去报错
            Err(_) => false,
        };
        if self.auth_mode == AuthMode::Off && (exposed || cors_any) {
            let why = if exposed { format!("监听地址 {} 不是本机地址", self.addr) } else { "LEDGER_CORS=* 允许任意网站调用接口".to_string() };
            if !self.allow_no_auth {
                return Err(format!(
                    "拒绝启动：没有开启登录（LEDGER_AUTH_MODE 未设置或为 off），但{why}，网络上任何能访问到的人都可以读取、修改、删除全部数据。\n\
                     请选一种处理方式：\n\
                     \u{20}\u{20}1. 开启登录：设置 LEDGER_AUTH_MODE=local（内置账号，推荐）、gateway 或 proxy；\n\
                     \u{20}\u{20}2. 只在本机使用：把 LEDGER_ADDR 设为 127.0.0.1:8787，并去掉 LEDGER_CORS=*；\n\
                     \u{20}\u{20}3. 确认已有其他手段保护（例如内网隔离 + 反向代理鉴权）：设置 LEDGER_ALLOW_NO_AUTH=1 明确放行。"
                ));
            }
            warnings.push(format!("没有开启登录，且{why}：任何能访问到的人都能读写全部数据（已通过 LEDGER_ALLOW_NO_AUTH 放行）"));
        }
        if cors_any && self.auth_mode != AuthMode::Off {
            warnings.push("LEDGER_CORS=* 允许任意网站调用接口；前后端分开部署时请改成前端的具体地址".to_string());
        }
        if self.admin_token.is_none() && exposed {
            warnings.push("未设置 LEDGER_ADMIN_TOKEN：运维接口 /api/admin/* 只允许服务器本机访问；需要远程调用备份等接口时再设置".to_string());
        }
        Ok(warnings)
    }

    /// 启动时打印的生效配置（不含令牌）
    pub fn summary(&self) -> String {
        format!(
            "db={} web={} memory_limit={} heavy={}（排队最长 {}s） threads={} backup={} keep={} dir={} cors={} admin={} inbox={} auth={}",
            self.db_path.display(),
            self.web_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "未启用".into()),
            match (&self.memory_limit, self.memory_limit_auto) {
                (Some(m), true) => format!("{m}（按机器内存 30% 自动设置）"),
                (Some(m), false) => m.clone(),
                (None, _) => "DuckDB 默认(80%内存)".into(),
            },
            self.heavy_queries,
            self.heavy_wait_secs,
            self.threads.map(|n| n.to_string()).unwrap_or_else(|| "默认(CPU核数)".into()),
            self.backup_at.map(|t| format!("每天 {}", t.format("%H:%M"))).unwrap_or_else(|| "关闭".into()),
            self.backup_keep,
            self.backup_dir.display(),
            self.cors.as_ref().map(|c| c.join(",")).unwrap_or_else(|| "关闭".into()),
            if self.admin_token.is_some() { "令牌" } else { "仅本机" },
            self.inbox_roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(","),
            match self.auth_mode {
                AuthMode::Off => "关闭".to_string(),
                AuthMode::Local => format!("内置账号（登录有效 {} 天）", self.session_days),
                AuthMode::Gateway => format!("网关校验（管理员 {} 人）", self.admins.len()),
                AuthMode::Proxy => format!("代理注入（管理员 {} 人）", self.admins.len()),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthMode, Config, auto_memory_limit, default_heavy, memory_bytes, parse_meminfo, parse_memory};

    fn cfg(addr: &str, auth: AuthMode, cors: Option<&str>, allow: bool) -> Config {
        let mut c = Config::from_env().unwrap();
        c.addr = addr.into();
        c.auth_mode = auth;
        c.cors = cors.map(|v| vec![v.to_string()]);
        c.allow_no_auth = allow;
        c.admin_token = Some("t".into());
        c
    }

    #[test]
    fn refuses_to_expose_without_login() {
        // 本机 + 不登录：开发 / 试用的默认方式，放行
        assert!(cfg("127.0.0.1:8787", AuthMode::Off, None, false).exposure_check().unwrap().is_empty());
        assert!(cfg("[::1]:8787", AuthMode::Off, None, false).exposure_check().unwrap().is_empty());
        assert!(cfg("localhost:8787", AuthMode::Off, None, false).exposure_check().unwrap().is_empty());
        // 对外监听 + 不登录：拒绝，提示里写明怎么处理
        let err = cfg("0.0.0.0:8787", AuthMode::Off, None, false).exposure_check().unwrap_err();
        assert!(err.contains("LEDGER_AUTH_MODE=local") && err.contains("LEDGER_ALLOW_NO_AUTH"), "{err}");
        assert!(cfg("192.168.1.5:8787", AuthMode::Off, None, false).exposure_check().is_err());
        assert!(cfg("[::]:8787", AuthMode::Off, None, false).exposure_check().is_err());
        // 不登录 + 任意来源跨域：本机也不行
        assert!(cfg("127.0.0.1:8787", AuthMode::Off, Some("*"), false).exposure_check().is_err());
        // 明确放行：可以启动，但有警告
        let w = cfg("0.0.0.0:8787", AuthMode::Off, None, true).exposure_check().unwrap();
        assert!(w.iter().any(|m| m.contains("没有开启登录")), "{w:?}");
        // 开了登录：对外监听没问题；跨域 * 只警告
        assert!(cfg("0.0.0.0:8787", AuthMode::Local, None, false).exposure_check().unwrap().is_empty());
        let w = cfg("0.0.0.0:8787", AuthMode::Local, Some("*"), false).exposure_check().unwrap();
        assert!(w.iter().any(|m| m.contains("LEDGER_CORS=*")), "{w:?}");
        // 对外监听却没设运维令牌：提醒
        let mut c = cfg("0.0.0.0:8787", AuthMode::Local, None, false);
        c.admin_token = None;
        assert!(c.exposure_check().unwrap().iter().any(|m| m.contains("LEDGER_ADMIN_TOKEN")));
    }

    #[test]
    fn memory_limit_format() {
        assert_eq!(parse_memory("8GB").unwrap(), "8GB");
        assert_eq!(parse_memory(" 512 mb ").unwrap(), "512MB");
        assert!(parse_memory("8").is_err());
        assert!(parse_memory("8GB'; DROP TABLE x; --").is_err());
        assert!(parse_memory("-1GB").is_err());
    }

    #[test]
    fn auto_memory_and_concurrency() {
        // 16 GB 机器：上限约 4.8 GB（30%），重计算并发 2
        let total = 16u64 << 30;
        let limit = auto_memory_limit(total);
        assert_eq!(limit, "4915MB");
        assert_eq!(default_heavy(memory_bytes(&limit)), 2);
        // 很小的机器不会低于 1 GB；并发至少 1
        assert_eq!(auto_memory_limit(2 << 30), "1024MB");
        assert_eq!(default_heavy(memory_bytes("1024MB")), 1);
        // 大机器最多 4 个
        assert_eq!(default_heavy(memory_bytes("64GB")), 4);
        assert_eq!(default_heavy(None), 2);
        assert_eq!(memory_bytes("2GB"), Some(2 << 30));
        assert_eq!(parse_meminfo("MemTotal:       16384000 kB\nMemFree: 1 kB\n"), Some(16384000 * 1024));
    }
}
