//! 拆分列的预览：按分隔符看每行能拆成几段，给出样例，前端据此起新列名、生成计算列
//!
//! 拆分 / 合并本身做成计算列（`SPLITPART` / `TEXTJOIN`，见 calc.rs），不改存储的数据：
//! 之后追加、同步进来的行也自动拆好，删掉计算列就回到原样

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::dataset;
use crate::error::{AppError, AppResult};

/// 拆出来最多几列（再多通常是分隔符选错了）
pub const MAX_PARTS: i64 = 10;
const SAMPLES: usize = 8;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitRequest {
    pub column: String,
    pub separator: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PartCount {
    /// 拆成几段
    pub parts: i64,
    pub rows: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SplitPreview {
    pub rows: i64,
    /// 非空的行
    pub non_empty: i64,
    /// 各段数的行数（从少到多）
    pub counts: Vec<PartCount>,
    /// 最多拆成几段（超过 MAX_PARTS 时按 MAX_PARTS）
    pub max_parts: i64,
    /// 最常见的段数（建议拆成几列）
    pub common_parts: i64,
    /// 样例：原值和拆出来的各段
    pub samples: Vec<(String, Vec<String>)>,
}

pub fn preview(conn: &Connection, id: &str, req: &SplitRequest) -> AppResult<SplitPreview> {
    if req.separator.is_empty() {
        return Err(AppError::bad("请填分隔符"));
    }
    let def = dataset::dataset(conn, id)?;
    let col = dataset::find_column(&def, &req.column)?;
    let v = format!("CAST(({}) AS VARCHAR)", dataset::column_sql(col));
    let filled = format!("NULLIF(trim({v}), '') IS NOT NULL");
    let k = format!("len(string_split({v}, ?))");
    let (rows, non_empty): (i64, i64) = conn.query_row(&format!("SELECT count(*), count(*) FILTER (WHERE {filled}) FROM {}", def.table), [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let counts: Vec<PartCount> = {
        let mut st = conn.prepare(&format!("SELECT least({k}, {MAX_PARTS}) AS k, count(*) FROM {} WHERE {filled} GROUP BY k ORDER BY k", def.table))?;
        st.query_map(params![req.separator], |r| Ok(PartCount { parts: r.get(0)?, rows: r.get(1)? }))?.collect::<Result<_, _>>()?
    };
    let max_parts = counts.iter().map(|c| c.parts).max().unwrap_or(0);
    let common_parts = counts.iter().max_by_key(|c| (c.rows, c.parts)).map(|c| c.parts).unwrap_or(0);
    // 样例：先给拆成最常见段数的值，再补几个段数不同的（少了 / 多了一段的往往是要注意的）
    let samples: Vec<(String, Vec<String>)> = {
        let mut st = conn.prepare(&format!(
            "SELECT v, CAST(to_json(string_split(v, ?)) AS VARCHAR) FROM (SELECT DISTINCT {v} AS v, least({k}, {MAX_PARTS}) AS k FROM {} WHERE {filled} LIMIT 2000)
             ORDER BY (k = ?) DESC, k DESC, v LIMIT {SAMPLES}",
            def.table
        ))?;
        st.query_map(params![req.separator, req.separator, common_parts], |r| {
            let parts: Vec<Option<String>> = serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default();
            Ok((r.get(0)?, parts.into_iter().map(Option::unwrap_or_default).collect()))
        })?
        .collect::<Result<_, _>>()?
    };
    Ok(SplitPreview { rows, non_empty, counts, max_parts, common_parts, samples })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_parts_and_picks_common_samples() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        let id = crate::dataset_import::create_empty(&conn, "地址", &[("地址".into(), "text".into())], &[]).unwrap();
        conn.execute_batch(&format!("INSERT INTO {id} VALUES ('广东-深圳', NULL), ('广东-广州', NULL), ('北京', NULL), ('浙江-杭州-西湖', NULL), ('', NULL), (NULL, NULL)")).unwrap();
        let p = preview(&conn, &id, &SplitRequest { column: "地址".into(), separator: "-".into() }).unwrap();
        assert_eq!((p.rows, p.non_empty, p.max_parts, p.common_parts), (6, 4, 3, 2));
        assert_eq!(p.counts.iter().map(|c| (c.parts, c.rows)).collect::<Vec<_>>(), vec![(1, 1), (2, 2), (3, 1)]);
        assert_eq!(p.samples[0].1.len(), 2, "最常见的段数排在前面");
        assert!(p.samples.iter().any(|(v, parts)| v == "浙江-杭州-西湖" && parts == &vec!["浙江".to_string(), "杭州".into(), "西湖".into()]));
        assert!(preview(&conn, &id, &SplitRequest { column: "地址".into(), separator: String::new() }).is_err());
        assert!(preview(&conn, &id, &SplitRequest { column: "没有".into(), separator: "-".into() }).is_err());
    }
}
