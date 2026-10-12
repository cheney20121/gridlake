//! 服务端主动发起的 HTTP 请求（网址导入、定时推送的 Webhook）的目的地址检查，防 SSRF
//!
//! 威胁：普通用户能填一个网址让服务器去访问。如果不加限制，可以让服务器去读本机或内网里不对外的服务
//! （云厂商的元数据接口 169.254.169.254 / 100.100.100.200 能拿到实例凭据），再通过错误提示或导入的内容把响应带出来。
//!
//! 规则：
//! - **一律拒绝**：回环（127.0.0.0/8、::1）、未指定（0.0.0.0）、链路本地（169.254.0.0/16、fe80::/10）、广播、组播、
//!   运营商级 NAT（100.64.0.0/10，阿里云元数据地址 100.100.100.200 在其中）；IPv4 映射的 IPv6（::ffff:127.0.0.1）先还原成 IPv4 再判断；
//! - **默认放行、可选拒绝**：私有地址（10/8、172.16/12、192.168/16、fc00::/7）。私有部署里的内网文件服务、企业微信/钉钉的内网网关
//!   都是正常用途，所以默认允许；对外网开放的部署设 `LEDGER_URL_BLOCK_PRIVATE=1` 一并拒绝；
//! - `LEDGER_URL_IMPORT_ALLOW_LOCAL=1`：全部放行（仅用于本机开发和测试）。
//!
//! 检查在三处生效：保存 / 请求前对网址解析一次（给用户明确的提示）；**连接时**由 [`Guard`] 解析器再次过滤（防 DNS 重绑定：
//! 两次解析之间域名改指向本机）；每一跳重定向重新检查。不查地址就直接连接的写法（IP 字面量）由第一处覆盖。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// 全部放行（开发 / 测试）
    pub allow_all: bool,
    /// 连私有地址也拒绝
    pub block_private: bool,
}

impl Policy {
    pub fn from_env() -> Self {
        let on = |k: &str| std::env::var(k).is_ok_and(|v| v == "1");
        Self { allow_all: on("LEDGER_URL_IMPORT_ALLOW_LOCAL"), block_private: on("LEDGER_URL_BLOCK_PRIVATE") }
    }
}

fn v4_forbidden(v: Ipv4Addr, p: Policy) -> Option<&'static str> {
    let o = v.octets();
    if v.is_loopback() || v.is_unspecified() {
        Some("本机地址")
    } else if v.is_link_local() {
        Some("链路本地地址（含云厂商元数据接口）")
    } else if v.is_broadcast() || v.is_multicast() {
        Some("广播 / 组播地址")
    } else if o[0] == 100 && (o[1] & 0xc0) == 64 {
        Some("运营商级 NAT 地址（含阿里云元数据接口）")
    } else if p.block_private && v.is_private() {
        Some("内网地址")
    } else {
        None
    }
}

fn v6_forbidden(v: Ipv6Addr, p: Policy) -> Option<&'static str> {
    if let Some(m) = v.to_ipv4_mapped() {
        return v4_forbidden(m, p);
    }
    let first = v.segments()[0];
    if v.is_loopback() || v.is_unspecified() {
        Some("本机地址")
    } else if (first & 0xffc0) == 0xfe80 {
        Some("链路本地地址")
    } else if v.is_multicast() {
        Some("组播地址")
    } else if (first & 0xfe00) == 0xfc00 && p.block_private {
        Some("内网地址")
    } else if v.segments()[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        // NAT64：后 32 位是 IPv4 地址
        let s = v.segments();
        let inner = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        v4_forbidden(inner, p)
    } else {
        None
    }
}

/// 这个地址是否不允许访问；返回拒绝原因
pub fn forbidden_with(ip: IpAddr, p: Policy) -> Option<&'static str> {
    if p.allow_all {
        return None;
    }
    match ip {
        IpAddr::V4(v) => v4_forbidden(v, p),
        IpAddr::V6(v) => v6_forbidden(v, p),
    }
}

/// 校验网址：http / https，且解析出的地址没有一个落在被禁止的范围里。会做一次（阻塞的）域名解析
pub fn check_url(url: &reqwest::Url) -> AppResult<()> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::bad("只支持 http / https 网址"));
    }
    let policy = Policy::from_env();
    if policy.allow_all {
        return Ok(());
    }
    let host = url.host_str().ok_or_else(|| AppError::bad("网址缺少主机名"))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<SocketAddr> = std::net::ToSocketAddrs::to_socket_addrs(&(host.trim_matches(['[', ']']), port))
        .map_err(|e| AppError::bad(format!("无法解析主机「{host}」：{e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(AppError::bad(format!("无法解析主机「{host}」")));
    }
    if let Some(why) = addrs.iter().find_map(|a| forbidden_with(a.ip(), policy)) {
        return Err(AppError::bad(format!("不能访问{why}（{host}）")));
    }
    Ok(())
}

/// 连接时的域名解析：丢掉被禁止的地址，剩下的才会被连接；一个都不剩就失败。
/// 检查和连接用的是同一次解析结果，所以域名在两次查询之间改指向（DNS 重绑定）也绕不过去
pub struct Guard;

impl Resolve for Guard {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let policy = Policy::from_env();
            let host = name.as_str().to_string();
            let all: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let allowed: Vec<SocketAddr> = all.iter().copied().filter(|a| forbidden_with(a.ip(), policy).is_none()).collect();
            if allowed.is_empty() {
                let why = all.iter().find_map(|a| forbidden_with(a.ip(), policy)).unwrap_or("无法解析");
                let e: Box<dyn std::error::Error + Send + Sync> = format!("不能访问{why}（{host}）").into();
                return Err(e);
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

/// 给用户输入的网址发请求用的客户端：连接时过滤地址，每一跳重定向重新检查，最多 5 跳
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().dns_resolver(Arc::new(Guard)).redirect(reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            attempt.error("重定向次数过多")
        } else if check_url(attempt.url()).is_err() {
            attempt.stop()
        } else {
            attempt.follow()
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: Policy = Policy { allow_all: false, block_private: false };
    const STRICT: Policy = Policy { allow_all: false, block_private: true };

    fn why(ip: &str, p: Policy) -> Option<&'static str> {
        forbidden_with(ip.parse().unwrap(), p)
    }

    #[test]
    fn always_forbidden() {
        for ip in [
            "127.0.0.1", "127.8.9.10", "0.0.0.0", "169.254.169.254", "100.100.100.200", "100.64.0.1", "100.127.255.254", "224.0.0.1", "255.255.255.255",
            "::1", "::", "fe80::1", "ff02::1",
        ] {
            assert!(why(ip, DEFAULT).is_some(), "{ip} 应被拒绝");
        }
    }

    #[test]
    fn ipv4_mapped_and_nat64_are_unwrapped() {
        // 用 IPv6 写法绕过 IPv4 的检查
        for ip in ["::ffff:127.0.0.1", "::ffff:169.254.169.254", "::ffff:100.100.100.200", "64:ff9b::7f00:1", "64:ff9b::a9fe:a9fe"] {
            assert!(why(ip, DEFAULT).is_some(), "{ip} 应被拒绝");
        }
        assert!(why("::ffff:8.8.8.8", DEFAULT).is_none());
        assert!(why("64:ff9b::808:808", DEFAULT).is_none());
    }

    #[test]
    fn public_addresses_pass() {
        for ip in ["8.8.8.8", "1.1.1.1", "203.0.113.5", "100.63.255.255", "100.128.0.1", "2606:4700:4700::1111"] {
            assert!(why(ip, DEFAULT).is_none(), "{ip} 应放行");
        }
    }

    #[test]
    fn private_is_allowed_by_default_and_blocked_when_strict() {
        for ip in ["10.1.2.3", "172.16.0.9", "172.31.255.255", "192.168.1.10", "fd00::1", "fc00::5"] {
            assert!(why(ip, DEFAULT).is_none(), "{ip} 默认应放行");
            assert!(why(ip, STRICT).is_some(), "{ip} 严格模式应拒绝");
        }
        // 边界：172.15 / 172.32 不是私有地址
        assert!(why("172.15.0.1", STRICT).is_none());
        assert!(why("172.32.0.1", STRICT).is_none());
        assert!(why("::ffff:10.0.0.1", STRICT).is_some());
    }

    #[test]
    fn allow_all_overrides_everything() {
        let p = Policy { allow_all: true, block_private: true };
        assert!(why("127.0.0.1", p).is_none());
        assert!(why("169.254.169.254", p).is_none());
    }

    #[test]
    fn url_scheme_and_literal_hosts() {
        let u = |s: &str| reqwest::Url::parse(s).unwrap();
        assert!(check_url(&u("ftp://example.com/a")).is_err());
        assert!(check_url(&u("file:///etc/passwd")).is_err());
        assert!(check_url(&u("http://127.0.0.1:8080/a")).is_err());
        assert!(check_url(&u("http://[::1]/a")).is_err());
        assert!(check_url(&u("http://[::ffff:127.0.0.1]/a")).is_err());
        assert!(check_url(&u("http://169.254.169.254/latest/meta-data/")).is_err());
        assert!(check_url(&u("http://100.100.100.200/latest/meta-data/")).is_err());
        // 十进制 / 十六进制 / 八进制写法：url 库会规范成点分十进制再判断
        assert!(check_url(&u("http://2130706433/")).is_err());
        assert!(check_url(&u("http://0x7f.0.0.1/")).is_err());
        assert!(check_url(&u("http://0177.0.0.1/")).is_err());
    }
}
