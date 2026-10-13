//! 明细查询：窗口取数（供前端虚拟滚动）、筛选汇总与分面统计、按「汇总项规则」下钻、CSV 导出
//!
//! 下钻规则与前端汇总表公式同源（前端 summary-template.ts 的声明式规则）：
//! 每条流水对某个汇总项的「贡献金额」= 各规则项之和，只返回贡献不为 0 的流水，
//! 其合计必然等于汇总表对应单元格的值，用于核对汇总是否正确

use duckdb::types::{Value, ValueRef};
use duckdb::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

use crate::error::{AppError, AppResult};
use crate::tabular::to_json;
use crate::schema::{DETAIL_COLUMNS, NUMERIC_COLUMNS, ident, is_numeric, literal};

pub const CONTRIBUTION: &str = "贡献金额";
const MAX_LIMIT: u32 = 1000;

// ── 请求结构 ──────────────────────────────────────────

/// 汇总项规则中的一项
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum RuleTerm {
    /// 某金额列（可只取负数/正数部分），如 销售收入 = 订单实付应结 + 订单退款
    Column {
        column: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        op: Option<SignFilter>,
        sign: i8,
    },
    /// 某动账场景（Excel 通配符，"" 表示场景为空）的资金净额：入账为正、出账为负
    Scene { scene: String, sign: i8 },
    /// 全部流水的资金净额
    Flow { sign: i8 },
}

#[derive(Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum SignFilter {
    Negative,
    Positive,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DetailFilter {
    pub period: String,
    pub shop: Option<String>,
    pub entity: Option<String>,
    /// 多选；"" 表示「动账场景为空」
    pub scenes: Vec<String>,
    pub directions: Vec<String>,
    /// 多选；"" 表示「计费类型为空」
    pub billing_types: Vec<String>,
    pub entities: Vec<String>,
    pub shops: Vec<String>,
    /// 模糊匹配订单号 / 子订单号 / 流水号 / 售后编号 / 商品名称 / 达人名称 / 备注
    pub keyword: Option<String>,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
    pub amount_min: Option<f64>,
    pub amount_max: Option<f64>,
    /// 汇总项规则：存在时只返回对该汇总项有贡献的流水，并附加「贡献金额」列
    pub rule: Option<Vec<RuleTerm>>,
    /// 经营分析下钻的固定上下文；"" 表示该字段为空（如无达人的订单、非订单流水）
    pub talent_id: Option<String>,
    pub product_id: Option<String>,
    /// 下单月份 YYYY-MM（取「下单时间」前 7 位）
    pub order_month: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsRequest {
    pub filter: DetailFilter,
    pub sort: Option<SortSpec>,
    #[serde(default)]
    pub offset: u32,
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub filter: DetailFilter,
    pub sort: Option<SortSpec>,
}

// ── SQL 构造 ──────────────────────────────────────────

/// 分面维度（左侧筛选面板）
#[derive(Clone, Copy, PartialEq)]
enum Dim {
    Scene,
    Direction,
    BillingType,
    Entity,
    Shop,
}

impl Dim {
    const ALL: [Dim; 5] = [Dim::Scene, Dim::Direction, Dim::BillingType, Dim::Entity, Dim::Shop];

    fn column_sql(self) -> String {
        match self {
            Dim::Scene => ident("动账场景"),
            Dim::Direction => ident("动账方向"),
            Dim::BillingType => ident("计费类型"),
            Dim::Entity => ident("商户主体名称"),
            Dim::Shop => "shop".into(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Dim::Scene => "动账场景",
            Dim::Direction => "动账方向",
            Dim::BillingType => "计费类型",
            Dim::Entity => "商户主体名称",
            Dim::Shop => "店铺",
        }
    }
}

fn signed_amount() -> String {
    format!(
        "CASE {dir} WHEN '入账' THEN COALESCE({amt}, 0) WHEN '出账' THEN -COALESCE({amt}, 0) ELSE 0 END",
        dir = ident("动账方向"),
        amt = ident("动账金额")
    )
}

/// Excel 通配符（* ?）→ SQL 条件；与 SUMIFS 一致："" 只匹配空值，带通配符的模式不匹配空值
fn scene_matcher(pattern: &str) -> String {
    let col = ident("动账场景");
    if pattern.is_empty() {
        return format!("{col} IS NULL");
    }
    if pattern.contains(['*', '?']) {
        let like: String = pattern
            .chars()
            .flat_map(|c| match c {
                '\\' | '%' | '_' => vec!['\\', c],
                '*' => vec!['%'],
                '?' => vec!['_'],
                c => vec![c],
            })
            .collect();
        return format!("{col} LIKE {} ESCAPE '\\'", literal(&like));
    }
    format!("{col} = {}", literal(pattern))
}

/// 规则 → 每行贡献金额的 SQL 表达式（DECIMAL，合计无浮点误差）
pub fn contribution_sql(rule: &[RuleTerm]) -> AppResult<String> {
    if rule.is_empty() {
        return Err(AppError::bad("汇总项规则不能为空"));
    }
    let mut parts = Vec::with_capacity(rule.len());
    for term in rule {
        let (sign, expr) = match term {
            RuleTerm::Column { column, op, sign } => {
                if !is_numeric(column) {
                    return Err(AppError::bad(format!("规则引用了未知的金额列：{column}")));
                }
                let c = ident(column);
                let expr = match op {
                    Some(SignFilter::Negative) => format!("CASE WHEN {c} < 0 THEN {c} ELSE 0 END"),
                    Some(SignFilter::Positive) => format!("CASE WHEN {c} > 0 THEN {c} ELSE 0 END"),
                    None => format!("COALESCE({c}, 0)"),
                };
                (*sign, expr)
            }
            RuleTerm::Scene { scene, sign } => {
                (*sign, format!("CASE WHEN {} THEN {} ELSE 0 END", scene_matcher(scene), signed_amount()))
            }
            RuleTerm::Flow { sign } => (*sign, signed_amount()),
        };
        let op = match sign {
            1 => "+",
            -1 => "-",
            _ => return Err(AppError::bad("规则项的 sign 只能是 1 或 -1")),
        };
        parts.push(format!("{op} ({expr})"));
    }
    Ok(format!("CAST(0 {} AS DECIMAL(18,2))", parts.join(" ")))
}

struct Query {
    conditions: Vec<String>,
    args: Vec<Value>,
}

impl Query {
    fn push(&mut self, cond: String, args: impl IntoIterator<Item = Value>) {
        self.conditions.push(cond);
        self.args.extend(args);
    }

    fn where_sql(&self) -> String {
        self.conditions.join(" AND ")
    }
}

/// 多选条件（"" 表示空值）
fn in_list(column: &str, values: &[String]) -> (String, Vec<Value>) {
    let has_blank = values.iter().any(|v| v.is_empty());
    let named: Vec<&String> = values.iter().filter(|v| !v.is_empty()).collect();
    let mut ors = Vec::new();
    if !named.is_empty() {
        ors.push(format!("{column} IN ({})", vec!["?"; named.len()].join(", ")));
    }
    if has_blank {
        ors.push(format!("{column} IS NULL"));
    }
    (format!("({})", ors.join(" OR ")), named.into_iter().map(|v| Value::Text(v.clone())).collect())
}

/// 构造 WHERE；exclude 用于分面统计时去掉该维度自身的筛选
fn build_query(f: &DetailFilter, contribution: Option<&str>, exclude: Option<Dim>) -> Query {
    let mut q = Query { conditions: vec!["period = ?".into()], args: vec![Value::Text(f.period.clone())] };
    if let Some(shop) = f.shop.as_ref().filter(|s| !s.is_empty()) {
        q.push("shop = ?".into(), [Value::Text(shop.clone())]);
    }
    if let Some(entity) = f.entity.as_ref().filter(|s| !s.is_empty()) {
        q.push(format!("{} = ?", ident("商户主体名称")), [Value::Text(entity.clone())]);
    }
    for (column, value) in [("达人ID", &f.talent_id), ("商品ID", &f.product_id)] {
        match value.as_deref() {
            Some("") => q.conditions.push(format!("{} IS NULL", ident(column))),
            Some(v) => q.push(format!("{} = ?", ident(column)), [Value::Text(v.to_string())]),
            None => {}
        }
    }
    match f.order_month.as_deref() {
        Some("") => q.conditions.push(format!("{} IS NULL", ident("下单时间"))),
        Some(m) => q.push(format!("substr({}, 1, 7) = ?", ident("下单时间")), [Value::Text(m.to_string())]),
        None => {}
    }
    for (dim, values) in [
        (Dim::Scene, &f.scenes),
        (Dim::Direction, &f.directions),
        (Dim::BillingType, &f.billing_types),
        (Dim::Entity, &f.entities),
        (Dim::Shop, &f.shops),
    ] {
        if !values.is_empty() && exclude != Some(dim) {
            let (cond, args) = in_list(&dim.column_sql(), values);
            q.push(cond, args);
        }
    }
    if let Some(kw) = f.keyword.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        // search_text 为入库时拼接的小写文本（见 schema::SEARCH_COLUMNS）
        let pattern = format!("%{}%", kw.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        q.push("search_text LIKE ? ESCAPE '\\'".into(), [Value::Text(pattern)]);
    }
    if let Some(t) = f.time_from.as_ref().filter(|s| !s.is_empty()) {
        q.push(format!("{} >= ?", ident("动账时间")), [Value::Text(t.clone())]);
    }
    if let Some(t) = f.time_to.as_ref().filter(|s| !s.is_empty()) {
        q.push(format!("{} <= ?", ident("动账时间")), [Value::Text(t.clone())]);
    }
    if let Some(v) = f.amount_min {
        q.push(format!("{} >= ?", ident("动账金额")), [Value::Double(v)]);
    }
    if let Some(v) = f.amount_max {
        q.push(format!("{} <= ?", ident("动账金额")), [Value::Double(v)]);
    }
    if let Some(expr) = contribution {
        q.conditions.push(format!("({expr}) <> 0"));
    }
    q
}

fn select_columns(contribution: Option<&str>) -> (String, Vec<String>) {
    let mut names = vec!["店铺".to_string()];
    let mut exprs = vec!["shop".to_string()];
    if let Some(expr) = contribution {
        names.push(CONTRIBUTION.into());
        exprs.push(format!("({expr})::DOUBLE"));
    }
    for c in DETAIL_COLUMNS {
        names.push(c.to_string());
        exprs.push(if is_numeric(c) { format!("{}::DOUBLE", ident(c)) } else { ident(c) });
    }
    (exprs.join(", "), names)
}

fn order_sql(sort: Option<&SortSpec>, contribution: Option<&str>) -> AppResult<String> {
    let time = ident("动账时间");
    let Some(sort) = sort else {
        return Ok(format!("{time}, row_no"));
    };
    let column = match sort.column.as_str() {
        "店铺" => "shop".to_string(),
        CONTRIBUTION => match contribution {
            Some(expr) => format!("({expr})"),
            None => return Err(AppError::bad("未指定汇总项规则，不能按贡献金额排序")),
        },
        c if DETAIL_COLUMNS.contains(&c) => ident(c),
        c => return Err(AppError::bad(format!("不支持按「{c}」排序"))),
    };
    let dir = if sort.desc { "DESC NULLS LAST" } else { "ASC NULLS LAST" };
    Ok(format!("{column} {dir}, {time}, row_no"))
}

fn contribution_of(f: &DetailFilter) -> AppResult<Option<String>> {
    f.rule.as_deref().map(contribution_sql).transpose()
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}


// ── 窗口取数 ──────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsResult {
    pub columns: Vec<String>,
    /// 行数组（列顺序同 columns），比对象数组体积小约一半
    pub rows: Vec<Vec<Json>>,
    pub offset: u32,
}

pub fn rows(conn: &Connection, req: &RowsRequest) -> AppResult<RowsResult> {
    let contribution = contribution_of(&req.filter)?;
    let q = build_query(&req.filter, contribution.as_deref(), None);
    let (select, columns) = select_columns(contribution.as_deref());
    let limit = req.limit.unwrap_or(200).clamp(1, MAX_LIMIT);
    let sql = format!(
        "SELECT {select} FROM ledger_detail WHERE {} ORDER BY {} LIMIT {limit} OFFSET {}",
        q.where_sql(),
        order_sql(req.sort.as_ref(), contribution.as_deref())?,
        req.offset
    );
    let mut stmt = conn.prepare(&sql)?;
    let n = columns.len();
    let rows = stmt
        .query_map(params_from_iter(q.args.iter()), |row| (0..n).map(|i| row.get_ref(i).map(to_json)).collect())?
        .collect::<Result<Vec<Vec<Json>>, _>>()?;
    Ok(RowsResult { columns, rows, offset: req.offset })
}

// ── 筛选汇总 + 分面 ───────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetItem {
    pub value: String,
    pub count: i64,
    /// 资金净额：入账为正、出账为负
    pub net: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailSummary {
    pub total_count: i64,
    pub in_amount: f64,
    pub out_amount: f64,
    pub net_amount: f64,
    /// 规则下钻时各行贡献金额之和，应等于汇总表对应单元格
    pub contribution: Option<f64>,
    /// 各金额列在当前筛选下的合计
    pub sums: Map<String, Json>,
    pub facets: Map<String, Json>,
}

pub fn summary(conn: &Connection, f: &DetailFilter) -> AppResult<DetailSummary> {
    let contribution = contribution_of(f)?;
    let q = build_query(f, contribution.as_deref(), None);
    let dir = ident("动账方向");
    let amt = ident("动账金额");
    let mut exprs = vec![
        "count(*)".to_string(),
        format!("COALESCE(sum({amt}) FILTER (WHERE {dir} = '入账'), 0)::DOUBLE"),
        format!("COALESCE(sum({amt}) FILTER (WHERE {dir} = '出账'), 0)::DOUBLE"),
        match &contribution {
            Some(expr) => format!("COALESCE(sum({expr}), 0)::DOUBLE"),
            None => "NULL::DOUBLE".into(),
        },
    ];
    exprs.extend(NUMERIC_COLUMNS.iter().map(|c| format!("COALESCE(sum({}), 0)::DOUBLE", ident(c))));
    let sql = format!("SELECT {} FROM ledger_detail WHERE {}", exprs.join(", "), q.where_sql());

    let (total_count, in_amount, out_amount, contribution_sum, sums) =
        conn.query_row(&sql, params_from_iter(q.args.iter()), |r| {
            let mut sums = Map::new();
            for (i, c) in NUMERIC_COLUMNS.iter().enumerate() {
                sums.insert(c.to_string(), Json::from(round2(r.get::<_, f64>(4 + i)?)));
            }
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?, r.get::<_, Option<f64>>(3)?, sums))
        })?;

    // 5 个分面互相独立：各用一个克隆连接并行查询（DuckDB 支持同进程多连接并发读）
    // Connection 不是 Sync：先在当前线程克隆，再把各自的连接转移进子线程
    let conns = Dim::ALL.iter().map(|_| conn.try_clone()).collect::<Result<Vec<_>, _>>()?;
    let facet_results: Vec<AppResult<(Dim, Vec<FacetItem>)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = Dim::ALL
            .into_iter()
            .zip(conns)
            .map(|(dim, conn)| {
                let contribution = contribution.as_deref();
                scope.spawn(move || -> AppResult<(Dim, Vec<FacetItem>)> { Ok((dim, facet(&conn, f, contribution, dim)?)) })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(AppError::bad("分面统计线程异常")))).collect()
    });
    let mut facets = Map::new();
    for result in facet_results {
        let (dim, items) = result?;
        facets.insert(dim.label().into(), serde_json::to_value(items)?);
    }

    Ok(DetailSummary {
        total_count,
        in_amount: round2(in_amount),
        out_amount: round2(out_amount),
        net_amount: round2(in_amount - out_amount),
        contribution: contribution_sum.map(round2),
        sums,
        facets,
    })
}

fn facet(conn: &Connection, f: &DetailFilter, contribution: Option<&str>, dim: Dim) -> AppResult<Vec<FacetItem>> {
    let fq = build_query(f, contribution, Some(dim));
    let sql = format!(
        "SELECT COALESCE({col}, ''), count(*), COALESCE(sum({net}), 0)::DOUBLE
         FROM ledger_detail WHERE {} GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 300",
        fq.where_sql(),
        col = dim.column_sql(),
        net = signed_amount(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(params_from_iter(fq.args.iter()), |r| {
            Ok(FacetItem { value: r.get(0)?, count: r.get(1)?, net: round2(r.get(2)?) })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(items)
}

// ── CSV 导出 ──────────────────────────────────────────

/// 当前筛选结果导出为 CSV（UTF-8 BOM，Excel 直接打开不乱码）；ID 列加 \t 前缀避免被 Excel 转为科学计数法
pub fn export_csv(conn: &Connection, req: &ExportRequest) -> AppResult<Vec<u8>> {
    let contribution = contribution_of(&req.filter)?;
    let q = build_query(&req.filter, contribution.as_deref(), None);
    let (select, columns) = select_columns(contribution.as_deref());
    let sql = format!(
        "SELECT {select} FROM ledger_detail WHERE {} ORDER BY {}",
        q.where_sql(),
        order_sql(req.sort.as_ref(), contribution.as_deref())?
    );
    let id_columns: Vec<bool> = columns.iter().map(|c| crate::schema::ID_COLUMNS.contains(&c.as_str())).collect();

    let mut out = Vec::with_capacity(1 << 20);
    out.extend_from_slice("\u{feff}".as_bytes());
    out.extend_from_slice(columns.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(",").as_bytes());
    out.push(b'\n');

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(q.args.iter()))?;
    while let Some(row) = rows.next()? {
        #[allow(clippy::needless_range_loop)] // i 同时是 get_ref 的列下标
        for i in 0..columns.len() {
            if i > 0 {
                out.push(b',');
            }
            let text = match row.get_ref(i)? {
                ValueRef::Null => String::new(),
                ValueRef::Double(f) => format!("{}", round2(f)),
                ValueRef::BigInt(n) => n.to_string(),
                ValueRef::Text(b) => {
                    let s = String::from_utf8_lossy(b);
                    if id_columns[i] { format!("\t{s}") } else { s.into_owned() }
                }
                other => format!("{other:?}"),
            };
            out.extend_from_slice(csv_field(&text).as_bytes());
        }
        out.push(b'\n');
    }
    Ok(out)
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}
