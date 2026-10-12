#!/usr/bin/env python3
"""生成 docs/api.md：REST 接口参考。

数据来源（都在后端源码里，所以文档不会和代码脱节）：
  - 每个处理函数上方形如 `/// POST /api/ledger/xxx  { 请求体 }  说明` 的文档注释；
  - server/src/auth.rs 的 ROUTES（权限规则与审计名称）和 RESTRICTED_OK（数据范围受限用户可用的接口）。

    scripts/gen-api-docs.py            # 重新生成 docs/api.md
    scripts/gen-api-docs.py --check    # 只检查 docs/api.md 是否是最新的（CI 用）

新增 / 修改接口后：在处理函数上写好 `/// METHOD /api/路径  说明`，再运行本脚本。
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "server" / "src"
OUT = ROOT / "docs" / "api.md"

ROUTE_DOC = re.compile(r"^\s*///\s*(GET|POST|PUT|DELETE)\s+(/api/\S+)\s*(.*)$")
METHOD_ORDER = {"GET": 0, "POST": 1, "PUT": 2, "DELETE": 3}

GROUPS = [
    ("health", "存活检查与运维"),
    ("auth", "登录与账号"),
    ("accounts", "登录与账号"),
    ("me", "登录与账号"),
    ("audit", "操作审计"),
    ("workbooks", "工作簿、版本与共享"),
    ("folders", "工作簿、版本与共享"),
    ("datasets", "数据集（导入、取数、透视、公式下推）"),
    ("writebacks", "数据集（导入、取数、透视、公式下推）"),
    ("sql", "SQL 工作台"),
    ("recons", "逐笔对账"),
    ("connectors", "数据连接"),
    ("reports", "定时推送"),
    ("report-channels", "定时推送"),
    ("templates", "模板"),
    ("ai", "AI 助手"),
    ("mcp", "外部 AI 接入（MCP）"),
    ("comments", "评论、协作与通知"),
    ("collab", "评论、协作与通知"),
    ("notifications", "评论、协作与通知"),
    ("approvals", "审批"),
    ("users", "审批"),
    ("data-scope", "数据范围（行级权限）"),
    ("close", "月结与资金台账（电商场景）"),
    ("imports", "月结与资金台账（电商场景）"),
    ("shops", "月结与资金台账（电商场景）"),
    ("details", "月结与资金台账（电商场景）"),
    ("balances", "月结与资金台账（电商场景）"),
    ("bank", "月结与资金台账（电商场景）"),
    ("withdrawals", "月结与资金台账（电商场景）"),
    ("analysis", "月结与资金台账（电商场景）"),
    ("coverage", "月结与资金台账（电商场景）"),
    ("pivot", "月结与资金台账（电商场景）"),
    ("periods", "月结与资金台账（电商场景）"),
    ("data", "月结与资金台账（电商场景）"),
    ("entity-aliases", "月结与资金台账（电商场景）"),
]
GROUP_ORDER = []
for _, title in GROUPS:
    if title not in GROUP_ORDER:
        GROUP_ORDER.append(title)
GROUP_ORDER.append("其他")
SEGMENT_GROUP = dict(GROUPS)

NEED = {"View": "查看", "Edit": "编辑", "Manage": "管理（所有者或管理员）"}


def doc_routes():
    found = {}
    for f in sorted(SRC.rglob("*.rs")):
        lines = f.read_text(encoding="utf-8").split("\n")
        for i, line in enumerate(lines):
            m = ROUTE_DOC.match(line)
            if not m:
                continue
            method, full, rest = m.groups()
            path, _, query = full.partition("?")
            desc = [rest.strip()] if rest.strip() else []
            if query:
                desc.append(f"查询参数：`{query}`")
            for nxt in lines[i + 1:]:
                if not nxt.strip().startswith("///") or ROUTE_DOC.match(nxt):
                    break
                text = nxt.strip()[3:].strip()
                if text:
                    desc.append(text)
            found[(method, path)] = " ".join(desc)
    return found


def auth_tables():
    text = (SRC / "auth.rs").read_text(encoding="utf-8")
    routes_block = text[text.index("const ROUTES"):]
    routes_block = routes_block[: routes_block.index("\n];")]
    rules = {}
    for m in re.finditer(r'\("(GET|POST|PUT|DELETE)", "([^"]+)", ([A-Za-z]+)(?:\(([^)]*)\))?, "([^"]*)"\)', routes_block):
        method, path, rule, arg, label = m.groups()
        rules[(method, "/api" + path)] = (rule, arg or "", label)
    ok_block = text[text.index("const RESTRICTED_OK"):]
    ok_block = ok_block[: ok_block.index("\n];")]
    restricted = {(m, "/api" + p) for m, p in re.findall(r'\("(GET|POST|PUT|DELETE)", "([^"]+)"\)', ok_block)}
    return rules, restricted


def permission(method, path, rules):
    if path in ("/api/health",) or path == "/api/ledger/me" or path == "/api/ledger/auth/login":
        return "无需登录"
    if path.startswith("/api/admin/"):
        return "运维令牌"
    if path == "/api/mcp":
        return "MCP 令牌"
    rule, arg, _ = rules.get((method, path), ("Open", "", ""))
    if rule == "Open":
        return "登录用户"
    if rule == "Admin":
        return "仅管理员"
    if rule == "Wb":
        need = arg.split(",")[0].replace("Need::", "").strip()
        return f"工作簿{NEED.get(need, need)}权限" + ("（含回收站）" if "true" in arg else "")
    if rule == "Dataset":
        return "数据集所有者或管理员"
    if rule == "Thread":
        return "会话创建人或管理员"
    if rule == "Action":
        return "所属会话的创建人或管理员"
    if rule in ("WbBody", "WbQuery"):
        return "所指工作簿的查看权限"
    return rule


def group_of(path):
    parts = path.split("/")  # ['', 'api', 'ledger', 'workbooks', ...] 或 ['', 'api', 'health']
    seg = parts[3] if len(parts) > 3 and parts[2] == "ledger" else parts[2]
    if parts[2] in ("admin", "health"):
        return "存活检查与运维"
    if parts[2] == "mcp":
        return "外部 AI 接入（MCP）"
    return SEGMENT_GROUP.get(seg, "其他")


def esc(s):
    return s.replace("|", "\\|")


HEAD = """# REST 接口参考

> 本文件由 `scripts/gen-api-docs.py` 根据后端源码里的文档注释与权限表生成，**请不要手工编辑**；改了接口后重新运行脚本。
> 请求体 / 参数的详细字段以对应的 Rust 结构体（`server/src/` 里处理函数的参数类型）为准，下表「说明」列给出的是接口注释里的概要。

## 约定

- **基础路径**：所有接口都在 `/api` 下（页面与服务同源时即 `http://主机:8787/api/...`）。
- **响应格式**：JSON 信封 `{ "code": 200, "data": ..., "msg": "success" }`；出错时 `code` 是 HTTP 状态码、`msg` 是可以直接展示给用户的说明。文件下载、导出、事件流等接口返回原始内容。
- **状态码**：`400` 参数不合法 · `401` 未登录或登录已失效 · `403` 没有权限 · `404` 接口不存在 · `503` 服务繁忙（大计算排队超时）或身份服务不可用 · `500` 内部错误。
- **认证**（`LEDGER_AUTH_MODE` 不为 `off` 时）：
  - 脚本调用：登录取得令牌（`POST /api/ledger/auth/login`，内置账号模式），放进请求头 `x-user-token`；网关模式用网关发的令牌，同样放在 `x-user-token`；
  - 浏览器：登录时服务端写入 `HttpOnly` 会话 cookie；用 cookie 的**写请求**必须带请求头 `x-requested-with`（任意非空值），防跨站请求伪造；
  - `/api/admin/*`：请求头 `x-admin-token` 或 `Authorization: Bearer <令牌>`（未配置 `LEDGER_ADMIN_TOKEN` 时只允许本机访问）；
  - `/api/mcp`：`Authorization: Bearer <MCP 令牌>`（在页面「数据中心 → 外部 AI 接入」里创建）。
- **操作人**：写请求里的 `operator` 字段（查询参数或 JSON 顶层）会被服务端替换为登录用户，客户端传的值无效。
- **数据范围受限的用户**（管理员给用户设置了「只能看某些店铺」）：只能调用下表「受限用户」列为 ✓ 的接口，其余一律返回 403；✓ 的接口读取的数据也已按范围过滤。
- **审计**：「审计」列非空的接口会记入操作审计，列里是审计里显示的操作名。

"""


def registered_routes():
    """main.rs 里注册的全部（方法，路径）"""
    main = (SRC / "main.rs").read_text(encoding="utf-8")
    body = main[main.index(".route("):main.index(".fallback(routes::api_not_found)")]
    found = set()
    for chunk in body.split(".route(")[1:]:
        path = re.match(r'\s*"([^"]+)"', chunk).group(1)
        rest = chunk[chunk.index('"', chunk.index('"') + 1) + 1:]
        for m in re.finditer(r"(?<![A-Za-z0-9_])(get|post|put|delete)\(", rest):
            found.add((m.group(1).upper(), "/api" + path))
    return found


def build():
    docs = doc_routes()
    missing = registered_routes() - set(docs)
    extra = set(docs) - registered_routes()
    if missing or extra:
        sys.exit("接口注释和路由注册对不上：\n"
                 + "".join(f"  缺少文档注释：{m} {p}（在处理函数上方写 `/// {m} {p}  说明`）\n" for m, p in sorted(missing))
                 + "".join(f"  注释里有但没有注册：{m} {p}\n" for m, p in sorted(extra)))
    rules, restricted = auth_tables()
    groups = {g: [] for g in GROUP_ORDER}
    for (method, path), desc in docs.items():
        groups[group_of(path)].append((method, path, desc))
    out = [HEAD.rstrip("\n"), ""]
    total = 0
    for g in GROUP_ORDER:
        rows = groups[g]
        if not rows:
            continue
        rows.sort(key=lambda r: (r[1], METHOD_ORDER[r[0]]))
        out += [f"## {g}", "", "| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |", "|---|---|---|---|---|---|"]
        for method, path, desc in rows:
            perm = permission(method, path, rules)
            label = rules.get((method, path), ("", "", ""))[2]
            if perm in ("仅管理员", "运维令牌", "MCP 令牌") or perm == "无需登录":
                ro = "—" if perm != "无需登录" else "✓"
            else:
                ro = "✓" if (method, path) in restricted else "✗"
            out.append(f"| {method} | `{path}` | {esc(desc)} | {perm} | {ro} | {esc(label)} |")
            total += 1
        out.append("")
    out.insert(2, f"共 {total} 个接口。\n")
    return "\n".join(out).rstrip("\n") + "\n"


def main():
    text = build()
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print("docs/api.md 已过期：请运行 scripts/gen-api-docs.py 并提交结果", file=sys.stderr)
            sys.exit(1)
        print("docs/api.md 是最新的")
        return
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"已生成 {OUT.relative_to(ROOT)}（{text.count(chr(10))} 行）")


main()
