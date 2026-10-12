use duckdb::Connection;

use super::*;
use crate::dataset::StoredColumn;

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
    conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
    conn
}

/// 建一个数据集：cols = [(列名, 类型)]，rows = SQL VALUES 文本
fn table(conn: &Connection, id: &str, name: &str, cols: &[(&str, &str)], rows: &str) {
    let defs = cols.iter().map(|(c, k)| format!("\"{c}\" {}", if matches!(*k, "money" | "number") { "DOUBLE" } else { "VARCHAR" })).collect::<Vec<_>>().join(", ");
    conn.execute_batch(&format!("CREATE TABLE {id} ({defs}, search_text VARCHAR); INSERT INTO {id} VALUES {rows};")).unwrap();
    let stored: Vec<StoredColumn> = cols.iter().map(|(c, k)| StoredColumn { key: c.to_string(), title: c.to_string(), kind: k.to_string(), width: 100, source: c.to_string() }).collect();
    dataset::register(conn, id, name, &stored, &[]).unwrap();
}

fn side(ds: &str, label: &str, keys: &[&str]) -> Side {
    Side { dataset: ds.into(), label: label.into(), amount: "金额".into(), sign: 1, date: Some("日期".into()), keys: keys.iter().map(|s| s.to_string()).collect(), memo: Some("摘要".into()) }
}

const COLS: &[(&str, &str)] = &[("日期", "datetime"), ("金额", "money"), ("单号", "text"), ("摘要", "text")];

/// 银行 / 账面两个数据集；rows 的第 4 列是 search_text 占位
fn fixture(bank: &str, book: &str) -> Connection {
    let conn = db();
    table(&conn, "ds_bank", "银行流水", COLS, bank);
    table(&conn, "ds_book", "企业账", COLS, book);
    conn
}

fn def(keys: &[&str], tolerance: f64, days: Option<u32>, group: bool) -> ReconDef {
    ReconDef { left: side("ds_bank", "银行", keys), right: side("ds_book", "账面", keys), tolerance, days, group_by_key: group }
}

fn status(conn: &Connection, id: &str) -> Vec<(String, String, f64, String)> {
    let mut stmt = conn.prepare(&format!("SELECT \"来源\", \"日期\", \"金额\", \"状态\" FROM {id} ORDER BY \"来源\", \"日期\", \"金额\", \"行标识\"")).unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

fn open_count(conn: &Connection, id: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {id} WHERE \"状态\" = '未勾对'"), [], |r| r.get(0)).unwrap()
}

#[test]
fn exact_pass_pairs_equal_amounts_in_date_order() {
    // 100 元三笔对两笔：只能勾对两笔，剩一笔银行未达；金额不同的不勾
    let conn = fixture(
        "('2026-09-01', 100, 'A', 'x', NULL), ('2026-09-02', 100, 'B', 'x', NULL), ('2026-09-03', 100, 'C', 'x', NULL), ('2026-09-04', 55, 'D', 'x', NULL)",
        "('2026-09-01', 100, 'A', 'y', NULL), ('2026-09-03', 100, 'B', 'y', NULL), ('2026-09-05', 77, 'E', 'y', NULL)",
    );
    let id = create(&conn, "银行对账", &def(&[], 0.0, None, false)).unwrap();
    let ctx = context(&conn, &id).unwrap();
    let r = auto_match(&conn, &ctx, "t", false).unwrap();
    assert_eq!((r.exact, r.tolerance, r.sum, r.matched_rows), (2, 0, 0, 4));
    assert_eq!((r.left_open, r.right_open), (2, 1));
    // 按日期顺序一一对应：银行 09-01/09-02 对 账面 09-01/09-03，银行 09-03 未达
    let open: Vec<_> = status(&conn, &id).into_iter().filter(|x| x.3 == "未勾对").collect();
    assert_eq!(open.len(), 3);
    assert!(open.contains(&("银行".into(), "2026-09-03".into(), 100.0, "未勾对".into())));
    assert!(open.contains(&("银行".into(), "2026-09-04".into(), 55.0, "未勾对".into())));
    assert!(open.contains(&("账面".into(), "2026-09-05".into(), 77.0, "未勾对".into())));
    // 再跑一次：没有新的可勾对
    assert_eq!(auto_match(&conn, &ctx, "t", false).unwrap().groups, 0);
}

#[test]
fn keys_must_match_and_blank_keys_are_skipped() {
    let conn = fixture(
        "('2026-09-01', 100, 'A', 'x', NULL), ('2026-09-01', 100, 'B', 'x', NULL), ('2026-09-01', 100, NULL, 'x', NULL)",
        "('2026-09-01', 100, 'B', 'y', NULL), ('2026-09-01', 100, 'C', 'y', NULL), ('2026-09-01', 100, NULL, 'y', NULL)",
    );
    let id = create(&conn, "按单号", &def(&["单号"], 0.0, None, false)).unwrap();
    let ctx = context(&conn, &id).unwrap();
    let r = auto_match(&conn, &ctx, "t", false).unwrap();
    // 只有单号 B 对得上；A、C 单号不同，空单号不参与
    assert_eq!(r.exact, 1);
    assert_eq!(open_count(&conn, &id), 4);
}

#[test]
fn tolerance_and_date_window_prefer_the_closest() {
    // 银行 100.00，账面有 100.04（差 0.04）和 100.01（差 0.01）：容差 0.05 内取差得最少的
    let conn = fixture("('2026-09-01', 100.00, 'x', 'x', NULL)", "('2026-09-01', 100.04, 'x', 'y', NULL), ('2026-09-01', 100.01, 'x', 'y', NULL)");
    let id = create(&conn, "容差", &def(&[], 0.05, None, false)).unwrap();
    let ctx = context(&conn, &id).unwrap();
    let r = auto_match(&conn, &ctx, "t", false).unwrap();
    assert_eq!((r.exact, r.tolerance), (0, 1));
    let left: Vec<_> = status(&conn, &id).into_iter().filter(|x| x.3 == "未勾对").collect();
    assert_eq!(left, vec![("账面".to_string(), "2026-09-01".to_string(), 100.04, "未勾对".to_string())]);
    // 差额记在组上
    let diff: i64 = conn.query_row("SELECT diff_cents FROM ledger_recon_group", [], |r| r.get(0)).unwrap();
    assert_eq!(diff, -1);

    // 日期窗口：同样金额，相差 5 天，窗口 3 天不勾，窗口 5 天勾
    let conn = fixture("('2026-09-01', 50, 'x', 'x', NULL)", "('2026-09-06', 50, 'x', 'y', NULL)");
    let id = create(&conn, "窗口", &def(&[], 0.0, Some(3), false)).unwrap();
    assert_eq!(auto_match(&conn, &context(&conn, &id).unwrap(), "t", false).unwrap().groups, 0);
    update(&conn, &id, "窗口", &def(&[], 0.0, Some(5), false), false).unwrap();
    assert_eq!(auto_match(&conn, &context(&conn, &id).unwrap(), "t", false).unwrap().groups, 1);
}

#[test]
fn window_rejects_ordinal_pairs_but_second_pass_recovers_better_ones() {
    // 银行 100@1日、100@20日；账面 100@18日。按顺序 1日↔18日 超出窗口 3 天，但 20日↔18日 在窗口内
    let conn = fixture("('2026-09-01', 100, 'x', 'x', NULL), ('2026-09-20', 100, 'x', 'x', NULL)", "('2026-09-18', 100, 'x', 'y', NULL)");
    let id = create(&conn, "窗口", &def(&[], 0.0, Some(3), false)).unwrap();
    let r = auto_match(&conn, &context(&conn, &id).unwrap(), "t", false).unwrap();
    assert_eq!((r.exact, r.tolerance), (1, 0), "金额完全相同，只是被窗口挡掉了顺序对应：仍算「自动」");
    let open: Vec<_> = status(&conn, &id).into_iter().filter(|x| x.3 == "未勾对").collect();
    assert_eq!(open, vec![("银行".to_string(), "2026-09-01".to_string(), 100.0, "未勾对".to_string())]);
}

#[test]
fn sum_pass_matches_many_to_many_under_one_key() {
    // 订单 O1：银行一笔 300，账面三笔 100+120+80；订单 O2 合计对不上
    let conn = fixture(
        "('2026-09-01', 300, 'O1', 'x', NULL), ('2026-09-01', 50, 'O2', 'x', NULL)",
        "('2026-09-01', 100, 'O1', 'y', NULL), ('2026-09-02', 120, 'O1', 'y', NULL), ('2026-09-03', 80, 'O1', 'y', NULL), ('2026-09-01', 49, 'O2', 'y', NULL)",
    );
    let id = create(&conn, "汇总", &def(&["单号"], 0.0, None, true)).unwrap();
    let r = auto_match(&conn, &context(&conn, &id).unwrap(), "t", false).unwrap();
    assert_eq!((r.exact, r.tolerance, r.sum, r.matched_rows), (0, 0, 1, 4));
    assert_eq!(open_count(&conn, &id), 2);
    // 不开汇总则不勾
    let conn = fixture("('2026-09-01', 300, 'O1', 'x', NULL)", "('2026-09-01', 100, 'O1', 'y', NULL), ('2026-09-02', 200, 'O1', 'y', NULL)");
    let id = create(&conn, "汇总", &def(&["单号"], 0.0, None, false)).unwrap();
    assert_eq!(auto_match(&conn, &context(&conn, &id).unwrap(), "t", false).unwrap().groups, 0);
}

#[test]
fn dry_run_saves_nothing() {
    let conn = fixture("('2026-09-01', 10, 'x', 'x', NULL)", "('2026-09-01', 10, 'x', 'y', NULL)");
    let id = create(&conn, "试算", &def(&[], 0.0, None, false)).unwrap();
    let ctx = context(&conn, &id).unwrap();
    let r = auto_match(&conn, &ctx, "t", true).unwrap();
    assert!(r.dry_run);
    assert_eq!(r.groups, 1);
    assert_eq!(open_count(&conn, &id), 2);
    assert_eq!(auto_match(&conn, &ctx, "t", false).unwrap().groups, 1);
    assert_eq!(open_count(&conn, &id), 0);
}

#[test]
fn manual_match_checks_totals_and_requires_a_reason_for_differences() {
    let conn = fixture("('2026-09-01', 300, 'x', 'x', NULL), ('2026-09-02', 99.5, 'x', 'x', NULL)", "('2026-09-01', 100, 'x', 'y', NULL), ('2026-09-01', 200, 'x', 'y', NULL), ('2026-09-02', 100, 'x', 'y', NULL)");
    let id = create(&conn, "手工", &def(&[], 0.0, None, false)).unwrap();
    let ctx = context(&conn, &id).unwrap();
    // 行标识在匹配前先取好（匹配后这些行就不是「未勾对」了）；账面有两笔 100，按日期区分
    let key = |label: &str, amount: f64, date: &str| -> String {
        conn.query_row(&format!("SELECT \"行标识\" FROM {id} WHERE \"来源\" = ? AND \"金额\" = ? AND \"日期\" = ?"), duckdb::params![label, amount, date], |r| r.get(0)).unwrap()
    };
    let (b300, b995) = (key("银行", 300.0, "2026-09-01"), key("银行", 99.5, "2026-09-02"));
    let (k100a, k200, k100b) = (key("账面", 100.0, "2026-09-01"), key("账面", 200.0, "2026-09-01"), key("账面", 100.0, "2026-09-02"));
    // 一对多：300 = 100 + 200
    let m = manual_match(&conn, &ctx, &ManualRequest { left: vec![b300.clone()], right: vec![k100a.clone(), k200.clone()], note: None, force: false }, "t").unwrap();
    assert_eq!((m.left_total, m.right_total, m.diff), (300.0, 300.0, 0.0));
    // 被勾对过的行不能再选
    let again = manual_match(&conn, &ctx, &ManualRequest { left: vec![b300.clone()], right: vec![k100a.clone()], note: None, force: false }, "t");
    assert!(again.err().unwrap().message().contains("已经被勾对"));
    // 99.5 对 100：差 -0.5，超过容差：要 force + 原因
    let req = |force: bool, note: Option<&str>| ManualRequest { left: vec![b995.clone()], right: vec![k100b.clone()], note: note.map(String::from), force };
    let e = manual_match(&conn, &ctx, &req(false, None), "t").err().unwrap();
    assert!(e.message().contains("超过了容差"), "{:?}", e.message());
    let e = manual_match(&conn, &ctx, &req(true, None), "t").err().unwrap();
    assert!(e.message().contains("原因"));
    let m = manual_match(&conn, &ctx, &req(true, Some("银行手续费 0.5")), "t").unwrap();
    assert_eq!(m.diff, -0.5);
    assert_eq!(open_count(&conn, &id), 0);
    // 差额记在组的第一行上，勾对备注可见
    let (diff, note): (f64, String) = conn.query_row(&format!("SELECT sum(\"勾对差额\"), max(\"勾对备注\") FROM {id} WHERE \"勾对组\" = {}", m.gno), [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((diff, note.as_str()), (-0.5, "银行手续费 0.5"));
    // 只有一边不能勾对
    assert!(manual_match(&conn, &ctx, &ManualRequest { left: vec![b300.clone()], right: vec![], note: None, force: false }, "t").is_err());
    // 取消
    let u = unmatch(&conn, &ctx, &UnmatchRequest { groups: vec![m.gno], mode: None }).unwrap();
    assert_eq!((u.groups, u.rows), (1, 2));
    assert_eq!(open_count(&conn, &id), 2);
    let u = unmatch(&conn, &ctx, &UnmatchRequest { groups: vec![], mode: Some("all".into()) }).unwrap();
    assert_eq!(u.groups, 1);
    assert_eq!(open_count(&conn, &id), 5);
}

#[test]
fn matches_survive_reimport_but_not_edits() {
    let conn = fixture(
        "('2026-09-01', 10, 'A', 'x', NULL), ('2026-09-02', 20, 'B', 'x', NULL), ('2026-09-03', 30, 'C', 'x', NULL)",
        "('2026-09-01', 10, 'A', 'y', NULL), ('2026-09-02', 20, 'B', 'y', NULL), ('2026-09-03', 30, 'C', 'y', NULL)",
    );
    let id = create(&conn, "稳定", &def(&["单号"], 0.0, None, false)).unwrap();
    let ctx = context(&conn, &id).unwrap();
    assert_eq!(auto_match(&conn, &ctx, "t", false).unwrap().groups, 3);

    // 整表删除后按相反顺序重新导入（rowid 全变）：勾对不受影响
    conn.execute_batch("DELETE FROM ds_bank; INSERT INTO ds_bank VALUES ('2026-09-03', 30, 'C', 'x', NULL), ('2026-09-02', 20, 'B', 'x', NULL), ('2026-09-01', 10, 'A', 'x', NULL);").unwrap();
    assert_eq!(open_count(&conn, &id), 0);
    assert_eq!(summary(&conn, &ctx, None).unwrap().groups.broken, 0);

    // 改了一行的金额：这一组失效，两侧回到未勾对并有提示
    conn.execute_batch("UPDATE ds_bank SET \"金额\" = 21 WHERE \"单号\" = 'B'").unwrap();
    assert_eq!(open_count(&conn, &id), 2);
    let tip: i64 = conn.query_row(&format!("SELECT count(*) FROM {id} WHERE \"提示\" IS NOT NULL"), [], |r| r.get(0)).unwrap();
    assert_eq!(tip, 1, "只有还在的那一行（账面）带提示");
    assert_eq!(summary(&conn, &ctx, None).unwrap().groups.broken, 1);
    // 再自动勾对：先清掉失效的组，21 对 20 超出容差 0，不勾
    let r = auto_match(&conn, &ctx, "t", false).unwrap();
    assert_eq!((r.cleared_broken, r.groups), (1, 0));
    assert_eq!(summary(&conn, &ctx, None).unwrap().groups.broken, 0);

    // 整行删除：同样失效
    conn.execute_batch("DELETE FROM ds_book WHERE \"单号\" = 'A'").unwrap();
    assert_eq!(open_count(&conn, &id), 3);
}

#[test]
fn outstanding_as_of_counts_matches_that_settle_later() {
    // 企业 09-30 付了 500，银行 10-02 才扣：截至 09-30 这是未达账项，截至 10-02 才算两清
    let conn = fixture("('2026-09-10', 100, 'a', 'x', NULL), ('2026-10-02', -500, 'b', 'x', NULL)", "('2026-09-10', 100, 'a', 'y', NULL), ('2026-09-30', -500, 'b', 'y', NULL)");
    let id = create(&conn, "截至", &def(&[], 0.0, None, false)).unwrap();
    let ctx = context(&conn, &id).unwrap();
    assert_eq!(auto_match(&conn, &ctx, "t", false).unwrap().exact, 2);

    let s = summary(&conn, &ctx, Some("2026-09-30")).unwrap();
    assert_eq!((s.left.all.rows, s.right.all.rows), (1, 2), "银行 10-02 的不在范围内");
    assert_eq!((s.right.open_out.rows, s.right.open_out.total), (1, -500.0), "账面 09-30 付的 500，银行还没扣");
    assert_eq!((s.left.open_in.rows, s.left.open_out.rows), (0, 0));
    let s = summary(&conn, &ctx, Some("2026-10-02")).unwrap();
    assert_eq!((s.right.open_out.rows, s.left.open_out.rows), (0, 0));

    // 视图里的「勾对完成日」让公式也能表达同一个口径
    let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {id} WHERE \"日期\" <= '2026-09-30' AND \"勾对完成日\" > '2026-09-30'"), [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn validation_messages() {
    let conn = fixture("('2026-09-01', 1, 'x', 'x', NULL)", "('2026-09-01', 1, 'x', 'y', NULL)");
    let bad = |d: ReconDef| create(&conn, "x", &d).err().unwrap().message().to_string();
    let mut d = def(&[], 0.0, None, false);
    d.right.dataset = "ds_bank".into();
    assert!(bad(d).contains("同一个数据集"));
    let mut d = def(&[], 0.0, None, false);
    d.right.label = "银行".into();
    assert!(bad(d).contains("名称不能相同"));
    let mut d = def(&[], 0.0, None, false);
    d.left.amount = "摘要".into();
    assert!(bad(d).contains("不是数值列"));
    let mut d = def(&[], 0.0, None, false);
    d.left.date = Some("摘要".into());
    assert!(bad(d).contains("不是日期列"));
    let mut d = def(&["单号"], 0.0, None, false);
    d.right.keys = vec![];
    assert!(bad(d).contains("个数要一样"));
    assert!(bad(def(&[], 0.0, None, true)).contains("对账键"));
    let mut d = def(&[], 0.0, Some(3), false);
    d.left.date = None;
    assert!(bad(d).contains("日期列"));
    let mut d = def(&[], 0.0, None, false);
    d.left.sign = 2;
    assert!(bad(d).contains("金额方向"));
    assert!(bad(def(&[], -1.0, None, false)).contains("容差"));
}

#[test]
fn flipped_sign_and_calculated_amount() {
    // 企业账的收款记为负（贷方），取反后与银行收入对上
    let conn = fixture("('2026-09-01', 100, 'x', 'x', NULL)", "('2026-09-01', -100, 'x', 'y', NULL)");
    let mut d = def(&[], 0.0, None, false);
    d.right.sign = -1;
    let id = create(&conn, "取反", &d).unwrap();
    assert_eq!(auto_match(&conn, &context(&conn, &id).unwrap(), "t", false).unwrap().exact, 1);
    let sum: f64 = conn.query_row(&format!("SELECT sum(\"金额\") FROM {id} WHERE \"来源\" = '账面'"), [], |r| r.get(0)).unwrap();
    assert_eq!(sum, 100.0, "结果里的金额是统一符号之后的");

    // 预览提示符号相反
    let d = def(&[], 0.0, None, false);
    let p = preview(&conn, &d).unwrap();
    assert!(p.warnings.iter().any(|w| w.contains("符号相反")), "{:?}", p.warnings);
}

#[test]
fn recon_dataset_integrates_with_dataset_lifecycle() {
    let conn = fixture("('2026-09-01', 10, 'x', 'x', NULL)", "('2026-09-01', 10, 'x', 'y', NULL)");
    let id = create(&conn, "银行对账", &def(&[], 0.0, None, false)).unwrap();
    // 出现在数据集清单里，列固定
    let defs = dataset::datasets(&conn).unwrap();
    let d = defs.iter().find(|d| d.id == id).unwrap();
    assert_eq!(d.kind, "recon");
    assert!(d.columns.iter().any(|c| c.title == "勾对完成日"));
    // 重名
    assert!(create(&conn, "银行对账", &def(&[], 0.0, None, false)).err().unwrap().message().contains("已存在"));
    // 被引用的数据集不能删
    let e = dataset::delete(&conn, "ds_bank", "t").err().unwrap();
    assert!(e.message().contains("银行对账"), "{:?}", e.message());
    // 不能填报
    assert!(crate::writeback::submit(&conn, &id, &crate::writeback::WritebackRequest { mode: "append".into(), columns: vec!["金额".into()], rows: vec![], keys: vec![], operator: String::new(), source: None }).err().unwrap().message().contains("对账结果"));
    // 加了勾对后换金额列要确认
    auto_match(&conn, &context(&conn, &id).unwrap(), "t", false).unwrap();
    let mut d2 = def(&[], 0.0, None, false);
    d2.left.sign = -1;
    assert!(update(&conn, &id, "银行对账", &d2, false).err().unwrap().message().contains("清空勾对"));
    update(&conn, &id, "银行对账2", &d2, true).unwrap();
    assert_eq!(matches_count(&conn, &id).unwrap(), 0);
    // 回收站：删除、恢复、彻底删除
    dataset::delete(&conn, &id, "t").unwrap();
    assert!(dataset::dataset(&conn, &id).is_err());
    dataset::delete(&conn, "ds_bank", "t").unwrap();
    assert!(crate::dataset_trash::restore(&conn, &id).err().unwrap().message().contains("银行流水"));
    crate::dataset_trash::restore(&conn, "ds_bank").unwrap();
    crate::dataset_trash::restore(&conn, &id).unwrap();
    assert!(dataset::dataset(&conn, &id).is_ok());
    dataset::delete(&conn, &id, "t").unwrap();
    assert_eq!(crate::dataset_trash::purge(&conn, &id).unwrap(), vec!["银行对账2".to_string()]);
    assert!(conn.execute_batch(&format!("SELECT * FROM {id}")).is_err());
}
