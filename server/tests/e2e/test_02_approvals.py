"""N2 文件审批与月结审批

审批人从登录过的用户里选；只有审批人能审批、发起人不能自批；审批版本保留；管理员代批留痕；月结三种视角的 can。
"""
from lib import *  # noqa: F403

# 用户目录：登录过（任何请求）才会出现
for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
s, r = call("GET", "/ledger/users?keyword=", A)
names = sorted(u["name"] for u in r["data"]); check("用户目录含四人", names == ["Alice", "Bob", "Carol", "Root"], names)
ids = {u["name"]: u["id"] for u in r["data"]}
s, r = call("GET", "/ledger/users?keyword=bo", A); check("按名字搜索", [u["name"] for u in r["data"]] == ["Bob"], r)

# 文件审批
wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {"0": {"0": {"v": "利润"}}}}}}).encode())
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("月度利润表"), A, raw=wbj); wb = r["data"]["id"]
s, r = call("GET", f"/ledger/workbooks/{wb}/content", Bo); check("审批前 Bob 看不到 Alice 的私有工作簿", s == 403)
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Alice"]}); check("不能选自己当审批人", s == 400 and "自己" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": "nobody"}); check("审批人必须登录过", s == 400, (s, r))
s, r = call("POST", "/ledger/approvals", C, body={"workbookId": wb, "approverId": ids["Bob"]}); check("无权访问的工作簿不能发起审批", s == 403, (s, r))
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Bob"], "comment": "请审批 9 月利润", "summary": {"rows": 12}, "operator": "冒充"})
check("Alice 发起审批", s == 200 and r["data"]["status"] == "pending" and r["data"]["initiatorName"] == "Alice" and r["data"]["approverName"] == "Bob", (s, r))
ap = r["data"]["id"]; check("记录审批版本", r["data"]["workbookVersion"] == 1 and r["data"]["currentVersion"] == 1)
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Carol"]}); check("同一工作簿不能重复发起", s == 400 and "已有一张审批中" in msg((s, r)), (s, r))
s, r = call("GET", f"/ledger/workbooks/{wb}/content", Bo); check("审批人自动获得查看权限", s == 200, (s, msg((s, r))))
s, r = call("PUT", f"/ledger/workbooks/{wb}/content?baseVersion=1", Bo, raw=wbj); check("审批人只能看不能改", s == 403, (s, r))
s, r = call("GET", f"/ledger/workbooks/{wb}/content", C); check("无关的 Carol 仍看不到", s == 403)
s, r = call("GET", "/ledger/approvals/count", Bo); check("Bob 待办 1", r["data"]["todo"] == 1, r)
s, r = call("GET", "/ledger/approvals/count", C); check("Carol 待办 0", r["data"]["todo"] == 0, r)
s, r = call("GET", "/ledger/approvals?box=todo", Bo); check("Bob 待我审批列表可处理", len(r["data"]) == 1 and r["data"][0]["canDecide"] and not r["data"][0]["canCancel"], r)
s, r = call("GET", "/ledger/approvals?box=started", A); check("Alice 我发起的可撤回不可审批", len(r["data"]) == 1 and r["data"][0]["canCancel"] and not r["data"][0]["canDecide"], r)
s, r = call("GET", f"/ledger/approvals/{ap}", C); check("Carol 看不到审批单", s == 403, (s, r))
s, r = call("GET", "/ledger/approvals?box=all", C); check("普通用户不能看全部", s == 403)
s, r = call("GET", "/ledger/approvals?box=all", R); check("管理员看全部", s == 200 and len(r["data"]) == 1)
s, r = call("POST", f"/ledger/approvals/{ap}/decide", A, body={"approve": True}); check("发起人不能审批自己的", s == 403 and "自己" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/approvals/{ap}/decide", C, body={"approve": True}); check("无关的人不能审批", s == 403, (s, r))
s, r = call("POST", f"/ledger/approvals/{ap}/decide", Bo, body={"approve": False, "comment": ""}); check("驳回必须填原因", s == 400, (s, r))
s, r = call("POST", f"/ledger/approvals/{ap}/decide", Bo, body={"approve": True, "comment": "同意", "operator": "冒充"})
check("Bob 通过", s == 200 and r["data"]["status"] == "approved" and r["data"]["decidedBy"] == "Bob", (s, r))
check("历史含发起与通过", [e["action"] for e in r["data"]["events"]] == ["submit", "approve"] and r["data"]["events"][1]["userName"] == "Bob")
s, r = call("POST", f"/ledger/approvals/{ap}/decide", Bo, body={"approve": True}); check("已处理不能再处理", s == 400, (s, r))
s, r = call("GET", f"/ledger/workbooks/{wb}/content", Bo); check("处理后审批人仍能复核", s == 200)
s, r = call("GET", f"/ledger/workbooks/{wb}/versions", A); check("审批版本被永久保留", any(v["version"] == 1 and not v["isAuto"] for v in r["data"]), r)
# 撤回与管理员代审
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Carol"]}); ap2 = r["data"]["id"]
s, r = call("POST", f"/ledger/approvals/{ap2}/cancel", C, body={}); check("非发起人不能撤回", s == 403)
s, r = call("POST", f"/ledger/approvals/{ap2}/cancel", A, body={"comment": "换人"}); check("发起人撤回", s == 200 and r["data"]["status"] == "cancelled", (s, r))
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": wb, "approverId": ids["Carol"]}); ap3 = r["data"]["id"]
s, r = call("POST", f"/ledger/approvals/{ap3}/decide", R, body={"approve": True, "comment": "代批"})
check("管理员代为审批并留痕", s == 200 and r["data"]["decisionComment"].startswith("（管理员代为处理）"), (s, r))

# 月结审批
call("POST", "/ledger/shops", A, body={"name": "旗舰店", "status": "active"})
period = "2026-09"
def close(tok, action, **kw):
    return call("POST", "/ledger/close", tok, body={"shop": "旗舰店", "period": period, "action": action, "operator": "冒充", **kw})
s, r = close(A, "submit", comment="差异说明", acceptRisk=True); check("月结提交必须选审批人", s == 400 and "审批人" in msg((s, r)), (s, r))
s, r = close(A, "submit", approverId=ids["Alice"], comment="x", acceptRisk=True); check("月结不能选自己为审批人", s == 400 and "自己" in msg((s, r)), (s, r))
s, r = close(A, "submit", approverId=ids["Bob"], comment="差异说明", acceptRisk=True); check("Alice 提交月结", s == 200, (s, r))
s, r = call("GET", f"/ledger/close?period={period}", A)
row = [x for x in r["data"]["shops"] if x["shop"] == "旗舰店"]
check("提交人视角：可撤回，不可审批", row and row[0]["can"]["cancel"] and not row[0]["can"]["decide"] and "自己" in (row[0]["can"].get("decideBlocked") or ""), row and row[0]["can"])
s, r = call("GET", f"/ledger/close?period={period}", Bo); row = [x for x in r["data"]["shops"] if x["shop"] == "旗舰店"]
check("审批人视角：可审批", row and row[0]["can"]["decide"], row and row[0]["can"])
check("记录里审批人是 Bob，提交人是 Alice", row[0]["record"]["approver"] == "Bob" and row[0]["record"]["submittedBy"] == "Alice")
s, r = call("GET", f"/ledger/close?period={period}", C); row = [x for x in r["data"]["shops"] if x["shop"] == "旗舰店"]
check("无关用户视角：都不可", not any(row[0]["can"][k] for k in ("decide", "cancel", "reopen")) and "Bob" in (row[0]["can"].get("decideBlocked") or ""), row[0]["can"])
s, r = close(A, "approve"); check("提交人不能自批", s == 403, (s, r))
s, r = close(C, "approve"); check("无关的人不能批", s == 403, (s, r))
s, r = close(C, "cancel"); check("无关的人不能撤回", s == 403, (s, r))
s, r = close(Bo, "reject", comment=""); check("驳回要写原因", s == 400, (s, r))
s, r = close(Bo, "approve", comment="核对无误"); check("Bob 审批通过并锁账", s == 200, (s, r))
s, r = call("GET", f"/ledger/close?period={period}", Bo); row = [x for x in r["data"]["shops"] if x["shop"] == "旗舰店"]
check("已锁账：审批人可反结账", row[0]["status"] == "locked" and row[0]["can"]["reopen"] and row[0]["record"]["decidedBy"] == "Bob", row[0])
s, r = close(C, "reopen", comment="想改"); check("无关的人不能反结账", s == 403, (s, r))
s, r = close(A, "reopen", comment="想改"); check("提交人也不能反结账", s == 403, (s, r))
s, r = close(R, "reopen", comment="管理员反结账"); check("管理员可以反结账", s == 200, (s, r))
s, r = call("GET", f"/ledger/close/logs?period={period}", A); acts = [(l["action"], l["operator"]) for l in r["data"]]
check("月结日志记真实账号", ("submit", "Alice") in acts and ("approve", "Bob") in acts and ("reopen", "Root") in acts and not any(o == "冒充" for _, o in acts), acts)
# 审计
time.sleep(3)
s, r = call("GET", "/ledger/audit?size=200&action=" + urllib.parse.quote("审批"), R); acts = [(i["userName"], i["action"], i["status"]) for i in r["data"]["items"]]
check("审计含发起 / 被拒的审批 / 通过", ("Alice", "发起文件审批", 200) in acts and ("Alice", "处理文件审批", 403) in acts and ("Bob", "处理文件审批", 200) in acts, acts[:6])

finish()
