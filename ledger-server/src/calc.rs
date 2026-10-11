//! 计算列：Excel 风格公式 → SQL 表达式（安全翻译，不拼接用户输入的任意 SQL）
//!
//! 语法：`=[订单实付应结]-[成本价]*[数量]`、`=ROUND([佣金]/[订单实付应结],4)`、`=IF([动账方向]="入账",[动账金额],-[动账金额])`、
//! `=LEFT([下单时间],7)`
//! - 列引用：`[列名]`（可引用已有的计算列）
//! - 运算：+ - * /（除数为 0 得空值）、& 文本连接、比较 = <> > >= < <=
//! - 函数：ROUND ABS IF IFNULL MAX MIN LEFT RIGHT MID LEN UPPER LOWER TRIM YEAR MONTH DAY
//! - 文本用双引号，数字直接写

use crate::error::{AppError, AppResult};
use crate::schema::literal;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(String),
    Str(String),
    Col(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn lex(src: &str) -> AppResult<Vec<Tok>> {
    let s: Vec<char> = src.trim().trim_start_matches('=').chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '[' => {
                let end = s[i + 1..].iter().position(|&x| x == ']').ok_or_else(|| AppError::bad("列引用缺少右方括号 ]"))?;
                let name: String = s[i + 1..i + 1 + end].iter().collect();
                if name.trim().is_empty() {
                    return Err(AppError::bad("列引用不能为空：[]"));
                }
                out.push(Tok::Col(name.trim().to_string()));
                i += end + 2;
            }
            '"' => {
                let mut j = i + 1;
                let mut text = String::new();
                loop {
                    if j >= s.len() {
                        return Err(AppError::bad("文本缺少右引号"));
                    }
                    if s[j] == '"' {
                        // Excel 转义：两个双引号表示一个
                        if j + 1 < s.len() && s[j + 1] == '"' {
                            text.push('"');
                            j += 2;
                            continue;
                        }
                        break;
                    }
                    text.push(s[j]);
                    j += 1;
                }
                out.push(Tok::Str(text));
                i = j + 1;
            }
            '0'..='9' | '.' => {
                let mut j = i;
                while j < s.len() && (s[j].is_ascii_digit() || s[j] == '.') {
                    j += 1;
                }
                let num: String = s[i..j].iter().collect();
                if num.parse::<f64>().is_err() {
                    return Err(AppError::bad(format!("无法识别的数字：{num}")));
                }
                out.push(Tok::Num(num));
                i = j;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' | '，' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '+' | '-' | '*' | '/' | '&' => {
                out.push(Tok::Op(match c {
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    _ => "&",
                }));
                i += 1;
            }
            '<' | '>' | '=' => {
                let two: String = s[i..(i + 2).min(s.len())].iter().collect();
                let op = match two.as_str() {
                    "<=" => "<=",
                    ">=" => ">=",
                    "<>" => "<>",
                    _ => match c {
                        '<' => "<",
                        '>' => ">",
                        _ => "=",
                    },
                };
                out.push(Tok::Op(op));
                i += op.len();
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut j = i;
                while j < s.len() && (s[j].is_alphanumeric() || s[j] == '_') {
                    j += 1;
                }
                out.push(Tok::Ident(s[i..j].iter().collect::<String>().to_uppercase()));
                i = j;
            }
            other => return Err(AppError::bad(format!("不支持的字符：{other}"))),
        }
    }
    Ok(out)
}

/// 表达式结果类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ty {
    Num,
    Text,
    Bool,
}

pub struct Compiled {
    pub sql: String,
    pub ty: Ty,
    /// 引用到的列
    pub columns: Vec<String>,
    /// 顶层是除法（可被 ROUND 包裹）：比率类结果，不应按金额显示
    pub ratio: bool,
}

/// 顶层（括号外）是否为除法，且没有加减：如 [毛利]/[收入]、ROUND([a]/[b],4)
fn is_ratio(toks: &[Tok]) -> bool {
    let mut body = toks;
    // 去掉外层 ROUND( … , n)
    if let [Tok::Ident(f), Tok::LParen, rest @ ..] = toks
        && f == "ROUND"
        && let Some(Tok::RParen) = rest.last()
    {
        let inner = &rest[..rest.len() - 1];
        let mut depth = 0;
        let comma = inner.iter().position(|t| {
            match t {
                Tok::LParen => depth += 1,
                Tok::RParen => depth -= 1,
                Tok::Comma if depth == 0 => return true,
                _ => {}
            }
            false
        });
        body = &inner[..comma.unwrap_or(inner.len())];
    }
    let mut depth = 0;
    let mut div = false;
    for t in body {
        match t {
            Tok::LParen => depth += 1,
            Tok::RParen => depth -= 1,
            Tok::Op(op) if depth == 0 => match *op {
                "/" => div = true,
                "+" | "-" | "&" | "=" | "<>" | ">" | ">=" | "<" | "<=" => return false,
                _ => {}
            },
            _ => {}
        }
    }
    div
}

/// 列解析：名称 → (SQL 表达式, 是否数值)
pub trait Resolver {
    fn column(&self, name: &str) -> Option<(String, bool)>;
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    resolver: &'a dyn Resolver,
    columns: Vec<String>,
}

type Node = (String, Ty);

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn expect(&mut self, t: Tok, what: &str) -> AppResult<()> {
        if self.next() != Some(t) {
            return Err(AppError::bad(format!("公式格式错误：缺少{what}")));
        }
        Ok(())
    }

    // 优先级：比较 < 连接 & < 加减 < 乘除 < 一元负号 < 基本项
    fn comparison(&mut self) -> AppResult<Node> {
        let left = self.concat()?;
        if let Some(Tok::Op(op)) = self.peek().cloned()
            && matches!(op, "=" | "<>" | ">" | ">=" | "<" | "<=")
        {
            self.next();
            let right = self.concat()?;
            return Ok((format!("({} {op} {})", left.0, right.0), Ty::Bool));
        }
        Ok(left)
    }
    fn concat(&mut self) -> AppResult<Node> {
        let mut left = self.additive()?;
        while let Some(Tok::Op("&")) = self.peek() {
            self.next();
            let right = self.additive()?;
            left = (format!("concat(CAST({} AS VARCHAR), CAST({} AS VARCHAR))", left.0, right.0), Ty::Text);
        }
        Ok(left)
    }
    fn additive(&mut self) -> AppResult<Node> {
        let mut left = self.term()?;
        while let Some(Tok::Op(op)) = self.peek().cloned()
            && matches!(op, "+" | "-")
        {
            self.next();
            let right = self.term()?;
            left = (format!("({} {op} {})", num(&left)?, num(&right)?), Ty::Num);
        }
        Ok(left)
    }
    fn term(&mut self) -> AppResult<Node> {
        let mut left = self.unary()?;
        while let Some(Tok::Op(op)) = self.peek().cloned()
            && matches!(op, "*" | "/")
        {
            self.next();
            let right = self.unary()?;
            left = if op == "/" {
                (format!("({} / NULLIF({}, 0))", num(&left)?, num(&right)?), Ty::Num)
            } else {
                (format!("({} * {})", num(&left)?, num(&right)?), Ty::Num)
            };
        }
        Ok(left)
    }
    fn unary(&mut self) -> AppResult<Node> {
        if let Some(Tok::Op("-")) = self.peek() {
            self.next();
            let v = self.unary()?;
            return Ok((format!("(-{})", num(&v)?), Ty::Num));
        }
        if let Some(Tok::Op("+")) = self.peek() {
            self.next();
            return self.unary();
        }
        self.primary()
    }
    fn args(&mut self) -> AppResult<Vec<Node>> {
        self.expect(Tok::LParen, "左括号")?;
        let mut out = Vec::new();
        if self.peek() == Some(&Tok::RParen) {
            self.next();
            return Ok(out);
        }
        loop {
            out.push(self.comparison()?);
            match self.next() {
                Some(Tok::Comma) => continue,
                Some(Tok::RParen) => break,
                _ => return Err(AppError::bad("公式格式错误：函数参数缺少逗号或右括号")),
            }
        }
        Ok(out)
    }
    fn primary(&mut self) -> AppResult<Node> {
        match self.next() {
            Some(Tok::Num(n)) => Ok((n, Ty::Num)),
            Some(Tok::Str(s)) => Ok((literal(&s), Ty::Text)),
            Some(Tok::Col(name)) => {
                let (sql, numeric) = self.resolver.column(&name).ok_or_else(|| AppError::bad(format!("没有列「{name}」")))?;
                if !self.columns.contains(&name) {
                    self.columns.push(name);
                }
                Ok((format!("({sql})"), if numeric { Ty::Num } else { Ty::Text }))
            }
            Some(Tok::LParen) => {
                let v = self.comparison()?;
                self.expect(Tok::RParen, "右括号")?;
                Ok(v)
            }
            Some(Tok::Ident(f)) => self.function(&f),
            Some(t) => Err(AppError::bad(format!("公式格式错误：意外的 {t:?}"))),
            None => Err(AppError::bad("公式不完整")),
        }
    }
    fn function(&mut self, f: &str) -> AppResult<Node> {
        if matches!(f, "TRUE" | "FALSE") {
            return Ok((f.to_string(), Ty::Bool));
        }
        let a = self.args()?;
        let n = a.len();
        let need = |lo: usize, hi: usize| -> AppResult<()> {
            if n < lo || n > hi {
                return Err(AppError::bad(format!("{f} 的参数个数不正确")));
            }
            Ok(())
        };
        let text = |x: &Node| format!("CAST({} AS VARCHAR)", x.0);
        Ok(match f {
            "ROUND" => {
                need(1, 2)?;
                let digits = a.get(1).map(|d| d.0.clone()).unwrap_or_else(|| "0".into());
                (format!("round({}, CAST({digits} AS INTEGER))", num(&a[0])?), Ty::Num)
            }
            "ABS" => {
                need(1, 1)?;
                (format!("abs({})", num(&a[0])?), Ty::Num)
            }
            "IF" => {
                need(2, 3)?;
                let other = a.get(2).map(|x| x.0.clone()).unwrap_or_else(|| "NULL".into());
                let ty = if a[1].1 == Ty::Text || a.get(2).is_some_and(|x| x.1 == Ty::Text) { Ty::Text } else { a[1].1 };
                let (yes, no) = if ty == Ty::Text { (text(&a[1]), format!("CAST({other} AS VARCHAR)")) } else { (a[1].0.clone(), other) };
                (format!("(CASE WHEN {} THEN {yes} ELSE {no} END)", a[0].0), ty)
            }
            "IFNULL" | "IFERROR" => {
                need(2, 2)?;
                (format!("COALESCE({}, {})", a[0].0, a[1].0), a[0].1)
            }
            "MAX" | "MIN" => {
                need(1, 16)?;
                let list = a.iter().map(num).collect::<AppResult<Vec<_>>>()?.join(", ");
                (format!("{}({list})", if f == "MAX" { "greatest" } else { "least" }), Ty::Num)
            }
            "LEFT" | "RIGHT" => {
                need(1, 2)?;
                let len = a.get(1).map(|x| x.0.clone()).unwrap_or_else(|| "1".into());
                (format!("{}({}, CAST({len} AS INTEGER))", f.to_lowercase(), text(&a[0])), Ty::Text)
            }
            "MID" => {
                need(3, 3)?;
                (format!("substr({}, CAST({} AS INTEGER), CAST({} AS INTEGER))", text(&a[0]), a[1].0, a[2].0), Ty::Text)
            }
            "LEN" => {
                need(1, 1)?;
                (format!("length({})", text(&a[0])), Ty::Num)
            }
            "UPPER" | "LOWER" | "TRIM" => {
                need(1, 1)?;
                (format!("{}({})", f.to_lowercase(), text(&a[0])), Ty::Text)
            }
            "YEAR" | "MONTH" | "DAY" => {
                need(1, 1)?;
                let (start, len) = match f {
                    "YEAR" => (1, 4),
                    "MONTH" => (6, 2),
                    _ => (9, 2),
                };
                (format!("TRY_CAST(substr({}, {start}, {len}) AS INTEGER)", text(&a[0])), Ty::Num)
            }
            other => return Err(AppError::bad(format!("不支持的函数：{other}（可用：ROUND ABS IF IFNULL MAX MIN LEFT RIGHT MID LEN UPPER LOWER TRIM YEAR MONTH DAY）"))),
        })
    }
}

/// 参与运算的数值。空值保持为空（结果也为空），不按 0 处理：
/// 关联不到成本价时「毛利」显示为空而不是虚高；确需按 0 处理时写 IFNULL([成本价],0)
fn num(n: &Node) -> AppResult<String> {
    match n.1 {
        Ty::Num => Ok(n.0.clone()),
        // 文本参与运算时按数字转换（转换失败为空值）
        Ty::Text => Ok(format!("TRY_CAST({} AS DOUBLE)", n.0)),
        Ty::Bool => Ok(format!("CAST({} AS INTEGER)", n.0)),
    }
}

/// 编译公式
pub fn compile(formula: &str, resolver: &dyn Resolver) -> AppResult<Compiled> {
    let toks = lex(formula)?;
    if toks.is_empty() {
        return Err(AppError::bad("公式不能为空"));
    }
    let ratio = is_ratio(&toks);
    let mut p = Parser { toks, pos: 0, resolver, columns: Vec::new() };
    let (sql, ty) = p.comparison()?;
    if p.pos < p.toks.len() {
        return Err(AppError::bad("公式格式错误：多余的内容"));
    }
    if p.columns.is_empty() {
        return Err(AppError::bad("计算列至少要引用一个列，如 [金额]"));
    }
    Ok(Compiled { sql, ty, columns: p.columns, ratio })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct R;
    impl Resolver for R {
        fn column(&self, name: &str) -> Option<(String, bool)> {
            match name {
                "金额" | "成本价" | "数量" => Some((format!("\"{name}\""), true)),
                "下单时间" | "方向" => Some((format!("\"{name}\""), false)),
                _ => None,
            }
        }
    }

    #[test]
    fn compiles() {
        let c = compile("=[金额]-[成本价]*[数量]", &R).unwrap();
        assert_eq!(c.ty, Ty::Num);
        assert!(c.sql.contains("\"成本价\""));
        assert_eq!(c.columns, vec!["金额", "成本价", "数量"]);
        assert_eq!(compile("=LEFT([下单时间],7)", &R).unwrap().ty, Ty::Text);
        assert_eq!(compile("=IF([方向]=\"入账\",[金额],-[金额])", &R).unwrap().ty, Ty::Num);
        assert!(compile("=[金额]/[数量]", &R).unwrap().sql.contains("NULLIF"));
        assert!(compile("=ROUND([金额]/[数量],4)", &R).unwrap().ratio);
        assert!(!compile("=[金额]-[成本价]/2", &R).unwrap().ratio);
        assert!(compile("=[不存在]+1", &R).is_err());
        assert!(compile("=1+2", &R).is_err());
        assert!(compile("=[金额]; DROP TABLE x", &R).is_err());
        assert!(compile("=SLEEP([金额])", &R).is_err());
        // 引号转义：字面量走 literal()，不可能逃逸出字符串
        let c = compile("=IF([方向]=\"a'b\",1,0)+[金额]", &R).unwrap();
        assert!(c.sql.contains("'a''b'"));
    }
}
