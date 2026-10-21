//! 抖店资金流水（动账明细）标准列与表结构
//!
//! 导入时按「表头名」把 CSV 对齐到这里的固定列：下个月导出的 CSV 列顺序变化、增删列都不影响入库与汇总

/// 标准列（顺序即明细查询返回顺序）
pub const DETAIL_COLUMNS: &[&str] = &[
    "动账时间",
    "动帐流水号",
    "动账方向",
    "动账金额",
    "动账账户",
    "动账场景",
    "计费类型",
    "子订单号",
    "订单号",
    "售后编号",
    "下单时间",
    "商品ID",
    "商品名称",
    "达人ID",
    "达人名称",
    "订单类型",
    "订单实付应结",
    "运费实付",
    "实际平台补贴_运费",
    "实际平台补贴",
    "其他平台补贴",
    "以旧换新抵扣",
    "政府补贴平台垫资",
    "实际达人补贴",
    "实际抖音支付补贴",
    "实际抖音月付营销补贴",
    "银行补贴",
    "订单退款",
    "平台服务费",
    "佣金",
    "服务商佣金",
    "渠道分成",
    "招商服务费",
    "站外推广费",
    "其他分成",
    "是否免佣",
    "免佣金额",
    "商户主体名称",
    "备注",
];

/// 金额列：以 DECIMAL(18,2) 存储，求和无浮点误差
pub const NUMERIC_COLUMNS: &[&str] = &[
    "动账金额",
    "订单实付应结",
    "运费实付",
    "实际平台补贴_运费",
    "实际平台补贴",
    "其他平台补贴",
    "以旧换新抵扣",
    "政府补贴平台垫资",
    "实际达人补贴",
    "实际抖音支付补贴",
    "实际抖音月付营销补贴",
    "银行补贴",
    "订单退款",
    "平台服务费",
    "佣金",
    "服务商佣金",
    "渠道分成",
    "招商服务费",
    "站外推广费",
    "其他分成",
    "免佣金额",
];

/// 缺少这些列无法汇总或无法确定月份，直接拒绝导入
pub const REQUIRED_COLUMNS: &[&str] = &["动账时间", "动账方向", "动账金额", "动账场景", "商户主体名称"];

/// ID 类列：导出时带前导单引号（'6955…）防止 Excel 转科学计数法，入库时去掉
pub const ID_COLUMNS: &[&str] = &["动帐流水号", "子订单号", "订单号", "售后编号", "商品ID", "达人ID"];

/// 关键字搜索覆盖的字段：入库时拼接成小写的 search_text 列，搜索只需匹配一列（比逐列 ILIKE 快得多）
pub const SEARCH_COLUMNS: &[&str] = &["订单号", "子订单号", "动帐流水号", "售后编号", "商品名称", "达人名称", "备注"];

pub fn search_text_sql() -> String {
    format!("lower(concat_ws(' ', {}))", SEARCH_COLUMNS.iter().map(|c| ident(c)).collect::<Vec<_>>().join(", "))
}

// ── 数据文件的结构版本 ────────────────────────────────

/// 数据文件的结构版本。**规则**：
/// - 只增加表 / 列（`CREATE TABLE IF NOT EXISTS`、`ADD COLUMN IF NOT EXISTS`）不用改它，升级是可重复执行的；
/// - 一旦做了旧版本程序无法兼容的改动（改列含义、搬数据、删表 / 列），把它加 1，并在 CHANGELOG 里写明；
///   （版本 2：数据范围规则从 ledger_data_scope 搬到 ledger_scope_rule——旧版读不到新规则会悄悄放开限制，所以必须拒绝打开）
///   （版本 3：账号可以启用两步验证——旧版不认识它，会让启用了两步验证的账号只凭密码登录，所以必须拒绝打开）
/// - 新版程序写过的数据文件，旧版程序拒绝打开（见 `check_version`），避免旧版悄悄读写出错。回退要用升级前的备份
pub const SCHEMA_VERSION: i64 = 3;

const META_SQL: &str = "CREATE TABLE IF NOT EXISTS ledger_meta (key VARCHAR PRIMARY KEY, value VARCHAR NOT NULL);";

/// 建好元信息表，读出数据文件当前的结构版本；比程序认识的更新时返回错误。没有记录（旧版数据文件）按 0 算
pub fn check_version(conn: &duckdb::Connection) -> anyhow::Result<i64> {
    conn.execute_batch(META_SQL)?;
    let stored: Option<String> = conn.query_row("SELECT value FROM ledger_meta WHERE key = 'schema_version'", [], |r| r.get(0)).ok();
    let v = stored.and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
    if v > SCHEMA_VERSION {
        let by: String = conn.query_row("SELECT value FROM ledger_meta WHERE key = 'app_version'", [], |r| r.get(0)).unwrap_or_else(|_| "未知".into());
        anyhow::bail!(
            "数据文件的结构版本（{v}，由程序 {by} 写入）比当前程序（{SCHEMA_VERSION}）更新，旧版本不能打开它。\n\
             请升级到新版本；如果要回退到旧版本，请用升级前的备份恢复（gridlake restore <备份文件>），不要直接用新版写过的数据文件"
        );
    }
    Ok(v)
}

/// 升级完成后记下结构版本与最近一次打开它的程序版本
pub fn stamp_version(conn: &duckdb::Connection, app_version: &str) -> anyhow::Result<()> {
    conn.execute_batch(META_SQL)?;
    for (k, v) in [("schema_version", SCHEMA_VERSION.to_string()), ("app_version", app_version.to_string())] {
        conn.execute("INSERT INTO ledger_meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value", duckdb::params![k, v])?;
    }
    Ok(())
}

/// 已有库的增量升级（建表语句之后执行，可重复执行）
pub fn migrate_sql() -> String {
    format!(
        "ALTER TABLE ledger_detail ADD COLUMN IF NOT EXISTS search_text VARCHAR;
         UPDATE ledger_detail SET search_text = {} WHERE search_text IS NULL;
         ALTER TABLE ledger_ai_action ADD COLUMN IF NOT EXISTS undo VARCHAR;
         ALTER TABLE ledger_notification ADD COLUMN IF NOT EXISTS ref_id VARCHAR;
         ALTER TABLE ledger_import ADD COLUMN IF NOT EXISTS file_hash VARCHAR;
         ALTER TABLE ledger_import ADD COLUMN IF NOT EXISTS mode VARCHAR;
         ALTER TABLE ledger_import ADD COLUMN IF NOT EXISTS inserted_rows BIGINT;
         ALTER TABLE ledger_import ADD COLUMN IF NOT EXISTS skipped_rows BIGINT;
         ALTER TABLE ledger_import ADD COLUMN IF NOT EXISTS changed_rows BIGINT;
         ALTER TABLE ledger_withdraw_match ADD COLUMN IF NOT EXISTS period VARCHAR;
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS kind VARCHAR DEFAULT 'table';
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS definition VARCHAR;
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMP;
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS deleted_by VARCHAR;
         ALTER TABLE ledger_workbook ADD COLUMN IF NOT EXISTS folder_id VARCHAR;
         ALTER TABLE ledger_workbook ADD COLUMN IF NOT EXISTS starred BOOLEAN DEFAULT false;
         ALTER TABLE ledger_workbook ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMP;
         ALTER TABLE ledger_workbook ADD COLUMN IF NOT EXISTS deleted_by VARCHAR;
         ALTER TABLE ledger_workbook ADD COLUMN IF NOT EXISTS owner_id VARCHAR;
         ALTER TABLE ledger_workbook ADD COLUMN IF NOT EXISTS owner_name VARCHAR;
         ALTER TABLE ledger_workbook ADD COLUMN IF NOT EXISTS visibility VARCHAR;
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS owner_id VARCHAR;
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS owner_name VARCHAR;
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS scope_column VARCHAR;
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS scope_dimension VARCHAR;
         ALTER TABLE ledger_account ADD COLUMN IF NOT EXISTS totp_secret_enc VARCHAR;
         ALTER TABLE ledger_account ADD COLUMN IF NOT EXISTS totp_enabled BOOLEAN DEFAULT false;
         ALTER TABLE ledger_account ADD COLUMN IF NOT EXISTS totp_last_step BIGINT;
         ALTER TABLE ledger_dataset ADD COLUMN IF NOT EXISTS scope_open BOOLEAN;
         INSERT INTO ledger_scope_rule (subject_id, dimension, subject_name, mode, vals, updated_by, updated_at)
           SELECT subject_id, '店铺', subject_name, CASE WHEN mode = 'shops' THEN 'values' ELSE mode END, shops, updated_by, updated_at FROM ledger_data_scope
           ON CONFLICT DO NOTHING;
         DELETE FROM ledger_data_scope;
         ALTER TABLE ledger_period_close ADD COLUMN IF NOT EXISTS submitted_by_id VARCHAR;
         ALTER TABLE ledger_period_close ADD COLUMN IF NOT EXISTS approver_id VARCHAR;
         ALTER TABLE ledger_writeback ADD COLUMN IF NOT EXISTS deleted BIGINT;
         ALTER TABLE ledger_sql_query ADD COLUMN IF NOT EXISTS params VARCHAR;
         ALTER TABLE ledger_sql_query ADD COLUMN IF NOT EXISTS param_values VARCHAR;
         ALTER TABLE ledger_sql_query ADD COLUMN IF NOT EXISTS template_id VARCHAR;
         ALTER TABLE ledger_ai_thread ADD COLUMN IF NOT EXISTS summary VARCHAR;
         ALTER TABLE ledger_ai_thread ADD COLUMN IF NOT EXISTS summary_seq BIGINT;
         ALTER TABLE ledger_ai_thread ADD COLUMN IF NOT EXISTS owner_id VARCHAR;
         ALTER TABLE ledger_collect ADD COLUMN IF NOT EXISTS reviewer_id VARCHAR;
         ALTER TABLE ledger_collect ADD COLUMN IF NOT EXISTS reviewer_name VARCHAR;
         ALTER TABLE ledger_collect ADD COLUMN IF NOT EXISTS remind_channels VARCHAR;
         ALTER TABLE ledger_collect_period ADD COLUMN IF NOT EXISTS deadline VARCHAR;
         INSERT INTO ledger_shop (name)
           SELECT DISTINCT shop FROM ledger_detail d WHERE NOT EXISTS (SELECT 1 FROM ledger_shop s WHERE s.name = d.shop);",
        search_text_sql()
    )
}

pub fn is_numeric(col: &str) -> bool {
    NUMERIC_COLUMNS.contains(&col)
}

/// SQL 标识符加双引号（中文列名）
pub fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// SQL 字符串字面量
pub fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn create_tables_sql() -> String {
    let columns = DETAIL_COLUMNS
        .iter()
        .map(|c| format!("  {} {}", ident(c), if is_numeric(c) { "DECIMAL(18,2)" } else { "VARCHAR" }))
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        r#"
CREATE TABLE IF NOT EXISTS ledger_detail (
  shop VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  import_id VARCHAR NOT NULL,
  row_no BIGINT NOT NULL,
{columns}
);
CREATE TABLE IF NOT EXISTS ledger_import (
  import_id VARCHAR PRIMARY KEY,
  shop VARCHAR NOT NULL,
  file_name VARCHAR,
  periods VARCHAR NOT NULL,
  entities VARCHAR NOT NULL,
  row_count BIGINT NOT NULL,
  replaced_rows BIGINT NOT NULL,
  missing_columns VARCHAR NOT NULL,
  extra_columns VARCHAR NOT NULL,
  elapsed_ms BIGINT NOT NULL,
  imported_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
-- 店铺档案（name 唯一，由程序保证；改名时同步更新明细与导入记录）
CREATE TABLE IF NOT EXISTS ledger_shop (
  name VARCHAR NOT NULL,
  douyin_shop_id VARCHAR,
  status VARCHAR NOT NULL DEFAULT 'active',
  remark VARCHAR,
  created_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
-- 汇总表第 1 行显示的主体简称（用户可修改）；没有记录时前端按规则从全称推导
CREATE TABLE IF NOT EXISTS ledger_entity_alias (
  entity VARCHAR PRIMARY KEY,
  alias VARCHAR NOT NULL,
  updated_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
-- 平台资金账户余额（抖店后台「资金账户」页），用于勾稽：期初 + 入账 - 出账 = 期末
CREATE TABLE IF NOT EXISTS ledger_balance (
  shop VARCHAR NOT NULL,
  entity VARCHAR NOT NULL,
  account VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  opening DECIMAL(18,2),
  closing DECIMAL(18,2),
  remark VARCHAR,
  updated_by VARCHAR,
  updated_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
-- 银行流水（按主体导入，用于提现到账对账）；amount 收入为正、支出为负
CREATE TABLE IF NOT EXISTS ledger_bank_txn (
  txn_id VARCHAR PRIMARY KEY,
  entity VARCHAR NOT NULL,
  account_no VARCHAR,
  txn_time VARCHAR NOT NULL,
  amount DECIMAL(18,2) NOT NULL,
  counterparty VARCHAR,
  summary VARCHAR,
  serial_no VARCHAR,
  batch_id VARCHAR NOT NULL,
  file_name VARCHAR,
  imported_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
-- 提现流水 ↔ 银行到账 一对一配对
CREATE TABLE IF NOT EXISTS ledger_withdraw_match (
  shop VARCHAR NOT NULL,
  period VARCHAR,
  serial_no VARCHAR NOT NULL,
  txn_id VARCHAR NOT NULL,
  method VARCHAR NOT NULL,
  matched_by VARCHAR,
  matched_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
-- 月结状态：pending 审批中 / locked 已锁账 / rejected 被驳回 / open 已反结账（后两者视同未结）
CREATE TABLE IF NOT EXISTS ledger_period_close (
  shop VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  status VARCHAR NOT NULL,
  submitted_by VARCHAR,
  submitted_at TIMESTAMP,
  approver VARCHAR,
  comment VARCHAR,
  decided_by VARCHAR,
  decided_at TIMESTAMP,
  decision_comment VARCHAR,
  checks VARCHAR
);
-- 千川后台实际消耗（手工录入；充值 ≠ 消耗，有消耗数据时 ROI 按消耗计算）
CREATE TABLE IF NOT EXISTS ledger_ad_spend (
  shop VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  spend DECIMAL(18,2) NOT NULL,
  remark VARCHAR,
  updated_by VARCHAR,
  updated_at TIMESTAMP
);
-- 用户导入的数据集登记（数据表名即 id：ds_YYYYMMDDHHMMSSmmm）
CREATE TABLE IF NOT EXISTS ledger_dataset (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  columns VARCHAR NOT NULL,
  primary_key VARCHAR NOT NULL DEFAULT '[]',
  created_at TIMESTAMP,
  updated_at TIMESTAMP
);
-- 服务端工作簿：data 为 gzip 压缩的 Univer 工作簿 JSON；version 用于乐观并发
CREATE TABLE IF NOT EXISTS ledger_workbook (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  version BIGINT NOT NULL,
  size BIGINT NOT NULL,
  stored_size BIGINT NOT NULL,
  data BLOB NOT NULL,
  created_by VARCHAR,
  created_at TIMESTAMP,
  updated_by VARCHAR,
  updated_at TIMESTAMP
);
-- 数据集访问次数（按天、按来源），保留 90 天
CREATE TABLE IF NOT EXISTS ledger_dataset_access (
  day DATE NOT NULL,
  dataset_id VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  hits BIGINT NOT NULL,
  PRIMARY KEY (day, dataset_id, kind)
);
-- 填报回写的提交记录：undo 为撤销用的前像（JSON）；stamp 为提交后数据集的 updated_at（微秒），用于判断之后有没有别的改动
CREATE TABLE IF NOT EXISTS ledger_writeback (
  id VARCHAR PRIMARY KEY,
  dataset_id VARCHAR NOT NULL,
  mode VARCHAR NOT NULL,
  operator VARCHAR,
  created_at TIMESTAMP,
  inserted BIGINT,
  updated BIGINT,
  unchanged BIGINT,
  source VARCHAR,
  undo VARCHAR,
  stamp VARCHAR,
  undone_at TIMESTAMP,
  undone_by VARCHAR
);
-- 操作审计：登录用户的写操作与导出（读取类请求不记）；超过 LEDGER_AUDIT_DAYS 天自动清理
CREATE TABLE IF NOT EXISTS ledger_audit (
  happened_at TIMESTAMP NOT NULL,
  user_id VARCHAR,
  user_name VARCHAR,
  ip VARCHAR,
  method VARCHAR,
  path VARCHAR,
  action VARCHAR,
  target_kind VARCHAR,
  target_id VARCHAR,
  target_name VARCHAR,
  status INTEGER,
  ms BIGINT
);
-- 数据范围（行级权限）：用户 → 可见的店铺。subject_id 为 * 是默认规则（没有单独规则的用户）；没有规则的用户不受限
-- 数据范围规则：按（用户，维度）设置可见的取值。维度是一个名字（店铺 / 地区 / 部门…），数据集绑定到某个维度的某一列。
-- 取代旧的 ledger_data_scope（只有「店铺」一个维度，主键只有用户）；旧表的规则在升级时迁移过来并清空，见 migrate_sql
CREATE TABLE IF NOT EXISTS ledger_scope_rule (
  subject_id VARCHAR NOT NULL,
  dimension VARCHAR NOT NULL,
  subject_name VARCHAR,
  mode VARCHAR NOT NULL,
  vals VARCHAR,
  updated_by VARCHAR,
  updated_at TIMESTAMP,
  PRIMARY KEY (subject_id, dimension)
);
CREATE TABLE IF NOT EXISTS ledger_data_scope (
  subject_id VARCHAR PRIMARY KEY,
  subject_name VARCHAR,
  mode VARCHAR NOT NULL,
  shops VARCHAR,
  updated_by VARCHAR,
  updated_at TIMESTAMP
);
-- 登录过的用户（审批人从这里选；id 与登录校验得到的 id 同源，不依赖网关的员工接口）
-- 内置账号（LEDGER_AUTH_MODE=local）：密码只存 PBKDF2-SHA256 哈希；会话只存令牌的 SHA-256
CREATE TABLE IF NOT EXISTS ledger_account (
  id VARCHAR PRIMARY KEY,
  username VARCHAR NOT NULL UNIQUE,
  name VARCHAR NOT NULL,
  password_hash VARCHAR NOT NULL,
  admin BOOLEAN NOT NULL DEFAULT false,
  disabled BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMP,
  updated_at TIMESTAMP,
  last_login_at TIMESTAMP,
  totp_secret_enc VARCHAR,
  totp_enabled BOOLEAN DEFAULT false,
  totp_last_step BIGINT
);
-- 两步验证（TOTP）的一次性恢复码（只存摘要；见 totp.rs）
CREATE TABLE IF NOT EXISTS ledger_totp_recovery (
  account_id VARCHAR NOT NULL,
  code_hash VARCHAR NOT NULL,
  PRIMARY KEY (account_id, code_hash)
);
-- 登录失败计数（按用户名；连续失败后临时锁定，重启不清零）
CREATE TABLE IF NOT EXISTS ledger_login_failure (
  key VARCHAR PRIMARY KEY,
  failures BIGINT NOT NULL,
  last_at BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS ledger_session (
  token_hash VARCHAR PRIMARY KEY,
  account_id VARCHAR NOT NULL,
  created_at TIMESTAMP,
  expires_at TIMESTAMP NOT NULL
);
-- 单点登录（OIDC）身份：身份提供方里的 (issuer, sub) 对应哪个内置账号（见 oidc.rs）
CREATE TABLE IF NOT EXISTS ledger_oidc_identity (
  issuer VARCHAR NOT NULL,
  subject VARCHAR NOT NULL,
  account_id VARCHAR NOT NULL,
  created_at TIMESTAMP,
  last_login_at TIMESTAMP,
  PRIMARY KEY (issuer, subject)
);
-- 个人访问令牌（只存摘要；见 pat.rs）
CREATE TABLE IF NOT EXISTS ledger_pat (
  id VARCHAR PRIMARY KEY,
  account_id VARCHAR NOT NULL,
  name VARCHAR NOT NULL,
  token_hash VARCHAR NOT NULL,
  prefix VARCHAR NOT NULL,
  created_at TIMESTAMP,
  expires_at TIMESTAMP,
  last_used_at TIMESTAMP
);
-- 工作簿的只读分享链接（只存摘要；见 share.rs）
CREATE TABLE IF NOT EXISTS ledger_share (
  id VARCHAR PRIMARY KEY,
  workbook_id VARCHAR NOT NULL,
  note VARCHAR,
  token_hash VARCHAR NOT NULL,
  prefix VARCHAR NOT NULL,
  created_by VARCHAR,
  created_by_name VARCHAR,
  creator_admin BOOLEAN DEFAULT false,
  created_at TIMESTAMP,
  expires_at TIMESTAMP,
  last_used_at TIMESTAMP,
  view_count BIGINT DEFAULT 0,
  -- 这个工作簿用到的数据集 id（JSON 数组），打开分享页时按当前内容刷新；分享链接只能读这些数据集
  datasets VARCHAR DEFAULT '[]',
  content_version BIGINT DEFAULT 0
);
CREATE TABLE IF NOT EXISTS ledger_user (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  admin BOOLEAN,
  first_seen TIMESTAMP,
  last_seen TIMESTAMP
);
-- 单元格评论（根评论 + 回复；parent_id 为空的是线程的第一条）与应用内通知
CREATE TABLE IF NOT EXISTS ledger_comment (
  id VARCHAR PRIMARY KEY,
  workbook_id VARCHAR NOT NULL,
  sheet_id VARCHAR NOT NULL,
  row_idx BIGINT NOT NULL,
  col_idx BIGINT NOT NULL,
  parent_id VARCHAR,
  body VARCHAR NOT NULL,
  mentions VARCHAR,
  author_id VARCHAR,
  author_name VARCHAR NOT NULL,
  created_at TIMESTAMP,
  edited_at TIMESTAMP,
  resolved BOOLEAN,
  resolved_by VARCHAR,
  resolved_at TIMESTAMP
);
CREATE TABLE IF NOT EXISTS ledger_notification (
  id VARCHAR PRIMARY KEY,
  user_id VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  workbook_id VARCHAR,
  workbook_name VARCHAR,
  sheet_id VARCHAR,
  row_idx BIGINT,
  col_idx BIGINT,
  comment_id VARCHAR,
  from_name VARCHAR,
  text VARCHAR,
  created_at TIMESTAMP,
  read_at TIMESTAMP
);
-- 文件审批：把工作簿的某个版本交给指定审批人。版本内容保留在 ledger_workbook_version（审批期间不会被自动清理）
CREATE TABLE IF NOT EXISTS ledger_approval (
  id VARCHAR PRIMARY KEY,
  workbook_id VARCHAR NOT NULL,
  workbook_name VARCHAR,
  workbook_version BIGINT NOT NULL,
  initiator_id VARCHAR,
  initiator_name VARCHAR,
  approver_id VARCHAR,
  approver_name VARCHAR,
  comment VARCHAR,
  summary VARCHAR,
  status VARCHAR NOT NULL,
  created_at TIMESTAMP,
  decided_at TIMESTAMP,
  decided_by VARCHAR,
  decision_comment VARCHAR
);
CREATE TABLE IF NOT EXISTS ledger_approval_event (
  approval_id VARCHAR NOT NULL,
  happened_at TIMESTAMP NOT NULL,
  action VARCHAR NOT NULL,
  user_id VARCHAR,
  user_name VARCHAR,
  comment VARCHAR
);
-- 工作簿文件夹（可嵌套；工作簿的 folder_id 为空表示在根目录）
CREATE TABLE IF NOT EXISTS ledger_folder (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  parent_id VARCHAR,
  created_by VARCHAR,
  created_at TIMESTAMP
);
-- 工作簿版本历史（手动 / 命名版本永久保留，自动保存只留最近若干个）
CREATE TABLE IF NOT EXISTS ledger_workbook_version (
  workbook_id VARCHAR NOT NULL,
  version BIGINT NOT NULL,
  data BLOB NOT NULL,
  size BIGINT NOT NULL,
  saved_by VARCHAR,
  saved_at TIMESTAMP,
  description VARCHAR,
  is_auto BOOLEAN NOT NULL DEFAULT false
);
-- 数据连接器：来源配置为 JSON（数据库密码加密存放于 passwordEnc），目标 / 计划同为 JSON
CREATE TABLE IF NOT EXISTS ledger_connector (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  source VARCHAR NOT NULL,
  target VARCHAR NOT NULL,
  schedule VARCHAR NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  watermark VARCHAR,
  created_at TIMESTAMP,
  updated_at TIMESTAMP
);
-- MCP 访问令牌：只存 SHA-256 摘要；datasets 为可访问的数据集 ID 列表（JSON，空为全部）
CREATE TABLE IF NOT EXISTS ledger_mcp_token (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  prefix VARCHAR NOT NULL,
  token_hash VARCHAR NOT NULL,
  allow_sql BOOLEAN NOT NULL DEFAULT TRUE,
  datasets VARCHAR,
  created_by VARCHAR,
  created_at TIMESTAMP,
  last_used_at TIMESTAMP,
  revoked_at TIMESTAMP
);
-- MCP 调用审计日志（保留最近 5000 条）
CREATE TABLE IF NOT EXISTS ledger_mcp_log (
  token_id VARCHAR,
  token_name VARCHAR,
  method VARCHAR,
  tool VARCHAR,
  arguments VARCHAR,
  ok BOOLEAN,
  message VARCHAR,
  elapsed_ms BIGINT,
  client VARCHAR,
  created_at TIMESTAMP
);
-- 已保存的 SQL 查询：dataset_id 为另存的数据集（重算时替换其数据）；schedule 为 JSON（manual / daily / after_sync）
CREATE TABLE IF NOT EXISTS ledger_sql_query (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  sql VARCHAR NOT NULL,
  description VARCHAR,
  dataset_id VARCHAR,
  column_kinds VARCHAR,
  schedule VARCHAR,
  created_by VARCHAR,
  created_at TIMESTAMP,
  updated_at TIMESTAMP,
  last_run_at TIMESTAMP,
  last_status VARCHAR,
  last_rows BIGINT,
  last_message VARCHAR
);
-- AI 助手会话（按工作簿）
CREATE TABLE IF NOT EXISTS ledger_ai_thread (
  id VARCHAR PRIMARY KEY,
  workbook_id VARCHAR NOT NULL,
  title VARCHAR NOT NULL,
  created_by VARCHAR,
  created_at TIMESTAMP,
  updated_at TIMESTAMP
);
-- AI 对话消息（OpenAI 消息格式：user / assistant(+tool_calls) / tool；display 为界面显示的提问原文，meta 为工具调用说明）
CREATE TABLE IF NOT EXISTS ledger_ai_message (
  id VARCHAR PRIMARY KEY,
  thread_id VARCHAR NOT NULL,
  seq BIGINT NOT NULL,
  role VARCHAR NOT NULL,
  content VARCHAR,
  tool_calls VARCHAR,
  tool_call_id VARCHAR,
  display VARCHAR,
  meta VARCHAR,
  created_at TIMESTAMP
);
-- AI 生成的建议（透视表 / 图表 / 公式 / 计算列）及处理结果，供审计
CREATE TABLE IF NOT EXISTS ledger_ai_action (
  id VARCHAR PRIMARY KEY,
  thread_id VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  title VARCHAR,
  payload VARCHAR NOT NULL,
  status VARCHAR NOT NULL,
  result VARCHAR,
  created_at TIMESTAMP,
  decided_at TIMESTAMP,
  decided_by VARCHAR
);
-- 自建模板：工作簿副本（gzip）+ 需要的数据集与列（requirements JSON）；内置模板随程序发布，不在表中
CREATE TABLE IF NOT EXISTS ledger_template (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  description VARCHAR,
  category VARCHAR,
  data BLOB NOT NULL,
  requirements VARCHAR NOT NULL,
  summary VARCHAR NOT NULL,
  warnings VARCHAR,
  sheet_names VARCHAR,
  source_workbook_id VARCHAR,
  created_by VARCHAR,
  created_at TIMESTAMP,
  updated_at TIMESTAMP
);
-- 模板使用记录（使用统计；内置模板也记录）
CREATE TABLE IF NOT EXISTS ledger_template_use (
  template_id VARCHAR NOT NULL,
  workbook_id VARCHAR NOT NULL,
  used_by VARCHAR,
  used_at TIMESTAMP
);
-- 定时推送渠道：Webhook 地址 / 收件人与加签密钥加密存放
CREATE TABLE IF NOT EXISTS ledger_report_channel (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  target_enc VARCHAR NOT NULL,
  secret_enc VARCHAR,
  created_at TIMESTAMP
);
-- 定时推送任务：工作簿中的仪表盘页 + 计划（JSON）+ 渠道 ID 列表（JSON）
CREATE TABLE IF NOT EXISTS ledger_report (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  workbook_id VARCHAR NOT NULL,
  sheet_id VARCHAR NOT NULL,
  title VARCHAR,
  schedule VARCHAR NOT NULL,
  channel_ids VARCHAR NOT NULL,
  attach_excel BOOLEAN NOT NULL DEFAULT FALSE,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  created_at TIMESTAMP,
  updated_at TIMESTAMP
);
-- 推送记录（results：各渠道结果 JSON）
CREATE TABLE IF NOT EXISTS ledger_report_run (
  id VARCHAR PRIMARY KEY,
  report_id VARCHAR NOT NULL,
  trigger_by VARCHAR NOT NULL,
  status VARCHAR NOT NULL,
  started_at TIMESTAMP,
  finished_at TIMESTAMP,
  message VARCHAR,
  results VARCHAR
);
-- 阈值告警规则：spec 是整条规则（数据集、指标、分组、条件、计划、渠道…）的 JSON，新增字段不用改表
CREATE TABLE IF NOT EXISTS ledger_alert (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  spec VARCHAR NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  created_by VARCHAR,
  created_by_name VARCHAR,
  created_at TIMESTAMP,
  updated_at TIMESTAMP
);
-- 告警状态：只记录当前处于「触发中」的分组（恢复后删除），用来避免同一个问题反复通知
CREATE TABLE IF NOT EXISTS ledger_alert_state (
  alert_id VARCHAR NOT NULL,
  group_key VARCHAR NOT NULL,
  since TIMESTAMP,
  last_notified TIMESTAMP,
  last_value DOUBLE,
  PRIMARY KEY (alert_id, group_key)
);
-- 告警评估记录（results：各渠道结果 JSON）
CREATE TABLE IF NOT EXISTS ledger_alert_run (
  id VARCHAR PRIMARY KEY,
  alert_id VARCHAR NOT NULL,
  trigger_by VARCHAR NOT NULL,
  status VARCHAR NOT NULL,
  started_at TIMESTAMP,
  finished_at TIMESTAMP,
  message VARCHAR,
  results VARCHAR,
  hits INTEGER,
  notified INTEGER
);
-- 多人填报：收集任务（绑定一个填报数据集，按「填报单位」列和「期次」列区分每个人每一期填的行）
CREATE TABLE IF NOT EXISTS ledger_collect (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  dataset_id VARCHAR NOT NULL,
  unit_column VARCHAR NOT NULL,
  period_column VARCHAR NOT NULL,
  dimension VARCHAR NOT NULL,
  created_by VARCHAR,
  created_by_name VARCHAR,
  created_at TIMESTAMP,
  updated_at TIMESTAMP
);
-- 收集任务里的填报单位与负责人（ord 保持录入顺序）
CREATE TABLE IF NOT EXISTS ledger_collect_unit (
  collect_id VARCHAR NOT NULL,
  unit VARCHAR NOT NULL,
  user_id VARCHAR,
  user_name VARCHAR,
  ord INTEGER NOT NULL,
  PRIMARY KEY (collect_id, unit)
);
-- 已开启的期次
CREATE TABLE IF NOT EXISTS ledger_collect_period (
  collect_id VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  opened_at TIMESTAMP,
  opened_by VARCHAR,
  PRIMARY KEY (collect_id, period)
);
-- 提交记录：某一期某个单位最近一次提交（没有记录就是还没填）
CREATE TABLE IF NOT EXISTS ledger_collect_submit (
  collect_id VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  unit VARCHAR NOT NULL,
  submitted_at TIMESTAMP,
  submitted_by VARCHAR,
  submitted_by_name VARCHAR,
  batch_id VARCHAR,
  PRIMARY KEY (collect_id, period, unit)
);
-- 收集任务生成的填报工作簿：同一个人对同一个任务、期次、单位只生成一个，再点「去填报」直接打开已有的
CREATE TABLE IF NOT EXISTS ledger_collect_workbook (
  collect_id VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  unit VARCHAR NOT NULL,
  user_id VARCHAR NOT NULL,
  workbook_id VARCHAR NOT NULL,
  created_at TIMESTAMP,
  PRIMARY KEY (collect_id, period, unit, user_id)
);
-- 审核记录：确认人对某一期某个单位的处理。confirmed = 确认并锁定（不能再改），returned = 退回重填（负责人再次提交后记录删除）；没有记录就是还在等确认
CREATE TABLE IF NOT EXISTS ledger_collect_review (
  collect_id VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  unit VARCHAR NOT NULL,
  state VARCHAR NOT NULL,
  reason VARCHAR,
  reviewed_at TIMESTAMP,
  reviewed_by VARCHAR,
  reviewed_by_name VARCHAR,
  PRIMARY KEY (collect_id, period, unit)
);
-- 催办记录：某一期某个单位上次被催的时间和次数（unit 为 '*' 的是整期推到群里的那一条）。用来限制频率、自动催办一天只催一次
CREATE TABLE IF NOT EXISTS ledger_collect_remind (
  collect_id VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  unit VARCHAR NOT NULL,
  last_at TIMESTAMP,
  times INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (collect_id, period, unit)
);
-- 同步记录
CREATE TABLE IF NOT EXISTS ledger_connector_run (
  id VARCHAR PRIMARY KEY,
  connector_id VARCHAR NOT NULL,
  trigger_by VARCHAR NOT NULL,
  status VARCHAR NOT NULL,
  started_at TIMESTAMP,
  finished_at TIMESTAMP,
  rows_read BIGINT DEFAULT 0,
  inserted BIGINT DEFAULT 0,
  updated BIGINT DEFAULT 0,
  skipped BIGINT DEFAULT 0,
  replaced BIGINT DEFAULT 0,
  files VARCHAR,
  watermark_from VARCHAR,
  watermark_to VARCHAR,
  message VARCHAR
);
-- 文件夹来源已导入的文件（同名文件被覆盖更新后大小 / 修改时间变化，会重新导入）
CREATE TABLE IF NOT EXISTS ledger_connector_file (
  connector_id VARCHAR NOT NULL,
  path VARCHAR NOT NULL,
  size BIGINT NOT NULL,
  mtime VARCHAR NOT NULL,
  run_id VARCHAR,
  imported_at TIMESTAMP
);
-- 计算列（对任意数据集，含内置数据集；公式翻译为 SQL 表达式在查询时计算）
CREATE TABLE IF NOT EXISTS ledger_calc_column (
  dataset_id VARCHAR NOT NULL,
  name VARCHAR NOT NULL,
  formula VARCHAR NOT NULL,
  kind VARCHAR,
  created_at TIMESTAMP
);
-- 逐笔对账（见 recon.rs）：勾对组与组内的行。行用「整行内容指纹 + 序号」标识（数据集重新导入后 rowid 会变）
CREATE TABLE IF NOT EXISTS ledger_recon_group (
  recon_id VARCHAR NOT NULL,
  gno BIGINT NOT NULL,
  method VARCHAR NOT NULL,
  note VARCHAR,
  diff_cents BIGINT NOT NULL DEFAULT 0,
  item_count INTEGER NOT NULL,
  matched_by VARCHAR,
  matched_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
CREATE TABLE IF NOT EXISTS ledger_recon_item (
  recon_id VARCHAR NOT NULL,
  gno BIGINT NOT NULL,
  side TINYINT NOT NULL,
  row_key VARCHAR NOT NULL,
  cents BIGINT NOT NULL
);
-- 月结操作留痕（只增不改）
CREATE TABLE IF NOT EXISTS ledger_close_log (
  shop VARCHAR NOT NULL,
  period VARCHAR NOT NULL,
  action VARCHAR NOT NULL,
  operator VARCHAR,
  comment VARCHAR,
  logged_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);
"#
    )
}

#[cfg(test)]
mod tests {
    use duckdb::Connection;

    use super::*;

    fn set_schema_version(conn: &Connection, v: i64) {
        conn.execute("INSERT INTO ledger_meta (key, value) VALUES ('schema_version', ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value", [v.to_string()]).unwrap();
    }

    #[test]
    fn legacy_file_is_version_zero_then_stamped() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(check_version(&conn).unwrap(), 0, "没有记录的旧数据文件按 0 算");
        stamp_version(&conn, "0.1.0").unwrap();
        assert_eq!(check_version(&conn).unwrap(), SCHEMA_VERSION);
        // 重复记录不报错，更新程序版本
        stamp_version(&conn, "0.2.0").unwrap();
        let by: String = conn.query_row("SELECT value FROM ledger_meta WHERE key = 'app_version'", [], |r| r.get(0)).unwrap();
        assert_eq!(by, "0.2.0");
    }

    #[test]
    fn refuses_data_file_written_by_a_newer_version() {
        let conn = Connection::open_in_memory().unwrap();
        stamp_version(&conn, "9.9.9").unwrap();
        set_schema_version(&conn, SCHEMA_VERSION + 1);
        let err = check_version(&conn).unwrap_err().to_string();
        assert!(err.contains("更新") && err.contains("9.9.9") && err.contains("restore"), "{err}");
        // 旧版本的数据文件（更小的版本号）正常通过
        set_schema_version(&conn, 0);
        assert_eq!(check_version(&conn).unwrap(), 0);
    }

    /// 整条启动路径：新建 → 记录版本；被改成更新的版本后，再次打开被拒绝，并且不改动数据文件
    #[test]
    fn opening_a_newer_data_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::from_env().unwrap();
        config.db_path = dir.path().join("data/ledger.duckdb");
        config.inbox_roots = vec![dir.path().join("inbox")];
        crate::db::Db::open(&config).map(drop).unwrap(); // 第一次打开应成功
        {
            let conn = crate::db::open_file(&config.db_path).unwrap();
            assert_eq!(check_version(&conn).unwrap(), SCHEMA_VERSION, "新建的数据文件已记录当前结构版本");
            set_schema_version(&conn, SCHEMA_VERSION + 5);
            conn.execute_batch("CHECKPOINT").unwrap();
        }
        let err = crate::db::Db::open(&config).err().expect("应拒绝打开").to_string();
        assert!(err.contains("更新") && err.contains("restore"), "{err}");
    }

    #[test]
    fn upgrade_is_repeatable() {
        let conn = Connection::open_in_memory().unwrap();
        for _ in 0..2 {
            conn.execute_batch(&create_tables_sql()).unwrap();
            conn.execute_batch(&migrate_sql()).unwrap();
        }
    }
}
