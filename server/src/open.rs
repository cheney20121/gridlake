//! `gridlake open [文件或目录...]`：一条命令在本机打开数据文件
//!
//! - 用独立的个人库（默认 `~/.gridlake/open.duckdb`，`--db` 可改），不碰 LEDGER_DB 指向的库
//! - 只监听本机地址、不需要登录；端口从 8787 起找第一个空闲的（`--port` 可指定）
//! - Parquet 文件、或含 Parquet 的目录：登记成外部数据集，直接查询、不复制（目录里有 `key=value` 子目录时按 Hive 分区读）
//! - CSV / TXT / XLSX / JSON：导入成数据集（编码、表头、列类型自动识别）
//! - 同一个文件再次打开：外部数据集刷新文件列表、导入的数据集整体替换数据，不会越开越多
//! - 前端页面：LEDGER_WEB_DIR，没设置时找程序旁边的 `web/`（发布包的布局）；服务起来后打开浏览器（`--no-browser` 不打开）

use std::path::{Path, PathBuf};

use duckdb::Connection;

use crate::config::{AuthMode, Config, PackSetting};
use crate::dataset;
use crate::external::ExternalInput;

pub const USAGE: &str = "用法：gridlake open [文件或目录...] [--port 端口] [--db 库文件] [--no-browser]
  Parquet 文件 / 含 Parquet 的目录：直接查询，不复制；CSV / XLSX / JSON：导入
  例：gridlake open 订单.parquet 客户.csv
      gridlake open /data/orders/            （目录下所有 .parquet，含子目录）";

/// 命令行参数
#[derive(Debug, PartialEq)]
pub struct OpenArgs {
    pub paths: Vec<PathBuf>,
    pub port: Option<u16>,
    pub db: Option<PathBuf>,
    pub browser: bool,
}

pub fn parse_args(args: &[String]) -> anyhow::Result<OpenArgs> {
    let mut out = OpenArgs { paths: vec![], port: None, db: None, browser: true };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" | "-p" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--port 后面要跟端口号\n{USAGE}"))?;
                out.port = Some(v.parse().map_err(|_| anyhow::anyhow!("端口号不对：{v}"))?);
            }
            "--db" => out.db = Some(PathBuf::from(it.next().ok_or_else(|| anyhow::anyhow!("--db 后面要跟库文件路径\n{USAGE}"))?)),
            "--no-browser" => out.browser = false,
            s if s.starts_with('-') => anyhow::bail!("不认识的选项：{s}\n{USAGE}"),
            s => out.paths.push(PathBuf::from(s)),
        }
    }
    Ok(out)
}

/// 个人库的默认位置：GRIDLAKE_HOME，其次 ~/.gridlake
fn default_db() -> PathBuf {
    let home = std::env::var_os("GRIDLAKE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".gridlake")))
        .unwrap_or_else(|| PathBuf::from(".gridlake"));
    home.join("open.duckdb")
}

/// 程序旁边的前端页面：发布包里是 bin/gridlake + web/
fn find_web_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?;
    [dir.join("../web"), dir.join("web"), dir.join("../../../web/dist")].into_iter().find(|d| d.join("index.html").is_file()).and_then(|d| d.canonicalize().ok())
}

fn free_port(from: u16) -> Option<u16> {
    (from..from.saturating_add(20)).find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
}

/// 要打开的文件：Parquet（外部数据集）或别的表格文件（导入）
#[derive(Debug, PartialEq)]
pub enum Source {
    /// 目录 + 文件名规则（单个文件时规则就是文件名）
    Parquet { dir: PathBuf, pattern: String, recursive: bool, hive: bool, name: String },
    Import { file: PathBuf, name: String },
}

fn stem(p: &Path) -> String {
    let s = p.file_stem().or_else(|| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "数据".into());
    s.chars().take(40).collect()
}

/// 命令行给的路径 → 要打开的来源
pub fn classify(path: &Path) -> anyhow::Result<Source> {
    let p = path.canonicalize().map_err(|_| anyhow::anyhow!("找不到：{}", path.display()))?;
    if p.is_dir() {
        // 目录：下面所有 .parquet（含子目录）；子目录名形如 key=value 时按 Hive 分区读成列
        let mut stack = vec![p.clone()];
        let (mut found, mut hive) = (false, false);
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)?.filter_map(Result::ok) {
                let name = e.file_name().to_string_lossy().to_string();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    hive |= name.contains('=');
                    stack.push(e.path());
                } else if name.to_lowercase().ends_with(".parquet") {
                    found = true;
                }
            }
        }
        if !found {
            anyhow::bail!("目录里没有 Parquet 文件：{}（CSV / XLSX 请直接写文件名）", p.display());
        }
        return Ok(Source::Parquet { name: stem(&p), dir: p, pattern: "*.parquet".into(), recursive: true, hive });
    }
    let file_name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    match crate::import_source::kind_of(&file_name) {
        Some(crate::import_source::Kind::Parquet) => {
            let dir = p.parent().map(Path::to_path_buf).ok_or_else(|| anyhow::anyhow!("找不到文件所在的目录：{}", p.display()))?;
            Ok(Source::Parquet { name: stem(&p), dir, pattern: file_name, recursive: false, hive: false })
        }
        Some(_) => Ok(Source::Import { name: stem(&p), file: p }),
        None => anyhow::bail!("不支持的文件：{}（支持 Parquet、CSV、TXT、XLS、XLSX、JSON）", p.display()),
    }
}

/// 登记好的数据集：(id, 说明)
pub struct Opened {
    pub id: String,
    pub summary: String,
}

/// 同名数据集已存在但类型对不上时，换一个名字（订单 → 订单_2）
fn unique_name(conn: &Connection, base: &str) -> anyhow::Result<String> {
    let names: Vec<String> = dataset::datasets(conn).map_err(app)?.into_iter().map(|d| d.name).collect();
    Ok((1..).map(|i| if i == 1 { base.to_string() } else { format!("{base}_{i}") }).find(|n| !names.contains(n)).unwrap_or_else(|| base.to_string()))
}

fn app(e: crate::error::AppError) -> anyhow::Error {
    anyhow::anyhow!("{}", e.message())
}

/// 把一个来源登记成数据集：已经登记过（同名、同类型）的刷新 / 替换
pub fn register(conn: &Connection, source: &Source, roots: &[PathBuf], tmp_dir: &Path) -> anyhow::Result<Opened> {
    let existing = |name: &str, kind: &str| -> anyhow::Result<Option<String>> {
        Ok(dataset::datasets(conn).map_err(app)?.into_iter().find(|d| d.name == name && d.kind == kind).map(|d| d.id))
    };
    match source {
        Source::Parquet { dir, pattern, recursive, hive, name } => {
            let input = ExternalInput { path: dir.display().to_string(), pattern: pattern.clone(), recursive: *recursive, hive: *hive };
            let id = match existing(name, "external")? {
                Some(id) => {
                    crate::external::refresh(conn, roots, &id).map_err(app)?;
                    id
                }
                None => crate::external::create(conn, roots, &unique_name(conn, name)?, &input).map_err(app)?,
            };
            let x = dataset::dataset(conn, &id).map_err(app)?.external.map(|x| (x.file_count, x.rows)).unwrap_or_default();
            Ok(Opened { id, summary: format!("{name}：直接查询 {} 个 Parquet 文件，{} 行（不复制）", x.0, x.1) })
        }
        Source::Import { file, name } => {
            let token = format!("open{}", chrono::Local::now().format("%Y%m%d%H%M%S%f"));
            let file_name = file.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            // 导入管线处理完会删掉上传的文件：先复制一份
            let upload = tmp_dir.join(format!("preview-{token}-src.upload"));
            std::fs::copy(file, &upload)?;
            let (staged_name, _) = crate::import_source::prepare(conn, tmp_dir, &token, vec![crate::import_source::Upload { name: file_name, path: upload }]).map_err(app)?;
            let staged = crate::dataset_import::staged_path(tmp_dir, &token, &staged_name).map_err(app)?;
            let result = (|| -> anyhow::Result<Opened> {
                let p = crate::dataset_import::preview(conn, &staged, &token, &staged_name).map_err(app)?;
                let target = existing(name, "table")?;
                let req = crate::dataset_import::CommitRequest {
                    token: token.clone(),
                    mode: if target.is_some() { crate::dataset_import::Mode::Replace } else { crate::dataset_import::Mode::Create },
                    dataset_id: target,
                    name: Some(unique_name(conn, name)?),
                    columns: p.columns.iter().map(|c| crate::dataset_import::CommitColumn { source: c.source.clone(), title: c.title.clone(), kind: c.kind.clone(), include: true }).collect(),
                    primary_key: vec![],
                    file_name: staged_name.clone(),
                };
                let o = crate::dataset_import::commit(conn, &staged, &req, tmp_dir).map_err(app)?;
                Ok(Opened { id: o.dataset_id, summary: format!("{}：导入 {} 行", o.name, o.inserted) })
            })();
            let _ = std::fs::remove_file(&staged);
            let _ = std::fs::remove_file(crate::import_source::hints_path(&staged));
            result
        }
    }
}

/// 调整配置：个人库、只听本机、不登录、不备份、不启用场景包，允许读这些文件所在的目录
pub fn configure(config: &mut Config, args: &OpenArgs, sources: &[Source]) -> anyhow::Result<()> {
    config.db_path = args.db.clone().unwrap_or_else(default_db);
    if let Some(dir) = config.db_path.parent() {
        std::fs::create_dir_all(dir)?;
        config.backup_dir = dir.join("backups");
    }
    let port = match args.port {
        Some(p) => p,
        None => free_port(8787).ok_or_else(|| anyhow::anyhow!("8787 ~ 8806 的端口都被占用了，请用 --port 指定"))?,
    };
    config.addr = format!("127.0.0.1:{port}");
    config.auth_mode = AuthMode::Off;
    config.cors = None;
    config.backup_at = None;
    config.packs = PackSetting::Only(vec![]);
    for s in sources {
        if let Source::Parquet { dir, .. } = s {
            config.inbox_roots.push(dir.clone());
        }
    }
    if config.web_dir.is_none() {
        config.web_dir = find_web_dir();
    }
    Ok(())
}

/// 用系统默认浏览器打开（失败只打印提示）
pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let r = std::process::Command::new("xdg-open").arg(url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    if r.is_err() {
        println!("（没能自动打开浏览器，请手动打开上面的地址）");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn args() {
        assert_eq!(parse_args(&s(&["a.parquet", "--port", "9000", "b.csv", "--no-browser"])).unwrap(), OpenArgs { paths: vec!["a.parquet".into(), "b.csv".into()], port: Some(9000), db: None, browser: false });
        assert!(parse_args(&s(&["--port"])).is_err());
        assert!(parse_args(&s(&["--port", "x"])).is_err());
        assert!(parse_args(&s(&["--what"])).is_err());
        assert_eq!(parse_args(&[]).unwrap().paths.len(), 0);
    }

    #[test]
    fn classify_and_register() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::schema::create_tables_sql()).unwrap();
        conn.execute_batch(&crate::schema::migrate_sql()).unwrap();
        let lake = dir.path().join("lake");
        std::fs::create_dir_all(lake.join("年=2026")).unwrap();
        let pq = lake.join("年=2026/a.parquet");
        conn.execute_batch(&format!("COPY (SELECT i AS id FROM range(5) t(i)) TO {} (FORMAT parquet)", crate::schema::literal(&pq.to_string_lossy()))).unwrap();
        let csv = dir.path().join("客户.csv");
        std::fs::write(&csv, "客户,数量\n甲,3\n乙,4\n").unwrap();
        std::fs::write(dir.path().join("说明.docx"), "x").unwrap();

        let single = classify(&pq).unwrap();
        assert!(matches!(&single, Source::Parquet { pattern, recursive: false, name, .. } if pattern == "a.parquet" && name == "a"));
        let folder = classify(&lake).unwrap();
        assert!(matches!(&folder, Source::Parquet { recursive: true, hive: true, name, .. } if name == "lake"));
        assert!(matches!(classify(&csv).unwrap(), Source::Import { ref name, .. } if name == "客户"));
        assert!(classify(&dir.path().join("说明.docx")).is_err());
        assert!(classify(&dir.path().join("没有.csv")).is_err());
        assert!(classify(dir.path()).is_ok(), "上级目录里也有 Parquet（在子目录里）");

        let tmp = dir.path().join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let roots = vec![lake.clone()];
        let a = register(&conn, &folder, &roots, &tmp).unwrap();
        let def = dataset::dataset(&conn, &a.id).unwrap();
        assert!(def.columns.iter().any(|c| c.title == "年"), "Hive 分区目录名读成列");
        assert!(a.summary.contains("5 行"), "{}", a.summary);
        // 再开一次：同一个数据集刷新，不新建
        assert_eq!(register(&conn, &folder, &roots, &tmp).unwrap().id, a.id);

        let b = register(&conn, &classify(&csv).unwrap(), &roots, &tmp).unwrap();
        assert!(b.summary.contains("导入 2 行"), "{}", b.summary);
        std::fs::write(&csv, "客户,数量\n甲,3\n乙,4\n丙,5\n").unwrap();
        let b2 = register(&conn, &classify(&csv).unwrap(), &roots, &tmp).unwrap();
        assert_eq!(b2.id, b.id, "再开一次：整体替换数据");
        assert_eq!(dataset::list(&conn).unwrap().into_iter().find(|d| d.def.id == b.id).unwrap().row_count, 3);
        assert!(csv.exists(), "原文件不动");
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0, "临时文件都清掉了");
    }
}
