# env: LEDGER_ADMIN_TOKEN=ops-secret-1 LEDGER_BACKUP_S3_BUCKET=data LEDGER_BACKUP_S3_ENDPOINT=http://127.0.0.1:19006 LEDGER_BACKUP_S3_ACCESS_KEY=AKIATEST LEDGER_BACKUP_S3_SECRET_KEY=s3cr3t-key LEDGER_BACKUP_S3_PREFIX=gl-backups LEDGER_BACKUP_S3_KEEP=2 LEDGER_BACKUP_S3_PART_MB=1 LEDGER_BACKUP_S3_VERIFY=download
"""备份上传到对象存储与恢复演练：备份完成后传到 S3（大文件分片上传）、核对大小、只保留最近 N 份、上传失败不影响本地备份但监控指标变红

模拟 S3 服务独立实现签名校验（含请求体的 SHA-256），分片上传的合并会核对每片的 ETag。
"""
from lib import *  # noqa: F403
import os
import re

from mock_s3 import BUCKET, OBJECTS, start, state  # noqa: E402

OPS = {"x-admin-token": "ops-secret-1"}
srv = start(port=19006)

# 造一份有点分量的数据（备份文件超过 1 MB，才会走分片上传）
boundary = "XB"
rows = "\n".join(f"O{i:06d},店{i % 97},{(i * 7919) % 100000}.{i % 100:02d},备注{i * 2654435761 % 1000003}" for i in range(60000))
csv = "单号,店铺,金额,备注\n" + rows + "\n"
mp = (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="big.csv"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
s, r = call("POST", "/ledger/datasets/import/preview", R, raw=mp, ctype=f"multipart/form-data; boundary={boundary}")
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", R, body={"token": p["token"], "mode": "create", "name": "备份测试", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": ["单号"], "fileName": p["fileName"]})
check("准备数据", s == 200, (s, r))


def backup():
    s, r = call("POST", "/admin/backup", headers=OPS)
    return s, (r["data"] if s == 200 else r)


def remote_backups():
    return sorted(k for k in OBJECTS if k.startswith("gl-backups/ledger-"))


s, b = backup()
check("备份成功", s == 200 and b["error"] is None and b["file"], (s, b))
check("已上传：地址是 s3://桶/前缀/文件名，没有上传错误", b["remote"] == f"s3://{BUCKET}/gl-backups/" + os.path.basename(b["file"]) and b["remoteError"] is None, b)
key = b["remote"].removeprefix(f"s3://{BUCKET}/")
check("对象存储上的大小与本地备份文件一致，是 DuckDB 数据文件", key in OBJECTS and len(OBJECTS[key][1]) == b["size"] == os.path.getsize(b["file"]) and OBJECTS[key][1][8:12] == b"DUCK", (key in OBJECTS, b["size"]))
check("文件超过一片（1 MB）：走分片上传并成功合并", b["size"] > 1 << 20 and state["puts"][-1][2].startswith("multipart:") and int(state["puts"][-1][2].split(":")[1]) >= 2, (b["size"], state["puts"]))
check("上传的内容与本地文件逐字节相同", OBJECTS[key][1] == open(b["file"], "rb").read())
check("备份生成后立即验证：用恢复时相同的方式重新打开，数据集和行数都读得出来", b["verify"] and b["verify"]["rows"] == 60000 and b["verify"]["datasets"] >= 1 and not b["verify"]["deep"], b.get("verify"))
check("上传后把备份下载回来验证过（LEDGER_BACKUP_S3_VERIFY=download）", key in state["gets"], state["gets"])
check("不上传连接器密钥 secret.key", not any("secret" in k for k in OBJECTS), list(OBJECTS))
check("签名（含请求体摘要）全部通过", not state["bad"], state["bad"])

# 只保留最近 2 份
import datetime
OBJECTS["gl-backups/notes.txt"] = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=30), b"keep me")
for _ in range(3):
    time.sleep(1.1)
    s, b = backup()
    check("再备份一次并上传", s == 200 and b["remote"] and b["remoteError"] is None, (s, b))
names = remote_backups()
check("对象存储上只保留最近 2 份，更早的被清理", len(names) == 2 and len(state["deleted"]) == 2 and names[-1].endswith(os.path.basename(b["file"])), (names, state["deleted"]))
check("没有残留未完成的分片上传", state["aborted"] == 0, state["aborted"])
check("同前缀下不是备份的对象不受清理影响", "gl-backups/notes.txt" in OBJECTS)

s, text = call("GET", "/admin/metrics", headers=OPS, binary=True)
text = text.decode()
check("监控指标：最近一次备份成功", re.search(r"^gridlake_last_backup_success 1$", text, re.M), text[-300:])

# ── 恢复演练 ──
s, r = call("POST", "/admin/backup/verify", headers=OPS)
check("恢复演练（本地最新备份）通过", s == 200 and r["data"]["rows"] == 60000 and r["data"]["deep"] is False and r["data"]["file"].endswith(".duckdb"), (s, r))
s, r = call("POST", "/admin/backup/verify?full=true", headers=OPS)
check("全量扫描（读出每张表的每一列）通过", s == 200 and r["data"]["deep"] is True and r["data"]["rows"] == 60000, (s, r))
s, r = call("POST", "/admin/backup/verify?source=remote", headers=OPS)
check("恢复演练（对象存储上最新的备份，下载回来验证）通过", s == 200 and r["data"]["file"].startswith(f"s3://{BUCKET}/gl-backups/ledger-") and r["data"]["rows"] == 60000, (s, r))
s, r = call("POST", "/admin/backup/verify?source=nowhere", headers=OPS)
check("source 不合法", s == 400 and "local 或 remote" in msg((s, r)), (s, r))
s, r = call("POST", "/admin/backup/verify", headers={"x-admin-token": "wrong"})
check("需要运维令牌", s == 403, (s, r))
latest = remote_backups()[-1]
good = OBJECTS[latest]
OBJECTS[latest] = (good[0], good[1][: len(good[1]) // 2])
s, r = call("POST", "/admin/backup/verify?source=remote", headers=OPS)
check("对象存储上的备份被截断：演练不通过并说明", s == 400 and "恢复演练未通过" in msg((s, r)), (s, r))
OBJECTS[latest] = (good[0], b"\x00" * len(good[1]))
s, r = call("POST", "/admin/backup/verify?source=remote", headers=OPS)
check("对象存储上的备份内容损坏：同样不通过", s == 400 and "恢复演练未通过" in msg((s, r)), (s, r))
OBJECTS[latest] = good
s, r = call("POST", "/admin/backup/verify?source=remote", headers=OPS)
check("恢复原样后又通过", s == 200, (s, r))
newest_local = sorted(f for f in os.listdir(os.path.dirname(b["file"])) if f.startswith("ledger-") and f.endswith(".duckdb"))[-1]
path = os.path.join(os.path.dirname(b["file"]), newest_local)
with open(path, "rb") as f:
    keep = f.read()
with open(path, "wb") as f:
    f.write(keep[: len(keep) // 3])
s, r = call("POST", "/admin/backup/verify", headers=OPS)
check("本地备份被截断：演练不通过", s == 400 and "恢复演练未通过" in msg((s, r)), (s, r))
with open(path, "wb") as f:
    f.write(keep)

# ── 对象存储不可用：本地备份照常成功，上传失败单独说明，监控指标变红 ──
srv.shutdown()
srv.server_close()
time.sleep(1.1)
s, b = backup()
check("对象存储连不上：本地备份仍然成功", s == 200 and b["error"] is None and os.path.exists(b["file"]) and b["remote"] is None, (s, b))
check("上传失败的原因单独给出", b["remoteError"] and "对象存储" in b["remoteError"], b)
s, text = call("GET", "/admin/metrics", headers=OPS, binary=True)
check("监控指标：最近一次备份标为失败（上传没成功也要报警）", re.search(r"^gridlake_last_backup_success 0$", text.decode(), re.M), text.decode()[-300:])
s, r = call("GET", "/admin/status", headers=OPS)
check("状态里能看到上传结果", s == 200 and r["data"]["lastBackup"]["remoteError"], (s, r))

finish()
