//! 导入来源规整：把各种来源统一成一个「首行是表头的 CSV」，之后的预览、类型识别、提交都沿用数据集导入管线
//!
//! - 单个 CSV / TXT / XLS / XLSX：原样暂存（编码、表头行由预览识别）
//! - Parquet、JSON（数组或逐行 JSON）：用 DuckDB 转成 CSV；源文件自带的列类型（整数、小数、布尔、日期）记在暂存文件旁边，
//!   预览时优先于按样本推断（见 [`kind_hints`]）
//! - 多个文件：各自规整（跳过表头上方的标题行、GBK 转码）后按列名对齐合并，追加「来源文件」列；
//!   列顺序不同、个别文件缺列都可以，缺的列为空值
//! - 网址：下载到临时文件后同上（只允许 http / https，限制大小与超时，不访问本机地址）

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use duckdb::Connection;
use serde::Serialize;

use crate::dataset_import::{detect_header, header_row, staged_path};
use crate::error::{AppError, AppResult};
use crate::schema::literal;

/// 合并时追加的列
pub const SOURCE_COLUMN: &str = "来源文件";
/// 一次最多合并的文件数
pub const MAX_FILES: usize = 50;

/// 已上传到临时目录的文件
pub struct Upload {
    /// 原始文件名
    pub name: String,
    pub path: PathBuf,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SourceFileInfo {
    pub name: String,
    pub rows: i64,
    /// 合并后有、这个文件没有的列（写入空值）
    pub missing: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Delimited,
    Excel,
    Parquet,
    Json,
}

pub fn kind_of(name: &str) -> Option<Kind> {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "csv" | "txt" | "tsv" => Some(Kind::Delimited),
        "xls" | "xlsx" | "xlsm" | "ods" => Some(Kind::Excel),
        "parquet" | "pq" => Some(Kind::Parquet),
        "json" | "jsonl" | "ndjson" => Some(Kind::Json),
        _ => None,
    }
}

fn unsupported(name: &str) -> AppError {
    AppError::bad(format!("不支持的文件格式：{name}（支持 CSV、TXT、XLS、XLSX、Parquet、JSON、JSONL）"))
}

fn duck_err(name: &str, e: duckdb::Error) -> AppError {
    AppError::bad(format!("读取「{name}」失败：{e}"))
}

fn copy_to_csv(conn: &Connection, select: &str, out: &Path, name: &str) -> AppResult<()> {
    conn.execute_batch(&format!("COPY ({select}) TO {} (HEADER, DELIMITER ',')", literal(&out.to_string_lossy()))).map_err(|e| duck_err(name, e))
}

fn csv_reader(path: &Path, skip: usize) -> String {
    format!(
        "read_csv({}, header = true, skip = {skip}, all_varchar = true, delim = ',', quote = '\"', escape = '\"', null_padding = true, strict_mode = false, ignore_errors = true)",
        literal(&path.to_string_lossy())
    )
}

/// 自带列类型的来源（Parquet、JSON）的查询
fn typed_select(up: &Upload) -> Option<String> {
    match kind_of(&up.name)? {
        Kind::Parquet => Some(format!("SELECT * FROM read_parquet({})", literal(&up.path.to_string_lossy()))),
        Kind::Json => Some(format!("SELECT * FROM read_json_auto({})", literal(&up.path.to_string_lossy()))),
        Kind::Delimited | Kind::Excel => None,
    }
}

/// DuckDB 的列类型 → 数据集列类型；文本等没有明确类型的返回 None（仍按样本推断）
pub fn kind_for_type(name: &str, sql_type: &str) -> Option<&'static str> {
    let t = sql_type.to_uppercase();
    if matches!(t.as_str(), "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "HUGEINT" | "UTINYINT" | "USMALLINT" | "UINTEGER" | "UBIGINT") {
        return Some("integer");
    }
    if t == "DOUBLE" || t == "FLOAT" || t == "REAL" || t.starts_with("DECIMAL") {
        let money_name = ["金额", "价", "费", "款", "收入", "支出", "余额", "成本", "利润", "佣金", "补贴", "amount", "price", "cost", "revenue", "fee"].iter().any(|k| name.to_lowercase().contains(k));
        // DECIMAL(p,2) 是典型的金额写法
        return Some(if money_name || t.ends_with(",2)") { "money" } else { "number" });
    }
    if t == "BOOLEAN" {
        return Some("boolean");
    }
    if t == "DATE" || t.starts_with("TIMESTAMP") {
        return Some("datetime");
    }
    None
}

/// 列类型提示文件：与暂存文件同名、扩展名为 .kinds.json（同样以 preview- 开头，过期清理规则相同）
pub fn hints_path(staged: &Path) -> PathBuf {
    staged.with_extension("kinds.json")
}

/// 读取暂存文件的列类型提示：{原始列名: 列类型}；没有提示文件时为空
pub fn kind_hints(staged: &Path) -> HashMap<String, String> {
    std::fs::read(hints_path(staged)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_hints(conn: &Connection, select: &str, staged: &Path, name: &str) -> AppResult<()> {
    let mut stmt = conn.prepare(&format!("DESCRIBE {select}")).map_err(|e| duck_err(name, e))?;
    let cols = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let hints: HashMap<String, &str> = cols.iter().filter_map(|(c, t)| kind_for_type(c, t).map(|k| (c.clone(), k))).collect();
    if !hints.is_empty() {
        std::fs::write(hints_path(staged), serde_json::to_vec(&hints)?)?;
    }
    Ok(())
}

/// 单个文件 → 首行为表头的 UTF-8 CSV（out）
fn to_clean_csv(conn: &Connection, up: &Upload, out: &Path) -> AppResult<()> {
    if let Some(select) = typed_select(up) {
        return copy_to_csv(conn, &select, out, &up.name);
    }
    match kind_of(&up.name).ok_or_else(|| unsupported(&up.name))? {
        // 已在上面按自带类型转换
        Kind::Parquet | Kind::Json => Ok(()),
        Kind::Delimited => {
            crate::tabular::ensure_utf8(&up.path)?;
            let skip = header_row(&up.path, "upload.csv")?;
            copy_to_csv(conn, &format!("SELECT * FROM {}", csv_reader(&up.path, skip)), out, &up.name)
        }
        Kind::Excel => {
            let rows = crate::tabular::read_rows(&up.path, &up.name)?;
            let header = detect_header(&rows);
            let width = rows.get(header).map(Vec::len).unwrap_or(0);
            let mut w = csv::Writer::from_path(out)?;
            for (i, r) in rows.iter().enumerate().skip(header) {
                if i > header && r.iter().all(String::is_empty) {
                    continue;
                }
                let mut rec: Vec<&str> = r.iter().map(String::as_str).collect();
                rec.resize(width, "");
                w.write_record(&rec)?;
            }
            w.flush()?;
            Ok(())
        }
    }
}

/// 规整上传的文件，写入预览暂存位置；返回（暂存用的文件名，各文件信息 —— 单文件时为空）
pub fn prepare(conn: &Connection, dir: &Path, token: &str, uploads: Vec<Upload>) -> AppResult<(String, Vec<SourceFileInfo>)> {
    let result = prepare_inner(conn, dir, token, &uploads);
    for u in &uploads {
        let _ = std::fs::remove_file(&u.path);
    }
    result
}

fn prepare_inner(conn: &Connection, dir: &Path, token: &str, uploads: &[Upload]) -> AppResult<(String, Vec<SourceFileInfo>)> {
    if uploads.is_empty() {
        return Err(AppError::bad("请选择要导入的文件"));
    }
    if uploads.len() > MAX_FILES {
        return Err(AppError::bad(format!("一次最多合并 {MAX_FILES} 个文件")));
    }
    for u in uploads {
        kind_of(&u.name).ok_or_else(|| unsupported(&u.name))?;
    }
    // 单个文件
    if let [up] = uploads {
        let target = staged_path(dir, token, &up.name)?;
        match kind_of(&up.name) {
            Some(Kind::Delimited | Kind::Excel) => std::fs::copy(&up.path, &target).map(|_| ())?,
            _ => {
                to_clean_csv(conn, up, &target)?;
                if let Some(select) = typed_select(up) {
                    write_hints(conn, &select, &target, &up.name)?;
                }
            }
        }
        return Ok((up.name.clone(), vec![]));
    }

    // 多个文件：各自规整后按列名对齐合并
    let clean: Vec<PathBuf> = (0..uploads.len()).map(|i| dir.join(format!("merge-{token}-{i}.csv"))).collect();
    let r = (|| -> AppResult<(String, Vec<SourceFileInfo>)> {
        for (up, out) in uploads.iter().zip(&clean) {
            to_clean_csv(conn, up, out)?;
        }
        // 各文件的列与行数
        let mut cols: Vec<Vec<String>> = Vec::new();
        let mut rows: Vec<i64> = Vec::new();
        for (up, p) in uploads.iter().zip(&clean) {
            let mut stmt = conn.prepare(&format!("DESCRIBE SELECT * FROM {}", csv_reader(p, 0))).map_err(|e| duck_err(&up.name, e))?;
            let names = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            if names.iter().any(|n| n == SOURCE_COLUMN) {
                return Err(AppError::bad(format!("「{}」已有名为「{SOURCE_COLUMN}」的列，无法合并", up.name)));
            }
            rows.push(conn.query_row(&format!("SELECT count(*) FROM {}", csv_reader(p, 0)), [], |r| r.get(0)).map_err(|e| duck_err(&up.name, e))?);
            cols.push(names);
        }
        let mut all: Vec<String> = Vec::new();
        for c in cols.iter().flatten() {
            if !all.contains(c) {
                all.push(c.clone());
            }
        }
        let infos = uploads
            .iter()
            .zip(cols.iter().zip(&rows))
            .map(|(up, (c, &n))| SourceFileInfo { name: up.name.clone(), rows: n, missing: all.iter().filter(|x| !c.contains(x)).cloned().collect() })
            .collect();

        let name = format!("合并{}个文件.csv", uploads.len());
        let target = staged_path(dir, token, &name)?;
        let list = clean.iter().map(|p| literal(&p.to_string_lossy())).collect::<Vec<_>>().join(", ");
        let source = uploads
            .iter()
            .zip(&clean)
            .map(|(up, p)| format!("WHEN {} THEN {}", literal(&p.to_string_lossy()), literal(&up.name)))
            .collect::<Vec<_>>()
            .join(" ");
        copy_to_csv(
            conn,
            &format!(
                "SELECT * EXCLUDE (__src_file), CASE __src_file {source} END AS \"{SOURCE_COLUMN}\"
                 FROM read_csv([{list}], header = true, all_varchar = true, union_by_name = true, filename = '__src_file',
                               delim = ',', quote = '\"', escape = '\"', null_padding = true, strict_mode = false, ignore_errors = true)"
            ),
            &target,
            "合并文件",
        )?;
        Ok((name, infos))
    })();
    for p in &clean {
        let _ = std::fs::remove_file(p);
    }
    r
}

// ── 网址导入 ──────────────────────────────────────────

/// 从网址取文件名：Content-Disposition，其次网址路径最后一段
fn file_name_of(url: &reqwest::Url, headers: &reqwest::header::HeaderMap) -> String {
    let from_header = headers
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            // filename*=UTF-8''%E8%AE%A2%E5%8D%95.csv 优先，其次 filename="a.csv"
            v.split(';').map(str::trim).find_map(|p| p.strip_prefix("filename*=").map(|x| x.rsplit("''").next().unwrap_or(x).to_string())).map(|x| percent_decode(&x)).or_else(|| {
                v.split(';').map(str::trim).find_map(|p| p.strip_prefix("filename=").map(|x| x.trim_matches('"').to_string()))
            })
        });
    let from_path = url.path_segments().and_then(|mut s| s.next_back()).filter(|s| !s.is_empty()).map(percent_decode);
    let name = from_header.or(from_path).unwrap_or_default();
    if kind_of(&name).is_some() {
        return name;
    }
    let ct = headers.get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let ext = if ct.contains("json") {
        "json"
    } else if ct.contains("spreadsheetml") {
        "xlsx"
    } else if ct.contains("parquet") {
        "parquet"
    } else {
        "csv"
    };
    format!("{}.{ext}", if name.is_empty() { "download" } else { &name })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 下载网址到临时文件（流式写入，超过上限即中止）
pub async fn download(url: &str, dir: &Path, token: &str, max_bytes: u64) -> AppResult<Upload> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    if std::env::var("LEDGER_URL_IMPORT").is_ok_and(|v| v == "off") {
        return Err(AppError::bad("服务端已关闭网址导入（LEDGER_URL_IMPORT=off）"));
    }
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| AppError::bad("网址格式不正确"))?;
    crate::netguard::check_url(&parsed)?;
    let client = crate::netguard::client_builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(10 * 60))
        .user_agent("gridlake-import/1.0")
        .build()
        .map_err(|e| AppError::bad(format!("无法创建下载客户端：{e}")))?;
    let resp = client.get(parsed.clone()).send().await.map_err(|e| AppError::bad(format!("下载失败：{e}")))?;
    if resp.status().is_redirection() {
        return Err(AppError::bad("网址重定向到了不允许的地址"));
    }
    if !resp.status().is_success() {
        return Err(AppError::bad(format!("下载失败：服务器返回 {}", resp.status())));
    }
    if resp.content_length().is_some_and(|n| n > max_bytes) {
        return Err(AppError::bad(format!("文件超过 {} MB 上限", max_bytes / 1024 / 1024)));
    }
    let name = file_name_of(resp.url(), resp.headers());
    let path = dir.join(format!("preview-{token}-dl.upload"));
    let mut file = tokio::fs::File::create(&path).await?;
    let mut stream = resp.bytes_stream();
    let mut total: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AppError::bad(format!("下载中断：{e}")))?;
        total += chunk.len() as u64;
        if total > max_bytes {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(AppError::bad(format!("文件超过 {} MB 上限", max_bytes / 1024 / 1024)));
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(Upload { name, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &[u8]) -> Upload {
        let p = dir.join(format!("up-{name}"));
        std::fs::write(&p, content).unwrap();
        Upload { name: name.into(), path: p }
    }

    fn read_csv(path: &Path) -> Vec<Vec<String>> {
        csv::ReaderBuilder::new().has_headers(false).from_path(path).unwrap().records().map(|r| r.unwrap().iter().map(String::from).collect()).collect()
    }

    #[test]
    fn merge_aligns_columns_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        // 第一个文件有标题行；第二个列顺序不同且缺「备注」；第三个是 GBK
        let a = write(dir.path(), "9月.csv", "某某店铺 9 月订单\n订单号,金额,备注\nA1,10.5,首单\nA2,20,\n".as_bytes());
        let b = write(dir.path(), "10月.csv", "金额,订单号\n30,B1\n".as_bytes());
        let (gbk, _, _) = encoding_rs::GBK.encode("订单号,金额,备注\nC1,40,退款\n");
        let c = write(dir.path(), "11月.csv", &gbk);
        let (name, infos) = prepare(&conn, dir.path(), "t1", vec![a, b, c]).unwrap();
        assert_eq!(name, "合并3个文件.csv");
        assert_eq!(infos.iter().map(|i| i.rows).collect::<Vec<_>>(), vec![2, 1, 1]);
        assert_eq!(infos[1].missing, vec!["备注".to_string()]);
        let rows = read_csv(&staged_path(dir.path(), "t1", &name).unwrap());
        assert_eq!(rows[0], vec!["订单号", "金额", "备注", SOURCE_COLUMN]);
        let find = |id: &str| rows.iter().find(|r| r[0] == id).unwrap().clone();
        assert_eq!(find("A1"), vec!["A1", "10.5", "首单", "9月.csv"]);
        assert_eq!(find("B1"), vec!["B1", "30", "", "10月.csv"]);
        assert_eq!(find("C1"), vec!["C1", "40", "退款", "11月.csv"]);
    }

    #[test]
    fn parquet_and_json_become_csv() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let pq = dir.path().join("src.parquet");
        conn.execute_batch(&format!(
            "COPY (SELECT 6900000000000000123::BIGINT AS order_id, 12.34::DECIMAL(10,2) AS amount, TIMESTAMP '2026-10-05 09:30:00' AS ts) TO {} (FORMAT parquet)",
            literal(&pq.to_string_lossy())
        ))
        .unwrap();
        let up = Upload { name: "订单.parquet".into(), path: pq };
        let (name, infos) = prepare(&conn, dir.path(), "t2", vec![up]).unwrap();
        assert!(infos.is_empty());
        let rows = read_csv(&staged_path(dir.path(), "t2", &name).unwrap());
        // 19 位编号不丢精度、金额与时间为文本
        assert_eq!(rows[1], vec!["6900000000000000123", "12.34", "2026-10-05 09:30:00"]);
        // 源文件自带的列类型记在提示文件里：BIGINT 为整数、DECIMAL(10,2) 为金额、TIMESTAMP 为日期时间
        let hints = kind_hints(&staged_path(dir.path(), "t2", &name).unwrap());
        assert_eq!((hints["order_id"].as_str(), hints["amount"].as_str(), hints["ts"].as_str()), ("integer", "money", "datetime"));

        let j = write(dir.path(), "a.jsonl", "{\"店铺\":\"旗舰店\",\"金额\":1.5}\n{\"店铺\":\"专卖店\",\"金额\":2}\n".as_bytes());
        let (name, _) = prepare(&conn, dir.path(), "t3", vec![j]).unwrap();
        let rows = read_csv(&staged_path(dir.path(), "t3", &name).unwrap());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec!["店铺", "金额"]);
    }

    #[test]
    fn duckdb_types_map_to_kinds() {
        assert_eq!(kind_for_type("qty", "INTEGER"), Some("integer"));
        assert_eq!(kind_for_type("id", "UBIGINT"), Some("integer"));
        assert_eq!(kind_for_type("ratio", "DOUBLE"), Some("number"));
        assert_eq!(kind_for_type("unit_price", "DOUBLE"), Some("money"));
        assert_eq!(kind_for_type("x", "DECIMAL(18,2)"), Some("money"));
        assert_eq!(kind_for_type("x", "DECIMAL(18,4)"), Some("number"));
        assert_eq!(kind_for_type("paid", "BOOLEAN"), Some("boolean"));
        assert_eq!(kind_for_type("d", "DATE"), Some("datetime"));
        assert_eq!(kind_for_type("t", "TIMESTAMP WITH TIME ZONE"), Some("datetime"));
        assert_eq!(kind_for_type("name", "VARCHAR"), None);
        assert_eq!(kind_for_type("tags", "VARCHAR[]"), None);
    }

    #[test]
    fn rejects_unsupported_and_local_urls() {
        assert!(kind_of("a.docx").is_none());
        assert!(crate::netguard::check_url(&reqwest::Url::parse("http://127.0.0.1:8787/api/ledger/datasets").unwrap()).is_err());
        assert!(crate::netguard::check_url(&reqwest::Url::parse("http://localhost/x.csv").unwrap()).is_err());
        assert!(crate::netguard::check_url(&reqwest::Url::parse("ftp://example.com/x.csv").unwrap()).is_err());
        assert_eq!(percent_decode("%E8%AE%A2%E5%8D%95.csv"), "订单.csv");
    }
}
