//! 电商资金台账场景包（`LEDGER_PACKS=ecommerce`，默认只在库里已有台账数据时自动启用）
//!
//! 项目最早的使用场景：导入抖店资金流水，按店铺 × 月份汇总，做月结（完整性 / 余额勾稽 / 提现到账 / 审批锁账）、
//! 银行到账对账、经营分析。这些功能和核心（数据集 / 透视表 / 公式 / SQL / 仪表盘）是分开的：
//!
//! - 代码都在本目录与 `routes/ecommerce.rs`，路由在 [`router`] 里，没启用时不注册（访问得到 404）；
//! - 内置的「资金流水」数据集、电商内置模板也只在启用时出现（见 `dataset::load`、`template`）；
//! - 核心代码不依赖这里的业务逻辑；通用的表格文件读取、取值转换在 `tabular.rs`。

#[cfg(not(test))]
use std::sync::OnceLock;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use duckdb::Connection;

use crate::config::PackSetting;
use crate::routes::{self, AppState};

pub mod analysis;
pub mod balance;
pub mod bank;
pub mod close;
pub mod coverage;
pub mod detail;
pub mod import;
pub mod query;
pub mod shop;

#[cfg(not(test))]
static ENABLED: OnceLock<bool> = OnceLock::new();

/// 启动时确定一次：显式设置 LEDGER_PACKS 就按设置；没设置时，库里已经有电商台账数据才启用（老部署升级后功能不会凭空消失），
/// 新库默认不启用
pub fn init(setting: &PackSetting, conn: &Connection) -> bool {
    let on = match setting {
        PackSetting::Only(v) => v.iter().any(|p| p == "ecommerce"),
        PackSetting::Auto => has_data(conn),
    };
    #[cfg(not(test))]
    let _ = ENABLED.set(on);
    on
}

/// 电商场景包是否启用。**启动完成前（没有 `init`）视为启用**：只有单元测试会走到这条路径，
/// 它们直接拿内存库测台账逻辑；真实运行里 `Db::open` 一定先调用 `init`
pub fn enabled() -> bool {
    // 单元测试并行跑，进程级的开关会互相影响：测试里改用线程局部的，默认启用
    #[cfg(test)]
    return TEST_ENABLED.with(std::cell::Cell::get);
    #[cfg(not(test))]
    return ENABLED.get().copied().unwrap_or(true);
}

#[cfg(test)]
thread_local! {
    static TEST_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// 测试里在「场景包未启用」的状态下运行一段代码（只影响当前线程）
#[cfg(test)]
pub fn with_enabled<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let prev = TEST_ENABLED.with(|c| c.replace(on));
    let out = f();
    TEST_ENABLED.with(|c| c.set(prev));
    out
}

/// 库里有没有电商台账数据（明细、店铺档案、银行流水、余额、月结记录任何一类）
fn has_data(conn: &Connection) -> bool {
    ["ledger_detail", "ledger_shop", "ledger_bank_txn", "ledger_balance", "ledger_period_close"]
        .iter()
        .any(|t| conn.query_row(&format!("SELECT EXISTS (SELECT 1 FROM {t})"), [], |r| r.get::<_, bool>(0)).unwrap_or(false))
}

/// 场景包的 HTTP 路由：导入明细、店铺档案、月结、余额勾稽、提现 / 银行对账、经营分析
pub fn router(max_upload_mb: usize) -> Router<AppState> {
    Router::new()
        .route(
            "/ledger/imports",
            post(routes::import_csv).layer(DefaultBodyLimit::max(max_upload_mb * 1024 * 1024)).get(routes::imports),
        )
        .route(
            "/ledger/imports/preview",
            post(routes::preview_import).layer(DefaultBodyLimit::max(max_upload_mb * 1024 * 1024)),
        )
        .route("/ledger/imports/commit", post(routes::commit_import))
        .route("/ledger/shops", get(routes::list_shops).post(routes::create_shop).put(routes::update_shop))
        .route("/ledger/coverage", get(routes::coverage))
        .route("/ledger/pivot", get(routes::pivot))
        .route("/ledger/details/rows", post(routes::detail_rows))
        .route("/ledger/details/summary", post(routes::detail_summary))
        .route("/ledger/details/export", post(routes::detail_export))
        .route("/ledger/periods", get(routes::periods))
        .route("/ledger/data", delete(routes::delete_data))
        .route("/ledger/entity-aliases", get(routes::entity_aliases).merge(put(routes::save_entity_aliases)))
        .route("/ledger/balances", get(routes::balances).put(routes::save_balances))
        .route(
            "/ledger/bank/imports",
            post(routes::import_bank)
                .layer(DefaultBodyLimit::max(max_upload_mb * 1024 * 1024))
                .get(routes::bank_batches)
                .delete(routes::delete_bank_batch),
        )
        .route("/ledger/withdrawals", get(routes::withdrawals))
        .route("/ledger/withdrawals/auto-match", post(routes::auto_match))
        .route("/ledger/withdrawals/candidates", get(routes::match_candidates))
        .route("/ledger/withdrawals/match", put(routes::manual_match))
        .route("/ledger/close", get(routes::close_overview).post(routes::close_action))
        .route("/ledger/close/logs", get(routes::close_logs))
        .route("/ledger/close/locks", get(routes::close_locks))
        .route("/ledger/analysis/breakdown", get(routes::analysis_breakdown))
        .route("/ledger/analysis/cross-period", get(routes::analysis_cross_period))
        .route("/ledger/analysis/qianchuan", get(routes::analysis_qianchuan).put(routes::save_ad_spend))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PackSetting;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        c.execute_batch(&crate::schema::migrate_sql()).unwrap();
        c
    }

    #[test]
    fn setting_is_parsed_strictly() {
        assert_eq!(PackSetting::parse(None).unwrap(), PackSetting::Auto);
        assert_eq!(PackSetting::parse(Some("")).unwrap(), PackSetting::Only(vec![]));
        assert_eq!(PackSetting::parse(Some("none")).unwrap(), PackSetting::Only(vec![]));
        assert_eq!(PackSetting::parse(Some(" ecommerce ")).unwrap(), PackSetting::Only(vec!["ecommerce".into()]));
        assert_eq!(PackSetting::parse(Some("ALL")).unwrap(), PackSetting::Only(vec!["ecommerce".into()]));
        assert_eq!(PackSetting::parse(Some("ecommerce,ecommerce")).unwrap(), PackSetting::Only(vec!["ecommerce".into()]));
        let err = PackSetting::parse(Some("ecommerse")).unwrap_err().to_string();
        assert!(err.contains("ecommerse") && err.contains("ecommerce"), "{err}");
    }

    #[test]
    fn auto_enables_only_when_the_database_already_has_ledger_data() {
        let c = db();
        assert!(!init(&PackSetting::Auto, &c), "新库不启用");
        c.execute("INSERT INTO ledger_shop (name) VALUES ('A店')", []).unwrap();
        assert!(init(&PackSetting::Auto, &c), "已有店铺档案就启用（老部署升级后功能不消失）");
        // 显式设置优先于自动判断
        assert!(!init(&PackSetting::Only(vec![]), &c));
        assert!(init(&PackSetting::Only(vec!["ecommerce".into()]), &db()));
    }

    #[test]
    fn disabled_pack_hides_the_builtin_dataset() {
        let c = db();
        let names = |on| with_enabled(on, || crate::dataset::load(&c).unwrap().0.into_iter().map(|d| d.name).collect::<Vec<_>>());
        assert!(names(true).contains(&"资金流水".to_string()));
        assert!(!names(false).contains(&"资金流水".to_string()), "没启用电商场景包时，内置的资金流水数据集不出现");
    }
}
