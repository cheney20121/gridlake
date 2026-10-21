#!/usr/bin/env python3
"""环境变量文档检查：服务读取的每个 LEDGER_* 环境变量都要写进部署手册，示例配置里的变量都要真实存在。

    scripts/check-env-docs.py

- 后端源码（排除测试模块）里出现的 `LEDGER_xxx`，必须出现在 deploy/部署手册.md；
- deploy/ledger.env.example 里出现的 `LEDGER_xxx`（含被注释的示例行），必须是源码里真实存在的变量（防止写错名字，用户照抄却不生效）。
只给测试、脚本用的变量放在下面的 INTERNAL 里。
"""
import glob
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
VAR = re.compile(r"\bLEDGER_[A-Z0-9]+(?:_[A-Z0-9]+)*\b")
# 只在测试 / 打包脚本里用，不属于部署配置
INTERNAL = {"LEDGER_BIN"}


def used_in_code():
    found = {}
    for f in sorted(glob.glob(str(ROOT / "server" / "src" / "**" / "*.rs"), recursive=True)):
        path = pathlib.Path(f)
        if path.name == "tests.rs":
            continue
        s = path.read_text(encoding="utf-8")
        cut = s.find("#[cfg(test)]")
        for m in VAR.finditer(s if cut < 0 else s[:cut]):
            found.setdefault(m.group(0), str(path.relative_to(ROOT)))
    return found


def main():
    code = used_in_code()
    manual = set(VAR.findall((ROOT / "deploy" / "部署手册.md").read_text(encoding="utf-8")))
    example = set(VAR.findall((ROOT / "deploy" / "ledger.env.example").read_text(encoding="utf-8")))
    bad = False
    for name, where in sorted(code.items()):
        if name not in manual and name not in INTERNAL:
            print(f"部署手册里没有：{name}（{where}）")
            bad = True
    for name in sorted(example - set(code)):
        print(f"ledger.env.example 里的 {name} 在源码里不存在（拼错了？）")
        bad = True
    print(f"{len(code)} 个变量，手册 {len(manual)} 个，示例 {len(example)} 个")
    if bad:
        print("\n请把新增的变量写进 deploy/部署手册.md 的变量表，并检查示例配置里的名字", file=sys.stderr)
        sys.exit(1)


main()
