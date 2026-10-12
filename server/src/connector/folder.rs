//! 文件夹来源：服务器上的目录（可以是挂载的共享盘），按文件名通配符收取 CSV / XLSX
//!
//! - 目录必须在 LEDGER_INBOX_ROOTS 之内（规范化路径后判断，防止 ../ 越界）
//! - 每次只导入没导入过的文件（文件名 + 大小 + 修改时间），按修改时间从旧到新
//! - 正在写入的文件（修改时间在 1 分钟内）先跳过，等下一轮再收

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 修改时间在这之内的文件视为可能仍在写入
const SETTLE: Duration = Duration::from_secs(60);

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderSource {
    /// 目录（绝对路径，或相对于第一个允许目录）
    pub path: String,
    /// 文件名通配符，* 任意字符、? 单个字符；多个用 ; 分隔
    pub pattern: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mtime: String,
    /// 是否已经导入过（文件名 + 大小 + 修改时间都相同）
    pub imported: bool,
    /// 刚修改过，可能仍在写入
    pub settling: bool,
}

/// 通配符匹配（不区分大小写）
pub fn wildcard(pattern: &str, name: &str) -> bool {
    fn m(p: &[char], s: &[char]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some('*'), _) => m(&p[1..], s) || (!s.is_empty() && m(p, &s[1..])),
            (Some('?'), Some(_)) => m(&p[1..], &s[1..]),
            (Some(a), Some(b)) => a == b && m(&p[1..], &s[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let s: Vec<char> = name.to_lowercase().chars().collect();
    m(&p, &s)
}

fn supported(name: &str) -> bool {
    crate::import_source::kind_of(name).is_some()
}

/// 解析并校验目录：必须存在、且在允许的根目录之内
pub fn resolve_dir(roots: &[PathBuf], path: &str) -> AppResult<PathBuf> {
    if roots.is_empty() {
        return Err(AppError::bad("未配置可用的收件目录（LEDGER_INBOX_ROOTS）"));
    }
    let p = Path::new(path.trim());
    let candidate = if p.is_absolute() { p.to_path_buf() } else { roots[0].join(p) };
    let canon = candidate.canonicalize().map_err(|_| AppError::bad(format!("目录不存在或无法访问：{}", candidate.display())))?;
    if !canon.is_dir() {
        return Err(AppError::bad(format!("不是目录：{}", canon.display())));
    }
    if !roots.iter().filter_map(|r| r.canonicalize().ok()).any(|r| canon.starts_with(&r)) {
        return Err(AppError::bad(format!(
            "只能使用以下目录（及其子目录）：{}",
            roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join("、")
        )));
    }
    Ok(canon)
}

fn fmt_time(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 列出匹配的文件（按修改时间从旧到新）；imported 判断由调用方提供
pub fn list(roots: &[PathBuf], src: &FolderSource, is_imported: impl Fn(&str, u64, &str) -> bool) -> AppResult<Vec<FileEntry>> {
    let dir = resolve_dir(roots, &src.path)?;
    let patterns: Vec<&str> = src.pattern.split([';', '；', ',']).map(str::trim).filter(|p| !p.is_empty()).collect();
    if patterns.is_empty() {
        return Err(AppError::bad("请填写文件名规则，如 *.csv"));
    }
    let now = SystemTime::now();
    let mut out: Vec<(SystemTime, FileEntry)> = Vec::new();
    for e in std::fs::read_dir(&dir)?.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.starts_with("~$") || !supported(&name) || !patterns.iter().any(|p| wildcard(p, &name)) {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(now);
        let mtime = fmt_time(modified);
        let path = e.path().display().to_string();
        let imported = is_imported(&path, meta.len(), &mtime);
        let settling = now.duration_since(modified).map(|d| d < SETTLE).unwrap_or(false);
        out.push((modified, FileEntry { path, name, size: meta.len(), mtime, imported, settling }));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    Ok(out.into_iter().map(|(_, f)| f).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_match() {
        assert!(wildcard("抖店资金流水_*.csv", "抖店资金流水_20261001.CSV"));
        assert!(wildcard("*.xlsx", "成本.xlsx"));
        assert!(!wildcard("*.xlsx", "成本.xlsx.bak"));
        assert!(wildcard("流水_????.csv", "流水_1001.csv"));
        assert!(!wildcard("流水_????.csv", "流水_100.csv"));
    }

    #[test]
    fn dir_must_be_inside_roots() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("抖店")).unwrap();
        let roots = vec![root.path().to_path_buf()];
        assert!(resolve_dir(&roots, "抖店").is_ok());
        assert!(resolve_dir(&roots, &root.path().join("抖店").display().to_string()).is_ok());
        assert!(resolve_dir(&roots, "../").is_err());
        assert!(resolve_dir(&roots, &other.path().display().to_string()).is_err());
        assert!(resolve_dir(&roots, "不存在").is_err());
    }
}
