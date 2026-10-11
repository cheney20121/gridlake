"""抖店开放平台连接器（自用型应用）

测试里起一个模拟的抖店网关，**用 Python 独立实现签名校验**（与服务端 Rust 实现互相印证，
但算法本身来自官方文档，没有用真实店铺验证过）：
- 请求体必须是键排序的紧凑 JSON（与参与签名的 param_json 逐字一致）
- 先 token.create（authorization_self + 店铺 ID），再带 access_token 调业务接口；令牌在多次同步间复用
- 页码从 0 开始分页、增量用 update_time_start；code != 10000 时错误原因原样带出
"""
import hashlib
import hmac
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from lib import *  # noqa: F403

APP_KEY, APP_SECRET, SHOP_ID = "ak1", "sk1", "123456"
PORT = 19201
ORDERS = [  # order_id, update_time（unix 秒）, 金额（分）
    ("O1", 1790000001, 1000), ("O2", 1790000002, 2500), ("O3", 1790000003, 300),
    ("O4", 1790000004, 8800), ("O5", 1790000005, 120),
]
state = {"token_created": 0, "calls": [], "bad": []}


def sign_ok(path, query, body, secret):
    """独立实现的签名校验"""
    method = path.strip("/").replace("/", ".")
    q = {k: v[0] for k, v in query.items()}
    canon = json.dumps(json.loads(body), sort_keys=True, separators=(",", ":"), ensure_ascii=False) if body else "{}"
    if body != canon:
        state["bad"].append(f"请求体不是规范 JSON：{body}")
        return False
    s = secret + "app_key" + q.get("app_key", "") + "method" + q.get("method", "") + "param_json" + body + "timestamp" + q.get("timestamp", "") + "v" + q.get("v", "") + secret
    want = hmac.new(secret.encode(), s.encode(), hashlib.sha256).hexdigest()
    return q.get("method") == method and q.get("v") == "2" and q.get("sign_method") == "hmac-sha256" and q.get("sign") == want


class Gateway(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, obj):
        data = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        u = urlparse(self.path)
        query = parse_qs(u.query)
        body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
        secret = APP_SECRET
        if query.get("app_key", [""])[0] != APP_KEY or not sign_ok(u.path, query, body, secret):
            return self.reply({"code": 40001, "msg": "sign error", "sub_code": "isv.sign-error", "sub_msg": "签名校验失败"})
        params = json.loads(body) if body else {}
        if u.path == "/token/create":
            if params.get("grant_type") != "authorization_self" or params.get("shop_id") != SHOP_ID:
                return self.reply({"code": 30002, "msg": "grant error", "sub_msg": "店铺未授权"})
            state["token_created"] += 1
            return self.reply({"code": 10000, "msg": "success", "data": {"access_token": f"at-{state['token_created']}", "expires_in": 604800, "shop_id": SHOP_ID}})
        if query.get("access_token", [""])[0] != f"at-{state['token_created']}":
            return self.reply({"code": 30001, "msg": "access_token invalid", "sub_msg": "token 无效"})
        if u.path == "/order/searchList":
            state["calls"].append(params)
            since = int(params.get("update_time_start", 0))
            rows = [o for o in ORDERS if o[1] >= since]
            page, size = int(params["page"]), int(params["size"])
            part = rows[page * size:(page + 1) * size]
            return self.reply({"code": 10000, "msg": "success", "log_id": "x", "data": {
                "total": len(rows), "page": page, "size": size,
                "shop_order_list": [{"order_id": o[0], "update_time": o[1], "pay_amount": o[2], "buyer": {"city": "杭州"}} for o in part]}})
        return self.reply({"code": 20000, "msg": "method not found"})


srv = ThreadingHTTPServer(("127.0.0.1", PORT), Gateway)
threading.Thread(target=srv.serve_forever, daemon=True).start()

URL = f"http://127.0.0.1:{PORT}/order/searchList"
BODY = '{"page":{{page}},"size":{{size}},"update_time_start":{{watermark}}}'


def source(**kw):
    s = {"kind": "http", "url": URL, "method": "post", "body": BODY, "auth": {"kind": "doudian", "appKey": APP_KEY, "shopId": SHOP_ID},
         "secret": APP_SECRET, "recordsPath": "data.shop_order_list", "pagination": {"kind": "page", "start": 0, "size": 2},
         "incrementalField": "update_time", "initialWatermark": "0"}
    s.update(kw)
    return s


# 保存前校验
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(url="http://127.0.0.1:%d/" % PORT)}); check("地址缺方法路径被拒绝", s == 400 and "方法路径" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(auth={"kind": "doudian", "appKey": "", "shopId": SHOP_ID})}); check("缺 App Key 被拒绝", s == 400 and "App Key" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(auth={"kind": "doudian", "appKey": APP_KEY, "shopId": "店铺"})}); check("店铺 ID 必须是数字", s == 400 and "店铺 ID" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(method="get")}); check("抖店接口必须 POST", s == 400 and "POST" in msg((s, r)), (s, r))

# 测试连接：签名与令牌
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(secret="wrong")})
check("App Secret 错误时带出平台原因", s == 400 and "签名校验失败" in msg((s, r)) and "App Secret" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source(auth={"kind": "doudian", "appKey": APP_KEY, "shopId": "999"})})
check("店铺未授权时提示检查店铺 ID", s == 400 and "店铺未授权" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors/test", R, body={"source": source()})
check("测试连接：第一页 2 条记录，嵌套字段展开", s == 200 and r["data"]["http"]["records"] == 2 and "buyer.city" in r["data"]["http"]["fields"], (s, r))
check("模拟网关没有收到任何签名问题", not state["bad"], state["bad"])

# 预览 → 新建 → 同步
s, r = call("POST", "/ledger/connectors/preview", R, body={"source": source()})
check("预览取到 5 条订单", s == 200 and r["data"]["estimatedRows"] >= 5, (s, r))
cols = [{"source": c["source"], "title": c["title"], "kind": c["kind"], "include": True} for c in r["data"]["columns"]]
body = {"name": "抖店订单", "source": source(), "schedule": {"kind": "manual"}, "enabled": True,
        "target": {"name": "抖店订单", "mode": "upsert", "primaryKey": ["order_id"], "columns": cols}}
s, r = call("POST", "/ledger/connectors", R, body=body); check("新建连接器", s == 200, (s, r)); cid = r["data"]
s, r = call("GET", "/ledger/connectors", R); item = [c for c in r["data"]["items"] if c["id"] == cid][0]
check("App Secret 永不返回（只有「已保存」标记）", item["hasPassword"] and "secret" not in json.dumps(item["source"]) and "sk1" not in json.dumps(item), item)
check("应用与店铺信息正常返回", item["source"]["auth"] == {"kind": "doudian", "appKey": APP_KEY, "shopId": SHOP_ID}, item["source"])


def run():
    s, r = call("POST", f"/ledger/connectors/{cid}/run", R); rid = r["data"]
    for _ in range(100):
        time.sleep(0.2)
        s, rr = call("GET", f"/ledger/connectors/{cid}/runs?limit=5", R)
        rec = [x for x in rr["data"] if x["id"] == rid][0]
        if rec["status"] != "running":
            return rec
    return rec


rec = run()
check("首次同步成功：读 5 条、新增 5 条", rec["status"] == "success" and rec["rowsRead"] == 5 and rec["inserted"] == 5, rec)
check("翻了 3 页（0、1、2 页，每页 2 条）", [c["page"] for c in state["calls"][-3:]] == [0, 1, 2], state["calls"])
check("增量位置推进到最大 update_time", rec["watermarkTo"] == "1790000005", rec)
tokens_after_first = state["token_created"]
ORDERS.append(("O6", 1790000010, 777))
ORDERS[1] = ("O2", 1790000006, 2600)  # O2 被修改（金额变了）
rec = run()
check("第二次按 update_time_start 增量：边界那条（>=）按主键跳过，新增 1、修改 1", rec["status"] == "success" and rec["rowsRead"] == 3 and rec["inserted"] == 1 and rec["updated"] == 1 and rec["skipped"] == 1, rec)
check("第二次请求带上次的增量位置", state["calls"][-1]["update_time_start"] == 1790000005, state["calls"][-1])
check("令牌在两次同步间复用（只换取一次）", state["token_created"] == tokens_after_first == 1, state["token_created"])
check("全程签名校验通过", not state["bad"], state["bad"])
s, r = call("GET", "/ledger/datasets", R); ds = [d for d in r["data"] if d["name"] == "抖店订单"]
check("数据集有 6 行", len(ds) == 1 and ds[0]["rowCount"] == 6, ds)

# 修改店铺：增量位置重置；留空密钥沿用
s, r = call("GET", "/ledger/connectors", R); item = [c for c in r["data"]["items"] if c["id"] == cid][0]
body["target"] = {"datasetId": item["target"]["datasetId"], "mode": "upsert", "primaryKey": ["order_id"]}
s, r = call("PUT", f"/ledger/connectors/{cid}", R, body={**body, "source": source(secret=None, auth={"kind": "doudian", "appKey": APP_KEY, "shopId": "999"})})
check("改成未授权的店铺：保存成功", s == 200, (s, r))
rec = run(); check("未授权店铺同步失败并说明原因", rec["status"] == "failed" and "店铺未授权" in (rec["message"] or ""), rec)
s, r = call("GET", "/ledger/connectors", R); item = [c for c in r["data"]["items"] if c["id"] == cid][0]
check("换店铺后增量位置重置", item["watermark"] is None, item["watermark"])
s, r = call("PUT", f"/ledger/connectors/{cid}", R, body={**body, "source": source(secret=None)})
rec = run(); check("改回后留空密钥沿用已保存的；增量位置已重置，重新读全部 6 条（与已有数据相同故跳过）", rec["rowsRead"] == 6 and rec["watermarkFrom"] is None and rec["skipped"] == 6 and rec["status"] != "failed", rec)

srv.shutdown()
finish()
