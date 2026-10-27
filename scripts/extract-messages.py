#!/usr/bin/env python3
"""接口错误信息的英文覆盖检查：源码里的中文错误信息模板，是不是都在 server/src/msg_en.json 里有英文。

    scripts/extract-messages.py            # 列出缺英文的模板，有缺失时退出码为 1（CI 用）
    scripts/extract-messages.py --stale    # 同时列出目录里已经不在源码中的模板（可能是改了原文没删旧的，也可能是手工补的嵌套短语）
    scripts/extract-messages.py --dump     # 把源码里的全部模板按 JSON 打印出来（给新增翻译做底稿）

提取规则：`AppError::bad / forbidden / unauthorized / unavailable(...)`、`bail!`、`anyhow!`、`.context(...)`、`Err(...)` 的
第一个参数里的中文字符串（含 `format!`）。占位符统一成 `{0}` `{1}`…（按在中文里出现的顺序，同名的是同一个）。
这个脚本只能找到**直接写在这些调用里**的信息；先拼进变量、再传出去的信息找不到——它们要靠端到端测试的
收集模式补：`LEDGER_E2E_COLLECT_UNTRANSLATED=1 server/tests/e2e/run.sh`，结束时会列出响应里仍含中文的错误信息。
"""
import glob
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "server" / "src"
CATALOG = SRC / "msg_en.json"
CJK = re.compile(r"[一-鿿]")
SKIP = {"msg_i18n.rs", "config.rs", "open.rs"}  # 本模块自己的示例；启动期的配置错误、gridlake open 的命令行提示不走接口
CALLS = re.compile(
    r"(?:AppError::(?:bad|forbidden|unauthorized|unavailable|BadRequest|Forbidden|Unauthorized|Unavailable)"
    r"|Self::(?:BadRequest|Forbidden|Unauthorized|Unavailable)|\bbail!|\banyhow!|\.context|\.with_context|\bErr)\s*\(\s*(?:\|\|\s*)?"
)


def read_string(s, i):
    """s[i] 是双引号：返回（字符串内容, 结束位置）。处理转义与行尾续行"""
    j = i + 1
    out = []
    while j < len(s):
        c = s[j]
        if c == "\\":
            n = s[j + 1]
            if n == "\n":
                j += 2
                while j < len(s) and s[j] in " \t\n":
                    j += 1
                continue
            out.append({"n": "\n", "t": "\t", '"': '"', "'": "'", "\\": "\\"}.get(n, "\\" + n))
            j += 2
            continue
        if c == '"':
            return "".join(out), j + 1
        out.append(c)
        j += 1
    return None, i


def placeholders(tpl):
    """Rust 格式串 → {0} {1} 形式；`{{` `}}` 是字面花括号"""
    names, out, k, i = [], [], 0, 0
    while i < len(tpl):
        if tpl.startswith("{{", i):
            out.append("{{")
            i += 2
        elif tpl.startswith("}}", i):
            out.append("}}")
            i += 2
        elif tpl[i] == "{":
            j = tpl.index("}", i)
            name = tpl[i + 1:j].split(":")[0]
            if name == "":
                key = ("pos", k)
                k += 1
            else:
                key = ("name", name)
            if key not in names:
                names.append(key)
            out.append("{%d}" % names.index(key))
            i = j + 1
        else:
            out.append(tpl[i])
            i += 1
    return "".join(out).replace("{{", "{").replace("}}", "}")


def extract():
    found = {}
    for f in sorted(glob.glob(str(SRC / "**" / "*.rs"), recursive=True)):
        path = pathlib.Path(f)
        if path.name in SKIP:
            continue
        s = path.read_text(encoding="utf-8")
        cut = s.find("#[cfg(test)]")
        body = s if cut < 0 else s[:cut]
        for m in CALLS.finditer(body):
            i = m.end()
            if body.startswith("format!(", i):
                i += len("format!(")
            while i < len(body) and body[i] in " \n\t":
                i += 1
            if i < len(body) and body[i] == '"':
                txt, _ = read_string(body, i)
                if txt and CJK.search(txt):
                    found.setdefault(placeholders(txt), []).append(f"{path.relative_to(ROOT)}:{body[:i].count(chr(10)) + 1}")
    return found


def main():
    found = extract()
    if "--dump" in sys.argv:
        print(json.dumps({k: v[0] for k, v in found.items()}, ensure_ascii=False, indent=1))
        return
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    missing = [k for k in found if k not in catalog]
    for k in missing:
        print(f"缺少英文：{found[k][0]}  {json.dumps(k, ensure_ascii=False)}")
    if "--stale" in sys.argv:
        for k in catalog:
            if k not in found:
                print(f"源码里没有（可能是手工补的嵌套短语，或已过期）：{json.dumps(k, ensure_ascii=False)}")
    print(f"{len(found)} 个模板，目录 {len(catalog)} 条，缺 {len(missing)} 条")
    if missing:
        print("\n请把它们加进 server/src/msg_en.json（占位符保持一致）", file=sys.stderr)
        sys.exit(1)


main()
