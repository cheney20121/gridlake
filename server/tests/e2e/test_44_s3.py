"""对象存储（S3 协议）数据连接：列对象（翻页、前缀、通配符、子目录）、下载导入、只导入新对象、签名、密钥不外泄

测试里起一个模拟的 S3 服务（路径风格），**用 Python 独立实现 AWS Signature V4 校验**，与服务端 Rust 实现互相印证；
虚拟主机风格的地址和 AWS 文档里的签名示例由 Rust 单元测试覆盖。
"""
from lib import *  # noqa: F403
import datetime
import os
import json

from mock_s3 import ACCESS, BUCKET, OBJECTS, PORT, REGION, SECRET, ago, csv, start, state  # noqa: E402

R = "tok-root"
now = datetime.datetime.now(datetime.timezone.utc)
srv = start()


def source(**kw):
    s = {"kind": "s3", "endpoint": f"http://127.0.0.1:{PORT}", "region": REGION, "bucket": BUCKET, "prefix": "reports/", "pattern": "*.csv",
         "recursive": False, "accessKey": ACCESS, "secretKey": SECRET}
    s.update(kw)
    return {k: v for k, v in s.items() if v is not None}


# ── 配置检查与错误说明 ──
for name, kw, word in [("没填桶", {"bucket": ""}, "存储桶"), ("没填 Access Key", {"accessKey": ""}, "访问密钥 ID"), ("没填规则", {"pattern": " "}, "文件名规则"),
                       ("地址带路径", {"endpoint": f"http://127.0.0.1:{PORT}/data"}, "不要带路径"), ("地址不是网址", {"endpoint": "minio"}, "网址")]:
    s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(**kw)})
    check(f"{name}：被拒绝并说明", s == 400 and word in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(secretKey="wrong")})
check("密钥错误：带出对象存储的原因和排查提示", s == 400 and "SignatureDoesNotMatch" in msg((s, r)) and "区域" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(bucket="nope")})
check("桶不存在：提示检查名称", s == 400 and "存储桶不存在" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(secretKey=None)})
check("没填 Secret Key（也没有已保存的）：提示", s == 400 and "Secret Key" in msg((s, r)), (s, r))

state["bad"].clear()  # 上面故意用错了密钥，签名对不上是预期的

# ── 列对象 ──
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source()})
files = r["data"]["files"] if s == 200 else []
names = [f["name"] for f in files]
check("按前缀 + 通配符 + 只看一层：5 个文件（子目录、txt、临时文件、别的前缀都不在），按修改时间从旧到新",
      s == 200 and names == ["2026-10-01.csv", "2026-10-02.csv", "2026-10-03.csv", "中文 报表.csv", "just-uploaded.csv"], (s, names))
check("路径是 s3://桶/键；刚上传的标为可能仍在写入", files[0]["path"] == f"s3://{BUCKET}/reports/2026-10-01.csv" and files[-1]["settling"] and not files[0]["settling"], files)
check("翻页了（每页 3 个）", state["list_calls"] >= 2, state["list_calls"])
check("签名全部通过（含中文和空格的键）", not state["bad"], state["bad"])
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(recursive=True)})
check("包含子目录后多出一个", s == 200 and any(f["path"].endswith("sub/2026-10-09.csv") for f in r["data"]["files"]), r)
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(pattern="2026-10-0?.csv;*.xlsx")})
check("通配符 ? 与多个规则", s == 200 and [f["name"] for f in r["data"]["files"]] == ["2026-10-01.csv", "2026-10-02.csv", "2026-10-03.csv"], r)

# ── 预览 → 新建 → 同步 ──
s, r = call("POST", "/ledger/connectors/preview", R, body={"source": source()})
check("预览：取最新的匹配文件，识别出两列", s == 200 and [c["title"] for c in r["data"]["columns"]] == ["order_id", "amount"] and r["data"]["sampleOf"].endswith("just-uploaded.csv"), (s, r))
state["gets"].clear()  # 预览下载了「最新的文件」，不算同步
cols = [{"source": c["source"], "title": c["title"], "kind": c["kind"], "include": True} for c in r["data"]["columns"]]
body = {"name": "对象存储订单", "source": source(), "schedule": {"kind": "manual"}, "enabled": True,
        "target": {"name": "对象存储订单", "mode": "upsert", "primaryKey": ["order_id"], "columns": cols}}
s, r = call("POST", "/ledger/connectors", R, body={**body, "source": source(secretKey=None)})
check("新建时必须有密钥", s == 400 and "Secret Key" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors", R, body=body)
check("新建连接器", s == 200, (s, r)); cid = r["data"]
s, r = call("GET", "/ledger/connectors", R)
item = [c for c in r["data"]["items"] if c["id"] == cid][0]
dump = json.dumps(item)
check("Secret Key 永不返回（只有「已保存」标记）；Access Key 和位置信息正常返回",
      item["kind"] == "s3" and item["hasPassword"] and SECRET not in dump and "secretKey" not in dump and "secretEnc" not in dump and item["source"]["accessKey"] == ACCESS and item["source"]["bucket"] == BUCKET, item)


def run():
    s, r = call("POST", f"/ledger/connectors/{cid}/run", R)
    rid = r["data"]
    for _ in range(100):
        time.sleep(0.2)
        s, rr = call("GET", f"/ledger/connectors/{cid}/runs?limit=5", R)
        rec = [x for x in rr["data"] if x["id"] == rid][0]
        if rec["status"] != "running":
            return rec
    return rec


rec = run()
check("首次同步：导入 4 个文件（刚上传的下次再导入），读 6 行", rec["status"] == "success" and rec["rowsRead"] == 6 and rec["inserted"] == 6 and len(rec["files"]) == 4, rec)
check("提示有一个文件可能仍在上传", "仍在写入" in (rec["message"] or ""), rec)
check("只下载了要导入的 4 个对象", sorted(state["gets"]) == sorted(["reports/2026-10-01.csv", "reports/2026-10-02.csv", "reports/2026-10-03.csv", "reports/中文 报表.csv"]), state["gets"])
s, r = call("GET", "/ledger/datasets", R)
ds = [d for d in r["data"] if d["name"] == "对象存储订单"]
check("数据集有 6 行", len(ds) == 1 and ds[0]["rowCount"] == 6, ds)

gets_before = len(state["gets"])
rec = run()
check("再同步一次：没有新对象就跳过，不再下载已导入的", rec["status"] == "skipped" and len(state["gets"]) == gets_before + 0 or state["gets"][gets_before:] == [], (rec, state["gets"]))

# 新对象到了；刚上传的那个过了一分钟也该导入
OBJECTS["reports/2026-10-05.csv"] = (ago(minutes=2), csv(("O9", 900), ("O1", 111)))
OBJECTS["reports/just-uploaded.csv"] = (ago(minutes=5), OBJECTS["reports/just-uploaded.csv"][1])
rec = run()
check("新增的对象按主键更新：新增 O8、O9，覆盖 O1", rec["status"] == "success" and rec["inserted"] == 2 and rec["updated"] == 1 and len(rec["files"]) == 2, rec)
rec = run()
check("导入过的对象不会重复导入", rec["status"] == "skipped", rec)
s, r = call("GET", "/ledger/datasets", R)
ds = [d for d in r["data"] if d["name"] == "对象存储订单"]
check("数据集 8 行", ds[0]["rowCount"] == 8, ds)

# 同一个对象内容变了（大小 / 修改时间变）会再导入一次
OBJECTS["reports/2026-10-02.csv"] = (ago(minutes=2), csv(("O3", 333), ("O10", 1000)))
rec = run()
check("对象被覆盖（修改时间变了）后重新导入", rec["status"] == "success" and rec["inserted"] == 1 and rec["updated"] == 1, rec)

# 编辑时密钥留空沿用已保存的
item = [c for c in call("GET", "/ledger/connectors", R)[1]["data"]["items"] if c["id"] == cid][0]
s, r = call("PUT", f"/ledger/connectors/{cid}", R, body={**body, "source": source(secretKey=None), "target": {"datasetId": item["target"]["datasetId"], "mode": "upsert", "primaryKey": ["order_id"]}})
check("编辑时密钥留空：沿用已保存的", s == 200, (s, r))
rec = run()
check("沿用密钥后仍能同步（全部已导入，跳过）", rec["status"] == "skipped", rec)
check("全程签名校验通过", not state["bad"], state["bad"])

# ── 内部 CA 签发的 https 存储（LEDGER_AUTH_CA_FILE） ──
if os.environ.get("LEDGER_E2E_TLS_DIR"):
    tls = start(port=19005, tls=True)
    s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(endpoint="https://127.0.0.1:19005")})
    check("https 存储：证书由 LEDGER_AUTH_CA_FILE 里的内部 CA 签发，能连上并列出对象", s == 200 and len(r["data"]["files"]) >= 5, (s, r))
    check("https 请求的签名同样通过", not state["bad"], state["bad"])
    tls.shutdown()
else:
    print("SKIP 内部 CA 的 https 测试（没有 openssl）")

srv.shutdown()
finish()
