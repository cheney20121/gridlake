//! 把已导入的数据集放大到指定行数（亿级压测用）：反复复制基础数据，订单号加后缀、金额随机浮动
//! cargo run --release --example inflate -- <库路径> <表名> <目标行数>
//! 先停掉服务（DuckDB 同一时间只允许一个进程写）；不要指向正式库。
use std::time::Instant;

use duckdb::Connection;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (path, table, target) = (&a[1], &a[2], a[3].parse::<i64>().unwrap());
    let conn = Connection::open(path).unwrap();
    let base: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap();
    conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE base AS SELECT * EXCLUDE (search_text) FROM {table}; SET preserve_insertion_order = false;")).unwrap();
    let cols: Vec<String> = {
        let mut stmt = conn.prepare(&format!("SELECT column_name FROM information_schema.columns WHERE table_name = '{table}' ORDER BY ordinal_position")).unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|x| x.unwrap()).filter(|c| c != "search_text").collect()
    };
    let list = cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ");
    let mut have = base;
    let mut i = 1;
    let t0 = Instant::now();
    while have < target {
        let take = (target - have).min(base);
        let t = Instant::now();
        conn.execute_batch(&format!(
            "INSERT INTO {table} ({list}, search_text)
             SELECT b.* REPLACE (b.\"订单号\" || '-{i}' AS \"订单号\", round(b.\"金额\" * (0.8 + (hash(b.\"订单号\" || '{i}') % 41) / 100.0), 2) AS \"金额\"), NULL
             FROM base b LIMIT {take}"
        ))
        .unwrap();
        have += take;
        println!("第 {i} 批 +{take} 行，累计 {have}，本批 {:.1}s，共 {:.0}s", t.elapsed().as_secs_f64(), t0.elapsed().as_secs_f64());
        i += 1;
        if i % 5 == 0 {
            conn.execute_batch("CHECKPOINT").unwrap();
        }
    }
    conn.execute_batch("CHECKPOINT").unwrap();
    let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap();
    println!("{table} 共 {n} 行，库文件 {:.2} GB", std::fs::metadata(path).unwrap().len() as f64 / 1e9);
}
