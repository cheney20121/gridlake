//! 透视表的「计算值」：用公式把已汇总出来的几个值组合成一个新值（毛利 = 金额 - 成本，毛利率 = 毛利 / 金额，客单价 = 金额 / 订单数）
//!
//! 公式在**汇总之后**按每个格子（明细行、小计、总计、每一列）单独计算：比率是「汇总值的比」，不是「明细比率的汇总」，
//! 所以总计行的毛利率是总毛利 / 总金额，不是各行毛利率的平均。
//! 语法：数字、`[值名称]`（引用透视表里已有的普通值，按表头文字或列名匹配）、`+ - * /`、括号、负号。
//! 任一被引用的值为空、或除数为 0，结果为空（透视表里显示为空白而不是 0 或无穷大）。

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    /// 第几个值（值列表里的下标）
    Value(usize),
    Neg(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
}

#[derive(Debug, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(char),
    LParen,
    RParen,
}

fn lex(src: &str) -> AppResult<Vec<Tok>> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '+' | '-' | '*' | '/' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            '×' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '(' | '（' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' | '）' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '[' | '【' => {
                let close = if c == '[' { ']' } else { '】' };
                let end = chars[i + 1..].iter().position(|&x| x == close).ok_or_else(|| AppError::bad("公式里的 [ 没有对应的 ]"))?;
                let name: String = chars[i + 1..i + 1 + end].iter().collect();
                if name.trim().is_empty() {
                    return Err(AppError::bad("公式里有空的 []"));
                }
                out.push(Tok::Name(name.trim().to_string()));
                i += end + 2;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                out.push(Tok::Num(text.parse().map_err(|_| AppError::bad(format!("公式里的数字「{text}」写法不对")))?));
            }
            c => return Err(AppError::bad(format!("公式里有不支持的字符「{c}」（只支持数字、[值名称]、+ - * /、括号）"))),
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    resolve: &'a dyn Fn(&str) -> AppResult<usize>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    // expr := term (('+' | '-') term)*
    fn expr(&mut self) -> AppResult<Expr> {
        let mut left = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek() {
            let op = *op;
            self.pos += 1;
            let right = self.term()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // term := factor (('*' | '/') factor)*
    fn term(&mut self) -> AppResult<Expr> {
        let mut left = self.factor()?;
        while let Some(Tok::Op(op @ ('*' | '/'))) = self.peek() {
            let op = *op;
            self.pos += 1;
            let right = self.factor()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // factor := '-' factor | '+' factor | number | [name] | '(' expr ')'
    fn factor(&mut self) -> AppResult<Expr> {
        let Some(t) = self.toks.get(self.pos) else {
            return Err(AppError::bad("公式没有写完（结尾缺少数字或 [值名称]）"));
        };
        self.pos += 1;
        match t {
            Tok::Op('-') => Ok(Expr::Neg(Box::new(self.factor()?))),
            Tok::Op('+') => self.factor(),
            Tok::Num(n) => Ok(Expr::Num(*n)),
            Tok::Name(name) => Ok(Expr::Value((self.resolve)(name)?)),
            Tok::LParen => {
                let e = self.expr()?;
                match self.peek() {
                    Some(Tok::RParen) => {
                        self.pos += 1;
                        Ok(e)
                    }
                    _ => Err(AppError::bad("公式里的括号没有配对")),
                }
            }
            Tok::RParen => Err(AppError::bad("公式里多了一个 )")),
            Tok::Op(c) => Err(AppError::bad(format!("公式里的「{c}」前面缺少数字或 [值名称]"))),
        }
    }
}

/// 解析公式；`resolve` 把 `[名称]` 换成值的下标（找不到时返回带说明的错误）
pub fn parse(src: &str, resolve: &dyn Fn(&str) -> AppResult<usize>) -> AppResult<Expr> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err(AppError::bad("计算值的公式不能为空"));
    }
    let mut p = Parser { toks, pos: 0, resolve };
    let e = p.expr()?;
    if p.pos < p.toks.len() {
        return Err(AppError::bad("公式里有多余的内容，请检查括号与运算符"));
    }
    Ok(e)
}

/// 计算一个格子：`values` 是该格子各个值的汇总结果；有空值参与或除数为 0 时返回 None
pub fn eval(e: &Expr, values: &[Option<f64>]) -> Option<f64> {
    match e {
        Expr::Num(n) => Some(*n),
        Expr::Value(i) => values.get(*i).copied().flatten(),
        Expr::Neg(x) => eval(x, values).map(|v| -v),
        Expr::Bin(op, a, b) => {
            let (a, b) = (eval(a, values)?, eval(b, values)?);
            let r = match op {
                '+' => a + b,
                '-' => a - b,
                '*' => a * b,
                _ => {
                    if b == 0.0 {
                        return None;
                    }
                    a / b
                }
            };
            r.is_finite().then_some(r)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &str) -> AppResult<usize> {
        match n {
            "金额" => Ok(0),
            "成本" => Ok(1),
            "订单数" => Ok(2),
            _ => Err(AppError::bad(format!("找不到「{n}」"))),
        }
    }
    fn calc(src: &str, v: &[Option<f64>]) -> Option<f64> {
        eval(&parse(src, &names).unwrap(), v)
    }
    const V: [Option<f64>; 3] = [Some(200.0), Some(150.0), Some(4.0)];

    #[test]
    fn precedence_and_parentheses() {
        assert_eq!(calc("[金额]-[成本]", &V), Some(50.0));
        assert_eq!(calc("[金额]-[成本]/[订单数]*2", &V), Some(125.0));
        assert_eq!(calc("([金额]-[成本])/[金额]", &V), Some(0.25));
        assert_eq!(calc("[金额]/[订单数]", &V), Some(50.0));
        assert_eq!(calc("2+3*4", &V), Some(14.0));
        assert_eq!(calc("(2+3)*4", &V), Some(20.0));
        assert_eq!(calc("10-2-3", &V), Some(5.0));
        assert_eq!(calc("100/10/5", &V), Some(2.0));
    }

    #[test]
    fn unary_and_fullwidth() {
        assert_eq!(calc("-[金额]+300", &V), Some(100.0));
        assert_eq!(calc("--[金额]", &V), Some(200.0));
        assert_eq!(calc("【金额】-1", &V), Some(199.0));
        assert_eq!(calc("（[金额]×2）÷[订单数]", &V), Some(100.0));
        assert_eq!(calc("1.5*[订单数]", &V), Some(6.0));
    }

    #[test]
    fn empty_or_zero_gives_none() {
        assert_eq!(calc("[金额]/[订单数]", &[Some(1.0), None, Some(0.0)]), None);
        assert_eq!(calc("[金额]-[成本]", &[Some(1.0), None, Some(0.0)]), None);
        assert_eq!(calc("[金额]*0", &V), Some(0.0));
        assert_eq!(calc("1/(1-1)", &V), None);
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let err = |s: &str| format!("{:?}", parse(s, &names).unwrap_err());
        assert!(err("[没有这个]+1").contains("找不到「没有这个」"));
        assert!(err("").contains("不能为空"));
        assert!(err("[金额").contains("没有对应的 ]"));
        assert!(err("[]").contains("空的"));
        assert!(err("([金额]").contains("括号"));
        assert!(err("[金额])").contains("多余"));
        assert!(err("[金额]+").contains("没有写完"));
        assert!(err("*[金额]").contains("缺少"));
        assert!(err("[金额]^2").contains("不支持的字符"));
        assert!(err("1..2").contains("写法不对"));
    }
}
