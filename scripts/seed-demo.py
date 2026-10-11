#!/usr/bin/env python3
"""向一个运行中的 WebExcel 导入示例数据集「订单」（合成数据，只用标准库）。

    scripts/seed-demo.py                                # 100 万行，导入 http://127.0.0.1:8787
    scripts/seed-demo.py --rows 20000000                # 想试试更大的
    scripts/seed-demo.py --url http://host:8787 --token <登录令牌> --rows 5000000

数据：订单号、店铺（300 个）、下单时间、商品（2 万个）、渠道、状态、数量、金额，2024-01 ~ 2026-09。
开了登录（LEDGER_AUTH_MODE=local）时，先用管理员账号登录取令牌：
    curl -s -X POST http://127.0.0.1:8787/api/ledger/auth/login -H 'Content-Type: application/json' \
         -d '{"username":"admin","password":"<密码>"}'     # 取返回里的 data.token，用 --token 传入
"""
import argparse
import datetime
import json
import os
import random
import sys
import tempfile
import time
import urllib.error
import urllib.request

SHOPS = [f"店铺{i:03d}" for i in range(300)]
GOODS = [f"商品{i:05d}" for i in range(20000)]
CHANNELS = ["直播", "商品卡", "短视频", "自然", "达人"]
STATUS = ["已完成", "已发货", "待发货", "已退款", "部分退款", "已取消"]
WEIGHTS = [60, 15, 8, 8, 5, 4]


def generate(path: str, rows: int) -> None:
    rnd = random.Random(7)
    start = datetime.datetime(2024, 1, 1).timestamp()
    span = datetime.datetime(2026, 9, 30).timestamp() - start
    with open(path, "w", encoding="utf-8") as f:
        f.write("订单号,店铺,下单时间,商品,渠道,状态,数量,金额\n")
        batch = []
        for i in range(rows):
            t = datetime.datetime.fromtimestamp(start + rnd.random() * span).strftime("%Y-%m-%d %H:%M:%S")
            q = rnd.randint(1, 5)
            batch.append(f"DY{i:012d},{rnd.choice(SHOPS)},{t},{rnd.choice(GOODS)},{rnd.choice(CHANNELS)},"
                         f"{rnd.choices(STATUS, WEIGHTS)[0]},{q},{round(q * (10 + rnd.random() * 290), 2)}\n")
            if len(batch) >= 50_000:
                f.write("".join(batch))
                batch.clear()
        f.write("".join(batch))


def call(base: str, method: str, path: str, token: str | None, body=None, raw=None, ctype=None):
    headers = {"x-requested-with": "XMLHttpRequest"}
    if token:
        headers["x-user-token"] = token
    data = None
    if raw is not None:
        data, headers["Content-Type"] = raw, ctype
    elif body is not None:
        data, headers["Content-Type"] = json.dumps(body).encode(), "application/json"
    req = urllib.request.Request(base + "/api" + path, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=3600) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        try:
            detail = json.loads(e.read()).get("msg")
        except Exception:
            detail = e.reason
        hint = "（开了登录时需要用 --token 传入登录令牌，见 --help）" if e.code == 401 else ""
        sys.exit(f"请求失败 {method} {path}：{e.code} {detail}{hint}")
    except urllib.error.URLError as e:
        sys.exit(f"连不上 {base}：{e.reason}（服务启动了吗？地址用 --url 指定）")


def main() -> None:
    ap = argparse.ArgumentParser(description="导入示例数据集")
    ap.add_argument("--url", default="http://127.0.0.1:8787", help="服务地址")
    ap.add_argument("--rows", type=int, default=1_000_000, help="行数（默认 100 万）")
    ap.add_argument("--name", default="订单", help="数据集名称")
    ap.add_argument("--token", default=os.environ.get("WEBEXCEL_TOKEN"), help="登录令牌（开了登录时必填；也可用环境变量 WEBEXCEL_TOKEN）")
    a = ap.parse_args()
    base = a.url.rstrip("/")

    with tempfile.TemporaryDirectory() as tmp:
        csv_path = os.path.join(tmp, "orders.csv")
        t = time.time()
        print(f"生成 {a.rows:,} 行示例订单……")
        generate(csv_path, a.rows)
        size = os.path.getsize(csv_path)
        print(f"  完成，{size / 1048576:.0f} MB，用时 {time.time() - t:.0f} 秒")

        boundary = "----webexcel-seed"
        with open(csv_path, "rb") as f:
            payload = f.read()
        raw = (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="orders.csv"\r\n'
               f'Content-Type: text/csv\r\n\r\n').encode() + payload + f"\r\n--{boundary}--\r\n".encode()
        t = time.time()
        print("上传并分析列类型……")
        pre = call(base, "POST", "/ledger/datasets/import/preview", a.token, raw=raw, ctype=f"multipart/form-data; boundary={boundary}")["data"]
        print("导入……")
        res = call(base, "POST", "/ledger/datasets/import/commit", a.token, body={
            "token": pre["token"], "mode": "create", "name": a.name,
            "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in pre["columns"]],
            "primaryKey": ["订单号"], "fileName": pre["fileName"],
        })["data"]
        print(f"完成：数据集「{a.name}」（{res.get('datasetId', '')}），用时 {time.time() - t:.0f} 秒")
        print(f"打开 {base}/excel/home → 数据集，试试筛选、排序、新建透视表。")


main()
