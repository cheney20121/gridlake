//! 登录身份、权限与操作审计（最小用户层）
//!
//! - 认证：`LEDGER_AUTH_MODE` = off（默认，不校验）/ local（内置账号，见 account.rs）/ gateway（拿请求头 `x-user-token` 去网关的用户信息接口校验）/
//!   proxy（信任反向代理注入的 `x-auth-user-id` / `x-auth-user-name` / `x-auth-user-admin`，仅限可信代理地址）
//! - 操作人：开启认证后，请求里的 `operator`（查询参数或 JSON 顶层字段）一律被登录用户的名字覆盖，客户端无法冒充
//! - 权限：在本模块的路由表里集中声明——谁能改某个工作簿 / 数据集、哪些接口只有管理员能用；不在表里的接口，登录即可使用
//! - 审计：登录用户的写操作与导出（含被拒绝的）异步批量写入 `ledger_audit`，读取类请求不记

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::{ConnectInfo, Extension, Query, Request, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use duckdb::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::config::{AuthMode, Config};
use crate::db::now_local;
use crate::error::{AppError, AppResult, Envelope, ok};
use crate::routes::AppState;
use crate::scope::{Scope, ScopeExt};

/// 当前请求的用户
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentUser {
    pub id: String,
    pub name: String,
    pub admin: bool,
    /// 是否开启了认证；false 时（没有登录体系）一切放行
    #[serde(skip)]
    pub enforced: bool,
}

impl CurrentUser {
    /// 未开启认证：不区分用户，权限检查一律通过
    pub fn open() -> Self {
        Self { id: String::new(), name: String::new(), admin: true, enforced: false }
    }
}

/// 没有登录信息的请求（放行的接口里用来判断是否已登录）
#[derive(Clone)]
pub struct Identity(pub Option<CurrentUser>);

/// 对工作簿的权限等级
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Need {
    /// 查看
    View = 1,
    /// 编辑：保存内容、改名、恢复版本
    Edit = 2,
    /// 管理：删除、改共享范围（所有者与管理员）
    Manage = 3,
}

/// 用户对一个对象的权限等级：0 为无权。owner_id 为空的是登录体系启用前就有的对象，只有管理员能管理
pub fn level(user: &CurrentUser, owner_id: Option<&str>, visibility: &str) -> u8 {
    if !user.enforced || user.admin || owner_id.is_some_and(|o| !o.is_empty() && o == user.id) {
        return Need::Manage as u8;
    }
    match visibility {
        "edit" => Need::Edit as u8,
        "view" => Need::View as u8,
        _ => 0,
    }
}

// ── 认证 ──────────────────────────────────────────────

#[derive(Clone)]
enum Cached {
    User(CurrentUser),
    Invalid(String),
}

pub struct Auth {
    pub mode: AuthMode,
    url: Option<String>,
    cache_secs: u64,
    admins: Vec<String>,
    trusted: Vec<IpAddr>,
    client: reqwest::Client,
    cache: Mutex<HashMap<String, (Instant, Cached)>>,
    audit_tx: UnboundedSender<AuditRecord>,
    audit_rx: Mutex<Option<UnboundedReceiver<AuditRecord>>>,
    /// 最近记过「用户目录」的用户：用户 id → (时间, 名字, 是否管理员)，名字和权限没变时 10 分钟内不重复写库
    seen: Mutex<HashMap<String, (Instant, String, bool)>>,
}

impl Auth {
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let (audit_tx, rx) = unbounded_channel();
        let mut http = reqwest::Client::builder().timeout(Duration::from_secs(5));
        if let Some(path) = &config.auth_ca_file {
            let pem = std::fs::read(path).map_err(|e| anyhow::anyhow!("读取 LEDGER_AUTH_CA_FILE 失败（{}）：{e}", path.display()))?;
            let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| anyhow::anyhow!("LEDGER_AUTH_CA_FILE 不是有效的 PEM 证书（{}）：{e}", path.display()))?;
            if certs.is_empty() {
                anyhow::bail!("LEDGER_AUTH_CA_FILE 里没有证书：{}", path.display());
            }
            for c in certs {
                http = http.add_root_certificate(c);
            }
        }
        Ok(Self {
            mode: config.auth_mode,
            url: config.auth_url.clone(),
            cache_secs: config.auth_cache_secs,
            admins: config.admins.clone(),
            trusted: config.trusted_proxies.clone(),
            client: http.build()?,
            cache: Mutex::new(HashMap::new()),
            audit_tx,
            audit_rx: Mutex::new(Some(rx)),
            seen: Mutex::new(HashMap::new()),
        })
    }

    pub fn enabled(&self) -> bool {
        self.mode != AuthMode::Off
    }

    /// 清掉身份缓存（内置账号退出、改密码、停用、改权限后立即生效）
    pub fn clear_cache(&self) {
        if let Ok(mut m) = self.cache.lock() {
            m.clear();
        }
    }

    fn is_admin_listed(&self, id: &str, name: &str) -> bool {
        self.admins.iter().any(|a| a == id || (!name.is_empty() && a == name))
    }

    /// 识别请求的用户；没有身份返回 Unauthorized，身份服务不可用返回 Unavailable
    pub async fn authenticate(&self, headers: &HeaderMap, peer: IpAddr) -> AppResult<CurrentUser> {
        match self.mode {
            AuthMode::Off => Ok(CurrentUser::open()),
            // 内置账号要查会话表，由中间件调用 local_user
            AuthMode::Local => Err(AppError::Unauthorized("请先登录".into())),
            AuthMode::Proxy => self.from_proxy(headers, peer),
            AuthMode::Gateway => {
                let token = headers.get("x-user-token").and_then(|v| v.to_str().ok()).map(str::trim).unwrap_or("");
                if token.is_empty() {
                    return Err(AppError::Unauthorized("请先登录".into()));
                }
                self.from_gateway(token).await
            }
        }
    }

    fn from_proxy(&self, headers: &HeaderMap, peer: IpAddr) -> AppResult<CurrentUser> {
        let peer = match peer {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(peer),
            v4 => v4,
        };
        if !self.trusted.contains(&peer) {
            return Err(AppError::Unauthorized("请求不是来自可信的反向代理（LEDGER_TRUSTED_PROXIES）".into()));
        }
        let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(|v| percent_decode(v.trim())).unwrap_or_default();
        let id = get("x-auth-user-id");
        if id.is_empty() {
            return Err(AppError::Unauthorized("请先登录".into()));
        }
        let name = Some(get("x-auth-user-name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
        let admin = matches!(get("x-auth-user-admin").to_lowercase().as_str(), "1" | "true" | "yes") || self.is_admin_listed(&id, &name);
        Ok(CurrentUser { id, name, admin, enforced: true })
    }

    async fn from_gateway(&self, token: &str) -> AppResult<CurrentUser> {
        let key = format!("{:x}", Sha256::digest(token.as_bytes()));
        if let Some((at, c)) = self.cache.lock().ok().and_then(|m| m.get(&key).cloned())
            && at.elapsed() < Duration::from_secs(if matches!(c, Cached::User(_)) { self.cache_secs } else { 5 })
        {
            return match c {
                Cached::User(u) => Ok(u),
                Cached::Invalid(m) => Err(AppError::Unauthorized(m)),
            };
        }
        let url = self.url.as_deref().ok_or_else(|| AppError::Unavailable("未配置 LEDGER_AUTH_URL".into()))?;
        let resp = self
            .client
            .get(url)
            .header("x-user-token", token)
            .header("x-client-type", "pc")
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| AppError::Unavailable(connect_error(url, &e)))?;
        let status = resp.status();
        if status.is_server_error() {
            return Err(AppError::Unavailable(format!("身份服务返回 {status}")));
        }
        let text = resp.text().await.map_err(|e| AppError::Unavailable(format!("读取身份服务响应失败：{e}")))?;
        let result = parse_gateway_user(status.as_u16(), &text, |id, name| self.is_admin_listed(id, name));
        if let Ok(mut m) = self.cache.lock() {
            if m.len() > 5000 {
                let ttl = Duration::from_secs(self.cache_secs.max(5));
                m.retain(|_, (at, _)| at.elapsed() < ttl);
            }
            m.insert(key, (Instant::now(), match &result {
                Ok(u) => Cached::User(u.clone()),
                Err(msg) => Cached::Invalid(msg.clone()),
            }));
        }
        result.map_err(AppError::Unauthorized)
    }

    /// 该用户需要（重新）写入用户目录吗
    fn needs_touch(&self, u: &CurrentUser) -> bool {
        let Ok(mut m) = self.seen.lock() else { return false };
        match m.get(&u.id) {
            Some((at, name, admin)) if at.elapsed() < Duration::from_secs(600) && *name == u.name && *admin == u.admin => false,
            _ => {
                m.insert(u.id.clone(), (Instant::now(), u.name.clone(), u.admin));
                true
            }
        }
    }

    pub fn audit(&self, rec: AuditRecord) {
        let _ = self.audit_tx.send(rec);
    }
}

/// 连不上身份服务的说明：带上底层原因（reqwest 自己只说「error sending request」），证书问题直接提示怎么办
fn connect_error(url: &str, e: &reqwest::Error) -> String {
    let mut chain = vec![e.to_string()];
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        chain.push(s.to_string());
        src = s.source();
    }
    let detail = chain.last().cloned().unwrap_or_default();
    let mut msg = format!("无法连接身份服务（{}）：{detail}", host_of(url));
    let lower = detail.to_lowercase();
    if lower.contains("certificate") || lower.contains("unknownissuer") || lower.contains("invalid peer") {
        msg.push_str("。网关的 HTTPS 证书不被信任：如果是内部 CA 签发的，请把 CA 根证书（PEM）放到服务器上并设置 LEDGER_AUTH_CA_FILE");
    }
    msg
}

fn host_of(url: &str) -> String {
    url.split("//").nth(1).and_then(|r| r.split('/').next()).unwrap_or(url).to_string()
}

/// 解析网关 /current_user 的响应：`{ code: 200, data: { id, username, accountName, superAdmin, tenantAdmin } }`（也接受不带信封的对象）
pub fn parse_gateway_user(http_status: u16, body: &str, is_listed_admin: impl Fn(&str, &str) -> bool) -> Result<CurrentUser, String> {
    let expired = || "登录已失效，请重新登录".to_string();
    if http_status == 401 || http_status == 403 {
        return Err(expired());
    }
    let v: Value = serde_json::from_str(body).map_err(|_| "身份服务返回的内容无法识别".to_string())?;
    if let Some(code) = v.get("code") {
        let okay = match code {
            Value::Number(n) => matches!(n.as_i64(), Some(200 | 0)),
            Value::String(s) => matches!(s.as_str(), "200" | "0"),
            _ => false,
        };
        if !okay {
            return Err(expired());
        }
    }
    let data = v.get("data").filter(|d| d.is_object()).unwrap_or(&v);
    let text = |k: &str| match data.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    let flag = |k: &str| data.get(k).is_some_and(|x| x.as_bool() == Some(true) || x.as_i64() == Some(1));
    let id = text("id").ok_or_else(expired)?;
    let name = text("accountName").or_else(|| text("username")).or_else(|| text("nickName")).or_else(|| text("name")).unwrap_or_else(|| id.clone());
    let username = text("username").unwrap_or_default();
    let admin = flag("superAdmin") || flag("tenantAdmin") || is_listed_admin(&id, &name) || (!username.is_empty() && is_listed_admin(&id, &username));
    Ok(CurrentUser { id, name, admin, enforced: true })
}

// ── 路由表：权限与审计 ────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum Rule {
    /// 登录即可
    Open,
    /// 仅管理员
    Admin,
    /// 路径参数 {id} 是工作簿（回收站里的也算：用于恢复 / 彻底删除）
    Wb(Need, bool),
    /// 路径参数 {id} 是数据集：改名 / 删除 / 改结构，只有所有者与管理员
    Dataset,
    /// 路径参数 {id} 是 AI 会话：要能查看所属工作簿，且是会话的创建人或管理员
    Thread,
    /// AI 建议：按它所在的会话判断
    Action,
    /// 请求体里的字段是工作簿 id（需要查看权限）
    WbBody(&'static str),
    /// 查询参数里的是工作簿 id（需要查看权限）
    WbQuery(&'static str),
}

use Rule::{Action, Admin, Dataset, Open, Thread, Wb, WbBody, WbQuery};

/// (方法, 路径模板, 权限, 审计名称)。审计名称为空表示读取类请求，不记审计；没有列出的写请求按「方法 路径」记
const ROUTES: &[(&str, &str, Rule, &str)] = &[
    // 流水与台账
    ("POST", "/ledger/imports", Open, "导入流水明细"),
    ("POST", "/ledger/imports/preview", Open, ""),
    ("POST", "/ledger/imports/commit", Open, "导入店铺流水"),
    ("POST", "/ledger/shops", Open, "新增店铺"),
    ("PUT", "/ledger/shops", Open, "修改店铺"),
    ("POST", "/ledger/details/rows", Open, ""),
    ("POST", "/ledger/details/summary", Open, ""),
    ("POST", "/ledger/details/export", Open, "导出流水明细"),
    ("DELETE", "/ledger/data", Admin, "删除店铺流水数据"),
    ("PUT", "/ledger/entity-aliases", Open, "保存主体简称"),
    ("PUT", "/ledger/balances", Open, "保存余额"),
    ("POST", "/ledger/bank/imports", Open, "导入银行流水"),
    ("DELETE", "/ledger/bank/imports", Open, "删除银行流水批次"),
    ("POST", "/ledger/withdrawals/auto-match", Open, "提现自动配对"),
    ("PUT", "/ledger/withdrawals/match", Open, "提现手动配对"),
    ("POST", "/ledger/close", Open, "月结操作"),
    ("PUT", "/ledger/analysis/qianchuan", Open, "录入千川消耗"),
    // 数据集
    ("POST", "/ledger/datasets/import/preview", Open, ""),
    ("POST", "/ledger/datasets/import/preview-url", Open, ""),
    ("POST", "/ledger/datasets/import/commit", Open, "导入数据集"),
    ("POST", "/ledger/datasets/views/preview", Open, ""),
    ("POST", "/ledger/datasets/views", Open, "创建关联视图"),
    ("PUT", "/ledger/datasets/views/{id}", Dataset, "修改关联视图"),
    ("DELETE", "/ledger/datasets/trash", Admin, "清空数据集回收站"),
    ("POST", "/ledger/datasets/input", Open, "新建填报数据集"),
    ("POST", "/ledger/datasets/{id}/writeback", Open, "填报提交"),
    ("POST", "/ledger/writebacks/{id}/undo", Open, "撤销填报提交"),
    ("POST", "/ledger/datasets/{id}/untrash", Dataset, "恢复数据集"),
    ("DELETE", "/ledger/datasets/{id}/purge", Dataset, "彻底删除数据集"),
    ("PUT", "/ledger/datasets/{id}", Dataset, "重命名数据集"),
    ("DELETE", "/ledger/datasets/{id}", Dataset, "删除数据集"),
    ("POST", "/ledger/datasets/{id}/cursor", Open, ""),
    ("POST", "/ledger/datasets/{id}/stats", Open, ""),
    ("POST", "/ledger/datasets/{id}/distinct", Open, ""),
    ("GET", "/ledger/datasets/{id}/export", Open, "导出数据集"),
    ("POST", "/ledger/datasets/{id}/aggregate", Open, ""),
    ("POST", "/ledger/datasets/{id}/pivot", Open, ""),
    ("POST", "/ledger/datasets/{id}/calc/preview", Open, ""),
    ("POST", "/ledger/datasets/{id}/calc", Dataset, "新增计算列"),
    ("PUT", "/ledger/datasets/{id}/calc/{name}", Dataset, "修改计算列"),
    ("DELETE", "/ledger/datasets/{id}/calc/{name}", Dataset, "删除计算列"),
    // 逐笔对账（对账方案是一种数据集：修改走数据集的所有者规则，删除 / 改名沿用数据集的接口）
    ("POST", "/ledger/recons/preview", Open, ""),
    ("POST", "/ledger/recons", Open, "创建对账"),
    ("PUT", "/ledger/recons/{id}", Dataset, "修改对账"),
    ("POST", "/ledger/recons/{id}/auto", Open, "自动勾对"),
    ("POST", "/ledger/recons/{id}/match", Open, "手工勾对"),
    ("POST", "/ledger/recons/{id}/unmatch", Open, "取消勾对"),
    // 工作簿
    ("POST", "/ledger/workbooks", Open, "新建工作簿"),
    ("DELETE", "/ledger/workbooks/trash", Admin, "清空工作簿回收站"),
    ("POST", "/ledger/workbooks/move", Open, "移动工作簿"),
    ("POST", "/ledger/workbooks/ranges", Open, ""),
    ("GET", "/ledger/workbooks/trash", Open, ""),
    ("GET", "/ledger/workbooks/{id}", Wb(Need::View, false), ""),
    ("PUT", "/ledger/workbooks/{id}", Wb(Need::Edit, false), "重命名工作簿"),
    ("DELETE", "/ledger/workbooks/{id}", Wb(Need::Manage, false), "删除工作簿"),
    ("GET", "/ledger/workbooks/{id}/content", Wb(Need::View, false), ""),
    ("PUT", "/ledger/workbooks/{id}/content", Wb(Need::Edit, false), "保存工作簿"),
    ("POST", "/ledger/workbooks/{id}/untrash", Wb(Need::Manage, true), "恢复工作簿"),
    ("DELETE", "/ledger/workbooks/{id}/purge", Wb(Need::Manage, true), "彻底删除工作簿"),
    ("PUT", "/ledger/workbooks/{id}/star", Wb(Need::View, false), ""),
    ("PUT", "/ledger/workbooks/{id}/share", Wb(Need::Manage, false), "修改工作簿共享范围"),
    ("POST", "/ledger/workbooks/{id}/duplicate", Wb(Need::View, false), "复制工作簿"),
    ("GET", "/ledger/workbooks/{id}/versions", Wb(Need::View, false), ""),
    ("GET", "/ledger/workbooks/{id}/versions/{version}/content", Wb(Need::View, false), ""),
    ("POST", "/ledger/workbooks/{id}/restore", Wb(Need::Edit, false), "恢复工作簿历史版本"),
    ("POST", "/ledger/folders", Open, "新建文件夹"),
    ("PUT", "/ledger/folders/{id}", Open, "修改文件夹"),
    ("DELETE", "/ledger/folders/{id}", Open, "删除文件夹"),
    // 数据连接、推送渠道、外部 AI 令牌：含密钥或能连外部系统，只有管理员
    ("POST", "/ledger/connectors", Admin, "新建数据连接"),
    ("POST", "/ledger/connectors/test", Admin, ""),
    ("POST", "/ledger/connectors/preview", Admin, ""),
    ("PUT", "/ledger/connectors/{id}", Admin, "修改数据连接"),
    ("DELETE", "/ledger/connectors/{id}", Admin, "删除数据连接"),
    ("POST", "/ledger/connectors/{id}/run", Admin, "手动同步数据连接"),
    ("POST", "/ledger/report-channels", Admin, "新建推送渠道"),
    ("PUT", "/ledger/report-channels/{id}", Admin, "修改推送渠道"),
    ("DELETE", "/ledger/report-channels/{id}", Admin, "删除推送渠道"),
    ("POST", "/ledger/report-channels/{id}/test", Admin, "测试推送渠道"),
    ("GET", "/ledger/mcp/tokens", Admin, ""),
    ("POST", "/ledger/mcp/tokens", Admin, "创建外部 AI 令牌"),
    ("PUT", "/ledger/mcp/tokens/{id}", Admin, "修改外部 AI 令牌"),
    ("DELETE", "/ledger/mcp/tokens/{id}", Admin, "吊销外部 AI 令牌"),
    ("GET", "/ledger/mcp/logs", Admin, ""),
    ("GET", "/ledger/audit", Admin, ""),
    ("GET", "/ledger/accounts", Admin, ""),
    ("POST", "/ledger/accounts", Admin, "新建账号"),
    ("PUT", "/ledger/accounts/{id}", Admin, "修改账号"),
    ("POST", "/ledger/auth/logout", Open, "退出登录"),
    ("POST", "/ledger/auth/password", Open, "修改密码"),
    ("GET", "/ledger/tokens", Open, ""),
    ("POST", "/ledger/tokens", Open, "创建访问令牌"),
    ("DELETE", "/ledger/tokens/{id}", Open, "吊销访问令牌"),
    ("GET", "/ledger/data-scope", Admin, ""),
    ("PUT", "/ledger/data-scope/rules", Admin, "设置用户数据范围"),
    ("DELETE", "/ledger/data-scope/rules/{id}", Admin, "删除用户数据范围"),
    ("PUT", "/ledger/data-scope/datasets/{id}", Admin, "设置数据集的范围绑定"),
    // 文件审批
    ("GET", "/ledger/users", Open, ""),
    ("GET", "/ledger/workbooks/{id}/comments", Wb(Need::View, false), ""),
    ("POST", "/ledger/workbooks/{id}/comments", Wb(Need::View, false), "添加评论"),
    ("PUT", "/ledger/comments/{id}", Open, "修改评论"),
    ("DELETE", "/ledger/comments/{id}", Open, "删除评论"),
    ("POST", "/ledger/comments/{id}/resolve", Open, "标记评论线程已解决"),
    ("GET", "/ledger/collab/{id}/events", Wb(Need::View, false), ""),
    ("POST", "/ledger/collab/{id}/send", Wb(Need::View, false), ""),
    ("GET", "/ledger/notifications", Open, ""),
    ("POST", "/ledger/notifications/read", Open, ""),
    ("GET", "/ledger/approvals", Open, ""),
    ("POST", "/ledger/approvals", WbBody("workbookId"), "发起文件审批"),
    ("GET", "/ledger/approvals/count", Open, ""),
    ("GET", "/ledger/approvals/{id}", Open, ""),
    ("POST", "/ledger/approvals/{id}/decide", Open, "处理文件审批"),
    ("POST", "/ledger/approvals/{id}/cancel", Open, "撤回文件审批"),
    // 定时推送、模板、SQL
    ("GET", "/ledger/reports/sheets", WbQuery("workbookId"), ""),
    ("POST", "/ledger/reports", WbBody("workbookId"), "新建定时推送"),
    ("POST", "/ledger/reports/preview", WbBody("workbookId"), ""),
    ("PUT", "/ledger/reports/{id}", Open, "修改定时推送"),
    ("DELETE", "/ledger/reports/{id}", Open, "删除定时推送"),
    ("POST", "/ledger/reports/{id}/send", Open, "立即发送推送"),
    ("POST", "/ledger/templates", WbBody("workbookId"), "发布模板"),
    ("PUT", "/ledger/templates/{id}", Open, "修改模板"),
    ("DELETE", "/ledger/templates/{id}", Open, "删除模板"),
    ("POST", "/ledger/templates/{id}/use", Open, "使用模板创建工作簿"),
    ("POST", "/ledger/sql/run", Open, "执行 SQL 查询"),
    ("POST", "/ledger/sql/queries", Open, "保存 SQL 查询"),
    ("PUT", "/ledger/sql/queries/{id}", Open, "修改 SQL 查询"),
    ("DELETE", "/ledger/sql/queries/{id}", Open, "删除 SQL 查询"),
    ("POST", "/ledger/sql/queries/{id}/refresh", Open, "重算 SQL 查询"),
    ("POST", "/ledger/sql/queries/{id}/instances", Open, "生成参数化查询的实例"),
    // AI 助手
    ("GET", "/ledger/ai/threads", WbQuery("workbookId"), ""),
    ("POST", "/ledger/ai/threads", WbBody("workbookId"), "新建 AI 会话"),
    ("GET", "/ledger/ai/threads/{id}", Thread, ""),
    ("DELETE", "/ledger/ai/threads/{id}", Thread, "删除 AI 会话"),
    ("PUT", "/ledger/ai/threads/{id}/summary", Thread, "编辑 AI 会话摘要"),
    ("POST", "/ledger/ai/threads/{id}/chat", Thread, "AI 提问"),
    ("GET", "/ledger/ai/threads/{id}/events", Thread, ""),
    ("POST", "/ledger/ai/threads/{id}/stop", Thread, ""),
    ("POST", "/ledger/ai/actions/{id}", Action, "处理 AI 建议"),
];

/// 不需要登录的接口：存活检查、运维（自带令牌）、外部 AI 接入（自带令牌）、当前用户（返回是否已登录）
fn is_public(path: &str) -> bool {
    path == "/health" || path == "/mcp" || path.starts_with("/admin/") || path == "/ledger/me" || path == "/ledger/auth/login"
}

/// 内置账号：按请求头 x-user-token 查会话（结果缓存 30 秒；账号变动时 clear_cache 立即作废）
async fn local_user(db: &AppState, headers: &HeaderMap) -> AppResult<CurrentUser> {
    let token = crate::account::token_of(headers);
    if token.is_empty() {
        return Err(AppError::Unauthorized("请先登录".into()));
    }
    let auth = &db.auth;
    let key = crate::account::token_hash(&token);
    if let Some((at, c)) = auth.cache.lock().ok().and_then(|m| m.get(&key).cloned())
        && at.elapsed() < Duration::from_secs(if matches!(c, Cached::User(_)) { 30 } else { 5 })
    {
        return match c {
            Cached::User(u) => Ok(u),
            Cached::Invalid(m) => Err(AppError::Unauthorized(m)),
        };
    }
    let found = run(db, move |c| if crate::pat::is_pat(&token) { crate::pat::user_for(c, &token) } else { crate::account::session_user(c, &token) }).await?;
    let result = match found {
        Some(mut u) => {
            u.admin = u.admin || auth.is_admin_listed(&u.id, &u.name);
            Ok(u)
        }
        None => Err("登录已失效，请重新登录".to_string()),
    };
    if let Ok(mut m) = auth.cache.lock() {
        if m.len() > 5000 {
            m.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(30));
        }
        m.insert(key, (Instant::now(), match &result {
            Ok(u) => Cached::User(u.clone()),
            Err(msg) => Cached::Invalid(msg.clone()),
        }));
    }
    result.map_err(AppError::Unauthorized)
}

/// 登录尝试写进操作审计（成功与失败都记，便于发现暴力尝试）
pub(crate) fn audit_login(db: &AppState, headers: &HeaderMap, peer: IpAddr, username: &str, status: u16, started: Instant) {
    let who = CurrentUser { id: username.to_string(), name: username.to_string(), admin: false, enforced: true };
    record(db, &who, &client_ip(headers, peer), &Method::POST, "/ledger/auth/login", if status < 400 { "登录" } else { "登录失败" }, None, None, status, started);
}

fn match_template(template: &str, path: &str) -> Option<Vec<(String, String)>> {
    let t: Vec<&str> = template.trim_matches('/').split('/').collect();
    let p: Vec<&str> = path.trim_matches('/').split('/').collect();
    if t.len() != p.len() {
        return None;
    }
    let mut params = Vec::new();
    for (ts, ps) in t.iter().zip(&p) {
        if let Some(name) = ts.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            params.push((name.to_string(), percent_decode(ps)));
        } else if ts != ps {
            return None;
        }
    }
    Some(params)
}

fn find_route(method: &Method, path: &str) -> Option<(&'static (&'static str, &'static str, Rule, &'static str), Vec<(String, String)>)> {
    ROUTES.iter().find_map(|r| if r.0 == method.as_str() { match_template(r.1, path).map(|p| (r, p)) } else { None })
}

/// 去掉 /api 或 /ledger-api/api 前缀（嵌套路由里看到的通常已经去掉）
fn normalize(path: &str) -> &str {
    let p = path.strip_prefix("/ledger-api").unwrap_or(path);
    p.strip_prefix("/api").unwrap_or(p)
}

// ── 请求改写 ──────────────────────────────────────────

pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
            out.push(v);
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 把查询参数里的 operator 换成登录用户名
pub fn rewrite_query(query: Option<&str>, name: &str) -> String {
    let mut parts: Vec<String> = query.unwrap_or("").split('&').filter(|p| !p.is_empty() && p.split('=').next() != Some("operator")).map(String::from).collect();
    parts.push(format!("operator={}", percent_encode(name)));
    parts.join("&")
}

/// 把 JSON 对象的顶层 operator 字段设为登录用户名；不是对象 / 解析失败返回 None（原样放行）
pub fn rewrite_json(body: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut v: Value = serde_json::from_slice(body).ok()?;
    v.as_object_mut()?.insert("operator".into(), Value::String(name.to_string()));
    serde_json::to_vec(&v).ok()
}

fn json_field(body: &[u8], key: &str) -> Option<String> {
    serde_json::from_slice::<Value>(body).ok()?.get(key)?.as_str().map(String::from)
}

fn query_param(uri: &Uri, key: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|p| p.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| percent_decode(v)))
}

// ── 中间件 ────────────────────────────────────────────

/// JSON 请求体读取上限（填报提交等可能较大）
const MAX_JSON_BODY: usize = 64 * 1024 * 1024;

struct Target {
    kind: &'static str,
    id: String,
    name: String,
}

pub async fn guard(State(db): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
    let auth = &db.auth;
    if !auth.enabled() {
        req.extensions_mut().insert(CurrentUser::open());
        req.extensions_mut().insert(Identity(None));
        req.extensions_mut().insert(ScopeExt(Arc::new(Scope::all())));
        return next.run(req).await;
    }
    if req.method() == Method::OPTIONS {
        return next.run(req).await;
    }
    // 防跨站请求伪造：会话 cookie 登录的写请求必须带 x-requested-with（/mcp、/admin 用自己的令牌，不看 cookie）
    let raw_path = normalize(req.uri().path());
    if auth.mode == AuthMode::Local && raw_path != "/mcp" && !raw_path.starts_with("/admin/") && crate::account::csrf_blocked(req.method(), req.headers()) {
        return AppError::Forbidden("缺少 x-requested-with 请求头：用会话 cookie 登录时，写操作必须带上它（防跨站请求伪造）；脚本调用请改用 x-user-token 请求头".into()).into_response();
    }
    let started = Instant::now();
    let method = req.method().clone();
    let path = normalize(req.uri().path()).to_string();

    let user = if auth.mode == AuthMode::Local { local_user(&db, req.headers()).await } else { auth.authenticate(req.headers(), peer.ip()).await };
    if is_public(&path) {
        // 身份服务连不上时，/me 如实报错（否则前端会误以为没登录而反复跳转登录页）
        if let Err(e @ AppError::Unavailable(_)) = &user
            && path == "/ledger/me"
        {
            return AppError::Unavailable(e.message()).into_response();
        }
        // 前端每次进页面都会先调 /ledger/me：在这里登记用户，审批人列表里才能选到刚登录的同事
        let mut scope = Arc::new(Scope::all());
        if let Ok(u) = &user {
            remember_user(&db, u);
            match resolve_scope(&db, u).await {
                Ok(s) => scope = s,
                Err(e) => return e.into_response(),
            }
        }
        req.extensions_mut().insert(Identity(user.ok()));
        req.extensions_mut().insert(ScopeExt(scope));
        return next.run(req).await;
    }
    let user = match user {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let ip = client_ip(req.headers(), peer.ip());
    remember_user(&db, &user);
    let scope = match resolve_scope(&db, &user).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    let route = find_route(&method, &path);
    let (rule, label, params) = match route {
        Some((r, ref p)) => (r.2, r.3, p.clone()),
        None => (Open, "", Vec::new()),
    };
    let mut label = label.to_string();
    if route.is_none() && !matches!(method, Method::GET | Method::HEAD) {
        label = format!("{method} {path}");
    }
    let id_param = params.iter().find(|(k, _)| k == "id").map(|(_, v)| v.clone());

    // 请求体：JSON 的先读出来（改写 operator、取工作簿 id）
    let wants_body = !matches!(method, Method::GET | Method::HEAD) && req.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("json"));
    let mut body_bytes: Option<axum::body::Bytes> = None;
    if wants_body {
        let (parts, body) = req.into_parts();
        match axum::body::to_bytes(body, MAX_JSON_BODY).await {
            Ok(b) => {
                body_bytes = Some(b.clone());
                req = Request::from_parts(parts, Body::from(b));
            }
            Err(_) => return AppError::bad("请求内容过大").into_response(),
        }
    }

    // 数据范围受限的用户：只能调用 RESTRICTED_OK 里登记的接口；会绕过过滤的操作（导入、建视图、定时推送、台账明细…）和新增的未登记接口一律不可用
    if scope.restricted && !restricted_ok(&method, &path) {
        let e = AppError::Forbidden(format!("这项操作需要完整的数据权限。{}", crate::scope::restriction_note(&scope).unwrap_or_default()));
        record(&db, &user, &ip, &method, &path, &label, None, id_param, e.status(), started);
        return e.into_response();
    }

    // 权限检查
    let uri = req.uri().clone();
    let checked = check_rule(&db, rule, &params, &user, &uri, body_bytes.as_deref()).await;
    let target = match checked {
        Ok(t) => t,
        Err(e) => {
            let status = e.status();
            record(&db, &user, &ip, &method, &path, &label, None, id_param, status, started);
            return e.into_response();
        }
    };

    // 用登录人覆盖 operator
    if !matches!(method, Method::GET | Method::HEAD) {
        let (mut parts, body) = req.into_parts();
        let q = rewrite_query(parts.uri.query(), &user.name);
        if let Ok(uri) = format!("{}?{q}", parts.uri.path()).parse::<Uri>() {
            parts.uri = uri;
        }
        let mut body = body;
        if let Some(b) = &body_bytes
            && let Some(new) = rewrite_json(b, &user.name)
        {
            parts.headers.remove(axum::http::header::CONTENT_LENGTH);
            body = Body::from(new);
        }
        req = Request::from_parts(parts, body);
    }

    req.extensions_mut().insert(user.clone());
    req.extensions_mut().insert(Identity(Some(user.clone())));
    req.extensions_mut().insert(ScopeExt(scope.clone()));
    // 整个请求（含处理函数里的数据库操作）在用户的数据范围内执行
    let resp = crate::scope::CURRENT.scope(scope, next.run(req)).await;
    record(&db, &user, &ip, &method, &path, &label, target, id_param, resp.status().as_u16(), started);
    resp
}

/// 用户的数据范围（管理员与没有规则的用户不受限）。规则读不出来时拒绝请求，而不是当作不受限
async fn resolve_scope(db: &AppState, user: &CurrentUser) -> AppResult<Arc<Scope>> {
    if !user.enforced || user.admin {
        return Ok(Arc::new(Scope::all()));
    }
    let (db2, u) = (db.clone(), user.clone());
    Ok(Arc::new(run(db, move |c| db2.scope.resolve(c, &u)).await?))
}

/// 数据范围受限的用户可以调用的接口（方法，路径模板）。**白名单**：不在表里的接口一律拒绝，
/// 所以新增接口默认对受限用户关闭，要开放必须在这里登记（`routes_are_all_classified` 测试会提醒）。
/// 只放确认过「读写都经过数据范围过滤」的接口；会绕过过滤读出 / 写入全部数据的功能放进下面的 `FULL_ONLY`。
/// 仅管理员的接口（ROUTES 里是 Admin）不用列：管理员不受数据范围限制，也就不存在「受限的管理员」
const RESTRICTED_OK: &[(&str, &str)] = &[
    ("GET", "/ledger/shops"),
    ("GET", "/ledger/periods"),
    ("GET", "/ledger/close"), ("POST", "/ledger/close"), ("GET", "/ledger/close/logs"), ("GET", "/ledger/close/locks"),
    ("GET", "/ledger/datasets"), ("POST", "/ledger/datasets/{id}/writeback"), ("GET", "/ledger/datasets/{id}/anomalies"), ("POST", "/ledger/datasets/{id}/cursor"), ("GET", "/ledger/datasets/{id}/rows"), ("POST", "/ledger/datasets/{id}/stats"), ("POST", "/ledger/datasets/{id}/distinct"), ("GET", "/ledger/datasets/{id}/export"), ("POST", "/ledger/datasets/{id}/aggregate"), ("POST", "/ledger/datasets/{id}/pivot"), ("POST", "/ledger/datasets/{id}/calc/preview"),
    ("POST", "/ledger/recons/preview"), ("POST", "/ledger/recons"), ("GET", "/ledger/recons/{id}"), ("PUT", "/ledger/recons/{id}"), ("GET", "/ledger/recons/{id}/summary"), ("GET", "/ledger/recons/{id}/items"), ("GET", "/ledger/recons/{id}/groups"), ("POST", "/ledger/recons/{id}/auto"), ("POST", "/ledger/recons/{id}/match"), ("POST", "/ledger/recons/{id}/unmatch"),
    ("GET", "/ledger/workbooks"), ("POST", "/ledger/workbooks"), ("GET", "/ledger/workbooks/trash"), ("POST", "/ledger/workbooks/move"), ("POST", "/ledger/workbooks/ranges"), ("GET", "/ledger/workbooks/{id}"), ("PUT", "/ledger/workbooks/{id}"), ("DELETE", "/ledger/workbooks/{id}"), ("POST", "/ledger/workbooks/{id}/untrash"), ("DELETE", "/ledger/workbooks/{id}/purge"), ("PUT", "/ledger/workbooks/{id}/star"), ("GET", "/ledger/workbooks/{id}/content"), ("PUT", "/ledger/workbooks/{id}/content"), ("POST", "/ledger/workbooks/{id}/duplicate"), ("GET", "/ledger/workbooks/{id}/versions"), ("GET", "/ledger/workbooks/{id}/versions/{version}/content"), ("POST", "/ledger/workbooks/{id}/restore"), ("GET", "/ledger/workbooks/{id}/comments"), ("POST", "/ledger/workbooks/{id}/comments"), ("PUT", "/ledger/workbooks/{id}/share"),
    ("GET", "/ledger/folders"), ("POST", "/ledger/folders"), ("PUT", "/ledger/folders/{id}"), ("DELETE", "/ledger/folders/{id}"),
    ("GET", "/ledger/reports/sheets"),
    ("POST", "/ledger/sql/run"),
    ("GET", "/ledger/ai/status"), ("GET", "/ledger/ai/threads"), ("POST", "/ledger/ai/threads"), ("GET", "/ledger/ai/threads/{id}"), ("DELETE", "/ledger/ai/threads/{id}"), ("PUT", "/ledger/ai/threads/{id}/summary"), ("POST", "/ledger/ai/threads/{id}/chat"), ("GET", "/ledger/ai/threads/{id}/events"), ("POST", "/ledger/ai/threads/{id}/stop"), ("POST", "/ledger/ai/actions/{id}"),
    ("GET", "/ledger/templates"), ("POST", "/ledger/templates/{id}/use"), ("GET", "/ledger/templates/{id}/uses"),
    ("POST", "/ledger/auth/logout"), ("POST", "/ledger/auth/password"),
    ("GET", "/ledger/tokens"), ("POST", "/ledger/tokens"), ("DELETE", "/ledger/tokens/{id}"),
    ("GET", "/ledger/users"),
    ("PUT", "/ledger/comments/{id}"), ("DELETE", "/ledger/comments/{id}"), ("POST", "/ledger/comments/{id}/resolve"),
    ("GET", "/ledger/collab/{id}/events"), ("POST", "/ledger/collab/{id}/send"),
    ("GET", "/ledger/notifications"), ("POST", "/ledger/notifications/read"),
    ("GET", "/ledger/approvals"), ("POST", "/ledger/approvals"), ("GET", "/ledger/approvals/count"), ("GET", "/ledger/approvals/{id}"), ("POST", "/ledger/approvals/{id}/decide"), ("POST", "/ledger/approvals/{id}/cancel"),
];

/// 数据范围受限的用户**明确不能**做的操作：会绕过过滤写入 / 读出全部数据的功能（方法，路径模板）。
/// 运行时不靠它拒绝（默认拒绝，见 `RESTRICTED_OK`），它记录「评估过、确认要关闭」的接口，测试要求每个接口二选一
#[cfg_attr(not(test), allow(dead_code))] // 只供测试核对分类，运行时靠 RESTRICTED_OK 默认拒绝
const FULL_ONLY: &[(&str, &str)] = &[
    // 台账明细与月结以外的台账功能
    ("GET", "/ledger/imports"), ("POST", "/ledger/imports"), ("POST", "/ledger/imports/preview"), ("POST", "/ledger/imports/commit"),
    ("POST", "/ledger/shops"), ("PUT", "/ledger/shops"),
    ("GET", "/ledger/coverage"), ("GET", "/ledger/pivot"),
    ("POST", "/ledger/details/rows"), ("POST", "/ledger/details/summary"), ("POST", "/ledger/details/export"),
    ("GET", "/ledger/entity-aliases"), ("PUT", "/ledger/entity-aliases"),
    ("GET", "/ledger/balances"), ("PUT", "/ledger/balances"),
    ("GET", "/ledger/bank/imports"), ("POST", "/ledger/bank/imports"), ("DELETE", "/ledger/bank/imports"),
    ("GET", "/ledger/withdrawals"), ("GET", "/ledger/withdrawals/candidates"), ("POST", "/ledger/withdrawals/auto-match"), ("PUT", "/ledger/withdrawals/match"),
    ("GET", "/ledger/analysis/breakdown"), ("GET", "/ledger/analysis/cross-period"), ("GET", "/ledger/analysis/qianchuan"), ("PUT", "/ledger/analysis/qianchuan"),
    // 数据集：导入、建视图、新建填报数据集、改结构、回收站、使用情况（填报本身对受限用户开放，见 writeback::submit）
    ("POST", "/ledger/datasets/import/preview"), ("POST", "/ledger/datasets/import/preview-url"), ("POST", "/ledger/datasets/import/commit"),
    ("POST", "/ledger/datasets/views/preview"), ("POST", "/ledger/datasets/views"), ("POST", "/ledger/datasets/summaries"), ("PUT", "/ledger/datasets/views/{id}"),
    ("POST", "/ledger/datasets/input"), ("GET", "/ledger/writebacks"), ("POST", "/ledger/writebacks/{id}/undo"),
    ("GET", "/ledger/datasets/trash"), ("DELETE", "/ledger/datasets/trash"), ("POST", "/ledger/datasets/{id}/untrash"), ("DELETE", "/ledger/datasets/{id}/purge"),
    ("PUT", "/ledger/datasets/{id}"), ("DELETE", "/ledger/datasets/{id}"),
    ("POST", "/ledger/datasets/{id}/calc"), ("PUT", "/ledger/datasets/{id}/calc/{name}"), ("DELETE", "/ledger/datasets/{id}/calc/{name}"),
    ("GET", "/ledger/datasets/{id}/usage"),
    // 保存的 SQL、定时推送、模板发布、数据连接、数据范围设置：按服务账号运行或会列出别人的配置
    ("GET", "/ledger/sql/queries"), ("POST", "/ledger/sql/queries"), ("PUT", "/ledger/sql/queries/{id}"), ("DELETE", "/ledger/sql/queries/{id}"), ("POST", "/ledger/sql/queries/{id}/refresh"), ("POST", "/ledger/sql/queries/{id}/instances"),
    ("GET", "/ledger/reports"), ("POST", "/ledger/reports"), ("POST", "/ledger/reports/preview"), ("PUT", "/ledger/reports/{id}"), ("DELETE", "/ledger/reports/{id}"),
    ("POST", "/ledger/reports/{id}/send"), ("GET", "/ledger/reports/{id}/runs"),
    ("POST", "/ledger/templates"), ("PUT", "/ledger/templates/{id}"), ("DELETE", "/ledger/templates/{id}"),
    ("GET", "/ledger/connectors"), ("GET", "/ledger/connectors/{id}/runs"),
    ("GET", "/ledger/data-scope"),
];

/// 受限用户能不能调用：只有登记在 `RESTRICTED_OK` 里的才行（HEAD 按 GET 算）
fn restricted_ok(method: &Method, path: &str) -> bool {
    let m = if method == Method::HEAD { "GET" } else { method.as_str() };
    RESTRICTED_OK.iter().any(|(rm, t)| *rm == m && match_template(t, path).is_some())
}

/// 把登录用户登记到用户目录（最多每 10 分钟写一次，名字或权限变了立即更新）
fn remember_user(db: &AppState, user: &CurrentUser) {
    if db.auth.needs_touch(user) {
        let (db2, u) = (db.clone(), user.clone());
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db2.connection().map_err(AppError::from).and_then(|c| crate::approval::touch_user(&c, &u)) {
                tracing::warn!("记录用户目录失败：{}", e.message());
            }
        });
    }
}

fn client_ip(headers: &HeaderMap, peer: IpAddr) -> String {
    if peer.is_loopback()
        && let Some(f) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next())
    {
        return f.trim().to_string();
    }
    peer.to_string()
}

#[allow(clippy::too_many_arguments)]
fn record(db: &AppState, user: &CurrentUser, ip: &str, method: &Method, path: &str, label: &str, target: Option<Target>, id_param: Option<String>, status: u16, started: Instant) {
    if label.is_empty() {
        return;
    }
    let (kind, id, name) = match target {
        Some(t) => (t.kind.to_string(), t.id, t.name),
        None => (String::new(), id_param.unwrap_or_default(), String::new()),
    };
    db.auth.audit(AuditRecord {
        at: now_local(),
        user_id: user.id.clone(),
        user_name: user.name.clone(),
        ip: ip.to_string(),
        method: method.to_string(),
        path: path.to_string(),
        action: label.to_string(),
        target_kind: kind,
        target_id: id,
        target_name: name,
        status,
        ms: started.elapsed().as_millis() as u64,
    });
}

async fn check_rule(db: &AppState, rule: Rule, params: &[(String, String)], user: &CurrentUser, uri: &Uri, body: Option<&[u8]>) -> Result<Option<Target>, AppError> {
    let id = params.iter().find(|(k, _)| k == "id").map(|(_, v)| v.clone());
    match rule {
        Rule::Open => Ok(None),
        Admin => {
            if user.admin {
                Ok(None)
            } else {
                Err(AppError::Forbidden("该操作仅限管理员".into()))
            }
        }
        Wb(need, trashed_ok) => {
            let id = id.unwrap_or_default();
            let u = user.clone();
            let id2 = id.clone();
            let name = run(db, move |c| crate::workbook::require(c, &id2, &u, need, trashed_ok)).await?;
            Ok(Some(Target { kind: "工作簿", id, name }))
        }
        Rule::WbBody(key) | Rule::WbQuery(key) => {
            let wb = if matches!(rule, Rule::WbBody(_)) { body.and_then(|b| json_field(b, key)) } else { query_param(uri, key) };
            let Some(wb) = wb.filter(|s| !s.is_empty()) else { return Ok(None) };
            let u = user.clone();
            let wb2 = wb.clone();
            let name = run(db, move |c| crate::workbook::require(c, &wb2, &u, Need::View, false)).await?;
            Ok(Some(Target { kind: "工作簿", id: wb, name }))
        }
        Dataset => {
            let id = id.unwrap_or_default();
            let u = user.clone();
            let id2 = id.clone();
            let name = run(db, move |c| crate::dataset::require_manage(c, &id2, &u)).await?;
            Ok(Some(Target { kind: "数据集", id, name }))
        }
        Thread => {
            let id = id.unwrap_or_default();
            let u = user.clone();
            run(db, move |c| crate::ai::require_thread(c, &id, &u)).await?;
            Ok(None)
        }
        Action => {
            let id = id.unwrap_or_default();
            let u = user.clone();
            run(db, move |c| crate::ai::require_action(c, &id, &u)).await?;
            Ok(None)
        }
    }
}

pub(crate) async fn run<T: Send + 'static>(db: &AppState, f: impl FnOnce(&Connection) -> AppResult<T> + Send + 'static) -> AppResult<T> {
    let db = db.clone();
    let scope = crate::scope::current_task_scope();
    tokio::task::spawn_blocking(move || {
        let conn = db.connection()?;
        crate::scope::run_with(scope, || f(&conn))
    })
    .await?
}

// ── 当前用户 ──────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    /// 服务端是否要求登录
    auth_required: bool,
    mode: &'static str,
    user: Option<CurrentUser>,
    /// 数据范围：受限时只能看到这些店铺的数据（见 scope.rs）
    scope: Scope,
    /// 启用了哪些场景包（前端据此显示 / 隐藏对应入口）
    packs: Packs,
}

#[derive(Serialize)]
pub struct Packs {
    ecommerce: bool,
}

/// GET /api/ledger/me  是否要求登录与当前用户（没登录时 user 为空，由前端跳转登录页）
pub async fn me(State(db): State<AppState>, Extension(id): Extension<Identity>, scope: Option<Extension<ScopeExt>>) -> AppResult<Json<Envelope<Me>>> {
    let mode = match db.auth.mode {
        AuthMode::Off => "off",
        AuthMode::Local => "local",
        AuthMode::Gateway => "gateway",
        AuthMode::Proxy => "proxy",
    };
    let scope = scope.map_or_else(Scope::all, |Extension(s)| (*s.0).clone());
    Ok(ok(Me { auth_required: db.auth.enabled(), mode, user: id.0, scope, packs: Packs { ecommerce: crate::ecommerce::enabled() } }))
}

// ── 审计 ──────────────────────────────────────────────

pub struct AuditRecord {
    at: String,
    user_id: String,
    user_name: String,
    ip: String,
    method: String,
    path: String,
    action: String,
    target_kind: String,
    target_id: String,
    target_name: String,
    status: u16,
    ms: u64,
}

fn write_batch(conn: &Connection, batch: &[AuditRecord]) -> AppResult<()> {
    conn.execute_batch("BEGIN")?;
    let r = (|| -> AppResult<()> {
        let mut stmt = conn.prepare(
            "INSERT INTO ledger_audit (happened_at, user_id, user_name, ip, method, path, action, target_kind, target_id, target_name, status, ms)
             VALUES (CAST(? AS TIMESTAMP), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
        for r in batch {
            stmt.execute(duckdb::params![r.at, r.user_id, r.user_name, r.ip, r.method, r.path, r.action, r.target_kind, r.target_id, r.target_name, r.status as i32, r.ms as i64])?;
        }
        Ok(())
    })();
    match r {
        Ok(()) => Ok(conn.execute_batch("COMMIT")?),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 后台任务：把审计记录攒一批（最多等 2 秒）再写库；每天清理一次过期记录
pub async fn run_audit_writer(db: AppState) {
    let Some(mut rx) = db.auth.audit_rx.lock().ok().and_then(|mut r| r.take()) else { return };
    let mut purge = tokio::time::interval(Duration::from_secs(24 * 3600));
    loop {
        tokio::select! {
            first = rx.recv() => {
                let Some(first) = first else { break };
                let mut batch = vec![first];
                let deadline = tokio::time::sleep(Duration::from_secs(2));
                tokio::pin!(deadline);
                while batch.len() < 500 {
                    tokio::select! {
                        more = rx.recv() => match more { Some(r) => batch.push(r), None => break },
                        () = &mut deadline => break,
                    }
                }
                flush(&db, batch).await;
            }
            _ = purge.tick() => {
                let days = db.config.audit_days;
                let _ = run(&db, move |c| {
                    c.execute("DELETE FROM ledger_audit WHERE happened_at < CAST(? AS TIMESTAMP) - to_days(CAST(? AS INTEGER))", duckdb::params![now_local(), days])?;
                    Ok(())
                })
                .await;
            }
        }
    }
}

async fn flush(db: &AppState, batch: Vec<AuditRecord>) {
    let n = batch.len();
    if let Err(e) = run(db, move |c| write_batch(c, &batch)).await {
        tracing::warn!("操作审计写入失败（丢弃 {n} 条）：{}", e.message());
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditQuery {
    /// 用户名或用户 id 包含
    pub user: Option<String>,
    /// 操作名称包含
    pub action: Option<String>,
    /// 起止日期（含），YYYY-MM-DD
    pub from: Option<String>,
    pub to: Option<String>,
    /// 只看失败（状态码 >= 400）的
    pub failed: Option<bool>,
    pub page: Option<i64>,
    pub size: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditItem {
    at: String,
    user_id: String,
    user_name: String,
    ip: String,
    action: String,
    target_kind: String,
    target_id: String,
    target_name: String,
    status: i32,
    ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditPage {
    total: i64,
    items: Vec<AuditItem>,
    /// 保留天数
    days: i64,
}

pub fn audit_list(conn: &Connection, q: &AuditQuery, days: i64) -> AppResult<AuditPage> {
    let mut wheres = vec!["1 = 1".to_string()];
    let mut args: Vec<String> = Vec::new();
    if let Some(u) = q.user.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        wheres.push("(user_name ILIKE ? OR user_id = ?)".into());
        args.push(format!("%{u}%"));
        args.push(u.to_string());
    }
    if let Some(a) = q.action.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        wheres.push("action ILIKE ?".into());
        args.push(format!("%{a}%"));
    }
    if let Some(f) = q.from.as_deref().filter(|s| !s.is_empty()) {
        wheres.push("happened_at >= CAST(? AS TIMESTAMP)".into());
        args.push(format!("{f} 00:00:00"));
    }
    if let Some(t) = q.to.as_deref().filter(|s| !s.is_empty()) {
        wheres.push("happened_at <= CAST(? AS TIMESTAMP)".into());
        args.push(format!("{t} 23:59:59"));
    }
    if q.failed == Some(true) {
        wheres.push("status >= 400".into());
    }
    let w = wheres.join(" AND ");
    let total: i64 = conn.query_row(&format!("SELECT count(*) FROM ledger_audit WHERE {w}"), params_from_iter(args.iter()), |r| r.get(0))?;
    let size = q.size.unwrap_or(50).clamp(1, 200);
    let offset = (q.page.unwrap_or(1).max(1) - 1) * size;
    let mut stmt = conn.prepare(&format!(
        "SELECT strftime(happened_at, '%Y-%m-%d %H:%M:%S'), COALESCE(user_id,''), COALESCE(user_name,''), COALESCE(ip,''), COALESCE(action,''), COALESCE(target_kind,''), COALESCE(target_id,''), COALESCE(target_name,''), COALESCE(status,0), COALESCE(ms,0)
         FROM ledger_audit WHERE {w} ORDER BY happened_at DESC LIMIT {size} OFFSET {offset}"
    ))?;
    let items = stmt
        .query_map(params_from_iter(args.iter()), |r| {
            Ok(AuditItem { at: r.get(0)?, user_id: r.get(1)?, user_name: r.get(2)?, ip: r.get(3)?, action: r.get(4)?, target_kind: r.get(5)?, target_id: r.get(6)?, target_name: r.get(7)?, status: r.get(8)?, ms: r.get(9)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AuditPage { total, items, days })
}

/// GET /api/ledger/audit?user=&action=&from=&to=&failed=&page=&size=  操作审计（仅管理员，由路由表限制）
pub async fn audit(State(db): State<AppState>, Query(q): Query<AuditQuery>) -> AppResult<Json<Envelope<AuditPage>>> {
    let days = db.config.audit_days;
    Ok(ok(run(&db, move |c| audit_list(c, &q, days)).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, admin: bool) -> CurrentUser {
        CurrentUser { id: id.into(), name: format!("用户{id}"), admin, enforced: true }
    }

    #[test]
    fn levels() {
        let (a, b, adm) = (user("a", false), user("b", false), user("x", true));
        assert_eq!(level(&a, Some("a"), "private"), 3);
        assert_eq!(level(&b, Some("a"), "private"), 0);
        assert_eq!(level(&b, Some("a"), "view"), 1);
        assert_eq!(level(&b, Some("a"), "edit"), 2);
        assert_eq!(level(&adm, Some("a"), "private"), 3);
        // 没有所有者的老数据：按共享范围，只有管理员能管理
        assert_eq!(level(&a, None, "edit"), 2);
        assert_eq!(level(&a, None, "private"), 0);
        assert_eq!(level(&CurrentUser::open(), Some("a"), "private"), 3);
    }

    #[test]
    fn templates_match() {
        let p = match_template("/ledger/datasets/{id}/calc/{name}", "/ledger/datasets/ds_1/calc/%E6%AF%9B%E5%88%A9").unwrap();
        assert_eq!(p, vec![("id".to_string(), "ds_1".to_string()), ("name".to_string(), "毛利".to_string())]);
        assert!(match_template("/ledger/datasets/{id}", "/ledger/datasets/ds_1/calc").is_none());
        assert!(match_template("/ledger/workbooks/trash", "/ledger/workbooks/trash").is_some());
    }

    #[test]
    fn static_routes_win_over_params() {
        // /workbooks/trash 与 /workbooks/{id} 同形：必须先匹配到具体的
        let (r, _) = find_route(&Method::DELETE, "/ledger/workbooks/trash").unwrap();
        assert_eq!(r.2, Admin);
        let (r, p) = find_route(&Method::DELETE, "/ledger/workbooks/wb_1").unwrap();
        assert_eq!(r.2, Wb(Need::Manage, false));
        assert_eq!(p[0].1, "wb_1");
        let (r, _) = find_route(&Method::GET, "/ledger/workbooks/trash").unwrap();
        assert_eq!(r.2, Open);
    }

    #[test]
    fn route_table_has_unique_entries() {
        for (i, a) in ROUTES.iter().enumerate() {
            for b in &ROUTES[i + 1..] {
                assert!(!(a.0 == b.0 && a.1 == b.1), "重复的路由：{} {}", a.0, a.1);
            }
        }
    }

    /// 从 main.rs 的路由注册里取出全部（方法，路径模板）
    fn registered_routes() -> Vec<(String, String)> {
        let main = include_str!("main.rs");
        let main_body = &main[main.find(".route(").unwrap()..main.find(".fallback(routes::api_not_found)").unwrap()];
        // 场景包的路由在自己的模块里注册（没启用时不注册），权限分类一样要覆盖
        let pack = include_str!("ecommerce/mod.rs");
        let pack_body = &pack[pack.find(".route(").unwrap()..];
        let body = format!("{main_body}{pack_body}");
        let body = body.as_str();
        let mut out = Vec::new();
        for chunk in body.split(".route(").skip(1) {
            let (path, handlers) = chunk.trim_start().strip_prefix('"').unwrap().split_once('"').unwrap();
            for m in ["get", "post", "put", "delete"] {
                for (i, _) in handlers.match_indices(&format!("{m}(")) {
                    // 排除 xxx_get( 之类的标识符，只认 get( 本身（含 routing::get( 写法）
                    if handlers[..i].chars().last().is_none_or(|c| !(c.is_alphanumeric() || c == '_')) {
                        out.push((m.to_uppercase(), path.to_string()));
                    }
                }
            }
        }
        out
    }

    /// 权限表与限制表里的路径必须是真实注册的路由：写错一个字，对应的限制就会悄悄失效
    #[test]
    fn tables_only_name_registered_routes() {
        let reg = registered_routes();
        assert!(reg.len() > 100, "路由解析异常：只找到 {} 条", reg.len());
        let has = |m: &str, t: &str| reg.iter().any(|(rm, rt)| rm == m && rt == t);
        for (m, t, _, _) in ROUTES {
            assert!(has(m, t), "ROUTES 里的路径没有注册：{m} {t}");
        }
        for (m, t) in FULL_ONLY {
            assert!(has(m, t), "FULL_ONLY 里的路径没有注册：{m} {t}");
        }
        for (m, t) in RESTRICTED_OK {
            assert!(has(m, t), "RESTRICTED_OK 里的路径没有注册：{m} {t}");
        }
    }

    /// 新增接口必须明确它对「数据范围受限的用户」开放还是关闭：
    /// 在 RESTRICTED_OK（开放）、FULL_ONLY（关闭）或 ROUTES 里标为 Admin 三者之一登记，否则这个测试失败。
    /// 运行时未登记的接口本来就默认拒绝，这个测试是提醒开发者别漏掉
    #[test]
    fn routes_are_all_classified() {
        let listed = |tbl: &[(&str, &str)], m: &str, t: &str| tbl.iter().any(|(a, b)| *a == m && *b == t);
        let admin = |m: &str, t: &str| ROUTES.iter().any(|(a, b, r, _)| *a == m && *b == t && *r == Admin);
        let mut missing = Vec::new();
        for (m, t) in registered_routes() {
            if is_public(&t) {
                continue;
            }
            let (ok, full, adm) = (listed(RESTRICTED_OK, &m, &t), listed(FULL_ONLY, &m, &t), admin(&m, &t));
            assert!(!(ok && full), "同时出现在 RESTRICTED_OK 和 FULL_ONLY：{m} {t}");
            assert!(!(ok && adm), "仅管理员的接口不该对受限用户开放：{m} {t}");
            if !(ok || full || adm) {
                missing.push(format!("{m} {t}"));
            }
        }
        assert!(missing.is_empty(), "这些接口还没有决定是否对数据范围受限的用户开放，请登记到 auth.rs 的 RESTRICTED_OK 或 FULL_ONLY：{missing:?}");
    }

    /// 默认拒绝：没登记的接口（包括将来新增的）受限用户调不了
    #[test]
    fn unlisted_routes_are_denied_for_restricted_users() {
        assert!(!restricted_ok(&Method::POST, "/ledger/brand/new"));
        assert!(!restricted_ok(&Method::GET, "/ledger/datasets/ds_1/something-new"));
        // 方法也要对得上：只登记了 GET 的路径，PUT 不行
        assert!(restricted_ok(&Method::GET, "/ledger/datasets/ds_1/rows"));
        assert!(!restricted_ok(&Method::PUT, "/ledger/datasets/ds_1/rows"));
        // HEAD 按 GET 算
        assert!(restricted_ok(&Method::HEAD, "/ledger/datasets/ds_1/rows"));
    }

    #[test]
    fn restricted_users_are_kept_off_bypassing_operations() {
        assert!(!restricted_ok(&Method::POST, "/ledger/datasets/import/commit"));
        assert!(!restricted_ok(&Method::PUT, "/ledger/datasets/ds_1"));
        // 填报：受限用户可以填自己范围内的行（服务端按范围列检查），撤销与提交记录仍需完整权限
        assert!(restricted_ok(&Method::POST, "/ledger/datasets/ds_1/writeback"));
        assert!(!restricted_ok(&Method::POST, "/ledger/writebacks/wb_1/undo"));
        assert!(!restricted_ok(&Method::GET, "/ledger/balances"));
        // 读取数据集、月结、工作簿、审批、AI、SQL 查询不受影响
        assert!(restricted_ok(&Method::POST, "/ledger/datasets/ds_1/cursor"));
        assert!(restricted_ok(&Method::GET, "/ledger/datasets/ds_1/rows"));
        assert!(restricted_ok(&Method::POST, "/ledger/datasets/ds_1/pivot"));
        assert!(restricted_ok(&Method::POST, "/ledger/sql/run"));
        assert!(restricted_ok(&Method::GET, "/ledger/close"));
        assert!(restricted_ok(&Method::POST, "/ledger/close"));
        assert!(restricted_ok(&Method::GET, "/ledger/shops"));
        assert!(restricted_ok(&Method::GET, "/ledger/workbooks"));
        assert!(restricted_ok(&Method::POST, "/ledger/approvals"));
        assert!(restricted_ok(&Method::POST, "/ledger/ai/threads/t1/chat"));
    }

    /// 网关用内部 CA 签发证书时，管理员通过 LEDGER_AUTH_CA_FILE 提供根证书：配错要在启动时就报错，而不是等到所有人登录失败
    #[test]
    fn auth_ca_file_is_validated_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let config = |file: Option<std::path::PathBuf>| {
            let mut c = crate::config::Config::from_env().unwrap();
            c.auth_ca_file = file;
            c
        };
        assert!(Auth::new(&config(None)).is_ok());
        let good = dir.path().join("ca.pem");
        std::fs::write(&good, TEST_CA_PEM).unwrap();
        assert!(Auth::new(&config(Some(good))).is_ok());
        let missing = Auth::new(&config(Some(dir.path().join("nope.pem")))).err().unwrap().to_string();
        assert!(missing.contains("读取 LEDGER_AUTH_CA_FILE 失败"), "{missing}");
        let bad = dir.path().join("bad.pem");
        std::fs::write(&bad, "这不是证书").unwrap();
        let err = Auth::new(&config(Some(bad))).err().unwrap().to_string();
        assert!(err.contains("PEM") || err.contains("里没有证书"), "{err}");
    }

    #[test]
    fn public_paths() {
        assert!(is_public("/health") && is_public("/mcp") && is_public("/admin/status") && is_public("/ledger/me"));
        assert!(!is_public("/ledger/workbooks"));
        assert_eq!(normalize("/ledger-api/api/ledger/me"), "/ledger/me");
        assert_eq!(normalize("/api/health"), "/health");
        assert_eq!(normalize("/ledger/me"), "/ledger/me");
    }

    #[test]
    fn operator_rewrite() {
        assert_eq!(rewrite_query(Some("name=a&operator=%E5%81%87&x=1"), "张 三"), "name=a&x=1&operator=%E5%BC%A0%20%E4%B8%89");
        assert_eq!(rewrite_query(None, "u"), "operator=u");
        let out = rewrite_json(r#"{"a":1,"operator":"假的","n":{"operator":"keep"}}"#.as_bytes(), "真").unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["operator"], "真");
        assert_eq!(v["n"]["operator"], "keep");
        assert_eq!(v["a"], 1);
        assert!(rewrite_json(b"[1,2]", "x").is_none());
        assert!(rewrite_json(b"not json", "x").is_none());
    }

    #[test]
    fn decode_roundtrip() {
        assert_eq!(percent_decode(&percent_encode("张三 a+b&c=d")), "张三 a+b&c=d");
        assert_eq!(percent_decode("a%2"), "a%2");
    }

    #[test]
    fn gateway_user() {
        let none = |_: &str, _: &str| false;
        let u = parse_gateway_user(200, r#"{"code":200,"data":{"id":"u1","username":"zs","accountName":"张三","superAdmin":false,"tenantAdmin":true}}"#, none).unwrap();
        assert_eq!((u.id.as_str(), u.name.as_str(), u.admin), ("u1", "张三", true));
        let u = parse_gateway_user(200, r#"{"code":"200","data":{"id":12,"username":"ls"}}"#, |id, _| id == "12").unwrap();
        assert_eq!((u.id.as_str(), u.name.as_str(), u.admin), ("12", "ls", true));
        // 不带信封
        assert!(!parse_gateway_user(200, r#"{"id":"u2","accountName":"王五"}"#, none).unwrap().admin);
        // 失效
        assert!(parse_gateway_user(200, r#"{"code":401,"msg":"x"}"#, none).is_err());
        assert!(parse_gateway_user(401, "", none).is_err());
        assert!(parse_gateway_user(200, r#"{"code":200,"data":{}}"#, none).is_err());
        assert!(parse_gateway_user(200, "<html>", none).is_err());
    }

    #[test]
    fn audit_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        let rec = |user: &str, action: &str, status: u16| AuditRecord {
            at: "2026-10-07 10:00:00".into(),
            user_id: user.into(),
            user_name: format!("名{user}"),
            ip: "1.2.3.4".into(),
            method: "POST".into(),
            path: "/x".into(),
            action: action.into(),
            target_kind: String::new(),
            target_id: String::new(),
            target_name: String::new(),
            status,
            ms: 3,
        };
        write_batch(&conn, &[rec("a", "删除数据集", 200), rec("b", "保存工作簿", 200), rec("a", "删除工作簿", 403)]).unwrap();
        let q = |user: Option<&str>, failed: Option<bool>| AuditQuery { user: user.map(String::from), action: None, from: None, to: None, failed, page: None, size: None };
        assert_eq!(audit_list(&conn, &q(None, None), 365).unwrap().total, 3);
        assert_eq!(audit_list(&conn, &q(Some("a"), None), 365).unwrap().total, 2);
        assert_eq!(audit_list(&conn, &q(Some("名b"), None), 365).unwrap().items[0].action, "保存工作簿");
        assert_eq!(audit_list(&conn, &q(None, Some(true)), 365).unwrap().items.len(), 1);
        let by_action = AuditQuery { action: Some("删除".into()), ..q(None, None) };
        assert_eq!(audit_list(&conn, &by_action, 365).unwrap().total, 2);
        let by_date = AuditQuery { from: Some("2026-10-08".into()), ..q(None, None) };
        assert_eq!(audit_list(&conn, &by_date, 365).unwrap().total, 0);
    }

    /// 测试用的自签根证书（只用于验证文件能被解析，不代表任何真实的 CA）
    const TEST_CA_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDDTCCAfWgAwIBAgIUAvI23k5UTX7hGq1ClqgdlYkQzg8wDQYJKoZIhvcNAQEL\nBQAwFjEUMBIGA1UEAwwLaW50ZXJuYWwtY2EwHhcNMjYxMDA3MDkxNjU5WhcNMzYx\nMDA0MDkxNjU5WjAWMRQwEgYDVQQDDAtpbnRlcm5hbC1jYTCCASIwDQYJKoZIhvcN\nAQEBBQADggEPADCCAQoCggEBALJnqITsJXKZndcg0kPn2tZiDBl+WVAo/LW7IvBf\nrJXaLoJDk3ko4L2Zg5Qr58/exf5+ZFTpjPiGxF/avlG6aNvhat3UpdJ37AoJh4kx\nhtQU2Ish9Bovb1vFGTGBF3qFjDepVtmrhDJF8Nv9F6WmAlTOjNzwKE2OIqGnWJZD\n6SSV3t/fQeh7U2jUVCQEXz/hDFbf9wZxoCxu5yP9tRJtErFgW/BVcdpJ/+8sE/0M\nkW4NS4FTMebvXSnM/2qUPyb7J5nzt579hIMSRRnaNT2FeTK1CgOa59b9477Fphk6\n6pdGmpTbRl6gc45LqEfoYZc2G05+Y7xfjQW6/PNHYFhwgLECAwEAAaNTMFEwHQYD\nVR0OBBYEFJPR20MeqHHu6fyLXwT8zcOUPIe8MB8GA1UdIwQYMBaAFJPR20MeqHHu\n6fyLXwT8zcOUPIe8MA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZIhvcNAQELBQADggEB\nAD8xmLwgEBwqUpJeskGXM2qSh3Uu9m5M1xIOgt5AsneTtdq+8STuon/fbYkDZ5r8\nGhgNJ4tJfpIKm6rJ+iY0S9WFUMHcA1LYak0CDeNwujpYPsVTofoYjj5ryVrkFCQe\ntd0OenhGpdszyqqcvXVpvmbTBWWccQL01Qq9NqO42cw0bMP6Zm5PAoU5Yupg5EiF\nRfb+Mo9GM5BMXNBR9jngQJZ69d1HxW5fDvMziFKL6A6dom/KgKVAQU81Eqx5/mNZ\nKdhqOjAItzk2S4ypCPOj7EP0bEE8GUsH7hFOhsoFYV1iFE101rLvuPuan6qyLch6\n/9nJ7w1Ej+y6rvj7KcTjrBM=\n-----END CERTIFICATE-----\n";
}
