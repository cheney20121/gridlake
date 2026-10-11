//! 千万行性能实测：用真实资金流水复制出 1000 万行，测量大数据表需要的各类查询耗时
//!
//! cargo run --release --example bench_10m -- <源库路径> <测试库路径>
//! 源库只读挂载，不会被修改

use std::time::Instant;

use duckdb::Connection;

const TARGET_ROWS: i64 = 10_000_000;

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    println!("{:>8.1} ms  {label}", t.elapsed().as_secs_f64() * 1000.0);
    r
}

fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or_else(|e| panic!("{e}\n{sql}"))
}

fn rows(conn: &Connection, sql: &str) -> usize {
    let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{e}\n{sql}"));
    let mut n = 0;
    let mut rs = stmt.query([]).unwrap();
    while rs.next().unwrap().is_some() {
        n += 1;
    }
    n
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = args.get(1).expect("源库路径");
    let dst = args.get(2).expect("测试库路径");
    let fresh = !std::path::Path::new(dst).exists();
    let conn = Connection::open(dst).unwrap();

    if fresh {
        conn.execute_batch(&format!("ATTACH '{src}' AS src (READ_ONLY)")).unwrap();
        let base = scalar(&conn, "SELECT count(*) FROM src.ledger_detail");
        let copies = (TARGET_ROWS + base - 1) / base;
        println!("源数据 {base} 行，复制 {copies} 份");
        time("建表并复制到千万行", || {
            // 每份视为一家店：店铺名、流水号加后缀，保证主键不重复
            conn.execute_batch(&format!(
                "CREATE TABLE t AS
                 SELECT d.* REPLACE (d.shop || '-' || c.i AS shop, d.\"动帐流水号\" || '-' || c.i AS \"动帐流水号\")
                 FROM src.ledger_detail d, range({copies}) c(i)
                 LIMIT {TARGET_ROWS}"
            ))
            .unwrap();
        });
        conn.execute_batch("DETACH src; CHECKPOINT").unwrap();
    }
    println!("总行数 {}，文件 {:.1} GB", scalar(&conn, "SELECT count(*) FROM t"), std::fs::metadata(dst).unwrap().len() as f64 / 1e9);
    let cols = "shop, \"动账时间\", \"动帐流水号\", \"动账方向\", \"动账金额\", \"动账场景\", \"计费类型\", \"订单号\", \"商品名称\", \"达人名称\", \"订单实付应结\", \"佣金\", \"商户主体名称\"";

    println!("\n── 按自然顺序翻页（每页 200 行）");
    for off in [0i64, 5_000_000, 9_999_000] {
        time(&format!("OFFSET {off}（LIMIT/OFFSET）"), || rows(&conn, &format!("SELECT {cols} FROM t LIMIT 200 OFFSET {off}")));
    }
    for off in [0i64, 5_000_000, 9_999_000] {
        time(&format!("rowid 区间 {off}"), || rows(&conn, &format!("SELECT {cols} FROM t WHERE rowid BETWEEN {off} AND {} ", off + 199)));
    }

    println!("\n── 排序后翻页");
    time("按金额倒序 第 1 页", || rows(&conn, &format!("SELECT {cols} FROM t ORDER BY \"动账金额\" DESC LIMIT 200")));
    time("按金额倒序 第 500 万行（每次重新排序）", || rows(&conn, &format!("SELECT {cols} FROM t ORDER BY \"动账金额\" DESC LIMIT 200 OFFSET 5000000")));
    time("物化排序位置表（排序一次）", || {
        conn.execute_batch("CREATE OR REPLACE TEMP TABLE pos AS SELECT row_number() OVER (ORDER BY \"动账金额\" DESC, rowid) - 1 AS p, rowid AS rid FROM t")
            .unwrap()
    });
    for off in [0i64, 5_000_000, 9_999_000] {
        time(&format!("位置表取第 {off} 行起"), || {
            rows(&conn, &format!("SELECT {cols} FROM pos JOIN t ON t.rowid = pos.rid WHERE pos.p BETWEEN {off} AND {} ORDER BY pos.p", off + 199))
        });
    }

    println!("\n── 筛选");
    time("关键字搜索 计数", || scalar(&conn, "SELECT count(*) FROM t WHERE search_text LIKE '%某某%'"));
    time("关键字搜索 第 1 页", || rows(&conn, &format!("SELECT {cols} FROM t WHERE search_text LIKE '%某某%' LIMIT 200")));
    time("条件筛选 计数（场景 + 金额区间）", || {
        scalar(&conn, "SELECT count(*) FROM t WHERE \"动账场景\" = '货款结算入账' AND \"动账金额\" BETWEEN 100 AND 500")
    });
    time("店铺等值筛选 计数", || scalar(&conn, "SELECT count(*) FROM t WHERE shop = '某某店-7'"));

    println!("\n── 聚合");
    time("店铺 × 场景 × 方向 汇总（汇总表数据）", || {
        rows(&conn, "SELECT shop, \"动账场景\", \"动账方向\", sum(\"动账金额\") FROM t GROUP BY ALL")
    });
    time("场景分面（值 + 计数）", || rows(&conn, "SELECT \"动账场景\", count(*) FROM t GROUP BY 1"));
    time("达人分面（去重值数量多）", || rows(&conn, "SELECT \"达人名称\", count(*) FROM t GROUP BY 1 ORDER BY 2 DESC LIMIT 100"));
    time("选区统计：前 500 万行金额求和", || scalar(&conn, "SELECT CAST(sum(\"动账金额\") AS BIGINT) FROM t WHERE rowid < 5000000"));
    time("SUMIFS 等价：某店某场景求和", || {
        scalar(&conn, "SELECT CAST(COALESCE(sum(\"动账金额\"), 0) AS BIGINT) FROM t WHERE shop = '某某店-7' AND \"动账场景\" LIKE '%退款%'")
    });
}
