//! 给导出的 xlsx 补上行的分级（Excel 左侧可折叠的 1 / 2 按钮）
//!
//! 数据页签用 rust_xlsxwriter 的常量内存模式逐行写（几十万行也不占内存），但这个模式写 `<row>` 时只带 hidden，
//! 不带 outlineLevel / collapsed。所以保存后再过一遍：其他文件原样复制（不解压），数据页签按块流式改写 `<row …>` 标签，
//! 内存占用与行数无关

use std::io::{Read, Write};
use std::path::Path;

use crate::error::AppResult;

/// 分级：每行的级别（Excel 左侧的 1 / 2 / 3 按钮）和折叠组下面的小计行
pub struct Outline {
    /// (起始行, 级别)：从这一行起到下一项之前的行都是这一级，按行号排序
    steps: Vec<(u32, u8)>,
    collapsed: Vec<u32>,
    max: u8,
}

impl Outline {
    /// ranges：每一组的明细行区间（Excel 行号，从 1 数，含两端；可以嵌套，一行的级别 = 包含它的区间个数），
    /// collapsed：折叠的组下面那一行（小计行）
    pub fn new(ranges: &[(u32, u32)], mut collapsed: Vec<u32>) -> Self {
        let mut ev: Vec<(u32, i32)> = ranges.iter().flat_map(|&(a, b)| [(a, 1), (b + 1, -1)]).collect();
        ev.sort_unstable();
        let (mut steps, mut level, mut max, mut i) = (Vec::<(u32, u8)>::new(), 0i32, 0u8, 0);
        while i < ev.len() {
            let row = ev[i].0;
            while i < ev.len() && ev[i].0 == row {
                level += ev[i].1;
                i += 1;
            }
            // Excel 最多 7 级
            let l = level.clamp(0, 7) as u8;
            if steps.last().map(|s| s.1) != Some(l) {
                steps.push((row, l));
            }
            max = max.max(l);
        }
        collapsed.sort_unstable();
        collapsed.dedup();
        Outline { steps, collapsed, max }
    }
    fn level(&self, row: u32) -> u8 {
        let i = self.steps.partition_point(|&(r, _)| r <= row);
        if i == 0 { 0 } else { self.steps[i - 1].1 }
    }
    fn is_collapsed(&self, row: u32) -> bool {
        self.collapsed.binary_search(&row).is_ok()
    }
}

/// 改写一个 `<row …>` 或 `<sheetFormatPr …>` 标签（tag 含尖括号）
fn rewrite_tag(tag: &[u8], outline: &Outline, out: &mut Vec<u8>) {
    let close = if tag.ends_with(b"/>") { tag.len() - 2 } else { tag.len() - 1 };
    let head = &tag[..close];
    let mut extra = String::new();
    if tag.starts_with(b"<sheetFormatPr") {
        if outline.max > 0 && !contains(head, b"outlineLevelRow=") {
            extra.push_str(&format!(" outlineLevelRow=\"{}\"", outline.max));
        }
    } else if let Some(row) = attr(head, b" r=\"").and_then(|v| std::str::from_utf8(v).ok()?.parse::<u32>().ok()) {
        let level = outline.level(row);
        if level > 0 && !contains(head, b"outlineLevel=") {
            extra.push_str(&format!(" outlineLevel=\"{level}\""));
        }
        if outline.is_collapsed(row) && !contains(head, b"collapsed=") {
            extra.push_str(" collapsed=\"1\"");
        }
    }
    out.extend_from_slice(head);
    out.extend_from_slice(extra.as_bytes());
    out.extend_from_slice(&tag[close..]);
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// 属性值：attr(b"<row r=\"12\" …", b" r=\"") → b"12"
fn attr<'a>(tag: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    let start = tag.windows(prefix.len()).position(|w| w == prefix)? + prefix.len();
    let len = tag[start..].iter().position(|&b| b == b'"')?;
    Some(&tag[start..start + len])
}

/// 下一个要改写的标签的起点（`<row ` 或 `<sheetFormatPr `）
fn next_tag(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while let Some(p) = buf[i..].iter().position(|&b| b == b'<') {
        let at = i + p;
        let rest = &buf[at..];
        if rest.starts_with(b"<row ") || rest.starts_with(b"<sheetFormatPr ") {
            return Some(at);
        }
        // 剩下的字节不够判断是不是要找的标签：留到下一块
        if rest.len() < 15 {
            return Some(at);
        }
        i = at + 1;
    }
    None
}

/// 流式改写 sheet XML
fn transform(mut input: impl Read, mut output: impl Write, outline: &Outline) -> std::io::Result<()> {
    const CHUNK: usize = 1 << 20;
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let mut eof = false;
    let mut out = Vec::with_capacity(CHUNK + 4096);
    loop {
        if !eof {
            let n = input.read(&mut buf)?;
            if n == 0 {
                eof = true;
            } else {
                pending.extend_from_slice(&buf[..n]);
            }
        }
        let mut done = 0;
        let mut i = 0;
        while let Some(start) = next_tag(&pending, i) {
            let Some(end) = pending[start..].iter().position(|&b| b == b'>').map(|p| start + p) else { break };
            let tag = &pending[start..=end];
            out.extend_from_slice(&pending[done..start]);
            if tag.starts_with(b"<row ") || tag.starts_with(b"<sheetFormatPr ") {
                rewrite_tag(tag, outline, &mut out);
            } else {
                out.extend_from_slice(tag);
            }
            done = end + 1;
            i = done;
        }
        // 没有未完成的标签：剩下的都可以写出去
        let keep_from = match next_tag(&pending, done) {
            Some(start) if !eof => start,
            _ => pending.len(),
        };
        out.extend_from_slice(&pending[done..keep_from]);
        output.write_all(&out)?;
        out.clear();
        pending.drain(..keep_from);
        if eof && pending.is_empty() {
            return Ok(());
        }
        if eof {
            // 文件末尾还有半个标签（不应出现）：原样写出
            output.write_all(&pending)?;
            return Ok(());
        }
    }
}

/// 改写 path 处的 xlsx：给 sheet（如 xl/worksheets/sheet2.xml）的行补上分级
pub fn apply(path: &Path, sheet: &str, outline: &Outline) -> AppResult<()> {
    let tmp = path.with_extension("outline.xlsx");
    {
        let mut src = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(|e| anyhow::anyhow!("读取 Excel 失败：{e}"))?;
        let mut dst = zip::ZipWriter::new(std::fs::File::create(&tmp)?);
        for i in 0..src.len() {
            let file = src.by_index(i).map_err(|e| anyhow::anyhow!("读取 Excel 失败：{e}"))?;
            if file.name() == sheet {
                let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
                let name = file.name().to_string();
                dst.start_file(name, opts).map_err(|e| anyhow::anyhow!("写 Excel 失败：{e}"))?;
                transform(file, &mut dst, outline)?;
            } else {
                dst.raw_copy_file(file).map_err(|e| anyhow::anyhow!("写 Excel 失败：{e}"))?;
            }
        }
        dst.finish().map_err(|e| anyhow::anyhow!("写 Excel 失败：{e}"))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(xml: &str, outline: &Outline, chunk_split: bool) -> String {
        let mut out = Vec::new();
        if chunk_split {
            // 一次只给 3 个字节：标签被切开也要对
            struct Slow<'a>(&'a [u8]);
            impl Read for Slow<'_> {
                fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                    let n = self.0.len().min(3).min(buf.len());
                    buf[..n].copy_from_slice(&self.0[..n]);
                    self.0 = &self.0[n..];
                    Ok(n)
                }
            }
            transform(Slow(xml.as_bytes()), &mut out, outline).unwrap();
        } else {
            transform(xml.as_bytes(), &mut out, outline).unwrap();
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn adds_levels_and_collapsed() {
        let xml = r#"<worksheet><sheetFormatPr defaultRowHeight="15"/><sheetData><row r="1"><c r="A1"/></row><row r="2" hidden="1"><c r="A2"/></row><row r="3"><c r="A3"/></row><row r="4"/><row r="5"><c r="A5"/></row></sheetData></worksheet>"#;
        let o = Outline::new(&[(2, 2), (4, 4)], vec![3]);
        let want = r#"<worksheet><sheetFormatPr defaultRowHeight="15" outlineLevelRow="1"/><sheetData><row r="1"><c r="A1"/></row><row r="2" hidden="1" outlineLevel="1"><c r="A2"/></row><row r="3" collapsed="1"><c r="A3"/></row><row r="4" outlineLevel="1"/><row r="5"><c r="A5"/></row></sheetData></worksheet>"#;
        assert_eq!(run(xml, &o, false), want);
        assert_eq!(run(xml, &o, true), want, "按块读取、标签被切开时结果一样");
    }

    #[test]
    fn nested_levels() {
        // 2 ~ 6 是外层的组，里面 2 ~ 3、5 两个内层的组（4、6 是内层小计）；7 是外层小计
        let o = Outline::new(&[(2, 6), (2, 3), (5, 5)], vec![4]);
        let levels: Vec<u8> = (1..=8).map(|r| o.level(r)).collect();
        assert_eq!(levels, [0, 2, 2, 1, 2, 1, 0, 0]);
        let xml = r#"<sheetFormatPr defaultRowHeight="15"/><row r="2"/><row r="4"/><row r="7"/>"#;
        assert_eq!(
            run(xml, &o, false),
            r#"<sheetFormatPr defaultRowHeight="15" outlineLevelRow="2"/><row r="2" outlineLevel="2"/><row r="4" outlineLevel="1" collapsed="1"/><row r="7"/>"#
        );
    }
}
