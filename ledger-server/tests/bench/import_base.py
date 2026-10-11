import os, sys, time, json
os.environ["LEDGER_E2E_BASE"] = "http://127.0.0.1:18950/api"
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "e2e"))
from lib import call
data = open(os.environ.get("BENCH_CSV", "base.csv"), "rb").read()
b = "XB"
raw = (f'--{b}\r\nContent-Disposition: form-data; name="file"; filename="orders.csv"\r\nContent-Type: text/csv\r\n\r\n').encode() + data + f"\r\n--{b}--\r\n".encode()
t = time.time()
s, r = call("POST", "/ledger/datasets/import/preview", None, raw=raw, ctype=f"multipart/form-data; boundary={b}")
print("preview", s, round(time.time() - t, 1), "s", (r["data"]["estimatedRows"], [(c["title"], c["kind"]) for c in r["data"]["columns"]]) if s == 200 else r)
p = r["data"]
t = time.time()
s, r = call("POST", "/ledger/datasets/import/commit", None, body={"token": p["token"], "mode": "create", "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": ["订单号"], "fileName": p["fileName"]})
print("commit", s, round(time.time() - t, 1), "s", r)
