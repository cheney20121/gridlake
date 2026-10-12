//! 推送渠道：企业微信 / 钉钉 / 飞书群机器人（Webhook），邮件（SMTP）
//!
//! - 机器人消息用各家支持的 Markdown / 卡片：指标（数值 + 同比 / 环比，▲▼ + 颜色）、排行列表、查看完整看板的链接
//! - 钉钉、飞书可配置加签密钥（HMAC-SHA256）
//! - 企业微信可附带 Excel（先上传文件再发送文件消息）；钉钉、飞书机器人不支持附件
//! - 邮件：HTML 正文 + 可选 Excel 附件；SMTP 由环境变量配置（LEDGER_SMTP_*）

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::content::{Block, ReportContent, compact, percent};
use crate::error::{AppError, AppResult};

pub struct Message<'a> {
    pub title: &'a str,
    pub content: &'a ReportContent,
    /// 查看完整看板的链接（配置了 LEDGER_PUBLIC_URL 时）
    pub link: Option<String>,
}

fn delta_text(k: &super::content::KpiBlock) -> Option<(String, bool)> {
    let d = k.delta?;
    let arrow = if d > 0.0 { "▲" } else if d < 0.0 { "▼" } else { "—" };
    let good = (d > 0.0) == k.up_is_good;
    Some((format!("{arrow}{} {}", percent(Some(d)).trim_start_matches('+').trim_start_matches('-'), k.delta_label), good || d == 0.0))
}

/// 通用 Markdown 正文；color(text, good) 由各渠道决定颜色写法
fn markdown(msg: &Message<'_>, color: &dyn Fn(&str, bool) -> String, max_rows: usize) -> String {
    let c = msg.content;
    let mut out = format!("### {}\n", msg.title);
    out.push_str(&format!("> {} · {}{}\n\n", c.sheet_name, c.generated_at.get(..16).unwrap_or(&c.generated_at), if c.filters.is_empty() { String::new() } else { format!("\n> 筛选：{}", c.filters.join("；")) }));
    for b in &c.blocks {
        match b {
            Block::Kpi(k) => {
                let delta = delta_text(k).map(|(t, good)| format!("　{}", color(&t, good))).unwrap_or_default();
                let period = k.period.as_ref().map(|p| format!("（{p}）")).unwrap_or_default();
                out.push_str(&format!("**{}**{period}：{}{delta}\n\n", k.title, compact(k.value, k.money)));
            }
            Block::Table(t) => {
                out.push_str(&format!("**{}**\n", t.title));
                // 多列时每个数带上列名（如「专卖店 72.5万 · 折扣店 88.3万」）
                let cells = |r: &[String]| -> String {
                    if t.header.len() <= 2 {
                        return r[1..].join(" / ");
                    }
                    // 「订单金额（求和） · 占总计」这类列名只取最后一段，避免重复
                    r[1..].iter().zip(&t.header[1..]).map(|(v, h)| format!("{} {v}", h.rsplit(" · ").next().unwrap_or(h))).collect::<Vec<_>>().join(" · ")
                };
                // 时间序列（最近 N 期）列出最近的，排行列出靠前的
                let latest_first = t.rows.len() > max_rows && t.rows.first().is_some_and(|r| r[0].chars().next().is_some_and(|c| c.is_ascii_digit()));
                let picked: Vec<&Vec<String>> = if latest_first { t.rows.iter().skip(t.rows.len() - max_rows).collect() } else { t.rows.iter().take(max_rows).collect() };
                for (i, r) in picked.iter().enumerate() {
                    if latest_first {
                        out.push_str(&format!("- {}　{}\n", r[0], cells(r)));
                    } else {
                        out.push_str(&format!("{}. {}　{}\n", i + 1, r[0], cells(r)));
                    }
                }
                if t.rows.len() > max_rows {
                    out.push_str(&format!("…共 {} 项{}\n", t.rows.len(), if latest_first { "，以上为最近几期" } else { "" }));
                }
                if let Some(total) = &t.total {
                    out.push_str(&format!("{}：{}\n", total[0], cells(total)));
                }
                out.push('\n');
            }
            Block::Error { title, message } => out.push_str(&format!("**{title}**：{}\n\n", color(&format!("无法计算（{message}）"), false))),
        }
    }
    if let Some(link) = &msg.link {
        out.push_str(&format!("[查看完整看板]({link})\n"));
    }
    out
}

/// 企业微信 Markdown：content 不超过 4096 字节，超出时减少每个列表的行数
pub fn wecom_markdown(msg: &Message<'_>) -> String {
    let color = |t: &str, good: bool| format!("<font color=\"{}\">{t}</font>", if good { "info" } else { "warning" });
    for rows in [10, 5, 3, 1] {
        let s = markdown(msg, &color, rows);
        if s.len() <= 4000 {
            return s;
        }
    }
    let s = markdown(msg, &color, 1);
    s.char_indices().take_while(|(i, _)| *i < 3990).map(|(_, c)| c).collect::<String>() + "…"
}

pub fn dingtalk_markdown(msg: &Message<'_>) -> String {
    markdown(msg, &|t, good| format!("<font color=\"{}\">{t}</font>", if good { "#006300" } else { "#D03B3B" }), 10)
}

/// 飞书卡片：标题栏 + lark_md 正文
pub fn feishu_card(msg: &Message<'_>) -> serde_json::Value {
    // 飞书 lark_md 不支持 ### 标题，标题放在卡片头部
    let body = markdown(msg, &|t, good| format!("<font color='{}'>{t}</font>", if good { "green" } else { "red" }), 10);
    let body = body.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or(body).replace("> ", "");
    serde_json::json!({
        "msg_type": "interactive",
        "card": {
            "config": { "wide_screen_mode": true },
            "header": { "title": { "tag": "plain_text", "content": msg.title }, "template": "green" },
            "elements": [{ "tag": "div", "text": { "tag": "lark_md", "content": body } }]
        }
    })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// 邮件 HTML 正文
pub fn email_html(msg: &Message<'_>) -> String {
    let c = msg.content;
    let mut h = format!(
        "<div style=\"font-family:system-ui,-apple-system,'PingFang SC','Microsoft YaHei',sans-serif;color:#0b0b0b;max-width:760px\"><h2 style=\"margin:0 0 4px\">{}</h2><div style=\"color:#898781;font-size:12px;margin-bottom:16px\">{} · {}{}</div>",
        esc(msg.title),
        esc(&c.sheet_name),
        esc(&c.generated_at),
        if c.filters.is_empty() { String::new() } else { format!("<br>筛选：{}", esc(&c.filters.join("；"))) }
    );
    let kpis: Vec<_> = c.blocks.iter().filter_map(|b| if let Block::Kpi(k) = b { Some(k) } else { None }).collect();
    if !kpis.is_empty() {
        h.push_str("<table style=\"border-collapse:separate;border-spacing:8px 0;margin:0 -8px 16px\"><tr>");
        for k in &kpis {
            let delta = delta_text(k)
                .map(|(t, good)| format!("<div style=\"font-size:12px;font-weight:600;color:{}\">{}</div>", if good { "#006300" } else { "#d03b3b" }, esc(&t)))
                .unwrap_or_default();
            h.push_str(&format!(
                "<td style=\"border:1px solid #e1e0d9;border-radius:6px;padding:10px 14px;vertical-align:top\"><div style=\"color:#52514e;font-size:13px\">{}{}</div><div style=\"font-size:26px;font-weight:600\">{}</div>{delta}</td>",
                esc(&k.title),
                k.period.as_ref().map(|p| format!(" <span style=\"color:#898781;font-size:12px\">{p}</span>")).unwrap_or_default(),
                esc(&compact(k.value, k.money))
            ));
        }
        h.push_str("</tr></table>");
    }
    for b in &c.blocks {
        match b {
            Block::Table(t) => {
                h.push_str(&format!("<h3 style=\"font-size:14px;margin:16px 0 6px\">{}</h3><table style=\"border-collapse:collapse;font-size:13px;min-width:360px\"><tr>", esc(&t.title)));
                for (i, col) in t.header.iter().enumerate() {
                    h.push_str(&format!("<th style=\"background:#f9f9f7;color:#52514e;padding:5px 12px;text-align:{};border-bottom:1px solid #e1e0d9\">{}</th>", if i == 0 { "left" } else { "right" }, esc(col)));
                }
                h.push_str("</tr>");
                let mut row = |cells: &[String], bold: bool| {
                    h.push_str("<tr>");
                    for (i, v) in cells.iter().enumerate() {
                        h.push_str(&format!(
                            "<td style=\"padding:5px 12px;border-bottom:1px solid #f0efec;text-align:{};{}\">{}</td>",
                            if i == 0 { "left" } else { "right" },
                            if bold { "font-weight:600" } else { "" },
                            esc(v)
                        ));
                    }
                    h.push_str("</tr>");
                };
                for r in &t.rows {
                    row(r, false);
                }
                if let Some(total) = &t.total {
                    row(total, true);
                }
                h.push_str("</table>");
            }
            Block::Error { title, message } => h.push_str(&format!("<p style=\"color:#d03b3b\">{}：无法计算（{}）</p>", esc(title), esc(message))),
            Block::Kpi(_) => {}
        }
    }
    if let Some(link) = &msg.link {
        h.push_str(&format!("<p style=\"margin-top:20px\"><a href=\"{}\" style=\"color:#217346\">查看完整看板</a></p>", esc(link)));
    }
    h.push_str("</div>");
    h
}

// ── 发送 ──

fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::bad(format!("无法创建网络客户端：{e}")))
}

fn sign(secret: &str, ts: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(format!("{ts}\n{secret}").as_bytes()).expect("hmac accepts any key");
    // 飞书：以「时间戳\n密钥」为键、对空串签名
    mac.update(b"");
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

fn dingtalk_sign(secret: &str, ts: i64) -> String {
    // 钉钉：以密钥为键、对「时间戳\n密钥」签名
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(format!("{ts}\n{secret}").as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

fn url_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

async fn post_json(url: &str, body: &serde_json::Value) -> AppResult<serde_json::Value> {
    let resp = client()?.post(url).json(body).send().await.map_err(|e| AppError::bad(format!("发送失败：{e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::bad(format!("发送失败：服务器返回 {status}（{}）", text.chars().take(200).collect::<String>())));
    }
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
}

/// 机器人返回的错误码：企业微信 / 钉钉 errcode，飞书 code / StatusCode
fn check_bot(v: &serde_json::Value) -> AppResult<()> {
    let code = v.get("errcode").or_else(|| v.get("code")).or_else(|| v.get("StatusCode")).and_then(|c| c.as_i64()).unwrap_or(0);
    if code != 0 {
        let msg = v.get("errmsg").or_else(|| v.get("msg")).or_else(|| v.get("StatusMessage")).and_then(|m| m.as_str()).unwrap_or("");
        return Err(AppError::bad(format!("机器人返回错误 {code}：{msg}")));
    }
    Ok(())
}

pub async fn send_wecom(webhook: &str, msg: &Message<'_>, excel: Option<(&str, Vec<u8>)>) -> AppResult<()> {
    check_bot(&post_json(webhook, &serde_json::json!({ "msgtype": "markdown", "markdown": { "content": wecom_markdown(msg) } })).await?)?;
    if let Some((name, bytes)) = excel {
        // 上传文件：https://qyapi.weixin.qq.com/cgi-bin/webhook/upload_media?key=…&type=file
        let url = reqwest::Url::parse(webhook).map_err(|_| AppError::bad("企业微信 Webhook 地址不正确"))?;
        let key = url.query_pairs().find(|(k, _)| k == "key").map(|(_, v)| v.to_string()).ok_or_else(|| AppError::bad("企业微信 Webhook 地址缺少 key"))?;
        let mut upload = url.clone();
        upload.set_path(&url.path().replace("/send", "/upload_media"));
        upload.set_query(Some(&format!("key={}&type=file", url_encode(&key))));
        let part = reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()).mime_str("application/octet-stream").map_err(|e| AppError::bad(e.to_string()))?;
        let resp = client()?.post(upload).multipart(reqwest::multipart::Form::new().part("media", part)).send().await.map_err(|e| AppError::bad(format!("上传附件失败：{e}")))?;
        let v: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        check_bot(&v)?;
        let media_id = v.get("media_id").and_then(|m| m.as_str()).ok_or_else(|| AppError::bad("上传附件失败：没有返回 media_id"))?;
        check_bot(&post_json(webhook, &serde_json::json!({ "msgtype": "file", "file": { "media_id": media_id } })).await?)?;
    }
    Ok(())
}

pub async fn send_dingtalk(webhook: &str, secret: Option<&str>, msg: &Message<'_>) -> AppResult<()> {
    let mut url = webhook.to_string();
    if let Some(s) = secret.filter(|s| !s.is_empty()) {
        let ts = chrono::Utc::now().timestamp_millis();
        url.push_str(&format!("{}timestamp={ts}&sign={}", if url.contains('?') { '&' } else { '?' }, url_encode(&dingtalk_sign(s, ts))));
    }
    check_bot(&post_json(&url, &serde_json::json!({ "msgtype": "markdown", "markdown": { "title": msg.title, "text": dingtalk_markdown(msg) } })).await?)
}

pub async fn send_feishu(webhook: &str, secret: Option<&str>, msg: &Message<'_>) -> AppResult<()> {
    let mut body = feishu_card(msg);
    if let Some(s) = secret.filter(|s| !s.is_empty()) {
        let ts = chrono::Utc::now().timestamp();
        body["timestamp"] = serde_json::Value::String(ts.to_string());
        body["sign"] = serde_json::Value::String(sign(s, ts));
    }
    check_bot(&post_json(webhook, &body).await?)
}

/// SMTP 配置（环境变量）：LEDGER_SMTP_HOST / PORT / USER / PASSWORD / FROM / TLS（starttls | tls | none）；
/// HOST 为 file:///目录 时写入 .eml 文件（测试用）
pub fn smtp_configured() -> bool {
    std::env::var("LEDGER_SMTP_HOST").is_ok_and(|v| !v.trim().is_empty())
}

pub async fn send_email(to: &[String], msg: &Message<'_>, excel: Option<(&str, Vec<u8>)>) -> AppResult<()> {
    use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType};
    use lettre::{AsyncFileTransport, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let host = env("LEDGER_SMTP_HOST").ok_or_else(|| AppError::bad("服务端未配置邮件发送（LEDGER_SMTP_HOST）"))?;
    let from: Mailbox = env("LEDGER_SMTP_FROM")
        .or_else(|| env("LEDGER_SMTP_USER"))
        .unwrap_or_else(|| "gridlake@localhost".into())
        .parse()
        .map_err(|_| AppError::bad("发件人地址（LEDGER_SMTP_FROM）格式不正确"))?;
    let mut builder = lettre::Message::builder().from(from).subject(msg.title);
    for addr in to {
        builder = builder.to(addr.parse().map_err(|_| AppError::bad(format!("收件人地址格式不正确：{addr}")))?);
    }
    let html = SinglePart::html(email_html(msg));
    let body = match excel {
        Some((name, bytes)) => MultiPart::mixed().singlepart(html).singlepart(
            Attachment::new(name.to_string()).body(bytes, ContentType::parse("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet").expect("valid mime")),
        ),
        None => MultiPart::mixed().singlepart(html),
    };
    let email = builder.multipart(body).map_err(|e| AppError::bad(format!("邮件内容有误：{e}")))?;
    if let Some(dir) = host.strip_prefix("file://") {
        std::fs::create_dir_all(dir)?;
        AsyncFileTransport::<Tokio1Executor>::new(dir).send(email).await.map_err(|e| AppError::bad(format!("写入邮件文件失败：{e}")))?;
        return Ok(());
    }
    let port: u16 = env("LEDGER_SMTP_PORT").and_then(|p| p.parse().ok()).unwrap_or(465);
    let tls = env("LEDGER_SMTP_TLS").unwrap_or_else(|| if port == 465 { "tls".into() } else { "starttls".into() });
    let mut t = match tls.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host),
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host),
        _ => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&host)),
    }
    .map_err(|e| AppError::bad(format!("SMTP 配置有误：{e}")))?
    .port(port);
    if let (Some(u), Some(p)) = (env("LEDGER_SMTP_USER"), env("LEDGER_SMTP_PASSWORD")) {
        t = t.credentials(lettre::transport::smtp::authentication::Credentials::new(u, p));
    }
    t.build().send(email).await.map_err(|e| AppError::bad(format!("邮件发送失败：{e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures() {
        // 钉钉文档示例口径：HMAC-SHA256(key=secret, msg="ts\nsecret")，Base64
        let s = dingtalk_sign("SEC123", 1_700_000_000_000);
        assert_eq!(s.len(), 44);
        assert_ne!(s, sign("SEC123", 1_700_000_000));
        assert_eq!(url_encode("a+b/c="), "a%2Bb%2Fc%3D");
    }
}
