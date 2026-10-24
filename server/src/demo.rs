//! `gridlake demo`：一条命令体验 Gridlake
//!
//! - 用单独的演示库（默认 `~/.gridlake/demo.duckdb`，`--db` 可改），和 `gridlake open` 一样只听本机、不需要登录
//! - 第一次运行时生成示例数据（DuckDB 直接生成 CSV 再走导入管线，100 万行订单在发布版上几秒钟）：
//!   订单（店铺、下单时间、商品、渠道、状态、数量、订单金额、已退款）、库存流水、往来明细；时间截止到今天（订单约 26 个月，同比有完整的上年），
//!   「本月」「近 30 天」这类切片器打开就有数据
//! - 用三个内置模板（月度经营分析、库存进销存、应收应付对账）各生成一个工作簿，打开浏览器进入月度经营分析
//! - 再次运行直接打开，不重新生成；`--reset` 清空演示库重来，`--rows` 改订单行数

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use duckdb::Connection;

use crate::open::OpenArgs;
use crate::schema::literal;

pub const USAGE: &str = "用法：gridlake demo [--rows 行数] [--port 端口] [--db 库文件] [--reset] [--no-browser]
  生成示例数据（订单默认 100 万行、库存流水、往来明细），用内置模板做好三个工作簿，打开浏览器
  再次运行直接打开；--reset 清空演示库重新生成";

const DEFAULT_ROWS: u64 = 1_000_000;
const MAX_ROWS: u64 = 100_000_000;
/// 演示工作簿的名称前缀：已经有了就不再生成
const PREFIX: &str = "示例：";

#[derive(Debug, PartialEq)]
pub struct DemoArgs {
    pub rows: u64,
    pub port: Option<u16>,
    pub db: PathBuf,
    pub reset: bool,
    pub browser: bool,
}

pub fn parse_args(args: &[String]) -> anyhow::Result<DemoArgs> {
    let mut out = DemoArgs { rows: DEFAULT_ROWS, port: None, db: crate::open::home_dir().join("demo.duckdb"), reset: false, browser: true };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |what: &str| it.next().cloned().ok_or_else(|| anyhow::anyhow!("{a} 后面要跟{what}\n{USAGE}"));
        match a.as_str() {
            "--rows" => {
                let v = value("行数")?;
                out.rows = v.replace(['_', ','], "").parse().map_err(|_| anyhow::anyhow!("行数不对：{v}"))?;
                if !(1_000..=MAX_ROWS).contains(&out.rows) {
                    anyhow::bail!("行数应在 1000 ~ {MAX_ROWS} 之间");
                }
            }
            "--port" | "-p" => {
                let v = value("端口号")?;
                out.port = Some(v.parse().map_err(|_| anyhow::anyhow!("端口号不对：{v}"))?);
            }
            "--db" => out.db = PathBuf::from(value("库文件路径")?),
            "--reset" => out.reset = true,
            "--no-browser" => out.browser = false,
            s => anyhow::bail!("不认识的参数：{s}\n{USAGE}"),
        }
    }
    Ok(out)
}

/// 交给 `gridlake open` 的配置逻辑（个人库、只听本机、不登录、不备份）；--reset 时先删掉演示库
pub fn configure(config: &mut crate::config::Config, args: &DemoArgs) -> anyhow::Result<()> {
    if args.reset {
        for f in [args.db.clone(), PathBuf::from(format!("{}.wal", args.db.display()))] {
            match std::fs::remove_file(&f) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => anyhow::bail!("删除 {} 失败：{e}", f.display()),
                _ => {}
            }
        }
    }
    crate::open::configure(config, &OpenArgs { paths: vec![], port: args.port, db: Some(args.db.clone()), browser: args.browser }, &[])
}

/// 一个示例数据集：名称、生成 CSV 的 SQL、要指定的列类型、用哪个内置模板（模板里的数据集键）
struct Sample {
    name: &'static str,
    sql: String,
    kinds: &'static [(&'static str, &'static str)],
    template: &'static str,
    key: &'static str,
    workbook: &'static str,
}

fn samples(rows: u64, today: chrono::NaiveDate) -> Vec<Sample> {
    let day = |d: chrono::NaiveDate| format!("DATE '{}'", d.format("%Y-%m-%d"));
    // 订单：最近约 26 个月（「今年」同比时上年是完整的），越近越多（r^0.75 让时间偏向近期：同比约增长三成）；截止到今天
    let from = today - chrono::Duration::days(790);
    let span = (today - from).num_seconds() + 86_399;
    let orders = format!(
        "WITH t AS (
           SELECT i, random() AS r1, random() AS r2, random() AS r3, random() AS r4, random() AS r5, floor(random() * 200)::INT AS p, 1 + floor(random() * 5)::INT AS q
           FROM range({rows}) t(i))
         SELECT 'DY' || lpad(i::VARCHAR, 10, '0') AS 订单号,
                (['北京旗舰店', '上海旗舰店', '广州专卖店', '深圳专卖店', '杭州旗舰店', '成都专卖店', '武汉专卖店', '南京旗舰店', '西安专卖店', '重庆专卖店', '天津专卖店', '苏州旗舰店'])[1 + floor(r1 * r1 * 12)::INT] AS 店铺,
                strftime({from} + to_seconds((pow(r2, 0.75) * {span})::BIGINT), '%Y-%m-%d %H:%M:%S') AS 下单时间,
                '商品' || lpad(p::VARCHAR, 3, '0') AS 商品,
                (['直播', '商品卡', '短视频', '搜索', '达人'])[1 + floor(r3 * 5)::INT] AS 渠道,
                CASE WHEN r4 < 0.75 THEN '已完成' WHEN r4 < 0.85 THEN '已发货' WHEN r4 < 0.9 THEN '待发货' WHEN r4 < 0.97 THEN '已退款' ELSE '已取消' END AS 状态,
                q AS 数量,
                round(q * (19.9 + (p * 37 % 480)) * (0.85 + r5 * 0.15), 2) AS 订单金额,
                r4 >= 0.9 AND r4 < 0.97 AS 已退款
         FROM t",
        from = day(from),
    );
    // 库存流水：50 个商品，一年前录期初，之后每天随机入库 / 出库
    let year_ago = today - chrono::Duration::days(365);
    let stock = format!(
        "SELECT '商品' || lpad(i::VARCHAR, 3, '0') AS 商品, strftime({start}, '%Y-%m-%d') AS 日期, '期初' AS 单据类型, 200 + (i * 53 % 300) AS 数量 FROM range(50) t(i)
         UNION ALL
         SELECT '商品' || lpad(floor(random() * 50)::INT::VARCHAR, 3, '0'), strftime({start} + (1 + floor(random() * 365))::INT, '%Y-%m-%d'),
                CASE WHEN random() < 0.35 THEN '入库' ELSE '出库' END, 1 + floor(random() * 40)::INT
         FROM range(8000)",
        start = day(year_ago),
    );
    // 往来明细：20 个客户（应收 / 收款）、10 个供应商（应付 / 付款），与订单同一段时间
    let partners = format!(
        "WITH t AS (SELECT random() AS r1, random() AS r2, random() AS r3, random() AS r4 FROM range(6000))
         SELECT CASE WHEN r1 < 0.65 THEN '客户' || lpad((1 + floor(r2 * 20))::INT::VARCHAR, 2, '0') ELSE '供应商' || lpad((1 + floor(r2 * 10))::INT::VARCHAR, 2, '0') END AS 往来单位,
                strftime({from} + floor(r3 * 790)::INT, '%Y-%m-%d') AS 日期,
                CASE WHEN r1 < 0.65 THEN (CASE WHEN r4 < 0.55 THEN '应收' ELSE '收款' END) ELSE (CASE WHEN r4 < 0.55 THEN '应付' ELSE '付款' END) END AS 类型,
                round(500 + random() * 49500, 2) AS 金额
         FROM t",
        from = day(from),
    );
    vec![
        Sample {
            name: "订单",
            sql: orders,
            kinds: &[("订单号", "id"), ("数量", "integer"), ("订单金额", "money"), ("下单时间", "datetime"), ("已退款", "boolean")],
            template: "builtin_monthly_ops",
            key: "$orders",
            workbook: "示例：月度经营分析",
        },
        Sample { name: "库存流水", sql: stock, kinds: &[("日期", "datetime"), ("数量", "number")], template: "builtin_stock", key: "$stock", workbook: "示例：库存进销存" },
        Sample { name: "往来明细", sql: partners, kinds: &[("日期", "datetime"), ("金额", "money")], template: "builtin_ar_ap", key: "$partners", workbook: "示例：应收应付对账" },
    ]
}

/// 生成示例数据和工作簿（已经有了就跳过），返回要打开的工作簿 id 与要打印的说明
pub fn seed(state: &crate::routes::AppState, conn: &Connection, rows: u64) -> anyhow::Result<(String, Vec<String>)> {
    let app = |e: crate::error::AppError| anyhow::anyhow!("{}", e.message());
    let existing: HashMap<String, String> =
        crate::workbook::list(conn).map_err(app)?.into_iter().filter(|w| w.name.starts_with(PREFIX)).map(|w| (w.name, w.id)).collect();
    let list = samples(rows, crate::db::today_local());
    if let Some(id) = existing.get(list[0].workbook) {
        return Ok((id.clone(), vec!["示例数据已经生成过（--reset 重新生成）".into()]));
    }
    let mut lines = Vec::new();
    let mut first = None;
    for s in &list {
        let started = std::time::Instant::now();
        let csv = state.tmp_dir.join(format!("demo-{}.csv", s.key.trim_start_matches('$')));
        write_csv(conn, &s.sql, &csv)?;
        let kinds: HashMap<&str, &str> = s.kinds.iter().copied().collect();
        let opened = crate::open::import_file(conn, &csv, s.name, &state.tmp_dir, &kinds);
        let _ = std::fs::remove_file(&csv);
        let opened = opened?;
        let input: crate::template::UseInput = serde_json::from_value(serde_json::json!({
            "name": s.workbook, "bindings": { s.key: { "datasetId": opened.id } }, "operator": "示例",
        }))?;
        let used = crate::template::use_template(state, conn, s.template, input).map_err(app)?;
        for w in &used.warnings {
            tracing::warn!("示例工作簿「{}」：{w}", s.workbook);
        }
        lines.push(format!("{}（{:.1} 秒）→ 工作簿「{}」", opened.summary, started.elapsed().as_secs_f64(), s.workbook));
        first.get_or_insert(used.workbook.id);
    }
    Ok((first.unwrap_or_default(), lines))
}

fn write_csv(conn: &Connection, sql: &str, path: &Path) -> anyhow::Result<()> {
    conn.execute_batch("SELECT setseed(0.42)")?;
    conn.execute_batch(&format!("COPY ({sql}) TO {} (HEADER, DELIMITER ',')", literal(&path.display().to_string())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn args() {
        let a = parse_args(&s(&["--rows", "20_000", "--port", "9000", "--db", "/tmp/d.duckdb", "--reset", "--no-browser"])).unwrap();
        assert_eq!(a, DemoArgs { rows: 20_000, port: Some(9000), db: PathBuf::from("/tmp/d.duckdb"), reset: true, browser: false });
        assert_eq!(parse_args(&[]).unwrap().rows, DEFAULT_ROWS);
        assert!(parse_args(&s(&["--rows", "10"])).is_err());
        assert!(parse_args(&s(&["订单.csv"])).is_err());
        assert!(parse_args(&s(&["--port"])).is_err());
    }

    #[test]
    fn sample_sql_runs_and_fits_templates() {
        let conn = Connection::open_in_memory().unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        for s in samples(5_000, today) {
            let (n, last): (i64, String) = conn.query_row(&format!("SELECT count(*), max({}::VARCHAR) FROM ({})", if s.name == "订单" { "下单时间" } else { "日期" }, s.sql), [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            assert!(n >= 5_000, "{}：{n}", s.name);
            assert!(last.as_str() <= "2026-10-24 23:59:59" && last.as_str() >= "2026-09", "{}：最近的数据应接近今天：{last}", s.name);
        }
        let types: Vec<String> = {
            let sql = &samples(1_000, today)[2].sql;
            let mut st = conn.prepare(&format!("SELECT DISTINCT 类型 FROM ({sql}) ORDER BY 1")).unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(types, vec!["付款", "应付", "应收", "收款"]);
    }
}
