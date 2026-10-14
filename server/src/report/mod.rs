//! 定时推送：把工作簿里的仪表盘页（指标卡、明细卡、图表）按计划算好，推送到企业微信 / 钉钉 / 飞书群或邮箱
//!
//! - 推送渠道（ledger_report_channel）：Webhook 地址、加签密钥、收件人加密保存，接口只返回打码后的地址
//! - 推送任务（ledger_report）：工作簿 + 仪表盘页 + 计划（手动 / 每天 / 每周 / 每月 / 数据连接同步后）+ 渠道
//! - 内容在服务端按仪表盘上保存的切片器选择计算（日期切片器的「本月」「近 30 天」按发送当天计算）
//! - 每次推送写一条记录（ledger_report_run），含各渠道的结果

pub mod alert;
pub mod channels;
pub mod content;

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime};
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::now_local;
use crate::error::{AppError, AppResult};
use crate::routes::AppState;
use channels::Message;
use content::{Block, ReportContent};

const KEEP_RUNS: i64 = 100;
const SEND_TIMEOUT: Duration = Duration::from_secs(180);

// ── 渠道 ──────────────────────────────────────────────

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind {
    Wecom,
    Dingtalk,
    Feishu,
    Email,
}

impl ChannelKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Wecom => "wecom",
            Self::Dingtalk => "dingtalk",
            Self::Feishu => "feishu",
            Self::Email => "email",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Wecom => "企业微信",
            Self::Dingtalk => "钉钉",
            Self::Feishu => "飞书",
            Self::Email => "邮件",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelInput {
    pub name: String,
    pub kind: ChannelKind,
    /// 机器人 Webhook 地址，或邮件收件人（逗号 / 分号 / 换行分隔）；修改时留空沿用已保存的
    #[serde(default)]
    pub target: Option<String>,
    /// 钉钉 / 飞书加签密钥；修改时 None 沿用、空串清除
    #[serde(default)]
    pub secret: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelView {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
    /// 打码后的地址 / 收件人
    pub target: String,
    pub has_secret: bool,
    pub created_at: String,
}

struct StoredChannel {
    id: String,
    name: String,
    kind: ChannelKind,
    target: String,
    secret: Option<String>,
    created_at: String,
}

fn mask_target(kind: ChannelKind, target: &str) -> String {
    if kind == ChannelKind::Email {
        return target.to_string();
    }
    // https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=abcd…wxyz
    match reqwest::Url::parse(target) {
        Ok(u) => {
            let tail = u.query().or_else(|| u.path_segments().and_then(|mut s| s.next_back())).unwrap_or("");
            let keep: String = tail.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            format!("{}://{}/…{keep}", u.scheme(), u.host_str().unwrap_or(""))
        }
        Err(_) => "…".into(),
    }
}

pub fn parse_emails(s: &str) -> Vec<String> {
    s.split([',', ';', '，', '；', '\n', ' ']).map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

fn validate_target(kind: ChannelKind, target: &str) -> AppResult<String> {
    let target = target.trim();
    if kind == ChannelKind::Email {
        let list = parse_emails(target);
        if list.is_empty() {
            return Err(AppError::bad("请填写收件人邮箱"));
        }
        for a in &list {
            if a.parse::<lettre::message::Mailbox>().is_err() {
                return Err(AppError::bad(format!("邮箱格式不正确：{a}")));
            }
        }
        return Ok(list.join(", "));
    }
    let url = reqwest::Url::parse(target).map_err(|_| AppError::bad("Webhook 地址格式不正确"))?;
    crate::netguard::check_url(&url).map_err(|e| AppError::bad(e.message().replace("导入", "推送")))?;
    if kind == ChannelKind::Wecom && !url.query_pairs().any(|(k, _)| k == "key") {
        return Err(AppError::bad("企业微信 Webhook 地址应包含 key 参数"));
    }
    Ok(target.to_string())
}

fn load_channels(conn: &Connection, state: &AppState) -> AppResult<Vec<StoredChannel>> {
    let mut stmt = conn.prepare("SELECT id, name, kind, target_enc, secret_enc, strftime(created_at, '%Y-%m-%d %H:%M:%S') FROM ledger_report_channel ORDER BY created_at")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let secret = &state.connectors.secret;
    rows.into_iter()
        .map(|(id, name, kind, target, sec, created)| {
            Ok(StoredChannel {
                kind: ChannelKind::parse(&kind).ok_or_else(|| AppError::bad(format!("渠道「{name}」类型未知")))?,
                target: secret.decrypt(&target).map_err(|_| AppError::bad(format!("渠道「{name}」的地址无法解密（密钥文件可能已更换），请重新填写")))?,
                secret: sec.filter(|s| !s.is_empty()).map(|s| secret.decrypt(&s)).transpose().map_err(|_| AppError::bad(format!("渠道「{name}」的密钥无法解密，请重新填写")))?,
                id,
                name,
                created_at: created.unwrap_or_default(),
            })
        })
        .collect()
}

pub fn list_channels(conn: &Connection, state: &AppState) -> AppResult<Vec<ChannelView>> {
    Ok(load_channels(conn, state)?
        .into_iter()
        .map(|c| ChannelView { target: mask_target(c.kind, &c.target), has_secret: c.secret.is_some(), id: c.id, name: c.name, kind: c.kind, created_at: c.created_at })
        .collect())
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"))
}

pub fn create_channel(conn: &Connection, state: &AppState, input: ChannelInput) -> AppResult<String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::bad("请填写渠道名称"));
    }
    let target = validate_target(input.kind, input.target.as_deref().unwrap_or(""))?;
    let s = &state.connectors.secret;
    let sec = input.secret.as_deref().map(str::trim).filter(|x| !x.is_empty()).map(|x| s.encrypt(x)).transpose()?;
    let id = new_id("rc");
    conn.execute(
        "INSERT INTO ledger_report_channel (id, name, kind, target_enc, secret_enc, created_at) VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params![id, name, input.kind.as_str(), s.encrypt(&target)?, sec, now_local()],
    )?;
    Ok(id)
}

pub fn update_channel(conn: &Connection, state: &AppState, id: &str, input: ChannelInput) -> AppResult<()> {
    let old = load_channels(conn, state)?.into_iter().find(|c| c.id == id).ok_or_else(|| AppError::bad("推送渠道不存在"))?;
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::bad("请填写渠道名称"));
    }
    let target = match input.target.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
        Some(t) => validate_target(input.kind, t)?,
        None if input.kind == old.kind => old.target,
        None => return Err(AppError::bad("更换渠道类型后请重新填写地址")),
    };
    let sec = match input.secret.as_deref().map(str::trim) {
        None => old.secret,
        Some("") => None,
        Some(x) => Some(x.to_string()),
    };
    let s = &state.connectors.secret;
    conn.execute(
        "UPDATE ledger_report_channel SET name = ?, kind = ?, target_enc = ?, secret_enc = ? WHERE id = ?",
        params![name, input.kind.as_str(), s.encrypt(&target)?, sec.map(|x| s.encrypt(&x)).transpose()?, id],
    )?;
    Ok(())
}

pub fn delete_channel(conn: &Connection, id: &str) -> AppResult<()> {
    let used: Vec<String> = load_reports(conn)?.into_iter().filter(|r| r.channel_ids.iter().any(|c| c == id)).map(|r| r.name).collect();
    if !used.is_empty() {
        return Err(AppError::bad(format!("推送任务「{}」正在使用该渠道，请先从任务中移除", used.join("」「"))));
    }
    conn.execute("DELETE FROM ledger_report_channel WHERE id = ?", params![id])?;
    Ok(())
}

// ── 计划 ──────────────────────────────────────────────

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    Manual,
    /// 每天 HH:MM
    Daily { at: String },
    /// 每周几（1 = 周一 … 7 = 周日）HH:MM
    Weekly { weekday: u32, at: String },
    /// 每月几号 HH:MM（大于当月天数时取月末）
    Monthly { day: u32, at: String },
    /// 数据连接同步成功（有新数据）后
    #[serde(rename_all = "camelCase")]
    AfterSync { connector_id: String },
}

fn parse_at(at: &str) -> AppResult<NaiveTime> {
    NaiveTime::parse_from_str(at, "%H:%M").map_err(|_| AppError::bad("推送时间格式应为 HH:MM"))
}

fn month_day(y: i32, m: u32, day: u32) -> NaiveDate {
    let mut d = day.max(1);
    loop {
        if let Some(x) = NaiveDate::from_ymd_opt(y, m, d) {
            return x;
        }
        d -= 1;
    }
}

impl Schedule {
    fn validate(&self, conn: &Connection) -> AppResult<()> {
        match self {
            Schedule::Manual => Ok(()),
            Schedule::Daily { at } => parse_at(at).map(|_| ()),
            Schedule::Weekly { weekday, at } if (1..=7).contains(weekday) => parse_at(at).map(|_| ()),
            Schedule::Weekly { .. } => Err(AppError::bad("星期应为 1~7")),
            Schedule::Monthly { day, at } if (1..=31).contains(day) => parse_at(at).map(|_| ()),
            Schedule::Monthly { .. } => Err(AppError::bad("日期应为 1~31")),
            Schedule::AfterSync { connector_id } => {
                let n: i64 = conn.query_row("SELECT count(*) FROM ledger_connector WHERE id = ?", params![connector_id], |r| r.get(0))?;
                if n == 0 {
                    return Err(AppError::bad("请选择数据连接"));
                }
                Ok(())
            }
        }
    }

    /// base（上次定时推送或任务修改的时间）之后的第一个计划时间点
    fn next_run(&self, base: NaiveDateTime) -> Option<NaiveDateTime> {
        let (at, matches): (&str, Box<dyn Fn(NaiveDate) -> bool>) = match self {
            Schedule::Manual | Schedule::AfterSync { .. } => return None,
            Schedule::Daily { at } => (at, Box::new(|_| true)),
            Schedule::Weekly { weekday, at } => {
                let w = *weekday;
                (at, Box::new(move |d: NaiveDate| d.weekday().number_from_monday() == w))
            }
            Schedule::Monthly { day, at } => {
                let day = *day;
                (at, Box::new(move |d: NaiveDate| d == month_day(d.year(), d.month(), day)))
            }
        };
        let t = NaiveTime::parse_from_str(at, "%H:%M").ok()?;
        let mut d = base.date();
        for _ in 0..400 {
            let candidate = d.and_time(t);
            if candidate > base && matches(d) {
                return Some(candidate);
            }
            d = d.succ_opt()?;
        }
        None
    }
}

// ── 任务 ──────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportInput {
    pub name: String,
    pub workbook_id: String,
    pub sheet_id: String,
    /// 消息标题（为空时用任务名称）
    #[serde(default)]
    pub title: Option<String>,
    pub schedule: Schedule,
    pub channel_ids: Vec<String>,
    #[serde(default)]
    pub attach_excel: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResult {
    pub channel_id: String,
    pub channel_name: String,
    pub ok: bool,
    pub message: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub report_id: String,
    pub trigger: String,
    /// running / success / partial / failed
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub message: Option<String>,
    pub results: Vec<ChannelResult>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportView {
    pub id: String,
    pub name: String,
    pub workbook_id: String,
    pub workbook_name: Option<String>,
    pub sheet_id: String,
    pub sheet_name: Option<String>,
    pub title: String,
    pub schedule: Schedule,
    pub channel_ids: Vec<String>,
    pub attach_excel: bool,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_run: Option<RunRecord>,
    pub next_run: Option<String>,
    pub running: bool,
}

struct StoredReport {
    id: String,
    name: String,
    workbook_id: String,
    sheet_id: String,
    title: String,
    schedule: Schedule,
    channel_ids: Vec<String>,
    attach_excel: bool,
    enabled: bool,
    created_at: String,
    updated_at: String,
}

fn running() -> &'static Mutex<HashSet<String>> {
    static R: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_running(id: &str) -> bool {
    running().lock().map(|r| r.contains(id)).unwrap_or(false)
}

fn parse_ts(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()
}

fn load_reports(conn: &Connection) -> AppResult<Vec<StoredReport>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, workbook_id, sheet_id, title, schedule, channel_ids, attach_excel, enabled,
                strftime(created_at, '%Y-%m-%d %H:%M:%S'), strftime(updated_at, '%Y-%m-%d %H:%M:%S')
         FROM ledger_report ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(StoredReport {
                id: r.get(0)?,
                name: r.get(1)?,
                workbook_id: r.get(2)?,
                sheet_id: r.get(3)?,
                title: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                schedule: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Schedule::Manual),
                channel_ids: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                attach_excel: r.get(7)?,
                enabled: r.get(8)?,
                created_at: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                updated_at: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_report(conn: &Connection, id: &str) -> AppResult<StoredReport> {
    load_reports(conn)?.into_iter().find(|r| r.id == id).ok_or_else(|| AppError::bad("推送任务不存在"))
}

const RUN_COLUMNS: &str = "id, report_id, trigger_by, status, strftime(started_at, '%Y-%m-%d %H:%M:%S'), strftime(finished_at, '%Y-%m-%d %H:%M:%S'), message, results";

fn run_from_row(r: &duckdb::Row<'_>) -> duckdb::Result<RunRecord> {
    Ok(RunRecord {
        id: r.get(0)?,
        report_id: r.get(1)?,
        trigger: r.get(2)?,
        status: r.get(3)?,
        started_at: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        finished_at: r.get(5)?,
        message: r.get(6)?,
        results: r.get::<_, Option<String>>(7)?.and_then(|x| serde_json::from_str(&x).ok()).unwrap_or_default(),
    })
}

pub fn runs(conn: &Connection, report_id: &str, limit: i64) -> AppResult<Vec<RunRecord>> {
    let mut stmt = conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM ledger_report_run WHERE report_id = ? ORDER BY started_at DESC, id DESC LIMIT ?"))?;
    Ok(stmt.query_map(params![report_id, limit], run_from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// 计划时间的起点：最近一次定时推送，没有则取任务修改时间（新建 / 修改后不补发已过的时间点）
fn schedule_base(conn: &Connection, r: &StoredReport) -> AppResult<NaiveDateTime> {
    let last: Option<String> = conn
        .query_row(
            "SELECT strftime(max(started_at), '%Y-%m-%d %H:%M:%S') FROM ledger_report_run WHERE report_id = ? AND trigger_by = 'schedule'",
            params![r.id],
            |row| row.get(0),
        )
        .unwrap_or(None);
    let updated = parse_ts(&r.updated_at).unwrap_or_else(|| chrono::Local::now().naive_local());
    Ok(last.as_deref().and_then(parse_ts).map(|l| l.max(updated)).unwrap_or(updated))
}

pub fn list(conn: &Connection) -> AppResult<Vec<ReportView>> {
    let mut names: std::collections::HashMap<String, (Option<String>, Vec<(String, String, usize)>)> = std::collections::HashMap::new();
    load_reports(conn)?
        .into_iter()
        .map(|r| {
            let (wb_name, sheets) = names
                .entry(r.workbook_id.clone())
                .or_insert_with(|| {
                    let name = crate::workbook::meta(conn, &r.workbook_id).ok().map(|m| m.name);
                    (name, content::sheets_of(conn, &r.workbook_id).unwrap_or_default())
                })
                .clone();
            let next = if r.enabled { r.schedule.next_run(schedule_base(conn, &r)?) } else { None };
            Ok(ReportView {
                last_run: runs(conn, &r.id, 1)?.into_iter().next(),
                next_run: next.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                running: is_running(&r.id),
                workbook_name: wb_name,
                sheet_name: sheets.into_iter().find(|s| s.0 == r.sheet_id).map(|s| s.1),
                id: r.id,
                name: r.name,
                workbook_id: r.workbook_id,
                sheet_id: r.sheet_id,
                title: r.title,
                schedule: r.schedule,
                channel_ids: r.channel_ids,
                attach_excel: r.attach_excel,
                enabled: r.enabled,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
        })
        .collect()
}

fn validate(conn: &Connection, input: &ReportInput) -> AppResult<()> {
    if input.name.trim().is_empty() {
        return Err(AppError::bad("请填写任务名称"));
    }
    if !content::sheets_of(conn, &input.workbook_id)?.iter().any(|s| s.0 == input.sheet_id) {
        return Err(AppError::bad("请选择要推送的仪表盘页（工作簿需先保存）"));
    }
    if input.channel_ids.is_empty() {
        return Err(AppError::bad("请至少选择一个推送渠道"));
    }
    let known: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM ledger_report_channel")?;
        stmt.query_map([], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?
    };
    if input.channel_ids.iter().any(|c| !known.contains(c)) {
        return Err(AppError::bad("所选推送渠道不存在"));
    }
    input.schedule.validate(conn)
}

pub fn create(conn: &Connection, input: ReportInput) -> AppResult<String> {
    validate(conn, &input)?;
    let id = new_id("rp");
    let now = now_local();
    conn.execute(
        "INSERT INTO ledger_report (id, name, workbook_id, sheet_id, title, schedule, channel_ids, attach_excel, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
        params![
            id,
            input.name.trim(),
            input.workbook_id,
            input.sheet_id,
            input.title.as_deref().map(str::trim).unwrap_or(""),
            serde_json::to_string(&input.schedule)?,
            serde_json::to_string(&input.channel_ids)?,
            input.attach_excel,
            input.enabled,
            now,
            now
        ],
    )?;
    Ok(id)
}

pub fn update(conn: &Connection, id: &str, input: ReportInput) -> AppResult<()> {
    load_report(conn, id)?;
    validate(conn, &input)?;
    conn.execute(
        "UPDATE ledger_report SET name = ?, workbook_id = ?, sheet_id = ?, title = ?, schedule = ?, channel_ids = ?, attach_excel = ?, enabled = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
        params![
            input.name.trim(),
            input.workbook_id,
            input.sheet_id,
            input.title.as_deref().map(str::trim).unwrap_or(""),
            serde_json::to_string(&input.schedule)?,
            serde_json::to_string(&input.channel_ids)?,
            input.attach_excel,
            input.enabled,
            now_local(),
            id
        ],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: &str) -> AppResult<()> {
    if is_running(id) {
        return Err(AppError::bad("正在推送中，请稍后再删除"));
    }
    conn.execute("DELETE FROM ledger_report_run WHERE report_id = ?", params![id])?;
    conn.execute("DELETE FROM ledger_report WHERE id = ?", params![id])?;
    Ok(())
}

/// 工作簿里含仪表盘部件的工作表（供选择）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetOption {
    pub id: String,
    pub name: String,
    /// 指标卡 / 明细卡 / 图表 / 切片器数量
    pub parts: usize,
}

pub fn sheets(conn: &Connection, workbook_id: &str) -> AppResult<Vec<SheetOption>> {
    Ok(content::sheets_of(conn, workbook_id)?.into_iter().map(|(id, name, parts)| SheetOption { id, name, parts }).collect())
}

// ── 内容与附件 ────────────────────────────────────────

/// 查看完整看板的链接：LEDGER_PUBLIC_URL（如 https://bi.example.com）+ /excel?wb=…&sheet=…
pub fn public_url() -> Option<String> {
    std::env::var("LEDGER_PUBLIC_URL").ok().map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty())
}

fn link_of(workbook_id: &str, sheet_id: &str) -> Option<String> {
    public_url().map(|base| format!("{base}/excel?wb={workbook_id}&sheet={sheet_id}"))
}

/// Excel 附件：筛选说明、指标（数值、对比）、各明细表（原始数值，可再计算）
pub fn excel_of(title: &str, c: &ReportContent) -> AppResult<Vec<u8>> {
    use rust_xlsxwriter::{Color, Format, Workbook};
    let e = |x: rust_xlsxwriter::XlsxError| AppError::Internal(anyhow::anyhow!("{x}"));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name(c.sheet_name.chars().filter(|ch| !"[]:*?/\\".contains(*ch)).take(31).collect::<String>()).map_err(e)?;
    let title_f = Format::new().set_bold().set_font_size(14);
    let head = Format::new().set_bold().set_background_color(Color::RGB(0xF2F2F2));
    let bold = Format::new().set_bold();
    let num = Format::new().set_num_format("#,##0.00");
    let pct = Format::new().set_num_format("+0.0%;-0.0%;0.0%");
    let muted = Format::new().set_font_color(Color::RGB(0x898781));
    ws.set_column_width(0, 24).map_err(e)?;
    ws.set_column_range_width(1, 12, 16).map_err(e)?;
    ws.write_with_format(0, 0, title, &title_f).map_err(e)?;
    ws.write_with_format(1, 0, format!("{} · 生成于 {}", c.workbook_name, c.generated_at), &muted).map_err(e)?;
    let mut row = 2u32;
    for f in &c.filters {
        ws.write_with_format(row, 0, format!("筛选：{f}"), &muted).map_err(e)?;
        row += 1;
    }
    row += 1;
    let kpis: Vec<_> = c.blocks.iter().filter_map(|b| if let Block::Kpi(k) = b { Some(k) } else { None }).collect();
    if !kpis.is_empty() {
        for (i, h) in ["指标", "期间", "数值", "对比", "变化"].iter().enumerate() {
            ws.write_with_format(row, i as u16, *h, &head).map_err(e)?;
        }
        row += 1;
        for k in kpis {
            ws.write(row, 0, &k.title).map_err(e)?;
            ws.write(row, 1, k.period.as_deref().unwrap_or("")).map_err(e)?;
            if let Some(v) = k.value {
                ws.write_number_with_format(row, 2, v, &num).map_err(e)?;
            }
            ws.write(row, 3, &k.delta_label).map_err(e)?;
            if let Some(d) = k.delta {
                ws.write_number_with_format(row, 4, d, &pct).map_err(e)?;
            }
            row += 1;
        }
        row += 1;
    }
    for b in &c.blocks {
        match b {
            Block::Table(t) => {
                ws.write_with_format(row, 0, &t.title, &bold).map_err(e)?;
                row += 1;
                for (i, h) in t.header.iter().enumerate() {
                    ws.write_with_format(row, i as u16, h, &head).map_err(e)?;
                }
                row += 1;
                for (i, (label, values)) in t.raw.iter().chain(t.raw_total.iter()).enumerate() {
                    if i == t.raw.len() {
                        ws.write_with_format(row, 0, label, &bold).map_err(e)?;
                    } else {
                        ws.write(row, 0, label).map_err(e)?;
                    }
                    for (j, v) in values.iter().enumerate() {
                        if let Some(v) = v {
                            let is_pct = t.kinds.get(j).is_some_and(|k| k == "percent");
                            ws.write_number_with_format(row, (j + 1) as u16, *v, if is_pct { &pct } else { &num }).map_err(e)?;
                        } else if t.kinds.get(j).is_some_and(|k| k == "text") {
                            // 数据表的文本列（订单号、店铺…）
                            if let Some(text) = t.rows.get(i).and_then(|r| r.get(j + 1)) {
                                ws.write(row, (j + 1) as u16, text).map_err(e)?;
                            }
                        }
                    }
                    row += 1;
                }
                row += 1;
            }
            Block::Error { title, message } => {
                ws.write(row, 0, format!("{title}：无法计算（{message}）")).map_err(e)?;
                row += 2;
            }
            Block::Kpi(_) => {}
        }
    }
    wb.save_to_buffer().map_err(e)
}

fn file_name(title: &str) -> String {
    let safe: String = title.chars().filter(|ch| !"\\/:*?\"<>|".contains(*ch)).collect();
    format!("{}_{}.xlsx", if safe.is_empty() { "报表" } else { &safe }, chrono::Local::now().format("%Y%m%d"))
}

async fn deliver(ch: &StoredChannel, msg: &Message<'_>, excel: Option<&(String, Vec<u8>)>) -> AppResult<()> {
    let attachment = || excel.map(|(n, b)| (n.as_str(), b.clone()));
    match ch.kind {
        ChannelKind::Wecom => {
            validate_target(ch.kind, &ch.target)?;
            channels::send_wecom(&ch.target, msg, attachment()).await
        }
        ChannelKind::Dingtalk => {
            validate_target(ch.kind, &ch.target)?;
            channels::send_dingtalk(&ch.target, ch.secret.as_deref(), msg).await
        }
        ChannelKind::Feishu => {
            validate_target(ch.kind, &ch.target)?;
            channels::send_feishu(&ch.target, ch.secret.as_deref(), msg).await
        }
        ChannelKind::Email => channels::send_email(&parse_emails(&ch.target), msg, attachment()).await,
    }
}

/// 其他模块（多人填报的催办）往已配置的渠道推一张小表格（标题、几条说明、表头和行），返回每个渠道的结果；渠道已被删除的忽略
pub struct SmallTable {
    pub title: String,
    pub heading: String,
    pub context: String,
    pub notes: Vec<String>,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub async fn push_table(state: &AppState, channel_ids: &[String], t: SmallTable) -> AppResult<Vec<ChannelResult>> {
    let title = t.title.as_str();
    let content = ReportContent {
        workbook_name: t.heading,
        sheet_name: t.context,
        filters: t.notes,
        blocks: vec![Block::Table(content::TableBlock { title: title.to_string(), header: t.header, rows: t.rows, total: None, raw: vec![], raw_total: None, kinds: vec![] })],
        generated_at: now_local(),
    };
    let content = &content;
    let st = state.clone();
    let wanted = channel_ids.to_vec();
    let chans: Vec<StoredChannel> = tokio::task::spawn_blocking(move || -> AppResult<_> { Ok(load_channels(&st.connection()?, &st)?.into_iter().filter(|c| wanted.contains(&c.id)).collect()) }).await??;
    let msg = Message { title, content, link: None };
    let mut results = Vec::new();
    for ch in &chans {
        let r = tokio::time::timeout(Duration::from_secs(60), deliver(ch, &msg, None)).await.unwrap_or_else(|_| Err(AppError::bad("发送超时")));
        if let Err(e) = &r {
            tracing::warn!("往「{}」推送「{title}」失败：{}", ch.name, e.message());
        }
        results.push(ChannelResult { channel_id: ch.id.clone(), channel_name: format!("{}（{}）", ch.name, ch.kind.label()), ok: r.is_ok(), message: r.err().map(|e| e.message()) });
    }
    Ok(results)
}

/// 给定的渠道 id 里，确实存在的那些（保存前校验用）
pub fn existing_channel_ids(conn: &Connection, ids: &[String]) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM ledger_report_channel")?;
    let have: std::collections::HashSet<String> = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?;
    Ok(ids.iter().filter(|i| have.contains(*i)).cloned().collect())
}

// ── 预览 / 发送 ──────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInput {
    pub workbook_id: String,
    pub sheet_id: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOutcome {
    pub content: ReportContent,
    /// 邮件正文（HTML）
    pub html: String,
    /// 机器人消息（企业微信 Markdown）
    pub markdown: String,
}

pub async fn preview(state: &AppState, input: PreviewInput) -> AppResult<PreviewOutcome> {
    let st = state.clone();
    let (wb, sheet) = (input.workbook_id.clone(), input.sheet_id.clone());
    let c = tokio::task::spawn_blocking(move || -> AppResult<ReportContent> {
        let conn = st.connection()?;
        content::build(&conn, &wb, &sheet, crate::db::today_local())
    })
    .await??;
    let title = input.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(String::from).unwrap_or_else(|| c.sheet_name.clone());
    let msg = Message { title: &title, content: &c, link: link_of(&input.workbook_id, &input.sheet_id) };
    let (html, markdown) = (channels::email_html(&msg), channels::wecom_markdown(&msg));
    Ok(PreviewOutcome { content: c, html, markdown })
}

/// 发送一次（等待完成）：计算内容 → 逐个渠道发送 → 记录结果
pub async fn send(state: &AppState, id: &str, trigger: &str) -> AppResult<RunRecord> {
    {
        let mut r = running().lock().map_err(|_| anyhow::anyhow!("推送状态锁已损坏"))?;
        if !r.insert(id.to_string()) {
            return Err(AppError::bad("正在推送中"));
        }
    }
    let result = send_inner(state, id, trigger).await;
    if let Ok(mut r) = running().lock() {
        r.remove(id);
    }
    result
}

async fn send_inner(state: &AppState, id: &str, trigger: &str) -> AppResult<RunRecord> {
    let st = state.clone();
    let (rid, tr) = (id.to_string(), trigger.to_string());
    let (report, chans, run_id, started) = tokio::task::spawn_blocking(move || -> AppResult<_> {
        let conn = st.connection()?;
        let report = load_report(&conn, &rid)?;
        let chans: Vec<StoredChannel> = load_channels(&conn, &st)?.into_iter().filter(|c| report.channel_ids.contains(&c.id)).collect();
        let run_id = format!("rr_{}_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"), &rid[rid.len().saturating_sub(4)..]);
        let started = now_local();
        conn.execute(
            "INSERT INTO ledger_report_run (id, report_id, trigger_by, status, started_at) VALUES (?, ?, ?, 'running', CAST(? AS TIMESTAMP))",
            params![run_id, rid, tr, started],
        )?;
        Ok((report, chans, run_id, started))
    })
    .await??;

    let outcome = tokio::time::timeout(SEND_TIMEOUT, async {
        let st = state.clone();
        let (wb, sheet) = (report.workbook_id.clone(), report.sheet_id.clone());
        let c = tokio::task::spawn_blocking(move || -> AppResult<ReportContent> {
            let conn = st.connection()?;
            content::build(&conn, &wb, &sheet, crate::db::today_local())
        })
        .await??;
        let title = if report.title.is_empty() { report.name.clone() } else { report.title.clone() };
        let msg = Message { title: &title, content: &c, link: link_of(&report.workbook_id, &report.sheet_id) };
        let excel = if report.attach_excel { Some((file_name(&title), excel_of(&title, &c)?)) } else { None };
        let mut results = Vec::new();
        if chans.is_empty() {
            return Err(AppError::bad("任务没有可用的推送渠道（可能已被删除）"));
        }
        for ch in &chans {
            let r = deliver(ch, &msg, excel.as_ref()).await;
            if let Err(e) = &r {
                tracing::warn!("推送任务「{}」发送到「{}」失败：{}", report.name, ch.name, e.message());
            }
            results.push(ChannelResult { channel_id: ch.id.clone(), channel_name: format!("{}（{}）", ch.name, ch.kind.label()), ok: r.is_ok(), message: r.err().map(|e| e.message()) });
        }
        let errors = c.blocks.iter().filter(|b| matches!(b, Block::Error { .. })).count();
        let note = if errors > 0 { Some(format!("{errors} 个部件无法计算，已在消息中注明")) } else { None };
        Ok((results, note))
    })
    .await
    .unwrap_or_else(|_| Err(AppError::bad("推送超过 3 分钟未完成，已中止")));

    let (status, results, message) = match outcome {
        Ok((results, note)) => {
            let ok = results.iter().filter(|r| r.ok).count();
            let status = if ok == results.len() { "success" } else if ok == 0 { "failed" } else { "partial" };
            (status, results, note)
        }
        Err(e) => ("failed", Vec::new(), Some(e.message())),
    };
    tracing::info!("推送任务「{}」{}：{}", report.name, trigger, status);
    let record = RunRecord {
        id: run_id.clone(),
        report_id: id.to_string(),
        trigger: trigger.to_string(),
        status: status.into(),
        started_at: started,
        finished_at: Some(now_local()),
        message,
        results,
    };
    let st = state.clone();
    let rec = record.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = st.connection()?;
        conn.execute(
            "UPDATE ledger_report_run SET status = ?, finished_at = CAST(? AS TIMESTAMP), message = ?, results = ? WHERE id = ?",
            params![rec.status, rec.finished_at, rec.message, serde_json::to_string(&rec.results)?, rec.id],
        )?;
        conn.execute(
            "DELETE FROM ledger_report_run WHERE report_id = ? AND id NOT IN (SELECT id FROM ledger_report_run WHERE report_id = ? ORDER BY started_at DESC, id DESC LIMIT ?)",
            params![rec.report_id, rec.report_id, KEEP_RUNS],
        )?;
        Ok(())
    })
    .await??;
    Ok(record)
}

/// 测试渠道：发送一条示例消息
pub async fn test_channel(state: &AppState, id: &str) -> AppResult<()> {
    let st = state.clone();
    let cid = id.to_string();
    let ch = tokio::task::spawn_blocking(move || -> AppResult<StoredChannel> {
        let conn = st.connection()?;
        load_channels(&conn, &st)?.into_iter().find(|c| c.id == cid).ok_or_else(|| AppError::bad("推送渠道不存在"))
    })
    .await??;
    let c = ReportContent {
        workbook_name: "Gridlake".into(),
        sheet_name: "推送渠道测试".into(),
        filters: vec![],
        blocks: vec![Block::Kpi(content::KpiBlock {
            title: "示例指标".into(),
            value: Some(1_234_567.0),
            money: true,
            delta: Some(0.052),
            delta_label: "环比".into(),
            period: None,
            up_is_good: true,
        })],
        generated_at: now_local(),
    };
    let msg = Message { title: "测试消息 · 定时推送", content: &c, link: public_url() };
    tokio::time::timeout(Duration::from_secs(60), deliver(&ch, &msg, None)).await.unwrap_or_else(|_| Err(AppError::bad("发送超时")))
}

// ── 调度 ──────────────────────────────────────────────

fn spawn_send(state: &AppState, id: String, name: String, trigger: &'static str) {
    let st = state.clone();
    tokio::spawn(async move {
        if let Err(e) = send(&st, &id, trigger).await {
            tracing::warn!("推送任务「{name}」未能执行：{}", e.message());
        }
    });
}

/// 每 30 秒检查一次到期的推送任务
pub async fn schedule(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let st = state.clone();
        let due = tokio::task::spawn_blocking(move || -> AppResult<Vec<(String, String)>> {
            let conn = st.connection()?;
            let now = chrono::Local::now().naive_local();
            let mut due = Vec::new();
            // 工作簿在回收站（或已彻底删除）的任务不推送
            for r in load_reports(&conn)?.into_iter().filter(|r| r.enabled && !is_running(&r.id) && crate::workbook::is_alive(&conn, &r.workbook_id)) {
                if r.schedule.next_run(schedule_base(&conn, &r)?).is_some_and(|t| t <= now) {
                    due.push((r.id, r.name));
                }
            }
            Ok(due)
        })
        .await
        .unwrap_or_else(|e| Err(AppError::Internal(e.into())));
        match due {
            Ok(due) => due.into_iter().for_each(|(id, name)| spawn_send(&state, id, name, "schedule")),
            Err(e) => tracing::warn!("检查定时推送失败：{}", e.message()),
        }
    }
}

/// 数据连接同步成功（有新数据）后：触发「同步后推送」的任务
pub fn on_connector_synced(state: &AppState, connector_id: &str) {
    let due = state.connection().map_err(AppError::from).and_then(|conn| Ok((load_reports(&conn)?, conn))).map(|(list, conn)| {
        list.into_iter()
            .filter(|r| r.enabled && matches!(&r.schedule, Schedule::AfterSync { connector_id: c } if c == connector_id) && crate::workbook::is_alive(&conn, &r.workbook_id))
            .map(|r| (r.id, r.name))
            .collect::<Vec<_>>()
    });
    match due {
        Ok(due) => due.into_iter().for_each(|(id, name)| spawn_send(state, id, name, "after_sync")),
        Err(e) => tracing::warn!("检查同步后推送失败：{}", e.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> NaiveDateTime {
        parse_ts(s).unwrap()
    }

    #[test]
    fn next_runs() {
        // 2026-10-05 是周一
        let base = ts("2026-10-05 10:00:00");
        assert_eq!(Schedule::Daily { at: "09:00".into() }.next_run(base), Some(ts("2026-10-06 09:00:00")));
        assert_eq!(Schedule::Daily { at: "10:30".into() }.next_run(base), Some(ts("2026-10-05 10:30:00")));
        assert_eq!(Schedule::Weekly { weekday: 1, at: "09:00".into() }.next_run(base), Some(ts("2026-10-12 09:00:00")));
        assert_eq!(Schedule::Weekly { weekday: 5, at: "18:00".into() }.next_run(base), Some(ts("2026-10-09 18:00:00")));
        assert_eq!(Schedule::Monthly { day: 1, at: "08:00".into() }.next_run(base), Some(ts("2026-11-01 08:00:00")));
        // 31 号：没有 31 号的月份取月末
        assert_eq!(Schedule::Monthly { day: 31, at: "08:00".into() }.next_run(ts("2026-11-01 00:00:00")), Some(ts("2026-11-30 08:00:00")));
        assert_eq!(Schedule::Manual.next_run(base), None);
    }

    #[test]
    fn targets() {
        assert_eq!(parse_emails("a@x.com, b@y.com；c@z.com"), vec!["a@x.com", "b@y.com", "c@z.com"]);
        assert!(validate_target(ChannelKind::Email, "bad").is_err());
        assert_eq!(mask_target(ChannelKind::Wecom, "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=abcdefgh1234"), "https://qyapi.weixin.qq.com/…1234");
    }
}
