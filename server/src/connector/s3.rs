//! 对象存储来源：S3 以及兼容 S3 协议的存储（MinIO、阿里云 OSS、腾讯云 COS、华为云 OBS、Cloudflare R2……）
//!
//! 和文件夹来源一样：按文件名通配符收取前缀下的 CSV / XLSX，每个对象只导入一次（对象键 + 大小 + 修改时间），
//! 按修改时间从旧到新。请求用 AWS Signature V4 签名（自己实现，不引入 AWS SDK）；列对象用 ListObjectsV2（自动翻页），
//! 读对象用 GetObject（边下载边写临时文件，超过上传上限就中止）。
//!
//! 地址风格：不填 endpoint 是 AWS S3（虚拟主机风格 `bucket.s3.区域.amazonaws.com`）；填了 endpoint 默认用路径风格
//! （`endpoint/bucket/key`，MinIO 等自建存储用这个），阿里云 OSS 等只支持虚拟主机风格的把「路径风格」关掉。
//! 访问密钥只需要读权限（s3:ListBucket + s3:GetObject）。

use std::path::Path;
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::folder::{FileEntry, wildcard};
use crate::error::{AppError, AppResult};

/// 修改时间在这之内的对象视为可能仍在上传
const SETTLE: Duration = Duration::from_secs(60);
/// 一次最多扫描的页数（每页 1000 个对象）
const MAX_PAGES: usize = 20;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct S3Source {
    /// 自建 / 其他云的服务地址，如 https://minio.corp:9000；不填是 AWS S3
    #[serde(default)]
    pub endpoint: Option<String>,
    /// 区域，默认 us-east-1（MinIO 等通常不关心）
    #[serde(default)]
    pub region: String,
    pub bucket: String,
    /// 对象键前缀，如 reports/2026/；可以为空
    #[serde(default)]
    pub prefix: String,
    /// 文件名通配符，* 任意字符、? 单个字符；多个用 ; 分隔
    pub pattern: String,
    /// 是否包含前缀下的子目录（默认只看前缀下一层）
    #[serde(default)]
    pub recursive: bool,
    /// 路径风格地址；不填时：填了 endpoint 用路径风格，AWS 用虚拟主机风格
    #[serde(default)]
    pub path_style: Option<bool>,
    pub access_key: String,
    /// 请求里的明文密钥（只在保存 / 测试时出现，保存后变成 secret_enc）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_enc: Option<String>,
}

impl S3Source {
    /// 两个配置读的是不是同一批对象（密钥换了不算）
    pub fn same_location(&self, o: &Self) -> bool {
        (&self.endpoint, self.region_or_default(), &self.bucket, &self.prefix, &self.pattern, self.recursive, self.use_path_style())
            == (&o.endpoint, o.region_or_default(), &o.bucket, &o.prefix, &o.pattern, o.recursive, o.use_path_style())
    }

    fn region_or_default(&self) -> &str {
        let r = self.region.trim();
        if r.is_empty() { "us-east-1" } else { r }
    }

    fn use_path_style(&self) -> bool {
        self.path_style.unwrap_or_else(|| self.endpoint.as_deref().is_some_and(|e| !e.trim().is_empty()))
    }
}

/// 保存前的检查（不联网）
pub fn check(s: &S3Source) -> AppResult<()> {
    if s.bucket.trim().is_empty() {
        return Err(AppError::bad("请填写存储桶（Bucket）名称"));
    }
    if s.access_key.trim().is_empty() {
        return Err(AppError::bad("请填写访问密钥 ID（Access Key）"));
    }
    if s.pattern.trim().is_empty() {
        return Err(AppError::bad("请填写文件名规则，如 *.csv"));
    }
    if let Some(e) = s.endpoint.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        let u = reqwest::Url::parse(e).map_err(|_| AppError::bad("服务地址不是有效的网址，应形如 https://minio.example.com:9000"))?;
        if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
            return Err(AppError::bad("服务地址应以 http:// 或 https:// 开头"));
        }
        if !u.path().trim_matches('/').is_empty() || u.query().is_some() {
            return Err(AppError::bad("服务地址只写到主机和端口，不要带路径（存储桶和前缀单独填）"));
        }
    }
    Ok(())
}

// ── 签名 ──────────────────────────────────────────────

/// URI 编码：只保留 A-Z a-z 0-9 - _ . ~；keep_slash 时 / 也保留（用于对象键路径）
fn encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("HMAC 接受任意长度的密钥");
    m.update(data.as_bytes());
    m.finalize().into_bytes().to_vec()
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// 规范化查询串（按编码后的键、值排序）；同一个字符串既用于签名也直接拼进请求地址，保证两边一致
fn canonical_query(query: &[(&str, &str)]) -> String {
    let mut q: Vec<(String, String)> = query.iter().map(|(k, v)| (encode(k, false), encode(v, false))).collect();
    q.sort();
    q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// AWS Signature V4 的 Authorization 头。headers 里是要签名的请求头（名称小写，含 host），amz_date 形如 20130524T000000Z
#[allow(clippy::too_many_arguments)]
fn authorization(method: &str, path: &str, query: &str, headers: &[(&str, &str)], payload_hash: &str, access: &str, secret: &str, region: &str, amz_date: &str) -> String {
    let mut hs: Vec<(&str, String)> = headers.iter().map(|(k, v)| (*k, v.trim().to_string())).collect();
    hs.sort_by(|a, b| a.0.cmp(b.0));
    let canonical_headers: String = hs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed = hs.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(";");
    let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed}\n{payload_hash}");
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/s3/aws4_request");
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex(Sha256::digest(canonical.as_bytes())));
    let k = hmac(format!("AWS4{secret}").as_bytes(), date);
    let k = hmac(&k, region);
    let k = hmac(&k, "s3");
    let k = hmac(&k, "aws4_request");
    format!("AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed}, Signature={}", hex(hmac(&k, &to_sign)))
}

/// 一个签好名的请求的各部分
struct Signed {
    url: String,
    host: String,
    amz_date: String,
    authorization: String,
}

impl S3Source {
    /// 对象 key 为 None 表示操作存储桶本身（列对象）
    fn sign(&self, secret: &str, key: Option<&str>, query: &[(&str, &str)], now: chrono::DateTime<chrono::Utc>) -> AppResult<Signed> {
        let base = self.endpoint.as_deref().map(str::trim).filter(|e| !e.is_empty()).map(str::to_string).unwrap_or_else(|| format!("https://s3.{}.amazonaws.com", self.region_or_default()));
        let u = reqwest::Url::parse(&base).map_err(|_| AppError::bad("服务地址不是有效的网址"))?;
        let bucket = self.bucket.trim();
        let base_host = u.host_str().ok_or_else(|| AppError::bad("服务地址缺少主机名"))?;
        let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
        let (host, path) = if self.use_path_style() {
            (format!("{base_host}{port}"), format!("/{}{}", encode(bucket, false), key.map(|k| format!("/{}", encode(k, true))).unwrap_or_else(|| "/".into())))
        } else {
            (format!("{bucket}.{base_host}{port}"), key.map(|k| format!("/{}", encode(k, true))).unwrap_or_else(|| "/".into()))
        };
        let q = canonical_query(query);
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let authorization = authorization(
            "GET",
            &path,
            &q,
            &[("host", &host), ("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", &amz_date)],
            EMPTY_SHA256,
            self.access_key.trim(),
            secret,
            self.region_or_default(),
            &amz_date,
        );
        let url = format!("{}://{host}{path}{}", u.scheme(), if q.is_empty() { String::new() } else { format!("?{q}") });
        Ok(Signed { url, host, amz_date, authorization })
    }

    async fn get(&self, client: &reqwest::Client, secret: &str, key: Option<&str>, query: &[(&str, &str)]) -> AppResult<reqwest::Response> {
        let s = self.sign(secret, key, query, chrono::Utc::now())?;
        let resp = client
            .get(&s.url)
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header("x-amz-date", &s.amz_date)
            .header(reqwest::header::AUTHORIZATION, &s.authorization)
            .send()
            .await
            .map_err(|e| AppError::bad(format!("连不上对象存储（{}）：{e}", s.host)))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(AppError::bad(error_message(status.as_u16(), &body)))
    }
}

/// 把对象存储返回的错误整理成能直接照着排查的说明
fn error_message(status: u16, body: &str) -> String {
    let code = first(body, "Code").map(unescape).unwrap_or_default();
    let msg = first(body, "Message").map(unescape).unwrap_or_default();
    let hint = match code.as_str() {
        "SignatureDoesNotMatch" => "：密钥不对，或区域（Region）/ 地址风格填错了",
        "InvalidAccessKeyId" => "：访问密钥 ID 不存在",
        "AccessDenied" | "AllAccessDisabled" => "：没有权限，需要 s3:ListBucket 与 s3:GetObject",
        "NoSuchBucket" => "：存储桶不存在（检查名称、区域和服务地址）",
        "NoSuchKey" => "：对象不存在（可能刚被删除）",
        "PermanentRedirect" | "AuthorizationHeaderMalformed" => "：区域填错了（存储桶不在这个区域）",
        _ => "",
    };
    match (code.is_empty(), msg.is_empty()) {
        (true, _) if status == 403 => "对象存储拒绝访问（403），请检查密钥与权限".into(),
        (true, _) => format!("对象存储返回 HTTP {status}"),
        (false, true) => format!("对象存储返回 {code}（HTTP {status}）{hint}"),
        (false, false) => format!("对象存储返回 {code}（HTTP {status}）{hint}。{msg}"),
    }
}

// ── 响应解析（ListObjectsV2 的 XML 很规整，不引入 XML 库）───────────

/// 第一个 <tag>…</tag> 的内容
fn first<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    all(xml, tag).into_iter().next()
}

/// 全部不嵌套的 <tag>…</tag> 的内容
fn all<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(&close) else { break };
        out.push(&after[..j]);
        rest = &after[j + close.len()..];
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let ent = &rest[1..end];
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let ch = ent.strip_prefix("#x").and_then(|h| u32::from_str_radix(h, 16).ok()).or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok())).and_then(char::from_u32);
                match ch {
                    Some(c) => out.push(c),
                    None => out.push_str(&rest[..=end]),
                }
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug, PartialEq)]
struct ListPage {
    /// （对象键，大小，修改时间 UTC）
    objects: Vec<(String, u64, chrono::DateTime<chrono::Utc>)>,
    next: Option<String>,
}

fn parse_list(xml: &str) -> ListPage {
    let objects = all(xml, "Contents")
        .into_iter()
        .filter_map(|c| {
            let key = unescape(first(c, "Key")?);
            let size = first(c, "Size")?.trim().parse().ok()?;
            let at = chrono::DateTime::parse_from_rfc3339(first(c, "LastModified")?.trim()).ok()?.with_timezone(&chrono::Utc);
            Some((key, size, at))
        })
        .collect();
    let truncated = first(xml, "IsTruncated").is_some_and(|t| t.trim() == "true");
    let next = if truncated { first(xml, "NextContinuationToken").map(unescape) } else { None };
    ListPage { objects, next }
}

// ── 列出与下载 ────────────────────────────────────────

pub fn client(ca_file: Option<&Path>) -> AppResult<reqwest::Client> {
    // 列表和小文件不该卡很久；大文件下载靠整体超时之外的「每次读取」超时兜底
    let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).read_timeout(Duration::from_secs(120));
    // 自建存储（MinIO 等）常用内部 CA 签发的证书：和网关 / 单点登录共用 LEDGER_AUTH_CA_FILE
    if let Some(path) = ca_file {
        for c in crate::config::load_root_certs(path).map_err(|e| AppError::bad(e.to_string()))? {
            b = b.add_root_certificate(c);
        }
    }
    b.build().map_err(|e| AppError::bad(format!("无法创建网络客户端：{e}")))
}

fn display_path(bucket: &str, key: &str) -> String {
    format!("s3://{bucket}/{key}")
}

/// 对象键 → 导入用的文件名（最后一段）
fn file_name(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

fn local_time(t: chrono::DateTime<chrono::Utc>) -> String {
    t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 列出匹配的对象（按修改时间从旧到新）；is_imported 判断由调用方提供
pub async fn list(src: &S3Source, secret: &str, client: &reqwest::Client, is_imported: impl Fn(&str, u64, &str) -> bool) -> AppResult<Vec<FileEntry>> {
    check(src)?;
    let patterns: Vec<&str> = src.pattern.split([';', '；', ',']).map(str::trim).filter(|p| !p.is_empty()).collect();
    let prefix = src.prefix.trim().trim_start_matches('/');
    let now = chrono::Utc::now();
    let mut out: Vec<(chrono::DateTime<chrono::Utc>, FileEntry)> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..MAX_PAGES {
        let mut query: Vec<(&str, &str)> = vec![("list-type", "2")];
        if !prefix.is_empty() {
            query.push(("prefix", prefix));
        }
        if !src.recursive {
            query.push(("delimiter", "/"));
        }
        if let Some(t) = token.as_deref() {
            query.push(("continuation-token", t));
        }
        let body = src.get(client, secret, None, &query).await?.text().await.map_err(|e| AppError::bad(format!("读取对象列表失败：{e}")))?;
        let p = parse_list(&body);
        for (key, size, at) in p.objects {
            let name = file_name(&key).to_string();
            if key.ends_with('/') || name.starts_with('.') || name.starts_with("~$") || crate::import_source::kind_of(&name).is_none() || !patterns.iter().any(|pt| wildcard(pt, &name)) {
                continue;
            }
            let path = display_path(src.bucket.trim(), &key);
            let mtime = local_time(at);
            let imported = is_imported(&path, size, &mtime);
            let settling = now.signed_duration_since(at).to_std().map(|d| d < SETTLE).unwrap_or(false);
            out.push((at, FileEntry { path, name, size, mtime, imported, settling }));
        }
        match p.next {
            Some(t) => token = Some(t),
            None => break,
        }
        if page + 1 == MAX_PAGES {
            return Err(AppError::bad(format!("前缀下的对象太多（已扫描 {} 个）：请把「前缀」写得更具体，如 reports/2026/", MAX_PAGES * 1000)));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)));
    Ok(out.into_iter().map(|(_, f)| f).collect())
}

/// 下载一个对象到本地文件，超过 max_bytes 就中止
pub async fn download(src: &S3Source, secret: &str, client: &reqwest::Client, entry: &FileEntry, dest: &Path, max_bytes: u64) -> AppResult<()> {
    let key = entry.path.strip_prefix(&format!("s3://{}/", src.bucket.trim())).ok_or_else(|| AppError::bad("对象路径与存储桶不符"))?;
    if entry.size > max_bytes {
        return Err(AppError::bad(format!("对象太大（{} MB），超过上传上限 {} MB", entry.size / 1048576, max_bytes / 1048576)));
    }
    let mut resp = src.get(client, secret, Some(key), &[]).await?;
    let mut f = tokio::fs::File::create(dest).await.map_err(|e| AppError::bad(format!("无法写入临时文件：{e}")))?;
    let mut total: u64 = 0;
    let result: AppResult<()> = async {
        while let Some(chunk) = resp.chunk().await.map_err(|e| AppError::bad(format!("下载「{}」中断：{e}", entry.name)))? {
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err(AppError::bad(format!("对象太大，超过上传上限 {} MB", max_bytes / 1048576)));
            }
            f.write_all(&chunk).await.map_err(|e| AppError::bad(format!("无法写入临时文件：{e}")))?;
        }
        f.flush().await.map_err(|e| AppError::bad(format!("无法写入临时文件：{e}")))?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(dest).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    fn src() -> S3Source {
        S3Source {
            endpoint: None,
            region: "us-east-1".into(),
            bucket: "examplebucket".into(),
            prefix: String::new(),
            pattern: "*.csv".into(),
            recursive: false,
            path_style: None,
            access_key: ACCESS.into(),
            secret_key: None,
            secret_enc: None,
        }
    }

    /// AWS 文档里的签名示例：GET Object（带 Range 头）
    #[test]
    fn signature_matches_the_aws_documentation_example() {
        let auth = authorization(
            "GET",
            "/test.txt",
            "",
            &[("host", "examplebucket.s3.amazonaws.com"), ("range", "bytes=0-9"), ("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", "20130524T000000Z")],
            EMPTY_SHA256,
            ACCESS,
            SECRET,
            "us-east-1",
            "20130524T000000Z",
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    /// AWS 文档里的签名示例：List Objects（带查询参数）
    #[test]
    fn signature_with_query_matches_the_aws_documentation_example() {
        let q = canonical_query(&[("max-keys", "2"), ("prefix", "J")]);
        assert_eq!(q, "max-keys=2&prefix=J");
        let auth = authorization(
            "GET",
            "/",
            &q,
            &[("host", "examplebucket.s3.amazonaws.com"), ("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", "20130524T000000Z")],
            EMPTY_SHA256,
            ACCESS,
            SECRET,
            "us-east-1",
            "20130524T000000Z",
        );
        assert!(auth.ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"), "{auth}");
    }

    #[test]
    fn urls_for_both_address_styles() {
        let now = chrono::DateTime::parse_from_rfc3339("2013-05-24T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
        // AWS：虚拟主机风格
        let s = src().sign(SECRET, Some("日报/2026 10.csv"), &[], now).unwrap();
        assert_eq!(s.url, "https://examplebucket.s3.us-east-1.amazonaws.com/%E6%97%A5%E6%8A%A5/2026%2010.csv");
        // 自建存储：路径风格，端口保留在 Host 里
        let mut m = src();
        m.endpoint = Some("http://minio.corp:9000".into());
        let s = m.sign(SECRET, None, &[("list-type", "2"), ("prefix", "a b/")], now).unwrap();
        assert_eq!(s.url, "http://minio.corp:9000/examplebucket/?list-type=2&prefix=a%20b%2F");
        assert_eq!(s.host, "minio.corp:9000");
        // 自定义地址 + 虚拟主机风格（阿里云 OSS 等）
        let mut o = src();
        o.endpoint = Some("https://oss-cn-hangzhou.aliyuncs.com".into());
        o.path_style = Some(false);
        assert_eq!(o.sign(SECRET, Some("a.csv"), &[], now).unwrap().url, "https://examplebucket.oss-cn-hangzhou.aliyuncs.com/a.csv");
    }

    #[test]
    fn parses_list_pages() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>b</Name><Prefix>日报/</Prefix><KeyCount>2</KeyCount><IsTruncated>true</IsTruncated>
<NextContinuationToken>1ueGcxLPRx1Tr/XYExHnhbYLgveDs2J/wm36Hy4vbOwM=</NextContinuationToken>
<Contents><Key>日报/a&amp;b.csv</Key><LastModified>2026-10-01T08:00:00.000Z</LastModified><ETag>&quot;x&quot;</ETag><Size>1234</Size><StorageClass>STANDARD</StorageClass></Contents>
<Contents><Key>日报/c.xlsx</Key><LastModified>2026-10-02T08:00:00.000Z</LastModified><Size>5</Size></Contents>
<CommonPrefixes><Prefix>日报/子目录/</Prefix></CommonPrefixes></ListBucketResult>"#;
        let p = parse_list(xml);
        assert_eq!(p.objects.len(), 2);
        assert_eq!((p.objects[0].0.as_str(), p.objects[0].1), ("日报/a&b.csv", 1234));
        assert_eq!(p.next.as_deref(), Some("1ueGcxLPRx1Tr/XYExHnhbYLgveDs2J/wm36Hy4vbOwM="));
        // 最后一页没有继续令牌
        assert_eq!(parse_list("<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>"), ListPage { objects: vec![], next: None });
        assert_eq!(unescape("&lt;a&gt; &#38; &#x4e2d; &unknown; &"), "<a> & 中 &unknown; &");
    }

    #[test]
    fn errors_are_explained() {
        let body = "<Error><Code>SignatureDoesNotMatch</Code><Message>The request signature we calculated does not match</Message></Error>";
        let m = error_message(403, body);
        assert!(m.contains("SignatureDoesNotMatch") && m.contains("区域") && m.contains("does not match"), "{m}");
        assert!(error_message(403, "").contains("403"));
        assert!(error_message(500, "<html/>").contains("500"));
        assert!(error_message(404, "<Error><Code>NoSuchBucket</Code></Error>").contains("存储桶不存在"));
    }

    #[test]
    fn config_checks() {
        assert!(check(&src()).is_ok());
        let mut s = src();
        s.bucket = " ".into();
        assert!(check(&s).is_err());
        s = src();
        s.access_key.clear();
        assert!(check(&s).is_err());
        s = src();
        s.endpoint = Some("ftp://x".into());
        assert!(check(&s).is_err());
        s.endpoint = Some("https://minio.corp:9000/bucket".into());
        assert!(check(&s).unwrap_err().message().contains("不要带路径"));
        s.endpoint = Some("https://minio.corp:9000".into());
        assert!(check(&s).is_ok());
        // 换密钥不算换了数据位置；换前缀算
        let (a, mut b) = (src(), src());
        b.secret_key = Some("new".into());
        assert!(a.same_location(&b));
        b.prefix = "x/".into();
        assert!(!a.same_location(&b));
        assert_eq!(file_name("a/b/c.csv"), "c.csv");
    }
}
