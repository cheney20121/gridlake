#!/usr/bin/env python3
"""生成 THIRD_PARTY_LICENSES.md：列出后端（Cargo）与前端（pnpm 生产依赖）用到的第三方库及其许可证。

    scripts/gen-third-party.py            # 重新生成 THIRD_PARTY_LICENSES.md
    scripts/gen-third-party.py --check    # 只检查有没有许可证不在允许清单里（CI 用），不写文件

前置：已执行 `pnpm install`（前端依赖）；后端用 cargo metadata（需要网络之外的本地 crates 缓存或已编译过）。
"""
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD_PARTY_LICENSES.md"

# 允许的许可证（SPDX 标识）。都是宽松许可证：可商用、可闭源集成，只要求保留版权与许可声明。
# 出现清单外的许可证（尤其是 GPL / AGPL / SSPL / 商业许可）要先评估再加
ALLOWED = {
    "MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unlicense", "0BSD",
    "CC0-1.0", "Unicode-3.0", "Unicode-DFS-2016", "BSL-1.0", "CDLA-Permissive-2.0", "MPL-2.0",
    "Apache-2.0 WITH LLVM-exception",
}
# 前端个别包的 package.json 写法不规范，人工核对过的别名
ALIASES = {"MIT/X11": "MIT", "BSD": "BSD-3-Clause", "MIT/Apache-2.0": "MIT OR Apache-2.0", "Apache-2.0/MIT": "MIT OR Apache-2.0", "Unlicense/MIT": "MIT OR Unlicense"}
# 许可证字段缺失但已人工核对（包里的 LICENSE 文件为 MIT）
MANUAL = {"buffers": "MIT"}


def satisfiable(expr: str) -> bool:
    """SPDX 表达式里，OR 任选一个允许的即可；AND 的每一项都必须允许"""
    expr = ALIASES.get(expr.strip(), expr).strip()
    expr = expr.replace("/", " OR ")
    tokens = re.findall(r"\(|\)|[A-Za-z0-9.\-+]+(?: WITH [A-Za-z0-9.\-]+)?|AND|OR", expr)
    pos = 0

    def parse_or():
        nonlocal pos
        v = parse_and()
        while pos < len(tokens) and tokens[pos] == "OR":
            pos += 1
            r = parse_and()
            v = v or r
        return v

    def parse_and():
        nonlocal pos
        v = parse_atom()
        while pos < len(tokens) and tokens[pos] == "AND":
            pos += 1
            r = parse_atom()
            v = v and r
        return v

    def parse_atom():
        nonlocal pos
        t = tokens[pos]
        pos += 1
        if t == "(":
            v = parse_or()
            pos += 1  # )
            return v
        return t in ALLOWED

    return parse_or()


def rust():
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--manifest-path", str(ROOT / "ledger-server/Cargo.toml"), "--format-version", "1", "--locked"],
        text=True))
    root_ids = {p["id"] for p in meta["packages"] if p["name"] == "ledger-server"}
    out = []
    for p in meta["packages"]:
        if p["id"] in root_ids:
            continue
        out.append((p["name"], p["version"], p.get("license") or "（未声明）", p.get("repository") or ""))
    return sorted(set(out))


def web():
    raw = subprocess.check_output(["pnpm", "licenses", "list", "--prod", "--json"], cwd=ROOT / "web", text=True)
    out = []
    for lic, pkgs in json.loads(raw).items():
        for p in pkgs:
            for v in p.get("versions", [p.get("version", "")]):
                name = p["name"]
                out.append((name, v, MANUAL.get(name, lic), p.get("homepage") or ""))
    return sorted(set(out))


def main():
    check_only = "--check" in sys.argv
    sections = [("后端（Rust crates）", rust()), ("前端（npm 生产依赖）", web())]
    bad = [(n, v, l) for _, rows in sections for n, v, l, _ in rows if not satisfiable(l)]
    if bad:
        print("以下依赖的许可证不在允许清单里，请评估后处理（或确认无误后加入 ALLOWED）：", file=sys.stderr)
        for n, v, l in bad:
            print(f"  {n} {v}: {l}", file=sys.stderr)
        sys.exit(1)
    if check_only:
        print("许可证检查通过：" + "，".join(f"{t} {len(r)} 个" for t, r in sections))
        return
    lines = [
        "# 第三方许可证",
        "",
        "WebExcel 本身以 [Apache-2.0](LICENSE) 发布，并使用了下列开源软件。它们各自的许可证和版权声明以对应项目为准；",
        "全部是宽松许可证（MIT / Apache-2.0 / BSD / ISC 等），没有 GPL / AGPL 等传染性许可证。",
        "",
        "重点依赖：[Univer](https://github.com/dream-num/univer)（Apache-2.0，表格界面）、[DuckDB](https://github.com/duckdb/duckdb)（MIT，数据引擎）、",
        "[Vue](https://github.com/vuejs/core)（MIT）、[Element Plus](https://github.com/element-plus/element-plus)（MIT）、[ECharts](https://github.com/apache/echarts)（Apache-2.0）、",
        "[axum](https://github.com/tokio-rs/axum)（MIT）、[tokio](https://github.com/tokio-rs/tokio)（MIT）。",
        "",
        "本文件由 `scripts/gen-third-party.py` 生成，依赖变化后请重新生成。",
        "",
    ]
    for title, rows in sections:
        lines += [f"## {title}（{len(rows)} 个）", "", "| 名称 | 版本 | 许可证 |", "|---|---|---|"]
        lines += [f"| {n} | {v} | {l} |" for n, v, l, _ in rows]
        lines.append("")
    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"已生成 {OUT.relative_to(ROOT)}：" + "，".join(f"{t} {len(r)} 个" for t, r in sections))


main()
