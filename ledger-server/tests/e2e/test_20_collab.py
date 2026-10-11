"""N10：实时协作的服务端中转（SSE 下行 + POST 上行）

房间与在线状态、变更的全局序号与回显、变更日志（新加入的人补齐）、保存时按序号截断日志并通知所有人、
别的方式写入时通知重新载入、权限（只读用户不能发变更、不能冒充别人的连接）、日志过大时要求保存并让新加入的人等待、断开通知。
"""
import queue
import threading

from lib import *  # noqa: F403
for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)

wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("协作测试"), A, raw=wbj); wb = r["data"]["id"]; v0 = r["data"]["version"]
call("PUT", f"/ledger/workbooks/{wb}/share", A, body={"visibility": "edit"})


class Conn:
    """一个 SSE 连接：后台线程读事件放进队列"""
    def __init__(self, tok, cid, version=None, wid=None):
        self.cid, self.tok, self.wid = cid, tok, wid or wb
        self.q = queue.Queue()
        url = f"{BASE}/ledger/collab/{self.wid}/events?client={cid}&version={v0 if version is None else version}"
        req = urllib.request.Request(url, headers={"x-user-token": tok, "Accept": "text/event-stream"})
        try:
            self.resp = urllib.request.urlopen(req, timeout=30)
        except urllib.error.HTTPError as e:
            self.status = e.code; self.resp = None; return
        self.status = 200
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        try:
            for line in self.resp:
                line = line.decode().strip()
                if line.startswith("data:"):
                    self.q.put(json.loads(line[5:]))
        except Exception:
            pass

    def get(self, typ=None, timeout=5):
        end = time.time() + timeout
        while time.time() < end:
            try:
                m = self.q.get(timeout=max(0.05, end - time.time()))
            except queue.Empty:
                break
            if typ is None or m.get("type") == typ:
                return m
        return None

    def send(self, kind, **kw):
        return call("POST", f"/ledger/collab/{self.wid}/send", self.tok, body={"client": self.cid, "kind": kind, **kw})

    def close(self):
        if self.resp: self.resp.close()


a = Conn(A, "ca1"); check("alice 连接", a.status == 200, a.status)
h = a.get("hello"); check("hello：房间是新的，没有别人，基准版本取自客户端", h and h["peers"] == [] and h["baseVersion"] == v0 and h["self"]["name"] == "Alice" and h["self"]["canEdit"] is True and h["log"] == [] and h["needSave"] is False, h)
b = Conn(Bo, "cb1"); hb = b.get("hello")
check("bob 加入：hello 里有 alice", hb and [p["name"] for p in hb["peers"]] == ["Alice"] and hb["self"]["color"] != h["self"]["color"], hb)
j = a.get("join"); check("alice 收到 bob 加入", j and j["peer"]["name"] == "Bob", j)

# 变更：所有人（含发送者）按同一顺序收到，带全局序号
s, r = a.send("ops", batchId="b1", ops=[{"id": "sheet.mutation.set-range-values", "params": {"cellValue": {"0": {"0": {"v": 1}}}}}])
check("alice 发一批变更", s == 200 and r["data"]["seq"] == 1, (s, r))
ma, mb = a.get("ops"), b.get("ops")
check("发送者收到回显，bob 收到变更，序号一致", ma and mb and ma["seq"] == mb["seq"] == 1 and ma["from"] == "ca1" and mb["batchId"] == "b1" and mb["ops"][0]["id"] == "sheet.mutation.set-range-values", (ma, mb))
s, r = b.send("ops", batchId="b2", ops=[{"id": "x", "params": {}}]); a.get("ops"); b.get("ops")
check("bob 的变更是序号 2", s == 200 and r["data"]["seq"] == 2, (s, r))

# 在线状态：转发给别人，不回显给自己
b.send("presence", presence={"sheetId": "s1", "row": 3, "col": 4})
p = a.get("presence"); check("alice 收到 bob 的光标", p and p["from"] == "cb1" and p["presence"]["row"] == 3, p)
check("bob 不会收到自己的光标", b.get("presence", timeout=0.5) is None)

# 新加入的人：hello 里带着还没保存的变更日志
c = Conn(R, "cr1"); hc = c.get("hello")
check("新加入的人拿到日志（序号 1、2）", hc and [e["seq"] for e in hc["log"]] == [1, 2] and hc["seq"] == 2 and len(hc["peers"]) == 2, hc)
a.get("join"); b.get("join")

# 保存：带 x-collab-seq 的保存截断日志并通知所有人
content = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {"0": {"0": {"v": 1}}}}}}).encode())
s, r = call("PUT", f"/ledger/workbooks/{wb}/content?baseVersion={v0}&operator=Alice", A, raw=content, headers={"x-collab-seq": "1"}); v1 = r["data"]["version"]
check("alice 保存（快照已含序号 1）", s == 200 and r["data"]["saved"], (s, r))
sv = b.get("saved"); check("bob 收到「已保存」与版本号", sv and sv["version"] == v1 and sv["seq"] == 1 and sv["by"] == "Alice", sv)
d = Conn(A, "ca2", version=v1); hd = d.get("hello")
check("之后加入的人：基准版本是新版本，日志只剩序号 2", hd and hd["baseVersion"] == v1 and [e["seq"] for e in hd["log"]] == [2], hd)
for x in (a, b, c): x.get("join")

# 别的方式写入（不带 x-collab-seq）：房间里的人需要重新载入，日志清空
s, r = call("PUT", f"/ledger/workbooks/{wb}/content?baseVersion={v1}&operator=Root", R, raw=content); v2 = r["data"]["version"]
rp = a.get("replaced"); check("不带序号的保存 → 通知重新载入", rp and rp["version"] == v2 and rp["by"] == "Root", rp)
e = Conn(A, "ca3", version=v2); he = e.get("hello"); check("日志已清空", he and he["log"] == [] and he["baseVersion"] == v2, he)
s, r = call("POST", f"/ledger/workbooks/{wb}/restore", A, body={"version": v0, "operator": "Alice"}); check("恢复历史版本", s == 200, (s, r))
check("恢复历史版本同样通知重新载入", b.get("replaced") is not None)

# 权限
call("PUT", f"/ledger/workbooks/{wb}/share", A, body={"visibility": "view"})
cv = Conn(C, "cc1"); hv = cv.get("hello")
check("只读用户可以连接，但 canEdit 为假", hv and hv["self"]["canEdit"] is False, hv)
s, r = cv.send("ops", batchId="x", ops=[{"id": "x", "params": {}}]); check("只读用户不能发变更", s == 403, (s, r))
s, r = cv.send("presence", presence={"row": 1}); check("只读用户可以发光标", s == 200, (s, r))
check("别人收到只读用户的光标", a.get("presence") is not None)
s, r = call("POST", f"/ledger/collab/{wb}/send", Bo, body={"client": "ca1", "kind": "presence", "presence": {}}); check("不能冒充别人的连接", s == 403, (s, r))
s, r = call("POST", f"/ledger/collab/{wb}/send", A, body={"client": "nobody", "kind": "presence", "presence": {}}); check("没有连接的客户端不能发", s == 400, (s, r))
s, r = a.send("whatever"); check("未知类型被拒绝", s == 400, (s, r))
s, r = call("POST", f"/ledger/collab/{wb}/send", A, raw=b"x" * (1024 * 1024 + 1), ctype="application/json"); check("超过 1 MB 的一批变更被拒绝", s == 400, (s, r))
call("PUT", f"/ledger/workbooks/{wb}/share", A, body={"visibility": "private"})
pv = Conn(Bo, "cb9"); check("私有工作簿：bob 不能加入", pv.status == 403, pv.status)
bad = Conn(A, "bad%20id!"); check("client 标识不合法被拒绝", bad.status == 400, bad.status)

# 断开：其他人收到 leave，最后一个人走了房间就没了
call("PUT", f"/ledger/workbooks/{wb}/share", A, body={"visibility": "edit"})
b.close()
lv = a.get("leave", timeout=8); check("bob 断开 → alice 收到离开", lv and lv["id"] == "cb1", lv)

# 日志过大：清空日志、要求保存；等待保存期间加入的人 needSave
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("大日志"), A, raw=wbj); wb2 = r["data"]["id"]; w2v = r["data"]["version"]
call("PUT", f"/ledger/workbooks/{wb2}/share", A, body={"visibility": "edit"})
big1 = Conn(A, "big1", version=w2v, wid=wb2); big1.get("hello")
payload = [{"id": "sheet.mutation.set-range-values", "params": {"pad": "x" * 900_000}}]
last = 0
for i in range(75):
    s, r = big1.send("ops", batchId=f"p{i}", ops=payload)
    if s != 200:
        break
    last = r["data"]["seq"]
rq = big1.get("request-save", timeout=10); check("日志超过上限 → 要求在线的人保存", rq is not None, last)
late = Conn(Bo, "late1", version=w2v, wid=wb2); hl = late.get("hello")
check("等待保存期间加入的人：needSave 且日志为空", hl and hl["needSave"] is True and hl["log"] == [], hl and {k: hl[k] for k in ("needSave", "seq")})
rq2 = big1.get("request-save", timeout=3); check("新加入的人也触发了一次保存请求", rq2 is not None)
s, r = call("PUT", f"/ledger/workbooks/{wb2}/content?baseVersion={w2v}&operator=Alice", A, raw=content, headers={"x-collab-seq": str(last)}); w2v1 = r["data"]["version"]
check("保存后日志恢复完整", late.get("saved") is not None)
nw = Conn(Bo, "late2", version=w2v1, wid=wb2); hn = nw.get("hello"); check("之后加入的人不再需要等待", hn and hn["needSave"] is False and hn["baseVersion"] == w2v1, hn and {k: hn[k] for k in ("needSave", "baseVersion")})
finish()
