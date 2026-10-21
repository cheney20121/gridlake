"""从 Rust 源码里的 serde 结构体 / 枚举推出 JSON Schema（给 gen-api-docs.py 生成 OpenAPI 的请求体用）。

只处理常见写法：`#[derive(Deserialize)]` 的结构体 / 枚举，`#[serde(rename_all / rename / alias / default / flatten / skip / tag)]`，
基本类型、Option / Vec / Box / HashMap / BTreeMap / 元组、嵌套结构体。认不出的类型一律当作「任意 JSON」，不会让生成失败。
"""
import pathlib
import re

SRC = pathlib.Path(__file__).resolve().parent.parent / "server" / "src"

PRIMS_INT = {"i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32", "u64", "usize"}
PRIMS_NUM = {"f32", "f64"}
STR_LIKE = {"String", "str", "NaiveDate", "NaiveDateTime", "NaiveTime", "DateTime", "PathBuf"}
WRAPPERS = {"Box", "Arc", "Rc", "Cow"}


def snake_to_camel(s):
    p = s.split("_")
    return p[0] + "".join(x[:1].upper() + x[1:] for x in p[1:])


def rename(name, rule):
    if rule == "camelCase":
        return snake_to_camel(name)
    if rule == "lowercase":
        return name.lower()
    if rule == "UPPERCASE":
        return name.upper()
    if rule == "snake_case":
        return re.sub(r"(?<!^)([A-Z])", r"_\1", name).lower()
    if rule == "SCREAMING_SNAKE_CASE":
        return re.sub(r"(?<!^)([A-Z])", r"_\1", name).upper()
    if rule == "kebab-case":
        return re.sub(r"(?<!^)([A-Z])", r"-\1", name).lower()
    return name


def serde_attrs(attr_lines):
    """合并 #[serde(...)] 里的内容，返回 {键: 值或 True}"""
    out = {}
    for a in attr_lines:
        m = re.match(r"#\[serde\((.*)\)\]\s*$", a.strip())
        if not m:
            continue
        for part in split_top(m.group(1), ","):
            part = part.strip()
            if "=" in part:
                k, v = part.split("=", 1)
                out[k.strip()] = v.strip().strip('"')
            elif part:
                out[part] = True
    return out


def split_top(s, sep):
    """按顶层分隔符拆分（忽略 <> () [] 里的、字符串里的）"""
    parts, depth, cur, in_str = [], 0, "", False
    for ch in s:
        if ch == '"':
            in_str = not in_str
        if not in_str:
            if ch in "<([{":
                depth += 1
            elif ch in ">)]}":
                depth -= 1
            elif ch == sep and depth == 0:
                parts.append(cur)
                cur = ""
                continue
        cur += ch
    if cur.strip():
        parts.append(cur)
    return parts


def match_brace(text, start):
    """text[start] 是 '{'，返回配对的 '}' 位置"""
    depth, in_str, i = 0, False, start
    while i < len(text):
        ch = text[i]
        if ch == '"' and text[i - 1] != "\\":
            in_str = not in_str
        elif not in_str:
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    return i
        i += 1
    return -1


class Index:
    def __init__(self):
        self.defs = {}  # 名称 -> [(文件, 种类, 属性行, 正文)]
        for f in sorted(SRC.rglob("*.rs")):
            self._scan(f)

    def _scan(self, f):
        text = f.read_text(encoding="utf-8")
        cut = text.find("#[cfg(test)]")
        if cut >= 0:
            text = text[:cut]
        lines = text.split("\n")
        offs, pos = [], 0
        for l in lines:
            offs.append(pos)
            pos += len(l) + 1
        for i, l in enumerate(lines):
            m = re.match(r"\s*(?:pub(?:\([a-z]+\))?\s+)?(struct|enum)\s+(\w+)(?:<[^>]*>)?\s*\{", l)
            if not m:
                continue
            attrs, j = [], i - 1
            while j >= 0 and (lines[j].strip().startswith("#[") or lines[j].strip().startswith("///")):
                attrs.append(lines[j].strip())
                j -= 1
            if not any("Deserialize" in a for a in attrs):
                continue
            brace = offs[i] + l.index("{", m.end(2) - 1 if "{" not in l[: m.end(2)] else m.end(2) - 1)
            brace = offs[i] + l.rindex("{") if l.count("{") == 1 else brace
            end = match_brace(text, brace)
            if end < 0:
                continue
            self.defs.setdefault(m.group(2), []).append((f, m.group(1), attrs[::-1], text[brace + 1:end]))

    def find(self, name, path_hint=None):
        cands = self.defs.get(name, [])
        if not cands:
            return None
        if len(cands) > 1 and path_hint:
            hint = [p for p in path_hint.split("::") if p not in ("crate", name)]
            for c in cands:
                rel = c[0].relative_to(SRC).with_suffix("").parts
                if all(h in rel or h == "super" for h in hint):
                    return c
        return cands[0]


INDEX = None
SOURCES = {}  # 登记名 -> 定义所在文件（同名但不在同一个文件的结构体加文件名后缀区分）


def schema_for_type(ty, comps, hint=None, depth=0):
    """Rust 类型字符串 -> (JSON Schema, 是否 Option)"""
    ty = ty.strip().lstrip("&").replace("'static ", "").replace("mut ", "")
    if depth > 12:
        return {}, False
    m = re.match(r"(?:[\w]+::)*(\w+)\s*(?:<(.*)>)?$", ty, re.S)
    if ty.startswith("("):
        inner = split_top(ty[1:-1], ",")
        return ({"type": "array", "items": {}, "minItems": len(inner), "maxItems": len(inner)} if inner else {}), False
    if ty.startswith("["):
        return {"type": "array", "items": schema_for_type(ty[1:].split(";")[0].rstrip("]"), comps, hint, depth + 1)[0]}, False
    if not m:
        return {}, False
    name, args = m.group(1), m.group(2)
    if name == "Option" and args:
        s, _ = schema_for_type(args, comps, hint, depth + 1)
        return s, True
    if name in WRAPPERS and args:
        return schema_for_type(args, comps, hint, depth + 1)
    if name == "bool":
        return {"type": "boolean"}, False
    if name in PRIMS_INT:
        return {"type": "integer"}, False
    if name in PRIMS_NUM:
        return {"type": "number"}, False
    if name in STR_LIKE or name == "char":
        return {"type": "string"}, False
    if name in ("Vec", "HashSet", "BTreeSet", "VecDeque") and args:
        return {"type": "array", "items": schema_for_type(args, comps, hint, depth + 1)[0]}, False
    if name in ("HashMap", "BTreeMap") and args:
        kv = split_top(args, ",")
        return {"type": "object", "additionalProperties": schema_for_type(kv[1], comps, hint, depth + 1)[0] if len(kv) > 1 else {}}, False
    if name in ("Value", "Json", "RawValue"):
        return {}, False
    ref = component(name, comps, ty if "::" in ty else hint)
    return ({"$ref": f"#/components/schemas/{ref}"} if ref else {}), False


def component(name, comps, hint=None):
    """把名为 name 的结构体 / 枚举登记为 components.schemas 里的一项，返回登记名（认不出返回 None）"""
    global INDEX
    if INDEX is None:
        INDEX = Index()
    d = INDEX.find(name, hint)
    if not d:
        return None
    f, kind, attrs, body = d
    key = name
    if key in comps and SOURCES.get(key) != f:
        key = f"{name}_{f.stem}"
    if key in comps:
        return key
    SOURCES[key] = f
    comps[key] = {}  # 先占位，处理递归类型
    comps[key] = build_struct(f, attrs, body, comps) if kind == "struct" else build_enum(f, attrs, body, comps)
    return key


def doc_of(lines):
    return " ".join(l.strip()[3:].strip() for l in lines if l.strip().startswith("///")).strip()


def fields_of(body):
    """结构体正文 -> [(属性行, 文档行, 字段名, 类型)]"""
    out, attrs, docs = [], [], []
    buf = ""
    for raw in body.split("\n"):
        l = raw.strip()
        if not buf:
            if l.startswith("///"):
                docs.append(l)
                continue
            if l.startswith("#["):
                attrs.append(l)
                continue
            if not l or l.startswith("//"):
                continue
        buf += (" " if buf else "") + l
        if split_top(buf, ",") and buf.rstrip().endswith(",") or raw is body.split("\n")[-1]:
            m = re.match(r"(?:pub(?:\([a-z]+\))?\s+)?(\w+)\s*:\s*(.*?),?\s*$", buf, re.S)
            if m:
                out.append((attrs, docs, m.group(1).lstrip("r#"), m.group(2)))
            attrs, docs, buf = [], [], ""
    return out


def build_struct(f, attrs, body, comps):
    cont = serde_attrs(attrs)
    rule = cont.get("rename_all")
    props, required, flat = {}, [], []
    for fattrs, docs, name, ty in fields_of(body):
        a = serde_attrs(fattrs)
        if a.get("skip") or a.get("skip_deserializing"):
            continue
        schema, optional = schema_for_type(ty, comps, str(f.relative_to(SRC)))
        if a.get("flatten"):
            flat.append(schema)
            continue
        key = a.get("rename") or rename(name, rule)
        d = doc_of(docs)
        if d and "$ref" not in schema:
            schema = {**schema, "description": d}
        elif d:
            schema = {"allOf": [schema], "description": d}
        props[key] = schema
        if not (optional or a.get("default") or "default" in a or ty.strip().startswith("Option<")):
            required.append(key)
    out = {"type": "object", "properties": props}
    if required:
        out["required"] = required
    if flat:
        out = {"allOf": [out] + flat}
    return out


def build_enum(f, attrs, body, comps):
    cont = serde_attrs(attrs)
    rule = cont.get("rename_all")
    tag = cont.get("tag")
    variants = [v.strip() for v in split_top(re.sub(r"//.*", "", body), ",") if v.strip()]
    names, one_of = [], []
    for v in variants:
        v = re.sub(r"(#\[[^\]]*\]\s*)+", "", v).strip()
        m = re.match(r"(\w+)\s*(\((.*)\)|\{(.*)\})?\s*$", v, re.S)
        if not m:
            continue
        vname = rename(m.group(1), rule)
        if m.group(2) is None:
            names.append(vname)
        elif tag and m.group(3):
            inner, _ = schema_for_type(m.group(3), comps, str(f.relative_to(SRC)))
            one_of.append({"allOf": [{"type": "object", "properties": {tag: {"type": "string", "enum": [vname]}}, "required": [tag]}, inner]})
    if one_of:
        return {"oneOf": one_of, "discriminator": {"propertyName": tag}}
    if names:
        return {"type": "string", "enum": names}
    return {}


def request_schema(type_name, comps):
    """请求体类型（可带路径，如 crate::report::alert::AlertInput 或 Vec<X>）-> (schema, 是否解析成功)"""
    s, _ = schema_for_type(type_name, comps, None)
    return s
