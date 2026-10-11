//! 生成千万行测试库：把资金流水复制成多家店铺（店铺名、流水号加后缀）
//! cargo run --release --example make_big -- <库路径>（先把真实库复制一份再运行，不要指向正式库）
use duckdb::Connection;

fn main() {
    let path = std::env::args().nth(1).expect("库路径");
    let conn = Connection::open(&path).unwrap();
    let base: i64 = conn.query_row("SELECT count(*) FROM ledger_detail", [], |r| r.get(0)).unwrap();
    let copies = (10_000_000 - base + base - 1) / base;
    conn.execute_batch(&format!(
        "INSERT INTO ledger_detail
         SELECT d.* REPLACE (d.shop || '-' || c.i AS shop, d.\"动帐流水号\" || '-' || c.i AS \"动帐流水号\")
         FROM (SELECT * FROM ledger_detail) d, range(1, {copies} + 1) c(i)
         LIMIT {};
         INSERT INTO ledger_shop (name) SELECT DISTINCT shop FROM ledger_detail d WHERE NOT EXISTS (SELECT 1 FROM ledger_shop s WHERE s.name = d.shop);
         CHECKPOINT;",
        10_000_000 - base
    ))
    .unwrap();
    let n: i64 = conn.query_row("SELECT count(*) FROM ledger_detail", [], |r| r.get(0)).unwrap();
    println!("ledger_detail {n} 行");
}
