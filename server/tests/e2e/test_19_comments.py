"""N10（精简版）：单元格评论线程与 @提醒、应用内通知

权限（能查看工作簿就能评论；只能改自己的；删除限作者 / 工作簿所有者 / 管理员）、@ 的人必须能查看工作簿（否则不通知并说明）、
回复通知线程作者、不通知自己、编辑时只通知新增的 @、解决线程、删除线程连带通知、通知已读、输入校验。
"""
from lib import *  # noqa: F403
for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}
alice, bob, carol = ids["Alice"], ids["Bob"], ids["Carol"]

wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("评论测试"), A, raw=wbj); wb = r["data"]["id"]
cm = lambda tok, body, **kw: call("POST", f"/ledger/workbooks/{wb}/comments", tok, body={"body": body, **kw})
inbox = lambda tok: call("GET", "/ledger/notifications", tok)[1]["data"]

# 私有工作簿：别人不能看、不能评论
s, r = call("GET", f"/ledger/workbooks/{wb}/comments", Bo); check("私有工作簿：bob 不能读评论", s == 403, (s, r))
s, r = cm(Bo, "偷偷评论", sheetId="s1", row=0, col=0); check("私有工作簿：bob 不能评论", s == 403, (s, r))
# @ 一个看不到工作簿的人：评论成功，但没有通知到他，并说明
s, r = cm(A, "请 @Bob 看一下", sheetId="s1", row=1, col=2, mentions=[bob]); check("alice 评论并 @bob", s == 200, (s, r))
t1 = r["data"]["comment"]["id"]
check("bob 看不到工作簿 → 未通知并说明", r["data"]["notNotified"] == ["Bob"], r["data"])
check("bob 没有收到通知", inbox(Bo)["unread"] == 0, inbox(Bo))

# 共享为只读：bob 能看、能评论（只读也可以评论）
call("PUT", f"/ledger/workbooks/{wb}/share", A, body={"visibility": "view"})
s, r = cm(A, "再 @ 一次 Bob，也 @ 自己", sheetId="s1", row=1, col=2, mentions=[bob, alice]); check("共享后评论", s == 200 and r["data"]["notNotified"] == [], (s, r))
t2 = r["data"]["comment"]["id"]
ib = inbox(Bo); check("bob 收到一条 @ 通知，未读 1", ib["unread"] == 1 and ib["items"][0]["kind"] == "mention" and ib["items"][0]["fromName"] == "Alice", ib)
n = ib["items"][0]
check("通知带位置，可跳转", (n["workbookId"], n["sheetId"], n["row"], n["col"], n["workbookName"]) == (wb, "s1", 1, 2, "评论测试"), n)
check("@ 自己不产生通知", inbox(A)["unread"] == 0, inbox(A))

s, r = call("GET", f"/ledger/workbooks/{wb}/comments", Bo); th = r["data"]
check("bob 读到两条线程（同一格子）", s == 200 and [t["id"] for t in th] == [t1, t2] and all(t["replies"] == [] for t in th), th)
check("评论带 @ 的人", th[1]["mentions"] == [{"id": bob, "name": "Bob"}, {"id": alice, "name": "Alice"}], th[1])

# 回复：通知线程作者；回复里 @ 作者本人只通知一次
s, r = cm(Bo, "收到，马上处理", parentId=t2, mentions=[alice]); check("bob 回复", s == 200, (s, r))
rp = r["data"]["comment"]["id"]
ia = inbox(A); check("alice 只收到一条（@ 和回复不重复）", ia["unread"] == 1 and ia["items"][0]["kind"] == "mention", ia)
s, r = cm(Bo, "补充一句", parentId=t2); rp2 = r["data"]["comment"]["id"]
ia = inbox(A); check("没有 @ 的回复通知线程作者（reply）", ia["unread"] == 2 and ia["items"][0]["kind"] == "reply", ia)
s, r = cm(A, "自己回复自己", parentId=t2); check("线程作者自己回复不通知自己", inbox(A)["unread"] == 2, inbox(A))
s, r = call("GET", f"/ledger/workbooks/{wb}/comments", A); thr = [t for t in r["data"] if t["id"] == t2][0]
check("回复挂在线程下，位置取自线程", len(thr["replies"]) == 3 and all((x["row"], x["col"], x["sheetId"]) == (1, 2, "s1") for x in thr["replies"]), thr["replies"])
s, r = cm(Bo, "回复一条回复", parentId=rp); check("只能回复线程的第一条，不能回复回复", s == 400, (s, r))

# 编辑：只有作者；只通知新增的 @
s, r = call("PUT", f"/ledger/comments/{rp}", A, body={"body": "alice 改 bob 的"}); check("alice 不能改 bob 的评论", s == 403, (s, r))
s, r = call("PUT", f"/ledger/comments/{rp}", Bo, body={"body": "收到，@Carol 也看一下", "mentions": [alice, carol]}); check("bob 改自己的并新增 @carol", s == 200 and r["data"]["comment"]["editedAt"], (s, r))
check("carol 收到通知（工作簿已共享可查看）", inbox(C)["unread"] == 1, inbox(C))
check("原来 @ 过的 alice 没有被重复通知", inbox(A)["unread"] == 2, inbox(A))
call("PUT", f"/ledger/comments/{rp}", Bo, body={"body": "再改一次", "mentions": [alice, carol]})
check("再次编辑不重复通知", inbox(C)["unread"] == 1, inbox(C))

# 解决
s, r = call("POST", f"/ledger/comments/{rp}/resolve", Bo, body={"resolved": True}); check("不能对回复标记已解决", s == 400, (s, r))
s, r = call("POST", f"/ledger/comments/{t2}/resolve", C, body={"resolved": True}); check("carol 标记线程已解决", s == 200 and r["data"]["resolved"] and r["data"]["resolvedBy"] == "Carol", (s, r))
s, r = call("POST", f"/ledger/comments/{t2}/resolve", C, body={"resolved": False}); check("取消已解决", s == 200 and not r["data"]["resolved"] and r["data"]["resolvedBy"] is None, (s, r))

# 校验
s, r = cm(A, "   ", sheetId="s1", row=0, col=0); check("空内容被拒绝", s == 400, (s, r))
s, r = cm(A, "字" * 2001, sheetId="s1", row=0, col=0); check("超过 2000 字被拒绝", s == 400 and "2000" in msg((s, r)), (s, r))
s, r = cm(A, "没选格子"); check("没有位置被拒绝", s == 400, (s, r))
s, r = cm(A, "@ 谁", sheetId="s1", row=0, col=0, mentions=["u_not_exist"]); check("@ 不存在的用户被拒绝", s == 400, (s, r))
s, r = cm(A, "x", parentId="nope"); check("回复不存在的线程", s == 400, (s, r))

# 删除：作者 / 所有者 / 管理员；别人不行
s, r = call("DELETE", f"/ledger/comments/{rp2}", C); check("carol 不能删别人的评论", s == 403, (s, r))
s, r = call("DELETE", f"/ledger/comments/{rp2}", A); check("工作簿所有者可以删 bob 的回复", s == 200 and r["data"] == 1, (s, r))
s, r = call("DELETE", f"/ledger/comments/{t1}", Bo); check("bob 不能删 alice 的线程", s == 403, (s, r))

# 通知已读
ib = inbox(Bo)
s, r = call("POST", "/ledger/notifications/read", A, body={"ids": [ib["items"][0]["id"]]}); check("alice 不能标记 bob 的通知", s == 200 and inbox(Bo)["unread"] == ib["unread"], inbox(Bo))
s, r = call("POST", "/ledger/notifications/read", Bo, body={"ids": [ib["items"][0]["id"]]}); check("bob 标记一条已读", s == 200 and inbox(Bo)["unread"] == ib["unread"] - 1, inbox(Bo))
call("POST", "/ledger/notifications/read", Bo, body={})
check("不带 ids = 全部已读", inbox(Bo)["unread"] == 0 and len(inbox(Bo)["items"]) == len(ib["items"]), inbox(Bo))

# 删除线程：连带回复与通知
before = len(inbox(A)["items"])
s, r = call("DELETE", f"/ledger/comments/{t2}", R); check("管理员删除线程（含 2 条回复，共 3 条）", s == 200 and r["data"] == 3, (s, r))
s, r = call("GET", f"/ledger/workbooks/{wb}/comments", A)
check("线程与回复都没了", [t["id"] for t in r["data"]] == [t1], r["data"])
check("相关通知一并删除", len(inbox(A)["items"]) < before and all(i["commentId"] == t1 for i in inbox(A)["items"] + inbox(Bo)["items"] + inbox(C)["items"]), inbox(A))

# 审计
time.sleep(3)
s, r = call("GET", "/ledger/audit?size=300", R); labels = [x["action"] for x in r["data"]["items"]]
check("审计记录了添加 / 修改 / 删除评论", all(x in labels for x in ("添加评论", "修改评论", "删除评论", "标记评论线程已解决")), sorted(set(labels)))

# 彻底删除工作簿清掉评论与通知
call("DELETE", f"/ledger/workbooks/{wb}", A); call("DELETE", f"/ledger/workbooks/{wb}/purge", A)
check("彻底删除工作簿后通知清空", inbox(A)["items"] == [] and inbox(Bo)["items"] == [] and inbox(C)["items"] == [], (inbox(A), inbox(Bo)))
finish()
