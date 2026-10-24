//! 列类型（kind）：数据集列的业务类型，以及它在 DuckDB 里的存储类型
//!
//! | kind | 含义 | 存储类型 |
//! |---|---|---|
//! | text | 文本 | VARCHAR |
//! | id | 编号（订单号、手机号：长数字串 / 前导零，按文本保存不丢精度） | VARCHAR |
//! | integer | 整数（数量、件数：精确到个位，不经过浮点） | BIGINT |
//! | number | 数值 | DOUBLE |
//! | money | 金额 | DOUBLE |
//! | datetime | 日期时间（统一写成 `2026-09-15 10:00:00` 形式的文本，透视按月 / 季分组、切片器都按这个约定） | VARCHAR |
//! | boolean | 是 / 否 | BOOLEAN |
//!
//! 判断「是不是数值列」一律用 [`is_numeric`]，不要再写 `matches!(kind, "money" | "number")`：新增类型时只改这里。

/// 全部列类型（导入、建填报数据集、模板校验时允许的取值）
pub const ALL: [&str; 7] = ["text", "id", "integer", "number", "money", "datetime", "boolean"];

/// 规范化为静态字符串；未知类型返回 None
pub fn parse(kind: &str) -> Option<&'static str> {
    ALL.iter().find(|k| **k == kind).copied()
}

/// 规范化为静态字符串；未知类型按文本处理（读取老数据 / 外部 JSON 时用）
pub fn normalize(kind: &str) -> &'static str {
    parse(kind).unwrap_or("text")
}

/// 数值列：可以求和 / 平均，筛选按数值比较
pub fn is_numeric(kind: &str) -> bool {
    matches!(kind, "integer" | "number" | "money")
}

/// 文本列：参与关键字搜索（search_text）
pub fn is_textual(kind: &str) -> bool {
    matches!(kind, "text" | "id")
}

/// 建表时的存储类型
pub fn sql_type(kind: &str) -> &'static str {
    match kind {
        "integer" => "BIGINT",
        "number" | "money" => "DOUBLE",
        "boolean" => "BOOLEAN",
        _ => "VARCHAR",
    }
}

/// 中文名称（提示文字、AI / MCP 的数据集说明里用）
pub fn label(kind: &str) -> &'static str {
    match kind {
        "id" => "编号",
        "integer" => "整数",
        "number" => "数值",
        "money" => "金额",
        "datetime" => "日期时间",
        "boolean" => "是/否",
        _ => "文本",
    }
}

/// 新建列时的默认列宽
pub fn default_width(kind: &str) -> u32 {
    match kind {
        "money" | "number" => 110,
        "integer" => 100,
        "boolean" => 80,
        "datetime" => 150,
        "id" => 180,
        _ => 140,
    }
}

/// 布尔值的文本写法（导入、回写时识别）：true / false、是 / 否、yes / no、1 / 0
pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "true" | "t" | "yes" | "y" | "1" | "是" | "真" | "√" => Some(true),
        "false" | "f" | "no" | "n" | "0" | "否" | "假" | "×" => Some(false),
        _ => None,
    }
}

/// 与 [`parse_bool`] 同口径的 SQL 表达式：raw 为已去空白的 VARCHAR 表达式，认不出的值为 NULL
pub fn bool_sql(raw: &str) -> String {
    format!(
        "CASE WHEN lower({raw}) IN ('true', 't', 'yes', 'y', '1', '是', '真', '√') THEN TRUE \
              WHEN lower({raw}) IN ('false', 'f', 'no', 'n', '0', '否', '假', '×') THEN FALSE END"
    )
}

/// 整数的文本写法：可带正负号、千分位逗号，允许 `12.0` / `12.00` 这样小数部分全是 0 的写法；超出 BIGINT 范围返回 None
pub fn parse_integer(s: &str) -> Option<i64> {
    let t: String = s.trim().chars().filter(|c| !matches!(c, ',' | '，')).collect();
    let t = match t.split_once('.') {
        Some((int, frac)) if !frac.is_empty() && frac.chars().all(|c| c == '0') => int.to_string(),
        Some(_) => return None,
        None => t,
    };
    let digits = t.strip_prefix(['+', '-']).unwrap_or(&t);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

/// 与 [`parse_integer`] 同口径的 SQL 表达式：raw 为已去空白的 VARCHAR 表达式，认不出（含有非零小数、超出范围）的值为 NULL
pub fn integer_sql(raw: &str) -> String {
    let plain = format!("replace(replace({raw}, ',', ''), '，', '')");
    format!("CASE WHEN regexp_full_match({plain}, '[+-]?[0-9]+(\\.0+)?') THEN TRY_CAST(regexp_replace({plain}, '\\.0+$', '') AS BIGINT) END")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_family() {
        assert!(is_numeric("integer") && is_numeric("number") && is_numeric("money"));
        assert!(!is_numeric("id") && !is_numeric("boolean") && !is_numeric("datetime"));
        assert_eq!(normalize("whatever"), "text");
        assert_eq!(parse("boolean"), Some("boolean"));
        assert_eq!(parse("date"), None);
    }

    #[test]
    fn integers() {
        assert_eq!(parse_integer("1,234"), Some(1234));
        assert_eq!(parse_integer("-7"), Some(-7));
        assert_eq!(parse_integer("12.00"), Some(12));
        assert_eq!(parse_integer("9007199254740993"), Some(9_007_199_254_740_993));
        assert_eq!(parse_integer("1.5"), None);
        assert_eq!(parse_integer("12."), None);
        assert_eq!(parse_integer("abc"), None);
        assert_eq!(parse_integer("99999999999999999999"), None);
    }

    #[test]
    fn booleans() {
        assert_eq!(parse_bool("TRUE"), Some(true));
        assert_eq!(parse_bool(" 否 "), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }
}
