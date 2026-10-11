"""审批通知：文件审批与月结审批的每一步都通知到该处理的人（应用内铃铛）

发起 → 审批人收到「待你审批」；通过 / 驳回 → 发起人收到结果与意见；撤回 → 审批人收到撤回；
月结：提交 → 审批人；锁账 / 驳回 → 提交人；撤回 → 审批人；反结账 → 提交人。
不通知操作人自己（管理员代批时发起人照常收到）；与评论通知共用收件箱、已读状态；失败的操作不留下通知。
"""
from lib import *  # noqa: F403
for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)

s, r = call("GET", "/ledger/users?keyword=", A)
ids = {u["name"]: u["id"] for u in r["data"]}
inbox = lambda tok: call("GET", "/ledger/notifications", tok)[1]["data"]
def latest(tok, kind=None):
    items = [n for n in inbox(tok)["items"] if kind is None or n["kind"] == kind]
    return items[0] if items else None
def count(tok, kind):
    return sum(1 for n in inbox(tok)["items"] if n["kind"] == kind)

wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {"0": {"0": {"v": "利润"}}}}}}).encode())
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("9 月利润表"), A, raw=wbj); wb = r["data"]["id"]

# ── 文件审批 ──
before = inbox(Bo)["unread"]
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Bob"], "comment": "请审批 9 月利润"})
check("Alice 发起审批", s == 200, (s, r)); ap = r["data"]["id"]
n = latest(Bo, "approval_request")
check("审批人 Bob 收到「待你审批」：发起人、工作簿、说明、审批单", n is not None and n["fromName"] == "Alice" and n["workbookId"] == wb and n["workbookName"] == "9 月利润表" and n["text"] == "请审批 9 月利润" and n["refId"] == ap and not n["read"], n)
check("Bob 未读数 +1", inbox(Bo)["unread"] == before + 1, inbox(Bo))
check("发起人自己没有收到", count(A, "approval_request") == 0, inbox(A))
check("无关的人没有收到", count(C, "approval_request") == 0, inbox(C))

s, r = call("POST", f"/ledger/approvals/{ap}/decide", Bo, body={"approve": False, "comment": "成本少了运费"})
check("Bob 驳回", s == 200, (s, r))
n = latest(A, "approval_rejected")
check("发起人 Alice 收到驳回与原因", n is not None and n["fromName"] == "Bob" and n["text"] == "成本少了运费" and n["refId"] == ap, n)
check("审批人自己没有收到结果通知", count(Bo, "approval_rejected") == 0, inbox(Bo))

s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Bob"]}); ap2 = r["data"]["id"]
s, r = call("POST", f"/ledger/approvals/{ap2}/decide", Bo, body={"approve": True})
n = latest(A, "approval_approved")
check("通过：没写意见时写「已通过」", s == 200 and n is not None and n["text"] == "已通过" and n["refId"] == ap2, (s, n))

s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Carol"]}); ap3 = r["data"]["id"]
check("Carol 收到待审批", latest(C, "approval_request")["refId"] == ap3, inbox(C))
s, r = call("POST", f"/ledger/approvals/{ap3}/cancel", A, body={"comment": "换人审批"})
n = latest(C, "approval_cancelled")
check("撤回：审批人 Carol 收到撤回与原因", s == 200 and n is not None and n["text"] == "换人审批" and n["refId"] == ap3, (s, n))

s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Carol"]}); ap4 = r["data"]["id"]
s, r = call("POST", f"/ledger/approvals/{ap4}/decide", R, body={"approve": True, "comment": "代批"})
n = latest(A, "approval_approved")
check("管理员代批：发起人收到，来自 Root，意见标明代为处理", s == 200 and n["refId"] == ap4 and n["fromName"] == "Root" and "代为处理" in n["text"], (s, n))

# 失败的操作不留下通知
c0 = count(C, "approval_request")
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": "不存在", "approverId": ids["Carol"]})
check("发起失败（工作簿不存在）不发通知", s != 200 and count(C, "approval_request") == c0, (s, inbox(C)))
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Bob"]}); ap5 = r["data"]["id"]
a0 = count(A, "approval_rejected")
s, r = call("POST", f"/ledger/approvals/{ap5}/decide", Bo, body={"approve": False, "comment": ""})
check("驳回失败（没写原因）不发通知", s == 400 and count(A, "approval_rejected") == a0, (s, inbox(A)))

# ── 月结 ──
call("POST", "/ledger/shops", A, body={"name": "旗舰店", "status": "active"})
period = "2026-09"
REF = f"close:旗舰店|{period}"
def close(tok, action, **kw):
    return call("POST", "/ledger/close", tok, body={"shop": "旗舰店", "period": period, "action": action, "operator": "x", **kw})
s, r = close(A, "submit", approverId=ids["Bob"], comment="差异已说明", acceptRisk=True)
n = latest(Bo, "close_request")
check("月结提交：审批人 Bob 收到，带店铺月份与说明", s == 200 and n is not None and n["refId"] == REF and "旗舰店 2026-09 月结待审批" in n["text"] and "差异已说明" in n["text"] and n["fromName"] == "Alice" and n["workbookId"] == "", (s, n))
s, r = close(A, "cancel")
check("月结撤回：审批人收到", s == 200 and latest(Bo, "close_cancelled")["refId"] == REF, (s, inbox(Bo)))
s, r = close(A, "submit", approverId=ids["Bob"], comment="再次提交", acceptRisk=True)
s, r = close(Bo, "reject", comment="银行余额对不上")
n = latest(A, "close_rejected")
check("月结驳回：提交人收到原因", s == 200 and n is not None and "银行余额对不上" in n["text"] and n["fromName"] == "Bob", (s, n))
s, r = close(A, "submit", approverId=ids["Bob"], comment="已补", acceptRisk=True)
s, r = close(Bo, "approve", comment="核对无误")
n = latest(A, "close_locked")
check("月结锁账：提交人收到", s == 200 and n is not None and "已通过并锁账" in n["text"] and "核对无误" in n["text"], (s, n))
s, r = close(R, "reopen", comment="补录一笔")
n = latest(A, "close_reopened")
check("反结账：提交人收到，来自管理员", s == 200 and n is not None and "补录一笔" in n["text"] and n["fromName"] == "Root", (s, n))

# ── 与评论通知共用收件箱 ──
ib = inbox(A)
check("审批 / 月结通知都在同一个收件箱里", {"approval_rejected", "approval_approved", "close_rejected", "close_locked", "close_reopened"} <= {n["kind"] for n in ib["items"]}, [n["kind"] for n in ib["items"]])
s, r = call("POST", "/ledger/notifications/read", A, body={})
check("全部标为已读", s == 200 and inbox(A)["unread"] == 0, inbox(A))
finish()
