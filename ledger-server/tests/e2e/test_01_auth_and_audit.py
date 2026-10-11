"""N1 登录身份、权限与操作审计

未登录 / 无效令牌；operator 冒充被覆盖；工作簿所有者与共享范围；数据集所有者；仅管理员接口；审计；身份服务不可用。
"""
from lib import *  # noqa: F403

# 1 未登录
s, r = call("GET", "/ledger/workbooks"); check("未登录读工作簿 401", s == 401, (s, r))
s, r = call("GET", "/health"); check("health 免登录", s == 200)
s, r = call("GET", "/ledger/me"); check("me 未登录: authRequired 且 user 为空", s == 200 and r["data"]["authRequired"] and r["data"]["user"] is None, r)
s, r = call("GET", "/ledger/workbooks", "tok-bad"); check("无效令牌 401", s == 401, (s, r))
s, r = call("GET", "/ledger/me", "tok-alice"); check("me alice", r["data"]["user"]["name"] == "Alice" and not r["data"]["user"]["admin"], r)
s, r = call("GET", "/ledger/me", "tok-root"); check("me root 是管理员", r["data"]["user"]["admin"] is True, r)

# 2 创建工作簿，operator 冒充被覆盖
wbjson = gzip.compress(json.dumps({"sheets": {"s1": {"name": "汇总", "cellData": {"0": {"0": {"v": "店铺"}}}}}, "sheetOrder": ["s1"]}).encode())
s, r = call("POST", "/ledger/workbooks?name=%E5%88%A9%E6%B6%A6%E8%A1%A8&operator=%E5%81%87%E5%86%92", "tok-alice", raw=wbjson)
check("alice 新建工作簿", s == 200, (s, r))
wb = r["data"]["id"]
check("操作人被覆盖为 Alice", r["data"]["createdBy"] == "Alice", r["data"])
check("所有者是 u1，默认 private", r["data"]["ownerId"] == "u1" and r["data"]["visibility"] == "private", r["data"])

# 3 bob 看不到也摸不到
s, r = call("GET", "/ledger/workbooks", "tok-bob"); check("bob 列表为空", s == 200 and r["data"] == [], r)
s, r = call("GET", f"/ledger/workbooks/{wb}", "tok-bob"); check("bob 读元数据 403", s == 403 and "Alice" in msg((s, r)), (s, r))
s, r = call("GET", f"/ledger/workbooks/{wb}/content", "tok-bob"); check("bob 读内容 403", s == 403)
s, r = call("GET", f"/ledger/workbooks/{wb}/versions", "tok-bob"); check("bob 读版本 403", s == 403)
s, r = call("POST", f"/ledger/workbooks/{wb}/duplicate", "tok-bob", body={"name": "偷看"}); check("bob 复制 403", s == 403)
s, r = call("GET", f"/ledger/ai/threads?workbookId={wb}", "tok-bob"); check("bob 读 AI 会话 403", s == 403, (s, r))
s, r = call("POST", "/ledger/workbooks/ranges", "tok-bob", body={"items": [{"workbook": "利润表", "range": "A1"}]})
check("bob 跨工作簿引用被拒", s == 200 and "无权" in (r["data"][0].get("error") or ""), r)
s, r = call("POST", "/ledger/workbooks/ranges", "tok-alice", body={"items": [{"workbook": "利润表", "range": "A1"}]})
check("alice 跨工作簿引用自己的", s == 200 and r["data"][0].get("error") in (None, "") or "引用了当前" in str(r), r)

# 4 共享：只读 -> 可编辑
s, r = call("PUT", f"/ledger/workbooks/{wb}/share", "tok-bob", body={"visibility": "edit"}); check("bob 改共享范围 403", s == 403, (s, r))
s, r = call("PUT", f"/ledger/workbooks/{wb}/share", "tok-alice", body={"visibility": "view"}); check("alice 共享为只读", s == 200 and r["data"]["visibility"] == "view", r)
s, r = call("GET", "/ledger/workbooks", "tok-bob"); check("bob 现在能看到", len(r["data"]) == 1)
s, r = call("GET", f"/ledger/workbooks/{wb}/content", "tok-bob"); check("bob 读内容 200", s == 200)
s, r = call("PUT", f"/ledger/workbooks/{wb}/content?baseVersion=1&operator=%E5%81%87", "tok-bob", raw=wbjson); check("只读时 bob 保存 403", s == 403 and "只读" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/workbooks/{wb}/share", "tok-alice", body={"visibility": "edit"}); check("alice 共享为可编辑", s == 200)
s, r = call("PUT", f"/ledger/workbooks/{wb}/content?baseVersion=1&operator=%E5%81%87", "tok-bob", raw=wbjson); check("可编辑时 bob 保存成功", s == 200 and r["data"]["saved"], (s, r))
s, r = call("GET", f"/ledger/workbooks/{wb}", "tok-bob"); check("保存人记为 Bob", r["data"]["updatedBy"] == "Bob", r["data"])
s, r = call("DELETE", f"/ledger/workbooks/{wb}?operator=x", "tok-bob"); check("bob 删除 403", s == 403 and "所有者或管理员" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/workbooks/move", "tok-bob", body={"ids": [wb], "folderId": None}); check("bob 移动(编辑权限)可以", s == 200, (s, r))

# 5 私有 + 管理员
s, r = call("PUT", f"/ledger/workbooks/{wb}/share", "tok-alice", body={"visibility": "private"})
s, r = call("GET", "/ledger/workbooks", "tok-root"); check("管理员看得到私有工作簿", len(r["data"]) == 1)
s, r = call("GET", f"/ledger/workbooks/{wb}/content", "tok-root"); check("管理员可读", s == 200)

# 6 数据集：导入 -> 归属 -> 他人不能改
boundary = "XBOUNDARY"
csv = "店铺,金额\nA,10\nB,20\n".encode("utf-8")
def multipart(fields, fname="订单.csv", content=csv):
    b = []
    b.append(f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{fname}"\r\nContent-Type: text/csv\r\n\r\n'.encode() + content + b"\r\n")
    b.append(f"--{boundary}--\r\n".encode())
    return b"".join(b)
s, r = call("POST", "/ledger/datasets/import/preview", "tok-alice", raw=multipart({}), ctype=f"multipart/form-data; boundary={boundary}")
check("alice 预览导入", s == 200, (s, str(r)[:200]))
prev = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", "tok-alice", body={"token": prev["token"], "mode": "create", "name": "订单", "columns": [{"source": c.get("source") or c["title"], "title": c["title"], "kind": c["kind"]} for c in prev["columns"]], "primaryKey": [], "fileName": prev["fileName"]})
check("alice 创建数据集", s == 200, (s, r))
ds = r["data"]["datasetId"]
s, r = call("GET", "/ledger/datasets", "tok-bob")
mine = [d for d in r["data"] if d["id"] == ds]
check("数据集列表带所有者", mine and mine[0]["ownerName"] == "Alice", mine)
s, r = call("PUT", f"/ledger/datasets/{ds}", "tok-bob", body={"name": "被改"}); check("bob 改名 403", s == 403 and "Alice" in msg((s, r)), (s, r))
s, r = call("DELETE", f"/ledger/datasets/{ds}?operator=x", "tok-bob"); check("bob 删除数据集 403", s == 403)
s, r = call("POST", f"/ledger/datasets/{ds}/calc", "tok-bob", body={"name": "x", "formula": "=1"}); check("bob 加计算列 403", s == 403)
s, r = call("POST", f"/ledger/datasets/{ds}/stats", "tok-bob", body={"cursor": "", "columns": []}); check("bob 读数据集(统计)不被拦截", s != 403, (s, msg((s, r))))
s, r = call("PUT", f"/ledger/datasets/{ds}", "tok-alice", body={"name": "订单2"}); check("alice 改名成功", s == 200, (s, r))
s, r = call("PUT", f"/ledger/datasets/ledger_detail", "tok-alice", body={"name": "x"}); check("非管理员改内置数据集 403", s == 403, (s, r))

# 7 仅管理员的接口
s, r = call("GET", "/ledger/connectors", "tok-bob"); check("bob 看连接器列表允许", s == 200, (s, r))
s, r = call("POST", "/ledger/connectors", "tok-bob", body={"name": "x"}); check("bob 新建连接器 403", s == 403 and "管理员" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/connectors", "tok-root", body={"name": "x"}); check("root 新建连接器不被权限拦截(400 参数错)", s in (400, 422), (s, r))
s, r = call("GET", "/ledger/mcp/tokens", "tok-bob"); check("bob 看 MCP 令牌 403", s == 403)
s, r = call("POST", "/ledger/report-channels", "tok-bob", body={}); check("bob 建推送渠道 403", s == 403)
s, r = call("DELETE", "/ledger/workbooks/trash", "tok-bob"); check("bob 清空回收站 403", s == 403)
s, r = call("DELETE", "/ledger/data?shop=x", "tok-bob"); check("bob 删店铺数据 403", s == 403, (s, r))

# 8 JSON 体 operator 覆盖：新建文件夹
s, r = call("POST", "/ledger/folders", "tok-bob", body={"name": "财务", "parentId": None, "operator": "假冒者"}); check("bob 建文件夹", s == 200, (s, r))
s, r = call("GET", "/ledger/folders", "tok-bob"); fo = [f for f in r["data"] if f["name"] == "财务"]
check("文件夹创建人是 Bob", fo and fo[0].get("createdBy") == "Bob", fo)

# 9 审计
time.sleep(3)
s, r = call("GET", "/ledger/audit", "tok-bob"); check("bob 看审计 403", s == 403)
s, r = call("GET", "/ledger/audit?size=100", "tok-root"); check("root 看审计", s == 200, (s, r))
items = r["data"]["items"]
acts = [(i["userName"], i["action"], i["status"]) for i in items]
check("审计含 Alice 新建工作簿", ("Alice", "新建工作簿", 200) in acts, acts[:5])
check("审计含 Bob 被拒的删除", ("Bob", "删除工作簿", 403) in acts, acts)
check("审计含 Bob 保存成功", ("Bob", "保存工作簿", 200) in acts)
check("审计含 Bob 被拒的保存", ("Bob", "保存工作簿", 403) in acts)
check("审计记目标名称", any(i["targetName"] == "利润表" and i["targetKind"] == "工作簿" for i in items))
check("读取类不记审计", not any(i["action"] in ("", None) for i in items) and not any("/stats" in i["action"] for i in items))
s, r = call("GET", "/ledger/audit?user=Bob&failed=true", "tok-root"); check("按用户+失败筛选", s == 200 and r["data"]["total"] >= 4 and all(i["userName"] == "Bob" and i["status"] >= 400 for i in r["data"]["items"]), r["data"]["total"])
print("audit sample:", json.dumps(items[0], ensure_ascii=False))

# 10 缓存与网关故障
stop_gateway()
s, r = call("GET", "/ledger/workbooks", "tok-alice"); check("网关挂了，缓存内的令牌仍可用", s == 200, (s, r))
s, r = call("GET", "/ledger/me", "tok-never-seen"); check("网关挂了，新令牌 503（/me 如实报错）", s == 503 and "身份服务" in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/workbooks", "tok-never-seen"); check("网关挂了，新令牌 503", s == 503, (s, r))

finish()
