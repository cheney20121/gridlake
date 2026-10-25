//! AI 助手可调用的工具
//!
//! - 查询类（直接执行，结果回给模型）：数据集列表、数据集结构、列取值、汇总查询、两期对比归因
//! - 产出类（只生成「建议」，由用户在界面上点「应用」后才创建）：透视表、图表、单元格公式、计算列。
//!   生成前先在服务端校验（列是否存在、能否算出结果），错误回给模型让它修正；建议里带预览，便于核对

use std::collections::HashMap;

use duckdb::Connection;
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::dataset::{self, DatasetDef, Filter, QuerySpec};
use crate::error::{AppError, AppResult};
use crate::pivot::{self, PivotField, PivotRequest, PivotResult, PivotSort, PivotValue};

/// 返回给模型的结果最长字符数（过长会挤占上下文）
const MAX_RESULT_CHARS: usize = 8000;
const MAX_ROWS: usize = 60;

/// 工具执行结果
pub struct Outcome {
    /// 回给模型的内容
    pub content: String,
    /// 界面上的一句话说明
    pub summary: String,
    pub ok: bool,
    /// 产出类工具的建议（kind + 参数 + 预览）
    pub action: Option<Json>,
}

impl Outcome {
    fn ok(content: String, summary: String) -> Self {
        Self { content, summary, ok: true, action: None }
    }
    fn err(msg: String) -> Self {
        Self { content: format!("错误：{msg}"), summary: msg, ok: false, action: None }
    }
}

// ── 工具定义（JSON Schema）──

fn filters_schema() -> Json {
    json!({
        "type": "array",
        "description": "筛选条件（同时满足）。op=in：values 等于其中之一（\"\" 表示空值）；op=between：min~max（含两端；日期写 \"2026-09-01\"，结束日期写当天即可）；op=contains：包含 value；op=prefix：以 value 开头（如月份 \"2026-09\"）",
        "items": {
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["in", "between", "contains", "prefix"] },
                "column": { "type": "string" },
                "values": { "type": "array", "items": { "type": "string" } },
                "min": { "type": ["string", "number", "null"] },
                "max": { "type": ["string", "number", "null"] },
                "value": { "type": "string" }
            },
            "required": ["op", "column"]
        }
    })
}

fn fields_schema(desc: &str) -> Json {
    json!({
        "type": "array",
        "description": desc,
        "items": {
            "type": "object",
            "properties": {
                "column": { "type": "string" },
                "granularity": { "type": ["string", "null"], "enum": ["year", "quarter", "month", "week", "day", null], "description": "日期时间列按年 / 季度 / 月 / 周（ISO 周）/ 日分组" }
            },
            "required": ["column"]
        }
    })
}

fn values_schema() -> Json {
    json!({
        "type": "array",
        "description": "汇总值。agg：sum 求和 / count 计数（可不填 column）/ avg 平均 / min / max / first 首个、last 末个（按行顺序第一个 / 最后一个非空值，如期初 / 期末余额、最新单价）/ count_distinct 去重计数（精确）/ approx_distinct 近似去重计数（订单号、用户 ID 这类上千万个不同值的列用它，快得多，误差通常在 5% 以内）/ median 中位数 / p10、p25、p75、p90、p95、p99 分位数（精确，几千万行以上很占内存）/ approx_median、approx_p90、approx_p95、approx_p99 近似分位数（大数据集用它，快且省内存，名次误差通常远小于 1%，越靠尾部值的偏差越大；对账用精确版） / stddev 标准差 / var 方差。calc 计算值（用公式组合同一张透视表里已有的值，如毛利率；需要 formula，可选 format：money / number / percent，column 不填；公式写 [值的表头] 或 [列名]，如 ([金额（求和）]-[成本（求和）])/[金额（求和）]，比率按汇总后的值相除，总计行也是对的；不能引用另一个计算值；计算值不要放在第一个）。show：raw 原值（默认）/ pct_total 占总计 / pct_row 占行合计 / pct_col 占列合计 / yoy 同比 / yoy_diff 同比增减 / mom 环比 / mom_diff 环比增减（同比环比需要行或列里有按年/季度/月/周/日分组的日期字段）",
        "items": {
            "type": "object",
            "properties": {
                "column": { "type": ["string", "null"] },
                "agg": { "type": "string", "enum": ["sum", "count", "avg", "min", "max", "first", "last", "count_distinct", "approx_distinct", "median", "approx_median", "approx_p90", "approx_p95", "approx_p99", "p10", "p25", "p75", "p90", "p95", "p99", "stddev", "var", "calc"] },
                "show": { "type": "string", "enum": ["raw", "pct_total", "pct_row", "pct_col", "yoy", "yoy_diff", "mom", "mom_diff"] },
                "label": { "type": "string", "description": "表头文字（计算值建议填，如「毛利率」）" },
                "formula": { "type": "string", "description": "agg 为 calc 时的公式" },
                "format": { "type": "string", "enum": ["money", "number", "percent"], "description": "agg 为 calc 时的数字格式" }
            },
            "required": ["agg"]
        }
    })
}

fn dataset_param() -> Json {
    json!({ "type": "string", "description": "数据集名称或 ID" })
}

/// 数据概况、数据清洗、窗口计算（单独一个 json!：放进上面的大数组会超出宏展开深度）
fn data_tool_definitions() -> Json {
    json!([
        {
            "type": "function",
            "function": {
                "name": "profile_dataset",
                "description": "数据概况（数据质量体检）：一次扫描列出每一列的空值个数、只有空白的个数、不同值个数、最小 / 最大值、数值列的合计 / 平均 / 中位数 / 零和负数个数、日期列里认不出的写法个数，并列出可能的问题。用户问「这张表的数据有没有问题 / 数据质量怎么样 / 哪些列缺数据」或要清洗数据前先调用它",
                "parameters": { "type": "object", "properties": { "dataset": dataset_param(), "filters": filters_schema(), "columns": { "type": "array", "items": { "type": "string" }, "description": "只看这些列（不填看全部列）" } }, "required": ["dataset"] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "propose_clean",
                "description": "建议对数据表做一次数据清洗（直接修改数据集里的数据，用户看过预览、确认后才执行，执行后可以撤销最近一次）。op：dedupe 删除重复行（columns 这些列都相同的只留最先导入的一行，不填为全部列）/ trim 去掉首尾空白（含全角空格，columns 不填为全部文本列）/ replace 查找替换（column、find、replace；whole=true 整格等于才替换，否则替换每一处，统一「北京」「北京市」这类写法时用 whole）/ fill 填充空值（column；value 填固定值，或 down=true 用上一行的值向下填充）/ delete_rows 删除筛选出的行（必须给 filters）。一次只做一种清洗；先用 profile_dataset、column_values 找出问题和准确写法。只能用于导入的数据表（不能用于关联视图、外部数据集），只有数据集所有者或管理员能执行",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "dataset": dataset_param(),
                        "op": { "type": "string", "enum": ["dedupe", "trim", "replace", "fill", "delete_rows"] },
                        "columns": { "type": "array", "items": { "type": "string" }, "description": "dedupe / trim 的列" },
                        "column": { "type": "string", "description": "replace / fill 的列" },
                        "find": { "type": "string" },
                        "replace": { "type": "string", "description": "替换成（空字符串即清空）" },
                        "whole": { "type": "boolean" },
                        "value": { "type": "string", "description": "fill 填的值" },
                        "down": { "type": "boolean", "description": "fill 用上一行的值向下填充" },
                        "filters": filters_schema()
                    },
                    "required": ["dataset", "op"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_window_dataset",
                "description": "建议用窗口计算生成一个新数据集（用户确认后创建；源数据集更新后自动重算）：在每一行旁边加累计、排名、和上一行比、移动平均、组内合计 / 占比等列，或每组只留前 N 行。用户要「每家店铺销售额最高的 10 天」「每个商品的累计销量」「环比变化率序列」并想保留结果时用；只是回答一个数字用 query_data / run_sql。calcs 的 func：running_sum 累计 / row_number 序号 / rank 排名（大的在前）/ rank_asc 排名（小的在前）/ prev 上一行的值 / diff 与上一行的差 / pct_change 较上一行的变化率 / moving_avg 移动平均（rows 行，默认 7）/ group_sum 组内合计 / group_avg 组内平均 / group_count 组内行数 / share 占组内合计的比例；除 row_number、group_count 外都要 column（数值列）。累计、序号、排名、上一行、移动平均、topN 需要 orderBy",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "dataset": dataset_param(),
                        "name": { "type": "string", "description": "新数据集名称，如「各店铺销售额前 10 天」" },
                        "calcs": { "type": "array", "minItems": 1, "items": { "type": "object", "properties": { "func": { "type": "string", "enum": ["running_sum", "row_number", "rank", "rank_asc", "prev", "diff", "pct_change", "moving_avg", "group_sum", "group_avg", "group_count", "share"] }, "column": { "type": ["string", "null"] }, "name": { "type": "string", "description": "新列名" }, "rows": { "type": "integer" } }, "required": ["func", "name"] } },
                        "partitionBy": { "type": "array", "items": { "type": "string" }, "description": "分组列（不填为整个数据集一组）" },
                        "orderBy": { "type": "array", "items": { "type": "object", "properties": { "column": { "type": "string" }, "desc": { "type": "boolean" } }, "required": ["column"] }, "description": "组内顺序" },
                        "topN": { "type": "integer", "description": "每组只保留按 orderBy 排在前面的 N 行" }
                    },
                    "required": ["dataset", "name", "calcs"]
                }
            }
        }
    ])
}

pub fn definitions() -> Json {
    let query_props = json!({
        "dataset": dataset_param(),
        "rows": fields_schema("行分组字段（可多个，如 [店铺]、[下单时间 按月]）；不填则只算总计"),
        "columns": fields_schema("列分组字段（可选，如按月横向展开）"),
        "values": values_schema(),
        "filters": filters_schema(),
        "sort": { "type": "object", "properties": { "by": { "type": "string", "enum": ["key", "value"] }, "desc": { "type": "boolean" } }, "description": "按分组名或按第一个值排序" },
        "top": { "type": "integer", "description": "只保留第一个行字段按第一个值排名的前 N 项" },
        "calcItems": { "type": "array", "description": "计算项：在某个行 / 列字段的取值之间做运算，新增一个取值（如列字段「单据类型」里新增「期末」= [入库] - [出库]）。与 values 里的计算值（agg: calc，在值之间运算）不同。公式对每个值、每个格子分别计算，引用该字段下已有的取值；不能与 top 或占比 / 同比 / 环比同时用", "items": { "type": "object", "properties": { "field": { "type": "string", "description": "行或列字段的列名" }, "name": { "type": "string", "description": "新取值的名称，如「期末」" }, "formula": { "type": "string", "description": "如 [入库] - [出库]" } }, "required": ["field", "name", "formula"] } }
    });
    let mut list = json!([
        {
            "type": "function",
            "function": {
                "name": "describe_dataset",
                "description": "查看数据集的列（类型）、行数、日期列的起止范围、文本列的常见取值。回答问题前先确认列名和取值写法",
                "parameters": { "type": "object", "properties": { "dataset": dataset_param() }, "required": ["dataset"] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "column_values",
                "description": "列出某列的取值及行数（最多 50 个），可按关键字搜索。用于确认筛选值的准确写法（如店铺全称、动账场景）",
                "parameters": { "type": "object", "properties": { "dataset": dataset_param(), "column": { "type": "string" }, "search": { "type": "string" } }, "required": ["dataset", "column"] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "query_data",
                "description": "在服务端对数据集分组汇总（千万行也很快），返回汇总表。所有数字都必须来自这个工具或 compare_periods，不能估算",
                "parameters": { "type": "object", "properties": query_props.clone(), "required": ["dataset", "values"] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "propose_plan",
                "description": "把需要多个产出的需求合成一张「方案卡」：2~8 个有序步骤（create_pivot / create_chart / write_formula / add_calc_column / create_data_table / set_table_view / set_column_format），用户预览整体后一次应用、一次撤销。例如「按店铺做月度销售透视表并配一张趋势图」「把退款订单放成一张表，金额用万元显示、超过 10 万的标红」。只有一个产出时直接用对应的单个工具。每一步的 args 与单独调用该工具时完全相同，并会逐步校验（有错会指明第几步）；后面的步骤不能引用前面步骤新建的计算列，需要时先单独提 add_calc_column；但可以调整本方案前面新建的数据表（用同样的表名，新建数据表的步骤要排在前面），一个方案里最多新建一张数据表",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "方案名称，如「各店铺月度销售分析」" },
                        "steps": {
                            "type": "array",
                            "minItems": 2,
                            "maxItems": 5,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "tool": { "type": "string", "enum": ["create_pivot", "create_chart", "write_formula", "add_calc_column", "create_data_table", "set_table_view", "set_column_format"] },
                                    "args": { "type": "object", "description": "该工具的参数" }
                                },
                                "required": ["tool", "args"]
                            }
                        }
                    },
                    "required": ["title", "steps"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_anomalies",
                "description": "自动扫描数据集，找出最近 7 天相对此前 28 天明显偏离的地方（某天行数 / 金额骤降或骤升、某个店铺 / 渠道 / 状态的日均大幅变化或消失）。用户问「有什么异常 / 哪里不对劲 / 最近有没有问题」时先调用它，再用 query_data / compare_periods 深入核实原因；结论里的数字来自这个工具",
                "parameters": { "type": "object", "properties": { "dataset": dataset_param() }, "required": ["dataset"] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "compare_periods",
                "description": "两期对比归因：给出两期的合计与变化，并按指定维度列出贡献最大的增加项和减少项。用于回答「为什么本月比上月下降」「哪些店铺 / 商品拖累了增长」",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "dataset": dataset_param(),
                        "value": { "type": "object", "properties": { "column": { "type": ["string", "null"] }, "agg": { "type": "string", "enum": ["sum", "count", "count_distinct"] } }, "required": ["agg"] },
                        "date_column": { "type": "string", "description": "用于划分两期的日期时间列" },
                        "current": { "type": "object", "properties": { "start": { "type": "string" }, "end": { "type": "string" } }, "required": ["start", "end"], "description": "本期起止日期，如 2026-09-01 ~ 2026-09-30" },
                        "base": { "type": "object", "properties": { "start": { "type": "string" }, "end": { "type": "string" } }, "required": ["start", "end"], "description": "对比期起止日期" },
                        "dimensions": { "type": "array", "items": { "type": "string" }, "description": "归因维度（列名，1~4 个），如 [店铺, 商品名称]" },
                        "filters": filters_schema()
                    },
                    "required": ["dataset", "value", "date_column", "current", "base", "dimensions"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_sql",
                "description": "对数据集执行只读 SQL（DuckDB 方言）。表名就是数据集名称、列名是中文列名（含空格或符号时用双引号）；只能 SELECT / WITH。用于 query_data 做不到的计算：跨数据集关联（JOIN）、窗口函数（排名、累计、环比序列）、条件聚合、复杂筛选。日期时间列是文本（如 2026-09-15 10:00:00），按月分组用 left(列,7)。别名前加 AS。最多返回 50 行，请先汇总再查",
                "parameters": { "type": "object", "properties": { "sql": { "type": "string" }, "purpose": { "type": "string", "description": "这条查询要算什么（给用户看）" } }, "required": ["sql"] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_pivot",
                "description": "建议新建一个数据透视表页签（用户确认后创建，结果可下钻核对明细）。当回答涉及一张汇总表、用户想保留结果或继续分析时使用",
                "parameters": {
                    "type": "object",
                    "properties": { "title": { "type": "string", "description": "页签名称，如「9月店铺退款」" }, "dataset": dataset_param(), "rows": query_props["rows"], "columns": query_props["columns"], "values": query_props["values"], "filters": query_props["filters"], "sort": query_props["sort"], "top": query_props["top"] },
                    "required": ["title", "dataset", "rows", "values"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_chart",
                "description": "建议插入一张数据集图表（用户确认后插入当前工作表，随数据刷新）。趋势用 line / area，比较用 column / bar，构成用 pie / donut（≤6 类），利润桥用 waterfall。只有一个数值轴：量纲不同的指标请分两张图",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "type": { "type": "string", "enum": ["column", "bar", "line", "area", "pie", "donut", "waterfall"] },
                        "dataset": dataset_param(),
                        "x": { "type": "object", "properties": { "column": { "type": "string" }, "granularity": { "type": ["string", "null"], "enum": ["year", "month", "day", null] } }, "required": ["column"], "description": "分类轴（日期列可按年/月/日）" },
                        "values": { "type": "array", "items": { "type": "object", "properties": { "column": { "type": ["string", "null"] }, "agg": { "type": "string", "enum": ["sum", "count", "avg", "min", "max", "count_distinct"] } }, "required": ["agg"] }, "description": "数值（每个一条系列）" },
                        "split_by": { "type": "string", "description": "按此字段拆分系列（与多个数值二选一），如按店铺" },
                        "filters": filters_schema(),
                        "sort": { "type": "string", "enum": ["x", "value"], "description": "按分类名（日期默认）或按数值降序" },
                        "top": { "type": "integer", "description": "只取前 N 个分类" }
                    },
                    "required": ["title", "type", "dataset", "x", "values"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_formula",
                "description": "建议在当前工作表的某个单元格写入公式（用户确认后写入）。大数据集用 DSUMIFS(\"数据集名\",\"求和列\",\"条件列\",条件,...)、DCOUNTIFS(\"数据集名\",\"条件列\",条件,...)、DAVERAGEIFS、DMAXIFS、DMINIFS（用法同 DSUMIFS，取最大 / 最小值）、DCOUNTDISTINCTIFS(\"数据集名\",\"去重列\",\"条件列\",条件,...)（不重复取值个数）、DLOOKUP(\"数据集名\",\"返回列\",\"查找列\",值)，条件写法同 SUMIFS（\">=2026-09-01\"、\"*退款*\"、单元格引用）",
                "parameters": {
                    "type": "object",
                    "properties": { "cell": { "type": "string", "description": "单元格地址，如 B3" }, "formula": { "type": "string", "description": "以 = 开头" }, "explanation": { "type": "string", "description": "公式含义（给用户看）" } },
                    "required": ["cell", "formula"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "add_calc_column",
                "description": "建议给数据集加一个计算列（用户确认后添加，之后透视 / 图表 / 公式都能用）。语法：=[列A]-[列B]、=ROUND([佣金]/[订单实付应结],4)、=IF([动账方向]=\"入账\",[动账金额],-[动账金额])、=LEFT([下单时间],7)；函数 ROUND ABS IF IFNULL MAX MIN LEFT RIGHT MID LEN UPPER LOWER TRIM YEAR MONTH DAY；拆分 / 合并文本：SPLITPART([收货地址],\"-\",2) 取第 2 段（负数从后往前数）、TEXTBEFORE / TEXTAFTER([列],\"-\",N) 第 N 个分隔符之前 / 之后、FIND(\"-\",[列])、SUBSTITUTE([列],\"旧\",\"新\")、CONCAT([省],[市])、TEXTJOIN(\"-\",TRUE,[省],[市],[区])（TRUE 忽略空值）。用户要「把一列拆成几列」时每一段各加一个计算列（多段用 propose_plan 一次加完），「把几列合成一列」用 TEXTJOIN；不修改原数据，之后新导入的行也会自动拆好",
                "parameters": {
                    "type": "object",
                    "properties": { "dataset": dataset_param(), "name": { "type": "string", "description": "新列名" }, "formula": { "type": "string" }, "kind": { "type": "string", "enum": ["money", "number", "text", "datetime"], "description": "结果类型（可不填，自动推断）" } },
                    "required": ["dataset", "name", "formula"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "set_table_view",
                "description": "建议调整工作表里已有的「数据表」（界面上下文里的 tables）的视图：筛选、排序、显示哪些列、关键字搜索（用户确认后生效，千万行也只取可视区域）。用户想「只看…」「按…排序」「隐藏某些列」时用它，而不是把明细行拉进回答。filters / sort / fields / keyword 每一项：给了就整体替换该项，不给就保持不变；要在现有筛选上追加，把现有条件（见界面上下文）和新条件一起写进 filters。给出前先用 column_values 确认筛选值的写法",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "table": { "type": "string", "description": "数据表名称（界面上下文 tables 里的 name）" },
                        "dataset": dataset_param(),
                        "filters": filters_schema(),
                        "sort": { "type": "array", "items": { "type": "object", "properties": { "column": { "type": "string" }, "desc": { "type": "boolean" } }, "required": ["column"] }, "description": "排序（可多列，靠前的优先）；传空数组表示取消排序" },
                        "fields": { "type": "array", "items": { "type": "string" }, "description": "显示的列，按此顺序；传空数组表示显示全部列" },
                        "keyword": { "type": "string", "description": "关键字搜索；传空字符串表示清除" }
                    },
                    "required": ["table", "dataset"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "save_query_as_dataset",
                "description": "建议把一条 SQL 的结果另存为一个新数据集（用户确认后才创建；之后可对它建透视表、图表、数据表）。用户说「把这个结果存成数据集 / 保存下来以后反复用」时用：先用 run_sql 确认结果对不对，再调用本工具。sql 只能是 SELECT / WITH，结果列名要各不相同（用 AS 起名），不能叫 rowid / __rid / search_text / 资金净额；表名就是已有的数据集名。数据集名称不能与已有的重名，最长 60 个字。不适合存很小的、几行的汇总——那种用透视表。保存后不会自动刷新，需要在 SQL 查询里手动重算",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "sql": { "type": "string", "description": "DuckDB 只读 SQL" },
                        "name": { "type": "string", "description": "新数据集的名称，如「9月店铺退款汇总」" },
                        "purpose": { "type": "string", "description": "这个数据集用来做什么（给用户看）" }
                    },
                    "required": ["sql", "name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_data_table",
                "description": "建议在当前工作表新建一张「数据表」：把数据集的明细放进工作表，可带筛选、排序、显示列、关键字（用户确认后插入到选中的单元格处）。用户想「把…放成一张表 / 在表格里看…的明细」时用它；要汇总用 create_pivot。mode：float 浮动表格（默认，内部滚动，千万行也只取可视区域）；cells 铺到单元格（命中 ≤ 1 万行时才行，可以用 A:A 等区域引用）。不要把明细行贴进回答。给出前先用 column_values 确认筛选值的写法",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "dataset": dataset_param(),
                        "name": { "type": "string", "description": "表名（公式按它引用，不含 [ ] \" ' !，最长 40 字；重名时自动加序号）；不填则为「数据集名+表」" },
                        "fields": { "type": "array", "items": { "type": "string" }, "description": "显示的列，按此顺序；不填显示全部列" },
                        "filters": filters_schema(),
                        "sort": { "type": "array", "items": { "type": "object", "properties": { "column": { "type": "string" }, "desc": { "type": "boolean" } }, "required": ["column"] }, "description": "排序（可多列，靠前的优先）" },
                        "keyword": { "type": "string", "description": "关键字搜索" },
                        "mode": { "type": "string", "enum": ["float", "cells"], "description": "float 浮动表格（默认）/ cells 铺到单元格（≤ 1 万行）" }
                    },
                    "required": ["dataset"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "set_column_format",
                "description": "建议设置数据表某一列的数字格式和条件格式（用户确认后生效）。只改显示、不改数据，千万行也只对屏幕上的格子求值。用户说「金额用万元显示」「超过 10 万的标红」「负数标红」「含退款的行加粗」时用它。numFmt：default 恢复默认 / int 整数 / dec2 两位小数 / percent 百分比（值 0.12 显示为 12.00%）/ plain 不加千分位 / wan 万元（只对浮动表格有效）。rules 给了就整体替换该列的全部规则（传空数组清除）；多条规则按顺序，第一条命中的生效。阈值要来自用户或查询结果，不要自己编。条件格式只对浮动表格有效，单元格模式的数据表只能设数字格式",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "table": { "type": "string", "description": "数据表名称（界面上下文 tables 里的 name）" },
                        "dataset": dataset_param(),
                        "column": { "type": "string", "description": "列名" },
                        "numFmt": { "type": "string", "enum": ["default", "int", "dec2", "percent", "wan", "plain"], "description": "数字格式，只用于金额 / 数值列" },
                        "rules": {
                            "type": "array",
                            "maxItems": 8,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "op": { "type": "string", "enum": [">", ">=", "<", "<=", "=", "!=", "between", "contains", "equals", "empty", "negative"], "description": "> >= < <= between negative 只用于金额 / 数值列；contains / equals 只用于文本 / 编号 / 日期列；= != empty 都可以" },
                                    "value": { "type": ["number", "string"], "description": "比较值；between 为下限" },
                                    "value2": { "type": "number", "description": "between 的上限" },
                                    "style": { "type": "object", "properties": { "color": { "type": "string", "description": "文字颜色，如 #C00000" }, "bg": { "type": "string", "description": "底色，如 #FDE8E8" }, "bold": { "type": "boolean" } }, "description": "命中时的样式，至少给一项" }
                                },
                                "required": ["op", "style"]
                            },
                            "description": "条件格式规则"
                        }
                    },
                    "required": ["table", "dataset", "column"]
                }
            }
        }
    ]);
    if let (Some(a), Json::Array(more)) = (list.as_array_mut(), data_tool_definitions()) {
        a.extend(more);
    }
    list
}

// ── 辅助 ──

fn resolve(catalog: &[DatasetDef], key: &str) -> AppResult<DatasetDef> {
    let k = key.trim();
    if let Some(d) = catalog.iter().find(|d| d.id == k || d.name == k) {
        return Ok(d.clone());
    }
    let hits: Vec<&DatasetDef> = catalog.iter().filter(|d| d.name.contains(k) || k.contains(d.name.as_str())).collect();
    if hits.len() == 1 {
        return Ok(hits[0].clone());
    }
    Err(AppError::bad(format!("没有数据集「{k}」。可用的数据集：{}", catalog.iter().map(|d| format!("「{}」", d.name)).collect::<Vec<_>>().join("、"))))
}

fn check_col(def: &DatasetDef, c: &str) -> AppResult<()> {
    if def.columns.iter().any(|x| x.title == c || x.key == c) {
        return Ok(());
    }
    Err(AppError::bad(format!("数据集「{}」没有列「{c}」。可用的列：{}", def.name, def.columns.iter().map(|x| x.title.as_str()).collect::<Vec<_>>().join("、"))))
}

fn kind_label(k: &str) -> &'static str {
    crate::kinds::label(k)
}

fn num(v: Option<f64>, kind: &str) -> String {
    match v {
        None => "-".into(),
        Some(x) if kind == "percent" => format!("{:.1}%", x * 100.0),
        Some(x) if x.fract() == 0.0 && x.abs() < 1e15 => format!("{x:.0}"),
        Some(x) => format!("{x:.2}"),
    }
}

/// 结束日期只写到日时补到当天结束
fn end_of_day(s: &str) -> String {
    if s.len() == 10 { format!("{s} 23:59:59") } else { s.to_string() }
}

fn normalize_filters(def: &DatasetDef, filters: Vec<Filter>) -> AppResult<Vec<Filter>> {
    filters
        .into_iter()
        .map(|f| {
            check_col(def, f.column())?;
            Ok(match f {
                Filter::Between { column, min, max } => {
                    let max = match max {
                        Some(Json::String(s)) => Some(Json::String(end_of_day(&s))),
                        other => other,
                    };
                    Filter::Between { column, min, max }
                }
                other => other,
            })
        })
        .collect()
}

fn truncate(s: String) -> String {
    if s.chars().count() <= MAX_RESULT_CHARS {
        return s;
    }
    let cut: String = s.chars().take(MAX_RESULT_CHARS).collect();
    format!("{cut}\n…（结果过长已截断，请缩小范围或用 top 只取前几项）")
}

/// 透视结果 → 文本表（制表符分隔）
pub fn table_text(r: &PivotResult) -> (Vec<String>, Vec<Vec<String>>, String) {
    let kinds: Vec<&str> = r.values.iter().map(|v| v.kind.as_str()).collect();
    let mut header: Vec<String> = r.row_fields.clone();
    let groups: Vec<Option<usize>> = if r.column_keys.is_empty() { vec![None] } else { (0..r.column_keys.len()).map(Some).chain(r.column_total.then_some(None)).collect() };
    let shown_groups: Vec<Option<usize>> = groups.iter().copied().take((24 / r.values.len().max(1)).max(1)).collect();
    for g in &shown_groups {
        for v in &r.values {
            let prefix = g.map(|i| r.column_keys[i].iter().map(|k| k.clone().unwrap_or_else(|| "（空白）".into())).collect::<Vec<_>>().join("/")).map(|s| format!("{s} · ")).unwrap_or_else(|| if r.column_keys.is_empty() { String::new() } else { "总计 · ".into() });
            header.push(format!("{prefix}{}", v.label));
        }
    }
    let mut rows = Vec::new();
    let data: Vec<&pivot::PivotRow> = r.rows.iter().filter(|x| x.kind != "subtotal").collect();
    // 应用了「前 N 项」时总计只含列出的项，标明以免误当作全部合计
    let shown = r.rows.iter().filter(|x| x.kind == "data").count();
    let total_label = match r.first_level_count {
        Some(n) if r.row_fields.len() == 1 && (shown as i64) < n => format!("前 {shown} 项合计"),
        _ => "总计".to_string(),
    };
    for row in data.iter().take(MAX_ROWS) {
        let mut line: Vec<String> = if row.kind == "total" {
            let mut l = vec![total_label.clone()];
            l.extend(std::iter::repeat_n(String::new(), r.row_fields.len().saturating_sub(1)));
            l
        } else {
            let mut l: Vec<String> = row.keys.iter().map(|k| k.clone().unwrap_or_else(|| "（空白）".into())).collect();
            l.resize(r.row_fields.len(), String::new());
            l
        };
        for g in &shown_groups {
            let ci = match g {
                Some(i) => *i,
                None => row.cells.len().saturating_sub(1),
            };
            for (vi, k) in kinds.iter().enumerate() {
                line.push(num(row.cells.get(ci).and_then(|c| c.get(vi)).copied().flatten(), k));
            }
        }
        rows.push(line);
    }
    let mut note = String::new();
    if data.len() > MAX_ROWS {
        note.push_str(&format!("共 {} 行，仅列出前 {MAX_ROWS} 行。", data.len()));
    }
    if shown_groups.len() < groups.len() {
        note.push_str(&format!("列分组共 {} 组，仅列出前 {} 组。", groups.len(), shown_groups.len()));
    }
    if let Some(n) = r.first_level_count.filter(|n| r.rows.iter().filter(|x| x.kind == "data").count() < *n as usize) {
        note.push_str(&format!("（共 {n} 项，只列出前 {shown} 项；全部合计请去掉 top 再查）"));
    }
    (header, rows, note)
}

fn render_table(header: &[String], rows: &[Vec<String>], note: &str) -> String {
    let mut s = header.join("\t");
    for r in rows {
        s.push('\n');
        s.push_str(&r.join("\t"));
    }
    if !note.is_empty() {
        s.push('\n');
        s.push_str(note);
    }
    s
}

// ── 参数 ──

#[derive(Deserialize)]
struct DatasetArg {
    dataset: String,
}

#[derive(Deserialize)]
struct ValuesArg {
    dataset: String,
    column: String,
    #[serde(default)]
    search: Option<String>,
}

#[derive(Deserialize, Default)]
struct SortArg {
    #[serde(default)]
    by: Option<String>,
    #[serde(default)]
    desc: Option<bool>,
}

#[derive(Deserialize)]
struct QueryArg {
    dataset: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    rows: Vec<PivotField>,
    #[serde(default)]
    columns: Vec<PivotField>,
    values: Vec<PivotValue>,
    #[serde(default)]
    filters: Vec<Filter>,
    #[serde(default)]
    sort: Option<SortArg>,
    #[serde(default)]
    top: Option<usize>,
    #[serde(default, rename = "calcItems")]
    calc_items: Vec<crate::pivot::CalcItem>,
}

#[derive(Deserialize)]
struct Range {
    start: String,
    end: String,
}

#[derive(Deserialize)]
struct CompareArg {
    dataset: String,
    value: PivotValue,
    date_column: String,
    current: Range,
    base: Range,
    #[serde(default)]
    dimensions: Vec<String>,
    #[serde(default)]
    filters: Vec<Filter>,
}

#[derive(Deserialize)]
struct ChartArg {
    title: String,
    #[serde(rename = "type")]
    kind: String,
    dataset: String,
    x: PivotField,
    values: Vec<PivotValue>,
    #[serde(default)]
    split_by: Option<String>,
    #[serde(default)]
    filters: Vec<Filter>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    top: Option<usize>,
}

#[derive(Deserialize)]
struct FormulaArg {
    cell: String,
    formula: String,
    #[serde(default)]
    explanation: Option<String>,
}

#[derive(Deserialize)]
struct CalcArg {
    dataset: String,
    name: String,
    formula: String,
    #[serde(default)]
    kind: Option<String>,
}

fn parse<T: for<'de> Deserialize<'de>>(args: &str) -> AppResult<T> {
    serde_json::from_str(args).map_err(|e| AppError::bad(format!("参数格式不正确：{e}")))
}

fn build_request(def: &DatasetDef, a: &QueryArg, subtotals: bool) -> AppResult<PivotRequest> {
    if a.values.is_empty() {
        return Err(AppError::bad("至少需要一个汇总值（values）"));
    }
    for f in a.rows.iter().chain(&a.columns) {
        check_col(def, &f.column)?;
    }
    for v in &a.values {
        if let Some(c) = v.column.as_deref().filter(|c| !c.is_empty()) {
            check_col(def, c)?;
        } else if v.agg == "calc" {
            if v.formula.as_deref().is_none_or(|f| f.trim().is_empty()) {
                return Err(AppError::bad("计算值（calc）需要公式（formula）"));
            }
        } else if v.agg != "count" {
            return Err(AppError::bad(format!("{} 需要指定列（column）", v.agg)));
        }
    }
    let sort = a.sort.as_ref();
    Ok(PivotRequest {
        rows: a.rows.clone(),
        columns: a.columns.clone(),
        values: a.values.clone(),
        query: QuerySpec { filters: normalize_filters(def, a.filters.clone())?, ..Default::default() },
        sort: PivotSort {
            by: sort.and_then(|s| s.by.clone()).unwrap_or_else(|| if a.top.is_some() { "value".into() } else { "key".into() }),
            value_index: 0,
            desc: sort.and_then(|s| s.desc).unwrap_or(a.top.is_some() || sort.and_then(|s| s.by.as_deref()) == Some("value")),
        },
        top: a.top,
        subtotals,
        grand_totals: true,
        base_field: None,
        aliases: Default::default(),
        calc_items: a.calc_items.clone(),
    })
}

pub fn filters_desc(filters: &[Filter]) -> String {
    filters
        .iter()
        .map(|f| match f {
            Filter::In { column, values } => format!("{column}={}", values.join("/")),
            Filter::Between { column, min, max } => format!("{column} {}~{}", min.as_ref().map(|v| v.as_str().map(String::from).unwrap_or(v.to_string())).unwrap_or_default(), max.as_ref().map(|v| v.as_str().map(String::from).unwrap_or(v.to_string())).unwrap_or_default()),
            Filter::Contains { column, value } => format!("{column}含「{value}」"),
            Filter::Prefix { column, value } => format!("{column}以「{value}」开头"),
            Filter::Prefixes { column, values } => format!("{column}以「{}」之一开头", values.join("」「")),
        })
        .collect::<Vec<_>>()
        .join("，")
}

// ── 执行 ──

pub fn execute(conn: &Connection, name: &str, args: &str) -> Outcome {
    execute_scoped(conn, name, args, None)
}

/// 执行工具，只能看到 scope 内的数据集（ID；None 为全部）——外部 MCP 接入按令牌授权范围
pub fn execute_scoped(conn: &Connection, name: &str, args: &str, scope: Option<&std::collections::HashSet<String>>) -> Outcome {
    let catalog: Vec<DatasetDef> = match dataset::datasets(conn) {
        Ok(c) => c.into_iter().filter(|d| scope.is_none_or(|s| s.contains(&d.id))).collect(),
        Err(e) => return Outcome::err(e.message()),
    };
    let r = match name {
        "describe_dataset" => describe(conn, &catalog, args),
        "column_values" => column_values(conn, &catalog, args),
        "query_data" => query(conn, &catalog, args),
        "find_anomalies" => anomalies(conn, &catalog, args),
        "propose_plan" => propose_plan(conn, &catalog, args),
        "compare_periods" => compare(conn, &catalog, args),
        "create_pivot" => create_pivot(conn, &catalog, args),
        "create_chart" => create_chart(conn, &catalog, args),
        "write_formula" => write_formula(&catalog, args),
        "add_calc_column" => add_calc(conn, &catalog, args),
        "set_table_view" => set_table_view(conn, &catalog, args),
        "set_column_format" => set_column_format(conn, &catalog, args),
        "create_data_table" => create_data_table(conn, &catalog, args),
        "save_query_as_dataset" => save_query_as_dataset(conn, &catalog, args, scope),
        "run_sql" => run_sql(conn, args, scope),
        "profile_dataset" => profile_dataset(conn, &catalog, args),
        "propose_clean" => propose_clean(conn, &catalog, args),
        "create_window_dataset" => create_window_dataset(conn, &catalog, args),
        other => Err(AppError::bad(format!("没有工具 {other}"))),
    };
    match r {
        Ok(mut o) => {
            o.content = truncate(o.content);
            o
        }
        Err(e) => Outcome::err(e.message()),
    }
}

/// 方案卡里最多的步骤数
const MAX_PLAN_STEPS: usize = 8;

#[derive(Deserialize)]
struct PlanArg {
    title: String,
    steps: Vec<PlanStep>,
}

#[derive(Deserialize)]
struct PlanStep {
    tool: String,
    #[serde(default)]
    args: Json,
}

/// 新建数据表建议里的表名（小写；没给名字时与前端默认的「数据集名+表」一致）
fn created_name(step: &Json) -> String {
    step["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("{}表", step["datasetName"].as_str().unwrap_or(""))).trim().to_lowercase()
}

/// 方案：逐步用对应工具生成并校验建议，合成一张卡（kind = plan，steps 里是各步的建议）
fn propose_plan(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: PlanArg = parse(args)?;
    let title = a.title.trim();
    if title.is_empty() {
        return Err(AppError::bad("方案需要名称"));
    }
    if !(2..=MAX_PLAN_STEPS).contains(&a.steps.len()) {
        return Err(AppError::bad(format!("方案需要 2~{MAX_PLAN_STEPS} 步（当前 {} 步）；只有一个产出时直接用对应的单个工具", a.steps.len())));
    }
    let mut steps = Vec::new();
    for (i, st) in a.steps.iter().enumerate() {
        let n = i + 1;
        let step_args = if st.args.is_string() { st.args.as_str().unwrap_or("{}").to_string() } else { st.args.to_string() };
        let r = match st.tool.as_str() {
            "create_pivot" => create_pivot(conn, catalog, &step_args),
            "create_chart" => create_chart(conn, catalog, &step_args),
            "write_formula" => write_formula(catalog, &step_args),
            "add_calc_column" => add_calc(conn, catalog, &step_args),
            "create_data_table" => create_data_table(conn, catalog, &step_args),
            "set_table_view" => set_table_view(conn, catalog, &step_args),
            "set_column_format" => set_column_format(conn, catalog, &step_args),
            other => return Err(AppError::bad(format!("第 {n} 步：方案里不能用 {other}，只能是 create_pivot / create_chart / write_formula / add_calc_column / create_data_table / set_table_view / set_column_format"))),
        };
        match r {
            Ok(o) if o.ok && o.action.is_some() => steps.push(o.action.expect("checked")),
            Ok(o) => return Err(AppError::bad(format!("第 {n} 步（{}）有问题：{}", st.tool, o.summary))),
            Err(e) => return Err(AppError::bad(format!("第 {n} 步（{}）有问题：{}", st.tool, e.message()))),
        }
    }
    // 同一个单元格被两步写入、同一个计算列名被两步创建、同一张数据表的同一项被两步调整：应用到一半才失败（或后一步悄悄盖掉前一步），不如现在就指出来
    let mut seen = std::collections::HashSet::new();
    for (i, st) in steps.iter().enumerate() {
        let table = st["table"].as_str().unwrap_or("").trim().to_lowercase();
        let (key, what) = match st["kind"].as_str() {
            Some("formula") => (st["cell"].as_str().map(|c| format!("cell:{c}")), "写入了同一个位置"),
            Some("calc") => (Some(format!("calc:{}:{}", st["datasetId"].as_str().unwrap_or(""), st["name"].as_str().unwrap_or(""))), "创建了同名计算列"),
            Some("table_view") => (Some(format!("view:{table}")), "调整了同一张数据表的视图（请合并成一步）"),
            Some("column_format") => (Some(format!("fmt:{table}:{}", st["column"].as_str().unwrap_or(""))), "设置了同一张数据表的同一列样式（请合并成一步）"),
            Some("table_create") => (Some(format!("create:{}", created_name(st))), "新建了同名数据表"),
            _ => (None, ""),
        };
        if let Some(k) = key
            && !seen.insert(k)
        {
            return Err(AppError::bad(format!("第 {} 步与前面的步骤{what}", i + 1)));
        }
    }
    // 一个方案最多新建一张数据表（新建的表都落在选中的单元格处，两张会叠在一起）；调整本方案新建的表时，新建要排在前面
    let created: Vec<(usize, String)> = steps.iter().enumerate().filter(|(_, s)| s["kind"] == "table_create").map(|(i, s)| (i, created_name(s))).collect();
    if created.len() > 1 {
        return Err(AppError::bad(format!("一个方案里最多新建一张数据表（第 {} 步和第 {} 步都在新建）；需要多张表时请分开提", created[0].0 + 1, created[1].0 + 1)));
    }
    for (i, st) in steps.iter().enumerate() {
        if !matches!(st["kind"].as_str(), Some("table_view" | "column_format")) {
            continue;
        }
        let table = st["table"].as_str().unwrap_or("").trim().to_lowercase();
        if let Some((j, _)) = created.iter().find(|(j, name)| *name == table && *j > i) {
            return Err(AppError::bad(format!("第 {} 步要调整的数据表「{}」在第 {} 步才新建，请把新建数据表的步骤排在前面", i + 1, st["table"].as_str().unwrap_or(""), j + 1)));
        }
    }
    let kinds: Vec<&str> = steps.iter().map(|s| match s["kind"].as_str().unwrap_or("") {
        "pivot" => "透视表",
        "chart" => "图表",
        "formula" => "公式",
        "calc" => "计算列",
        "table_view" => "数据表视图",
        "column_format" => "列样式",
        "table_create" => "新建数据表",
        _ => "?",
    }).collect();
    let summary = format!("方案「{title}」共 {} 步：{}", steps.len(), kinds.join(" → "));
    let content = format!("{summary}。已生成方案卡，等待用户预览并确认后一次应用（全部成功或全部撤回）。请用一两句话说明方案做了什么，不要重复列出每一步的参数。");
    let mut o = Outcome::ok(content, summary);
    o.action = Some(json!({ "kind": "plan", "title": title, "steps": steps }));
    Ok(o)
}

fn anomalies(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: DatasetArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    let r = crate::anomaly::scan(conn, &def)?;
    let mut text = format!("数据集「{}」", def.name);
    if let Some(d) = &r.latest_date {
        text += &format!("（最新数据日期 {d}，比较最近 {} 天与此前 {} 天）", crate::anomaly::RECENT, crate::anomaly::BASE);
    }
    text += "\n";
    if let Some(n) = &r.note {
        text += &format!("说明：{n}\n");
    }
    if r.findings.is_empty() && r.note.is_none() {
        text += "没有发现明显偏离（走势按星期折算，门槛：偏离 3.5 倍波动且幅度 ≥ 30%；分组变化 ≥ 50%）。\n";
    }
    for f in &r.findings {
        let sev = match f.severity {
            "high" => "高",
            _ => "中",
        };
        text += &format!("- 【{sev}】{}：{}\n", f.title, f.detail);
    }
    let summary = if r.findings.is_empty() { "没有发现明显异常".to_string() } else { format!("发现 {} 处异常", r.findings.len()) };
    Ok(Outcome::ok(text, summary))
}

fn describe(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: DatasetArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    let rows: i64 = conn.query_row(&format!("SELECT count(*) FROM {}", def.table), [], |r| r.get(0))?;
    let mut out = format!("数据集「{}」（ID {}），共 {rows} 行。列：\n", def.name, def.id);
    let mut text_cols = 0;
    for c in &def.columns {
        let mut line = format!("- {}（{}）", c.title, kind_label(c.kind));
        let need_values = c.kind == "datetime" || (matches!(c.kind, "text") && text_cols < 12);
        if need_values && rows > 0 {
            let d = dataset::distinct(conn, &def.id, &dataset::DistinctRequest { spec: QuerySpec::default(), column: c.title.clone(), search: None })?;
            if c.kind == "datetime" {
                let s = |v: &Option<Json>| v.as_ref().map(|x| x.as_str().map(String::from).unwrap_or(x.to_string())).unwrap_or_default();
                line.push_str(&format!("：{} ~ {}", s(&d.min), s(&d.max)));
            } else {
                text_cols += 1;
                let top: Vec<String> = d.values.iter().take(8).map(|v| if v.value.is_empty() { "（空）".into() } else { v.value.clone() }).collect();
                line.push_str(&format!("：{} 个不同值，如 {}", d.total_distinct, top.join("、")));
            }
        }
        if c.computed {
            line.push_str(" [计算列]");
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(Outcome::ok(out, format!("查看「{}」的结构（{rows} 行、{} 列）", def.name, def.columns.len())))
}

fn column_values(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: ValuesArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    check_col(&def, &a.column)?;
    let d = dataset::distinct(conn, &def.id, &dataset::DistinctRequest { spec: QuerySpec::default(), column: a.column.clone(), search: a.search.clone().filter(|s| !s.is_empty()) })?;
    let mut out = format!("「{}」.「{}」共 {} 个不同值{}：\n", def.name, a.column, d.total_distinct, if d.values.len() < d.total_distinct as usize { format!("，列出前 {} 个", d.values.len().min(50)) } else { String::new() });
    for v in d.values.iter().take(50) {
        out.push_str(&format!("{}\t{}\n", if v.value.is_empty() { "（空）" } else { &v.value }, v.count));
    }
    Ok(Outcome::ok(out, format!("查看「{}」的取值{}", a.column, a.search.map(|s| format!("（搜索「{s}」）")).unwrap_or_default())))
}

fn query(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: QueryArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    let req = build_request(&def, &a, false)?;
    crate::usage::hit(&def.id, crate::usage::AI);
    let r = pivot::pivot(conn, &def.id, &req)?;
    let (header, rows, note) = table_text(&r);
    let by: Vec<String> = a.rows.iter().map(|f| f.column.clone()).collect();
    let summary = format!(
        "查询「{}」：{}{}{}",
        def.name,
        r.values.iter().map(|v| v.label.clone()).collect::<Vec<_>>().join("、"),
        if by.is_empty() { String::new() } else { format!("，按{}", by.join("、")) },
        if req.query.filters.is_empty() { String::new() } else { format!("，筛选 {}", filters_desc(&req.query.filters)) }
    );
    Ok(Outcome::ok(render_table(&header, &rows, &note), summary))
}

fn compare(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: CompareArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    check_col(&def, &a.date_column)?;
    if let Some(c) = a.value.column.as_deref().filter(|c| !c.is_empty()) {
        check_col(&def, c)?;
    }
    if a.dimensions.len() > 4 {
        return Err(AppError::bad("归因维度最多 4 个"));
    }
    for d in &a.dimensions {
        check_col(&def, d)?;
    }
    let base_filters = normalize_filters(&def, a.filters.clone())?;
    let period = |r: &Range| {
        let mut f = base_filters.clone();
        f.push(Filter::Between { column: a.date_column.clone(), min: Some(Json::String(r.start.clone())), max: Some(Json::String(end_of_day(&r.end))) });
        f
    };
    let value = PivotValue { show: None, ..a.value.clone() };
    let run = |rows: Vec<PivotField>, filters: Vec<Filter>| -> AppResult<HashMap<String, f64>> {
        let req = PivotRequest {
            rows,
            columns: vec![],
            values: vec![value.clone()],
            query: QuerySpec { filters, ..Default::default() },
            sort: PivotSort::default(),
            top: None,
            subtotals: false,
            grand_totals: true,
            base_field: None,
            aliases: Default::default(),
            calc_items: vec![],
        };
        crate::usage::hit(&def.id, crate::usage::AI);
        let r = pivot::pivot(conn, &def.id, &req)?;
        Ok(r.rows
            .iter()
            .map(|x| {
                let key = if x.kind == "total" { "\u{0}总计".to_string() } else { x.keys.first().cloned().flatten().unwrap_or_else(|| "（空白）".into()) };
                (key, x.cells.last().and_then(|c| c.first().copied().flatten()).unwrap_or(0.0))
            })
            .collect())
    };
    let cur_total = run(vec![], period(&a.current))?.get("\u{0}总计").copied().unwrap_or(0.0);
    let base_total = run(vec![], period(&a.base))?.get("\u{0}总计").copied().unwrap_or(0.0);
    let delta = cur_total - base_total;
    let pct = if base_total != 0.0 { format!("{:+.1}%", delta / base_total.abs() * 100.0) } else { "-".into() };
    let agg_label = match a.value.agg.as_str() {
        "sum" => "求和",
        "count_distinct" => "去重计数",
        "avg" => "平均",
        "min" => "最小",
        "max" => "最大",
        _ => "计数",
    };
    let label = match (a.value.agg.as_str(), a.value.column.as_deref()) {
        ("count", _) => "行数".to_string(),
        (_, Some(c)) => format!("{c}（{agg_label}）"),
        (_, None) => agg_label.to_string(),
    };
    let mut out = format!(
        "{label}：本期（{} ~ {}）{}，对比期（{} ~ {}）{}，变化 {}（{pct}）{}\n",
        a.current.start,
        a.current.end,
        num(Some(cur_total), ""),
        a.base.start,
        a.base.end,
        num(Some(base_total), ""),
        num(Some(delta), ""),
        if base_filters.is_empty() { String::new() } else { format!("。筛选：{}", filters_desc(&base_filters)) }
    );
    for dim in &a.dimensions {
        let field = vec![PivotField { column: dim.clone(), granularity: None, label: None }];
        let cur = run(field.clone(), period(&a.current))?;
        let base = run(field, period(&a.base))?;
        let mut keys: Vec<&String> = cur.keys().chain(base.keys()).filter(|k| !k.starts_with('\u{0}')).collect();
        keys.sort();
        keys.dedup();
        let mut moves: Vec<(String, f64, f64, f64)> = keys.iter().map(|k| {
            let c = cur.get(*k).copied().unwrap_or(0.0);
            let b = base.get(*k).copied().unwrap_or(0.0);
            ((*k).clone(), c, b, c - b)
        }).collect();
        moves.sort_by(|x, y| x.3.partial_cmp(&y.3).unwrap_or(std::cmp::Ordering::Equal));
        let line = |m: &(String, f64, f64, f64)| {
            let share = if delta != 0.0 { format!("，占总变化 {:.0}%", m.3 / delta * 100.0) } else { String::new() };
            format!("  {}：本期 {}，对比期 {}，变化 {}{share}\n", m.0, num(Some(m.1), ""), num(Some(m.2), ""), num(Some(m.3), ""))
        };
        out.push_str(&format!("\n按「{dim}」（共 {} 项）\n减少最多：\n", moves.len()));
        for m in moves.iter().filter(|m| m.3 < 0.0).take(5) {
            out.push_str(&line(m));
        }
        out.push_str("增加最多：\n");
        for m in moves.iter().rev().filter(|m| m.3 > 0.0).take(5) {
            out.push_str(&line(m));
        }
    }
    Ok(Outcome::ok(out, format!("对比「{}」{label}：{} ~ {} 对 {} ~ {}{}", def.name, a.current.start, a.current.end, a.base.start, a.base.end, if a.dimensions.is_empty() { String::new() } else { format!("，按{}归因", a.dimensions.join("、")) })))
}

fn preview_json(header: Vec<String>, rows: Vec<Vec<String>>, note: String) -> Json {
    json!({ "header": header, "rows": rows.into_iter().take(12).collect::<Vec<_>>(), "note": note })
}

fn create_pivot(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: QueryArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    if a.rows.is_empty() && a.columns.is_empty() {
        return Err(AppError::bad("透视表至少需要一个行字段"));
    }
    let req = build_request(&def, &a, true)?;
    crate::usage::hit(&def.id, crate::usage::AI);
    let r = pivot::pivot(conn, &def.id, &req)?;
    let (header, rows, note) = table_text(&r);
    let title = a.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| format!("{}透视", def.name));
    let action = json!({
        "kind": "pivot",
        "title": title,
        "datasetId": def.id,
        "datasetName": def.name,
        "request": req,
        "preview": preview_json(header.clone(), rows.clone(), note.clone()),
    });
    let mut o = Outcome::ok(
        format!("已生成透视表建议「{title}」，等待用户确认创建。结果预览：\n{}", render_table(&header, &rows.into_iter().take(15).collect::<Vec<_>>(), &note)),
        format!("建议新建透视表「{title}」"),
    );
    o.action = Some(action);
    Ok(o)
}

fn create_chart(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: ChartArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    if a.values.is_empty() {
        return Err(AppError::bad("至少需要一个数值"));
    }
    if a.split_by.is_some() && a.values.len() > 1 {
        return Err(AppError::bad("按字段拆分系列时只能有一个数值"));
    }
    if matches!(a.kind.as_str(), "pie" | "donut") && (a.split_by.is_some() || a.values.len() > 1) {
        return Err(AppError::bad("饼图 / 环形图只能有一个数值、不能拆分系列"));
    }
    let q = QueryArg {
        dataset: def.id.clone(),
        title: None,
        rows: vec![a.x.clone()],
        columns: a.split_by.iter().map(|c| PivotField { column: c.clone(), granularity: None, label: None }).collect(),
        values: a.values.iter().map(|v| PivotValue { show: None, ..v.clone() }).collect(),
        filters: a.filters.clone(),
        sort: Some(SortArg { by: Some(if a.sort.as_deref() == Some("value") { "value".into() } else { "key".into() }), desc: Some(a.sort.as_deref() == Some("value")) }),
        top: a.top,
        calc_items: vec![],
    };
    let req = build_request(&def, &q, false)?;
    crate::usage::hit(&def.id, crate::usage::AI);
    let r = pivot::pivot(conn, &def.id, &req)?;
    let (header, rows, note) = table_text(&r);
    let spec = json!({
        "v": 1,
        "type": a.kind,
        "title": a.title,
        "source": {
            "kind": "dataset",
            "datasetId": def.id,
            "x": a.x,
            "values": a.values.iter().map(|v| json!({ "column": v.column, "agg": v.agg })).collect::<Vec<_>>(),
            "splitBy": a.split_by,
            "filters": req.query.filters,
            "sort": a.sort.clone().unwrap_or_else(|| "x".into()),
            "top": a.top,
        },
        "legend": "top",
        "labels": if a.kind == "waterfall" { "all" } else { "none" },
        "unit": "auto",
    });
    let action = json!({ "kind": "chart", "title": a.title, "datasetName": def.name, "spec": spec, "preview": preview_json(header.clone(), rows.clone(), note.clone()) });
    let mut o = Outcome::ok(
        format!("已生成图表建议「{}」，等待用户确认插入。图表数据预览：\n{}", a.title, render_table(&header, &rows.into_iter().take(15).collect::<Vec<_>>(), &note)),
        format!("建议插入图表「{}」", a.title),
    );
    o.action = Some(action);
    Ok(o)
}

fn write_formula(catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: FormulaArg = parse(args)?;
    let cell = a.cell.trim().to_uppercase().replace('$', "");
    let valid_cell = {
        let letters = cell.chars().take_while(char::is_ascii_alphabetic).count();
        let rest = &cell[letters..];
        (1..=3).contains(&letters) && !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) && !rest.starts_with('0')
    };
    if !valid_cell {
        return Err(AppError::bad(format!("单元格地址「{}」不正确，应如 B3", a.cell)));
    }
    let formula = a.formula.trim().to_string();
    if !formula.starts_with('=') {
        return Err(AppError::bad("公式必须以 = 开头"));
    }
    if formula.matches('(').count() != formula.matches(')').count() {
        return Err(AppError::bad("公式括号不配对"));
    }
    for call in crate::template::refs::d_calls(&formula) {
        let Some(name) = call.args.first().and_then(|x| x.literal.clone()) else { continue };
        let def = catalog.iter().find(|d| d.name == name).ok_or_else(|| AppError::bad(format!("{} 引用的数据集「{name}」不存在（需写数据集全名）。可用：{}", call.func, catalog.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join("、"))))?;
        for i in call.column_args() {
            if let Some(c) = call.args.get(i).and_then(|x| x.literal.as_deref()) {
                check_col(def, c)?;
            }
        }
    }
    let action = json!({ "kind": "formula", "cell": cell, "formula": formula, "explanation": a.explanation });
    let mut o = Outcome::ok(format!("已生成公式建议：{cell} {formula}，等待用户确认写入"), format!("建议在 {cell} 写入公式"));
    o.action = Some(action);
    Ok(o)
}

fn add_calc(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: CalcArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    let name = a.name.trim();
    if name.is_empty() {
        return Err(AppError::bad("请给计算列起一个名字"));
    }
    if def.columns.iter().any(|c| c.title == name) {
        return Err(AppError::bad(format!("数据集「{}」已有列「{name}」，请换个名字", def.name)));
    }
    if def.kind == "view" {
        return Err(AppError::bad("关联视图上不能直接加计算列，请加在其左侧数据集上"));
    }
    let formula = if a.formula.trim().starts_with('=') { a.formula.trim().to_string() } else { format!("={}", a.formula.trim()) };
    let p = dataset::preview_calc(conn, &def.id, &formula)?;
    let samples: Vec<String> = p.samples.iter().take(5).map(|s| s.to_string()).collect();
    let action = json!({
        "kind": "calc",
        "datasetId": def.id,
        "datasetName": def.name,
        "name": name,
        "formula": formula,
        "columnKind": a.kind,
        "preview": { "resultKind": p.kind, "nullRows": p.null_rows, "totalRows": p.total_rows, "samples": p.samples.iter().take(5).collect::<Vec<_>>() },
    });
    let mut o = Outcome::ok(
        format!("已生成计算列建议「{name}」{formula}（结果类型 {}，{} 行中 {} 行结果为空），等待用户确认添加。样例：{}", kind_label(p.kind), p.total_rows, p.null_rows, samples.join("；")),
        format!("建议给「{}」加计算列「{name}」", def.name),
    );
    o.action = Some(action);
    Ok(o)
}

#[derive(Deserialize)]
struct TableViewArg {
    table: String,
    dataset: String,
    #[serde(default)]
    filters: Option<Vec<Filter>>,
    #[serde(default)]
    sort: Option<Vec<dataset::SortSpec>>,
    #[serde(default)]
    fields: Option<Vec<String>>,
    #[serde(default)]
    keyword: Option<String>,
}

/// 把筛选里的列名换成列键（界面按列键认筛选列）
fn filter_with_key(def: &DatasetDef, f: Filter) -> Filter {
    let key = |c: String| def.columns.iter().find(|x| x.title == c || x.key == c).map(|x| x.key.clone()).unwrap_or(c);
    match f {
        Filter::In { column, values } => Filter::In { column: key(column), values },
        Filter::Between { column, min, max } => Filter::Between { column: key(column), min, max },
        Filter::Contains { column, value } => Filter::Contains { column: key(column), value },
        Filter::Prefix { column, value } => Filter::Prefix { column: key(column), value },
        Filter::Prefixes { column, values } => Filter::Prefixes { column: key(column), values },
    }
}

/// 1234567 → 1,234,567
fn group_digits(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn cell_text(v: &Json) -> String {
    match v {
        Json::Null => String::new(),
        Json::String(s) => s.clone(),
        Json::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.to_string(),
            (_, Some(x)) if x.fract() == 0.0 && x.abs() < 1e15 => format!("{x:.0}"),
            (_, Some(x)) => format!("{x:.2}"),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// 数据表视图建议：筛选 / 排序 / 显示列 / 关键字。服务端核对列名与筛选值写法，并给出命中行数与前几行
fn set_table_view(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: TableViewArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    let table = a.table.trim().to_string();
    if table.is_empty() {
        return Err(AppError::bad("需要数据表名称（table）"));
    }
    if a.filters.is_none() && a.sort.is_none() && a.fields.is_none() && a.keyword.is_none() {
        return Err(AppError::bad("没有要调整的内容：filters / sort / fields / keyword 至少给一项"));
    }

    let view = canon_view(&def, a.filters, a.sort, a.fields, a.keyword)?;
    let pv = view_preview(conn, &def, &view)?;
    let parts = view_parts(&def, &view);
    let title = format!("{table}：{}", parts.join("；"));

    let mut note = format!("命中 {} 行 / 共 {} 行", group_digits(pv.matched), group_digits(pv.total));
    if view.filters.is_none() && (view.keyword.as_deref().is_some_and(|k| !k.is_empty()) || view.sort.is_some() || view.fields.is_some()) {
        note.push_str("（预览不含表上现有的筛选）");
    }

    let mut action = serde_json::Map::new();
    action.insert("kind".into(), json!("table_view"));
    action.insert("title".into(), json!(title));
    action.insert("table".into(), json!(table));
    action.insert("datasetId".into(), json!(def.id));
    action.insert("datasetName".into(), json!(def.name));
    put_view(&mut action, &view);
    action.insert("preview".into(), preview_json(pv.header.clone(), pv.rows.clone(), note.clone()));

    let mut o = Outcome::ok(
        format!("已生成数据表视图建议「{title}」，{note}，等待用户确认应用。前几行预览：\n{}", render_table(&pv.header, &pv.rows, "")),
        format!("建议调整数据表「{table}」：{}", parts.join("；")),
    );
    o.action = Some(Json::Object(action));
    Ok(o)
}

const MAX_TABLE_NAME: usize = 40;
/// 单元格模式数据表最多的行数（与前端 MAX_CELL_ROWS 一致）
const MAX_CELL_ROWS: i64 = 10_000;

#[derive(Deserialize)]
struct TableCreateArg {
    dataset: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    fields: Option<Vec<String>>,
    #[serde(default)]
    filters: Option<Vec<Filter>>,
    #[serde(default)]
    sort: Option<Vec<dataset::SortSpec>>,
    #[serde(default)]
    keyword: Option<String>,
    /// float 浮动表格（默认）/ cells 铺到单元格
    #[serde(default)]
    mode: Option<String>,
}

/// 建议新建一个数据表：数据集的明细放进当前工作表（可带筛选、排序、显示列、关键字）。服务端核对并给出命中行数与前几行
fn create_data_table(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: TableCreateArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    let name = a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(String::from);
    if let Some(n) = &name {
        if n.chars().count() > MAX_TABLE_NAME {
            return Err(AppError::bad(format!("表名最长 {MAX_TABLE_NAME} 个字（当前 {} 个）", n.chars().count())));
        }
        if n.contains(['[', ']', '"', '\'', '!']) {
            return Err(AppError::bad("表名不能包含 [ ] \" ' ! 等字符（公式按表名引用）"));
        }
    }
    let mode = match a.mode.as_deref().unwrap_or("float") {
        m @ ("float" | "cells") => m.to_string(),
        other => return Err(AppError::bad(format!("没有模式「{other}」，只能是 float（浮动表格）或 cells（铺到单元格）"))),
    };
    let view = canon_view(&def, a.filters, a.sort, a.fields, a.keyword)?;
    let pv = view_preview(conn, &def, &view)?;
    if mode == "cells" && pv.matched > MAX_CELL_ROWS {
        return Err(AppError::bad(format!("命中 {} 行，超过单元格模式最多 {} 行的上限：请收紧筛选，或用浮动表格（mode=float，千万行也只取可视区域）", group_digits(pv.matched), group_digits(MAX_CELL_ROWS))));
    }
    let shown_name = name.clone().unwrap_or_else(|| format!("{}表", def.name));
    let mut parts = view_parts(&def, &view);
    parts.insert(0, format!("数据集「{}」", def.name));
    let title = format!("新建数据表「{shown_name}」：{}", parts.join("；"));
    let note = format!("{}，命中 {} 行 / 共 {} 行，插入到当前选中的单元格处", if mode == "cells" { "铺到单元格" } else { "浮动表格" }, group_digits(pv.matched), group_digits(pv.total));

    let mut action = serde_json::Map::new();
    action.insert("kind".into(), json!("table_create"));
    action.insert("title".into(), json!(title));
    action.insert("datasetId".into(), json!(def.id));
    action.insert("datasetName".into(), json!(def.name));
    action.insert("mode".into(), json!(mode));
    if let Some(n) = &name {
        action.insert("name".into(), json!(n));
    }
    put_view(&mut action, &view);
    action.insert("preview".into(), preview_json(pv.header.clone(), pv.rows.clone(), note.clone()));

    let mut o = Outcome::ok(
        format!("已生成新建数据表建议「{title}」，{note}，等待用户确认。前几行预览：\n{}", render_table(&pv.header, &pv.rows, "")),
        format!("建议新建数据表「{shown_name}」：{}", parts.join("；")),
    );
    o.action = Some(Json::Object(action));
    Ok(o)
}

/// 规整后的数据表视图：没给的项为 None（保持不变 / 用默认）
struct View {
    filters: Option<Vec<Filter>>,
    sort: Option<Vec<dataset::SortSpec>>,
    fields: Option<Vec<String>>,
    keyword: Option<String>,
}

/// 核对列名与筛选值写法，并整理成界面使用的写法：筛选 / 排序用列键，显示列用列标题
fn canon_view(def: &DatasetDef, filters: Option<Vec<Filter>>, sort: Option<Vec<dataset::SortSpec>>, fields: Option<Vec<String>>, keyword: Option<String>) -> AppResult<View> {
    let filters = match filters {
        Some(fs) => Some(normalize_filters(def, fs)?.into_iter().map(|f| filter_with_key(def, f)).collect()),
        None => None,
    };
    let sort = match sort {
        Some(ss) => Some(
            ss.into_iter()
                .map(|s| {
                    check_col(def, &s.column)?;
                    let key = def.columns.iter().find(|x| x.title == s.column || x.key == s.column).map(|x| x.key.clone()).unwrap_or(s.column);
                    Ok(dataset::SortSpec { column: key, desc: s.desc })
                })
                .collect::<AppResult<Vec<_>>>()?,
        ),
        None => None,
    };
    let fields = match fields {
        Some(fs) => {
            let mut out: Vec<String> = Vec::new();
            for f in fs {
                check_col(def, &f)?;
                let title = def.columns.iter().find(|x| x.title == f || x.key == f).map(|x| x.title.clone()).unwrap_or(f);
                if !out.contains(&title) {
                    out.push(title);
                }
            }
            Some(out)
        }
        None => None,
    };
    let keyword = keyword.map(|k| k.trim().to_string());
    if keyword.as_deref().is_some_and(|k| !k.is_empty()) && !def.searchable {
        return Err(AppError::bad(format!("数据集「{}」不支持关键字搜索", def.name)));
    }
    Ok(View { filters, sort, fields, keyword })
}

struct ViewPreview {
    matched: i64,
    total: i64,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// 命中行数、总行数与前 6 行；有筛选 / 搜索却一行都没命中时报错，让模型去核对筛选值的写法
fn view_preview(conn: &Connection, def: &DatasetDef, v: &View) -> AppResult<ViewPreview> {
    let spec = QuerySpec { filters: v.filters.clone().unwrap_or_default(), keyword: v.keyword.clone().filter(|k| !k.is_empty()), sort: v.sort.clone().unwrap_or_default(), rule: None };
    let shown: Vec<&dataset::ColumnDef> = match v.fields.as_ref().filter(|f| !f.is_empty()) {
        Some(fs) => fs.iter().filter_map(|t| def.columns.iter().find(|x| &x.title == t)).collect(),
        None => def.columns.iter().take(8).collect(),
    };
    crate::usage::hit(&def.id, crate::usage::AI);
    let (matched, rows) = dataset::preview_rows(conn, def, &spec, &shown, 6)?;
    let total = dataset::total_rows(conn, def)?;
    if matched == 0 && (!spec.filters.is_empty() || spec.keyword.is_some()) {
        return Err(AppError::bad("按这些条件一行都没有命中。请用 column_values 确认筛选值的写法（要与数据中完全一致），或放宽条件后重试"));
    }
    Ok(ViewPreview { matched, total, header: shown.iter().map(|c| c.title.clone()).collect(), rows: rows.iter().map(|r| r.iter().map(cell_text).collect()).collect() })
}

/// 视图的中文说明，如 ["筛选 店铺=甲", "排序 金额降序"]
fn view_parts(def: &DatasetDef, v: &View) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(fs) = v.filters.as_ref().filter(|f| !f.is_empty()) {
        parts.push(format!("筛选 {}", filters_desc(fs)));
    } else if v.filters.is_some() {
        parts.push("清除筛选".to_string());
    }
    if let Some(ss) = v.sort.as_ref().filter(|s| !s.is_empty()) {
        let label = |c: &str| def.columns.iter().find(|x| x.key == c).map(|x| x.title.clone()).unwrap_or_else(|| c.to_string());
        parts.push(format!("排序 {}", ss.iter().map(|s| format!("{}{}", label(&s.column), if s.desc { "降序" } else { "升序" })).collect::<Vec<_>>().join("、")));
    } else if v.sort.is_some() {
        parts.push("取消排序".to_string());
    }
    if let Some(fs) = v.fields.as_ref().filter(|f| !f.is_empty()) {
        parts.push(format!("显示 {} 列", fs.len()));
    } else if v.fields.is_some() {
        parts.push("显示全部列".to_string());
    }
    if let Some(k) = v.keyword.as_deref() {
        parts.push(if k.is_empty() { "清除搜索".to_string() } else { format!("搜索「{k}」") });
    }
    parts
}

/// 把给了的视图项写进建议
fn put_view(action: &mut serde_json::Map<String, Json>, v: &View) {
    if let Some(f) = &v.filters {
        action.insert("filters".into(), json!(f));
    }
    if let Some(s) = &v.sort {
        action.insert("sort".into(), json!(s));
    }
    if let Some(f) = &v.fields {
        action.insert("fields".into(), json!(f));
    }
    if let Some(k) = &v.keyword {
        action.insert("keyword".into(), json!(k));
    }
}

const NUM_FMTS: [&str; 6] = ["default", "int", "dec2", "percent", "wan", "plain"];
/// 一列最多几条条件格式规则
const MAX_RULES: usize = 8;

#[derive(Deserialize)]
struct ColumnFormatArg {
    table: String,
    dataset: String,
    column: String,
    #[serde(default, rename = "numFmt")]
    num_fmt: Option<String>,
    #[serde(default)]
    rules: Option<Vec<RuleArg>>,
}

#[derive(Deserialize)]
struct RuleArg {
    op: String,
    #[serde(default)]
    value: Option<Json>,
    #[serde(default)]
    value2: Option<f64>,
    #[serde(default)]
    style: RuleStyleArg,
}

#[derive(Deserialize, Default)]
struct RuleStyleArg {
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    bg: Option<String>,
    #[serde(default)]
    bold: Option<bool>,
}

/// #rgb / #rgba / #rrggbb / #rrggbbaa（颜色最终交给前端画布，只放行十六进制）
fn is_hex_color(s: &str) -> bool {
    s.strip_prefix('#').is_some_and(|h| matches!(h.len(), 3 | 4 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit()))
}

fn json_f64(v: &Json) -> Option<f64> {
    match v {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().parse().ok().filter(|x: &f64| x.is_finite()),
        _ => None,
    }
}

fn json_text(v: &Json) -> Option<String> {
    match v {
        Json::String(s) if !s.is_empty() => Some(s.clone()),
        Json::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 校验一条条件格式规则并整理成界面使用的写法（列类型要与运算匹配、颜色只能是十六进制）
fn check_rule(col: &dataset::ColumnDef, n: usize, r: &RuleArg) -> AppResult<Json> {
    let numeric = crate::kinds::is_numeric(col.kind);
    let bad = |m: String| AppError::bad(format!("第 {n} 条规则：{m}"));
    let mut out = serde_json::Map::new();
    out.insert("op".into(), json!(r.op));
    match r.op.as_str() {
        ">" | ">=" | "<" | "<=" => {
            if !numeric {
                return Err(bad(format!("{} 只能用于金额 / 数值列，「{}」是{}列", r.op, col.title, kind_label(col.kind))));
            }
            let v = r.value.as_ref().and_then(json_f64).ok_or_else(|| bad(format!("{} 需要数字（value）", r.op)))?;
            out.insert("value".into(), json!(v));
        }
        "between" => {
            if !numeric {
                return Err(bad(format!("between 只能用于金额 / 数值列，「{}」是{}列", col.title, kind_label(col.kind))));
            }
            let lo = r.value.as_ref().and_then(json_f64).ok_or_else(|| bad("between 需要下限（value）".into()))?;
            let hi = r.value2.filter(|x| x.is_finite()).ok_or_else(|| bad("between 需要上限（value2）".into()))?;
            if lo > hi {
                return Err(bad("下限不能大于上限".into()));
            }
            out.insert("value".into(), json!(lo));
            out.insert("value2".into(), json!(hi));
        }
        "negative" => {
            if !numeric {
                return Err(bad(format!("negative 只能用于金额 / 数值列，「{}」是{}列", col.title, kind_label(col.kind))));
            }
        }
        "=" | "!=" => {
            if numeric {
                let v = r.value.as_ref().and_then(json_f64).ok_or_else(|| bad(format!("{} 需要数字（value）", r.op)))?;
                out.insert("value".into(), json!(v));
            } else {
                let v = r.value.as_ref().and_then(json_text).ok_or_else(|| bad(format!("{} 需要非空的比较值（value）", r.op)))?;
                out.insert("value".into(), json!(v));
            }
        }
        "contains" | "equals" => {
            if numeric {
                return Err(bad(format!("{} 只能用于文本 / 编号 / 日期列，数值列请用 = / != / between", r.op)));
            }
            let v = r.value.as_ref().and_then(json_text).ok_or_else(|| bad(format!("{} 需要非空的文字（value）", r.op)))?;
            out.insert("value".into(), json!(v));
        }
        "empty" => {}
        other => return Err(bad(format!("没有运算 {other}"))),
    }
    let mut style = serde_json::Map::new();
    for (name, c) in [("color", &r.style.color), ("bg", &r.style.bg)] {
        if let Some(c) = c.as_deref().filter(|c| !c.is_empty()) {
            if !is_hex_color(c) {
                return Err(bad(format!("颜色「{c}」不是十六进制（如 #C00000）")));
            }
            style.insert(name.into(), json!(c));
        }
    }
    if r.style.bold == Some(true) {
        style.insert("bold".into(), json!(true));
    }
    if style.is_empty() {
        return Err(bad("style 至少给一项（color / bg / bold）".into()));
    }
    out.insert("style".into(), Json::Object(style));
    Ok(Json::Object(out))
}

/// 规则的中文说明（给用户看）
pub fn rule_label(col: &str, rule: &Json) -> String {
    let v = |k: &str| {
        rule.get(k)
            .map(|x| match x {
                Json::String(s) => s.clone(),
                // 150.0 显示成 150
                Json::Number(n) => n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 1e15).map(|f| format!("{f:.0}")).unwrap_or_else(|| n.to_string()),
                other => other.to_string(),
            })
            .unwrap_or_default()
    };
    let cond = match rule["op"].as_str().unwrap_or("") {
        "between" => format!("{col} 在 {} ~ {}", v("value"), v("value2")),
        "negative" => format!("{col} 为负数"),
        "empty" => format!("{col} 为空"),
        "contains" => format!("{col} 含「{}」", v("value")),
        "equals" => format!("{col} 等于「{}」", v("value")),
        op => format!("{col} {op} {}", v("value")),
    };
    let s = &rule["style"];
    let mut look = Vec::new();
    if let Some(c) = s["color"].as_str() {
        look.push(format!("文字 {c}"));
    }
    if let Some(c) = s["bg"].as_str() {
        look.push(format!("底色 {c}"));
    }
    if s["bold"].as_bool() == Some(true) {
        look.push("加粗".to_string());
    }
    format!("{cond} → {}", look.join("、"))
}

/// 一条规则的 SQL 条件与参数（colx 是列表达式；与界面 ruleMatches 的语义一致：比较只针对数字，文字比较不区分类型）
fn rule_sql(colx: &str, rule: &Json, numeric: bool) -> (String, Vec<duckdb::types::Value>) {
    use duckdb::types::Value;
    let text = format!("CAST(({colx}) AS VARCHAR)");
    let num = |k: &str| Value::Double(rule[k].as_f64().unwrap_or(0.0));
    let str_ = |k: &str| Value::Text(rule[k].as_str().map(String::from).unwrap_or_else(|| rule[k].to_string()));
    match rule["op"].as_str().unwrap_or("") {
        ">" => (format!("({colx}) > ?"), vec![num("value")]),
        ">=" => (format!("({colx}) >= ?"), vec![num("value")]),
        "<" => (format!("({colx}) < ?"), vec![num("value")]),
        "<=" => (format!("({colx}) <= ?"), vec![num("value")]),
        "between" => (format!("({colx}) BETWEEN ? AND ?"), vec![num("value"), num("value2")]),
        "negative" => (format!("({colx}) < 0"), vec![]),
        "=" if numeric => (format!("({colx}) = ?"), vec![num("value")]),
        "=" => (format!("({colx}) IS NOT NULL AND {text} = ?"), vec![str_("value")]),
        "!=" if numeric => (format!("({colx}) IS NOT NULL AND ({colx}) <> ?"), vec![num("value")]),
        "!=" => (format!("({colx}) IS NOT NULL AND {text} <> ?"), vec![str_("value")]),
        "contains" => (format!("contains(lower({text}), lower(?))"), vec![str_("value")]),
        "equals" => (format!("({colx}) IS NOT NULL AND {text} = ?"), vec![str_("value")]),
        _ => (format!("({colx}) IS NULL OR {text} = ''"), vec![]),
    }
}

/// 数据表列样式建议：数字格式 + 条件格式。服务端核对列类型与规则，并统计每条规则在整个数据集里命中多少行
fn set_column_format(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: ColumnFormatArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    let table = a.table.trim().to_string();
    if table.is_empty() {
        return Err(AppError::bad("需要数据表名称（table）"));
    }
    check_col(&def, &a.column)?;
    let col = def.columns.iter().find(|x| x.title == a.column || x.key == a.column).expect("checked");
    if a.num_fmt.is_none() && a.rules.is_none() {
        return Err(AppError::bad("没有要设置的内容：numFmt / rules 至少给一项"));
    }
    let numeric = crate::kinds::is_numeric(col.kind);
    if let Some(f) = a.num_fmt.as_deref() {
        if !NUM_FMTS.contains(&f) {
            return Err(AppError::bad(format!("没有数字格式「{f}」，可用：{}", NUM_FMTS.join(" / "))));
        }
        if f != "default" && !numeric {
            return Err(AppError::bad(format!("数字格式只能用于金额 / 数值列，「{}」是{}列", col.title, kind_label(col.kind))));
        }
    }
    let rules: Option<Vec<Json>> = match &a.rules {
        Some(rs) => {
            if rs.len() > MAX_RULES {
                return Err(AppError::bad(format!("一列最多 {MAX_RULES} 条规则（当前 {} 条）", rs.len())));
            }
            Some(rs.iter().enumerate().map(|(i, r)| check_rule(col, i + 1, r)).collect::<AppResult<Vec<_>>>()?)
        }
        None => None,
    };

    // 每条规则命中的行数：一次扫描
    crate::usage::hit(&def.id, crate::usage::AI);
    let total = dataset::total_rows(conn, &def)?;
    let mut hits: Vec<i64> = vec![];
    if let Some(rs) = rules.as_ref().filter(|r| !r.is_empty()) {
        let colx = dataset::column_sql(col);
        let mut conds = Vec::new();
        let mut params = Vec::new();
        for r in rs {
            let (c, p) = rule_sql(&colx, r, numeric);
            conds.push(format!("count(*) FILTER (WHERE {c})"));
            params.extend(p);
        }
        let sql = format!("SELECT {} FROM {}", conds.join(", "), def.table);
        let n = rs.len();
        hits = conn.query_row(&sql, duckdb::params_from_iter(params.iter()), |r| (0..n).map(|i| r.get::<_, i64>(i)).collect::<Result<Vec<_>, _>>())?;
    }

    let mut parts = Vec::new();
    match a.num_fmt.as_deref() {
        Some("default") => parts.push("恢复默认数字格式".to_string()),
        Some(f) => parts.push(format!("数字格式 {}", match f { "int" => "整数", "dec2" => "两位小数", "percent" => "百分比", "wan" => "万元", _ => "不分组" })),
        None => {}
    }
    match rules.as_ref() {
        Some(rs) if rs.is_empty() => parts.push("清除条件格式".to_string()),
        Some(rs) => parts.push(format!("{} 条条件格式", rs.len())),
        None => {}
    }
    let title = format!("{table}：{} {}", col.title, parts.join("；"));

    let pct = |n: i64| if total > 0 { format!("{:.2}%", n as f64 * 100.0 / total as f64) } else { "-".to_string() };
    let header = vec!["规则".to_string(), "命中行数".to_string(), "占整个数据集".to_string()];
    let preview_rows: Vec<Vec<String>> = rules.as_ref().map(|rs| rs.iter().zip(&hits).map(|(r, n)| vec![rule_label(&col.title, r), group_digits(*n), pct(*n)]).collect()).unwrap_or_default();
    let mut note = format!("数据集共 {} 行", group_digits(total));
    if !preview_rows.is_empty() {
        note.push_str("；命中行数按整个数据集统计（不含表上的筛选），各规则可能重叠，显示时按顺序取第一条命中的");
    }

    let mut action = serde_json::Map::new();
    action.insert("kind".into(), json!("column_format"));
    action.insert("title".into(), json!(title));
    action.insert("table".into(), json!(table));
    action.insert("datasetId".into(), json!(def.id));
    action.insert("datasetName".into(), json!(def.name));
    action.insert("column".into(), json!(col.title));
    if let Some(f) = &a.num_fmt {
        action.insert("numFmt".into(), json!(f));
    }
    if let Some(rs) = &rules {
        action.insert("rules".into(), json!(rs));
    }
    action.insert("preview".into(), preview_json(header.clone(), preview_rows.clone(), note.clone()));

    let zero: Vec<String> = rules.iter().flatten().zip(&hits).filter(|(_, n)| **n == 0).map(|(r, _)| rule_label(&col.title, r)).collect();
    let mut content = format!("已生成列样式建议「{title}」，{note}，等待用户确认应用。");
    if !preview_rows.is_empty() {
        content.push_str(&format!("\n{}", render_table(&header, &preview_rows, "")));
    }
    if !zero.is_empty() {
        content.push_str(&format!("\n注意：这些规则在整个数据集里没有命中任何行，请确认阈值：{}", zero.join("；")));
    }
    let mut o = Outcome::ok(content, format!("建议设置数据表「{table}」{}列的样式：{}", col.title, parts.join("；")));
    o.action = Some(Json::Object(action));
    Ok(o)
}

const MAX_DATASET_NAME: usize = 60;

#[derive(Deserialize)]
struct SaveDatasetArg {
    sql: String,
    name: String,
    #[serde(default)]
    purpose: Option<String>,
}

/// 建议把一条 SQL 的结果另存为数据集：服务端先执行检查（语法、只读、列名合规、总行数、前几行），用户确认后前端才真正创建
fn save_query_as_dataset(conn: &Connection, catalog: &[DatasetDef], args: &str, scope: Option<&std::collections::HashSet<String>>) -> AppResult<Outcome> {
    let a: SaveDatasetArg = parse(args)?;
    let name = a.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::bad("请给新数据集起个名字（name）"));
    }
    if name.chars().count() > MAX_DATASET_NAME {
        return Err(AppError::bad(format!("数据集名称最长 {MAX_DATASET_NAME} 个字（当前 {} 个）", name.chars().count())));
    }
    if name.contains('"') {
        return Err(AppError::bad("数据集名称不能包含英文双引号"));
    }
    if catalog.iter().any(|d| d.name == name) {
        return Err(AppError::bad(format!("已有名为「{name}」的数据集，请换一个名字")));
    }
    let sql = crate::sqlq::guard::trim_sql(&a.sql);
    let c = crate::sqlq::check_for_save(conn, &sql, 6, scope)?;
    if c.total == 0 {
        return Err(AppError::bad("这条 SQL 的结果是 0 行，不能另存为空数据集：请检查条件，或先用 run_sql 看一下结果"));
    }
    let header: Vec<String> = c.columns.iter().map(|x| x.name.clone()).collect();
    let rows: Vec<Vec<String>> = c.rows.iter().map(|r| r.iter().map(cell_text).collect()).collect();
    let used = if c.datasets.is_empty() { String::new() } else { format!("；用到数据集：{}", c.datasets.join("、")) };
    let note = format!("共 {} 行 × {} 列{used}；保存后不会自动刷新（需要时在 SQL 查询里手动重算）", group_digits(c.total), c.columns.len());
    let title = format!("另存为数据集「{name}」（{} 行 × {} 列）", group_digits(c.total), c.columns.len());

    let mut action = serde_json::Map::new();
    action.insert("kind".into(), json!("dataset_save"));
    action.insert("title".into(), json!(title));
    action.insert("name".into(), json!(name));
    action.insert("sql".into(), json!(sql));
    if let Some(p) = a.purpose.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        action.insert("purpose".into(), json!(p));
    }
    action.insert("rows".into(), json!(c.total));
    action.insert("preview".into(), preview_json(header.clone(), rows.clone(), note.clone()));

    let mut o = Outcome::ok(
        format!("已生成另存为数据集的建议「{title}」，{note}，等待用户确认（确认后才会创建）。前几行预览：\n{}", render_table(&header, &rows, "")),
        format!("建议把查询结果另存为数据集「{name}」"),
    );
    o.action = Some(Json::Object(action));
    Ok(o)
}

// ── 数据概况、数据清洗、窗口计算 ──

#[derive(Deserialize)]
struct ProfileArg {
    dataset: String,
    #[serde(default)]
    filters: Vec<Filter>,
    #[serde(default)]
    columns: Option<Vec<String>>,
}

fn pct(n: i64, total: i64) -> String {
    if total == 0 { "0".into() } else { format!("{}（{:.1}%）", group_digits(n), n as f64 * 100.0 / total as f64) }
}

fn profile_dataset(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: ProfileArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    let filters = normalize_filters(&def, a.filters)?;
    let filtered = !filters.is_empty();
    let req = crate::profile::ProfileRequest { spec: QuerySpec { filters, ..Default::default() }, columns: a.columns };
    let p = crate::profile::overview(conn, &def.id, &req)?;
    let header: Vec<String> = ["列", "类型", "空值", "不同值", "最小", "最大", "其他"].map(String::from).to_vec();
    let mut rows = Vec::new();
    let mut issues = Vec::new();
    for c in &p.columns {
        let mut other = Vec::new();
        if let Some(s) = c.sum {
            other.push(format!("合计 {}，平均 {}，中位数 {}", num(Some(s), ""), num(c.mean, ""), num(c.median, "")));
        }
        if let Some(n) = c.zeros.filter(|n| *n > 0) {
            other.push(format!("零 {}", group_digits(n)));
        }
        if let Some(n) = c.negatives.filter(|n| *n > 0) {
            other.push(format!("负数 {}", group_digits(n)));
        }
        if let Some(n) = c.trues {
            other.push(format!("是 {}", group_digits(n)));
        }
        if let Some(n) = c.blanks.filter(|n| *n > 0) {
            other.push(format!("只有空白 {}", group_digits(n)));
            issues.push(format!("「{}」有 {} 个只有空白的格子（可 trim 变成空值）", c.title, group_digits(n)));
        }
        if let Some(n) = c.invalid.filter(|n| *n > 0) {
            other.push(format!("认不出的日期 {}", group_digits(n)));
            issues.push(format!("「{}」有 {} 个认不出的日期写法", c.title, group_digits(n)));
        }
        if c.nulls > 0 && c.nulls * 5 >= p.rows {
            issues.push(format!("「{}」空值较多：{}", c.title, pct(c.nulls, p.rows)));
        }
        if c.non_null > 0 && c.distinct == 1 && p.rows > 1 {
            issues.push(format!("「{}」所有非空值都相同", c.title));
        }
        let distinct = if c.distinct_exact { group_digits(c.distinct) } else { format!("约 {}", group_digits(c.distinct)) };
        rows.push(vec![c.title.clone(), kind_label(c.kind).to_string(), pct(c.nulls, p.rows), distinct, cell_text(&c.min), cell_text(&c.max), other.join("；")]);
    }
    let scope = if filtered { "（按筛选）" } else { "" };
    let mut content = format!("数据集「{}」{scope}共 {} 行，{} 列：
{}", def.name, group_digits(p.rows), p.columns.len(), render_table(&header, &rows, ""));
    content.push_str(&if issues.is_empty() { "

没有发现明显的问题（重复行要用 propose_clean 的 dedupe 预览才知道）".to_string() } else { format!("

可能的问题：
- {}", issues.join("
- ")) });
    Ok(Outcome::ok(content, format!("查看「{}」的数据概况（{} 行 × {} 列）", def.name, group_digits(p.rows), p.columns.len())))
}

/// 清洗建议的参数：dataset + 与清洗接口相同的 op 字段；delete_rows 的条件写在 filters 里
fn clean_op(def: &DatasetDef, args: &str) -> AppResult<crate::clean::CleanOp> {
    let mut v: Json = parse(args)?;
    let obj = v.as_object_mut().ok_or_else(|| AppError::bad("参数应为对象"))?;
    obj.remove("dataset");
    let filters: Vec<Filter> = match obj.remove("filters") {
        Some(f) if !f.is_null() => serde_json::from_value(f).map_err(|e| AppError::bad(format!("filters 格式不正确：{e}")))?,
        _ => vec![],
    };
    if obj.get("op").and_then(Json::as_str) == Some("delete_rows") {
        if filters.is_empty() {
            return Err(AppError::bad("delete_rows 要给 filters（删除哪些行）"));
        }
        let filters: Vec<Filter> = normalize_filters(def, filters)?.into_iter().map(|f| filter_with_key(def, f)).collect();
        obj.insert("spec".into(), serde_json::to_value(QuerySpec { filters, ..Default::default() })?);
    }
    serde_json::from_value(v).map_err(|e| AppError::bad(format!("清洗参数不正确：{e}")))
}

fn no_restricted(what: &str) -> AppResult<()> {
    if crate::scope::restricted_now() {
        return Err(AppError::bad(format!("当前用户的数据范围受限，不能{what}")));
    }
    Ok(())
}

fn propose_clean(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    #[derive(Deserialize)]
    struct Head {
        dataset: String,
    }
    let head: Head = parse(args)?;
    let def = resolve(catalog, &head.dataset)?;
    no_restricted("清洗数据")?;
    let op = clean_op(&def, args)?;
    let p = crate::clean::preview(conn, &def.id, &op)?;
    if p.affected == 0 {
        return Ok(Outcome::ok(format!("「{}」：{}——没有需要处理的行，不用清洗", def.name, p.summary), format!("「{}」没有需要清洗的行", def.name)));
    }
    let verb = if p.deletes { "删除" } else { "修改" };
    let mut header = vec![String::new()];
    header.extend(p.columns.iter().cloned());
    let mut rows = Vec::new();
    for s in &p.samples {
        let line = |label: &str, vals: &[Json]| std::iter::once(label.to_string()).chain(vals.iter().map(cell_text)).collect::<Vec<_>>();
        match &s.after {
            Some(after) => {
                rows.push(line("改前", &s.before));
                rows.push(line("改后", after));
            }
            None => rows.push(line("删除", &s.before)),
        }
    }
    let note = format!("{} 行中将{verb} {} 行；执行后可以撤销（只能撤销最近一次）", group_digits(p.rows), group_digits(p.affected));
    let title = format!("清洗「{}」：{}", def.name, p.summary);
    let action = json!({
        "kind": "clean",
        "title": title,
        "datasetId": def.id,
        "datasetName": def.name,
        "op": op,
        "summary": p.summary,
        "affected": p.affected,
        "deletes": p.deletes,
        "preview": preview_json(header.clone(), rows.clone(), note.clone()),
    });
    let mut o = Outcome::ok(
        format!("已生成数据清洗建议「{title}」，{note}，等待用户确认（确认后才会修改数据）。样例：
{}", render_table(&header, &rows.iter().take(10).cloned().collect::<Vec<_>>(), "")),
        format!("建议{verb}「{}」的 {} 行", def.name, group_digits(p.affected)),
    );
    o.action = Some(action);
    Ok(o)
}

#[derive(Deserialize)]
struct WindowArg {
    dataset: String,
    name: String,
    #[serde(flatten)]
    spec: crate::window::WindowSpec,
}

fn create_window_dataset(conn: &Connection, catalog: &[DatasetDef], args: &str) -> AppResult<Outcome> {
    let a: WindowArg = parse(args)?;
    let def = resolve(catalog, &a.dataset)?;
    no_restricted("生成数据集")?;
    let name = a.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::bad("请给新数据集起个名字（name）"));
    }
    if name.chars().count() > MAX_DATASET_NAME {
        return Err(AppError::bad(format!("数据集名称最长 {MAX_DATASET_NAME} 个字（当前 {} 个）", name.chars().count())));
    }
    if catalog.iter().any(|d| d.name == name) {
        return Err(AppError::bad(format!("已有名为「{name}」的数据集，请换一个名字")));
    }
    let p = crate::window::preview(conn, &def.id, &a.spec)?;
    // 预览只列分组、顺序和新算出的列，免得表太宽
    let title_of = |k: &str| def.columns.iter().find(|c| c.key == k || c.title == k).map(|c| c.title.clone()).unwrap_or_else(|| k.to_string());
    let mut wanted: Vec<String> = a.spec.partition_by.iter().map(|k| title_of(k)).chain(a.spec.order_by.iter().map(|o| title_of(&o.column))).collect();
    wanted.extend(a.spec.calcs.iter().map(|c| c.name.trim().to_string()));
    wanted.dedup();
    let idx: Vec<usize> = wanted.iter().filter_map(|w| p.result.columns.iter().position(|c| &c.name == w)).collect();
    let header: Vec<String> = idx.iter().map(|&i| p.result.columns[i].name.clone()).collect();
    let rows: Vec<Vec<String>> = p.result.rows.iter().take(12).map(|r| idx.iter().map(|&i| r.get(i).map(cell_text).unwrap_or_default()).collect()).collect();
    let calcs = a.spec.calcs.iter().map(|c| c.name.trim()).collect::<Vec<_>>().join("、");
    let note = format!("在「{}」上算 {calcs}；保存为 SQL 查询并另存为数据集，「{}」更新后自动重算", def.name, def.name);
    let title = format!("窗口计算生成数据集「{name}」");
    let mut spec: Json = parse(args)?;
    if let Some(o) = spec.as_object_mut() {
        o.remove("dataset");
        o.remove("name");
    }
    let action = json!({
        "kind": "window",
        "title": title,
        "datasetId": def.id,
        "datasetName": def.name,
        "name": name,
        "spec": spec,
        "sql": p.sql,
        "preview": preview_json(header.clone(), rows.clone(), note.clone()),
    });
    let mut o = Outcome::ok(
        format!("已生成窗口计算建议「{title}」，{note}，等待用户确认（确认后才会创建）。前几行预览：
{}", render_table(&header, &rows, "")),
        format!("建议用窗口计算生成数据集「{name}」"),
    );
    o.action = Some(action);
    Ok(o)
}

#[derive(Deserialize)]
struct SqlArg {
    sql: String,
    #[serde(default)]
    purpose: Option<String>,
}

fn run_sql(conn: &Connection, args: &str, scope: Option<&std::collections::HashSet<String>>) -> AppResult<Outcome> {
    let a: SqlArg = parse(args)?;
    let r = crate::sqlq::run_scoped(conn, &a.sql, 50, scope)?;
    let header: Vec<String> = r.columns.iter().map(|c| c.name.clone()).collect();
    let rows: Vec<Vec<String>> = r
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .zip(&r.columns)
                .map(|(v, c)| match v {
                    Json::Null => "-".into(),
                    Json::Number(n) => num(n.as_f64(), if c.kind == "money" { "money" } else { "" }),
                    Json::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .collect();
    let note = if r.truncated { "结果超过 50 行，只列出前 50 行；请加 GROUP BY 汇总或 LIMIT。".to_string() } else { String::new() };
    let summary = format!("SQL 查询{}：{}（{} 行{}）", a.purpose.as_deref().map(|p| format!("「{p}」")).unwrap_or_default(), r.datasets.join("、"), r.rows.len(), if r.truncated { "，已截断" } else { "" });
    let mut o = Outcome::ok(format!("SQL：{}\n{}", a.sql.trim(), render_table(&header, &rows, &note)), summary);
    o.content = truncate(o.content);
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_valid() {
        let d = definitions();
        let names: Vec<&str> = d.as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 18);
        assert!(names.contains(&"profile_dataset") && names.contains(&"propose_clean") && names.contains(&"create_window_dataset"));
        assert!(names.contains(&"set_table_view") && names.contains(&"set_column_format") && names.contains(&"create_data_table") && names.contains(&"save_query_as_dataset"));
        for t in d.as_array().unwrap() {
            assert_eq!(t["function"]["parameters"]["type"], "object");
        }
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(10_000_000), "10,000,000");
        assert_eq!(group_digits(-1_234_567), "-1,234,567");
        assert_eq!(end_of_day("2026-09-30"), "2026-09-30 23:59:59");
        assert_eq!(num(Some(0.1234), "percent"), "12.3%");
        assert_eq!(num(Some(12.0), "money"), "12");
        assert_eq!(num(Some(12.345), "money"), "12.35");
    }

    fn col(kind: &'static str) -> dataset::ColumnDef {
        dataset::ColumnDef { key: "k".into(), title: "金额".into(), kind, width: 100, expr: None, computed: false, formula: None, origin: None }
    }

    fn rule(v: Json) -> RuleArg {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn hex_colors() {
        for ok in ["#fff", "#FFFF", "#C00000", "#c0000080"] {
            assert!(is_hex_color(ok), "{ok}");
        }
        for bad in ["", "red", "#12", "#12345", "#GGGGGG", "C00000", "#c00000;", "rgb(1,2,3)", "url(x)"] {
            assert!(!is_hex_color(bad), "{bad}");
        }
    }

    #[test]
    fn rule_checks() {
        let money = col("money");
        let text = col("text");
        let red = json!({ "color": "#C00000" });
        let ok = check_rule(&money, 1, &rule(json!({ "op": ">", "value": "100000", "style": red }))).unwrap();
        assert_eq!(ok["value"], 100000.0, "字符串数字转成数字");
        assert_eq!(ok["style"]["color"], "#C00000");
        assert!(check_rule(&money, 1, &rule(json!({ "op": "between", "value": 1, "value2": 9, "style": { "bold": true } }))).is_ok());
        assert!(check_rule(&money, 1, &rule(json!({ "op": "negative", "style": red }))).is_ok());
        assert!(check_rule(&text, 1, &rule(json!({ "op": "contains", "value": "退款", "style": red }))).is_ok());
        assert!(check_rule(&text, 1, &rule(json!({ "op": "empty", "style": red }))).is_ok());
        let err = |c: &dataset::ColumnDef, v: Json| check_rule(c, 2, &rule(v)).unwrap_err().message();
        let e = err(&text, json!({ "op": ">", "value": 1, "style": red }));
        assert!(e.contains("第 2 条规则") && e.contains("金额 / 数值列"), "{e}");
        assert!(err(&money, json!({ "op": "contains", "value": "x", "style": red })).contains("文本"));
        assert!(err(&money, json!({ "op": ">", "style": red })).contains("需要数字"));
        assert!(err(&money, json!({ "op": "between", "value": 9, "value2": 1, "style": red })).contains("下限不能大于上限"));
        assert!(err(&money, json!({ "op": ">", "value": 1, "style": { "color": "red" } })).contains("十六进制"));
        assert!(err(&money, json!({ "op": ">", "value": 1, "style": {} })).contains("至少给一项"));
        assert!(err(&money, json!({ "op": "like", "value": 1, "style": red })).contains("没有运算"));
        assert!(err(&text, json!({ "op": "equals", "value": "", "style": red })).contains("非空"));
    }

    #[test]
    fn rule_labels_and_sql() {
        let r = check_rule(&col("money"), 1, &rule(json!({ "op": ">", "value": 100000, "style": { "color": "#C00000", "bg": "#FDE8E8", "bold": true } }))).unwrap();
        assert_eq!(rule_label("金额", &r), "金额 > 100000 → 文字 #C00000、底色 #FDE8E8、加粗");
        let (sql, params) = rule_sql("t.a", &r, true);
        assert_eq!((sql.as_str(), params.len()), ("(t.a) > ?", 1));
        let b = check_rule(&col("money"), 1, &rule(json!({ "op": "between", "value": 1, "value2": 9, "style": { "bold": true } }))).unwrap();
        assert_eq!(rule_sql("x", &b, true).1.len(), 2);
        let c = check_rule(&col("text"), 1, &rule(json!({ "op": "contains", "value": "退款", "style": { "bold": true } }))).unwrap();
        assert!(rule_sql("x", &c, false).0.contains("contains(lower("));
    }
}
