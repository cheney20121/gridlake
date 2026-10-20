"""模拟 S3 服务（路径风格，ListObjectsV2 每页 3 个对象），用 Python 独立实现 AWS Signature V4 校验。只用于测试。

用法：python3 mock_s3.py [端口，默认 19004]  —— 桶 data，Access Key AKIATEST，Secret Key s3cr3t-key，区域 us-east-1
也可以被 test_44_s3.py 直接导入，共享 OBJECTS / state。
"""
import datetime
import os
import ssl
import hashlib
import hmac
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 and __name__ == "__main__" else 19004
BUCKET, ACCESS, SECRET, REGION = "data", "AKIATEST", "s3cr3t-key", "us-east-1"
now = datetime.datetime.now(datetime.timezone.utc)


def ago(**kw):
    return now - datetime.timedelta(**kw)


def csv(*rows):
    return ("order_id,amount\n" + "".join(f"{o},{a}\n" for o, a in rows)).encode()


OBJECTS = {
    "reports/2026-10-01.csv": (ago(days=4), csv(("O1", 100), ("O2", 200))),
    "reports/2026-10-02.csv": (ago(days=3), csv(("O3", 300))),
    "reports/2026-10-03.csv": (ago(days=2), csv(("O4", 400), ("O5", 500))),
    "reports/中文 报表.csv": (ago(days=1), csv(("O6", 600))),
    "reports/readme.txt": (ago(days=5), b"not a table"),
    "reports/~$lock.csv": (ago(days=5), b"x"),
    "reports/sub/2026-10-09.csv": (ago(hours=5), csv(("O7", 700))),
    "reports/just-uploaded.csv": (ago(seconds=10), csv(("O8", 800))),
    "other/x.csv": (ago(days=1), csv(("X", 1))),
}
state = {"bad": [], "list_calls": 0, "gets": [], "puts": [], "deleted": [], "aborted": 0}
UPLOADS = {}


def enc(s, slash=False):
    return urllib.parse.quote(s, safe="-_.~" + ("/" if slash else ""))


def expected_signature(method, raw_path, raw_query, headers, signed, amz_date, payload_hash):
    q = sorted((enc(urllib.parse.unquote(k)), enc(urllib.parse.unquote(v))) for k, _, v in (p.partition("=") for p in raw_query.split("&") if p))
    canon_q = "&".join(f"{k}={v}" for k, v in q)
    canon_h = "".join(f"{h}:{headers[h].strip()}\n" for h in signed)
    canonical = f"{method}\n{raw_path}\n{canon_q}\n{canon_h}\n{';'.join(signed)}\n{payload_hash}"
    scope = f"{amz_date[:8]}/{REGION}/s3/aws4_request"
    to_sign = f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashlib.sha256(canonical.encode()).hexdigest()}"
    k = ("AWS4" + SECRET).encode()
    for part in (amz_date[:8], REGION, "s3", "aws4_request"):
        k = hmac.new(k, part.encode(), hashlib.sha256).digest()
    return hmac.new(k, to_sign.encode(), hashlib.sha256).hexdigest()


def xml_error(code, msg):
    return f"<?xml version='1.0'?><Error><Code>{code}</Code><Message>{msg}</Message></Error>".encode()


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, status, body, ctype="application/xml", head=False, etag=None):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        if etag:
            self.send_header("ETag", etag)
        self.end_headers()
        if not head:
            self.wfile.write(body)

    def do_GET(self):
        self.dispatch("GET")

    def do_HEAD(self):
        self.dispatch("HEAD")

    def do_PUT(self):
        self.dispatch("PUT")

    def do_POST(self):
        self.dispatch("POST")

    def do_DELETE(self):
        self.dispatch("DELETE")

    def dispatch(self, method):
        raw_path, _, raw_query = self.path.partition("?")
        hdr = {k.lower(): v for k, v in self.headers.items()}
        body = self.rfile.read(int(hdr.get("content-length", 0)))
        auth = hdr.get("authorization", "")
        try:
            fields = dict(p.strip().split("=", 1) for p in auth[len("AWS4-HMAC-SHA256 "):].split(","))
            access = fields["Credential"].split("/")[0]
            signed = fields["SignedHeaders"].split(";")
            want = expected_signature(method, raw_path, raw_query, hdr, signed, hdr["x-amz-date"], hdr["x-amz-content-sha256"])
            ok = access == ACCESS and want == fields["Signature"]
            if ok and hdr["x-amz-content-sha256"] != hashlib.sha256(body).hexdigest():
                ok, want = False, "payload hash does not match the body"
        except Exception as e:  # noqa: BLE001
            ok = False
            want = repr(e)
        head = method == "HEAD"
        if not ok:
            if access_ok(auth):
                state["bad"].append((self.path, want))
            return self._send(403, xml_error("SignatureDoesNotMatch", "The request signature we calculated does not match the signature you provided."), head=head)
        path = urllib.parse.unquote(raw_path)
        bucket, _, key = path.lstrip("/").partition("/")
        if bucket != BUCKET:
            return self._send(404, xml_error("NoSuchBucket", "The specified bucket does not exist"), head=head)
        q = {k: urllib.parse.unquote(v) for k, _, v in (p.partition("=") for p in raw_query.split("&") if p)}
        if method == "GET" and not key:
            return self.list(q)
        if method in ("GET", "HEAD"):
            if key not in OBJECTS:
                return self._send(404, xml_error("NoSuchKey", "The specified key does not exist."), head=head)
            if method == "GET":
                state["gets"].append(key)
            return self._send(200, OBJECTS[key][1], "text/csv", head=head)
        if method == "PUT" and "partNumber" in q:
            UPLOADS[q["uploadId"]]["parts"][int(q["partNumber"])] = body
            return self._send(200, b"", etag=f'"{hashlib.md5(body).hexdigest()}"')
        if method == "PUT":
            OBJECTS[key] = (datetime.datetime.now(datetime.timezone.utc), body)
            state["puts"].append((key, len(body), "single"))
            return self._send(200, b"", etag=f'"{hashlib.md5(body).hexdigest()}"')
        if method == "POST" and "uploads" in q:
            uid = f"upload-{len(UPLOADS) + 1}"
            UPLOADS[uid] = {"key": key, "parts": {}}
            return self._send(200, f"<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key><UploadId>{uid}</UploadId></InitiateMultipartUploadResult>".encode())
        if method == "POST" and "uploadId" in q:
            up = UPLOADS.get(q["uploadId"])
            if not up:
                return self._send(404, xml_error("NoSuchUpload", "The specified upload does not exist."))
            import re
            listed = re.findall(r"<PartNumber>(\d+)</PartNumber><ETag>(.*?)</ETag>", body.decode())
            nums = [int(n) for n, _ in listed]
            if nums != sorted(up["parts"]) or any(e.replace("&quot;", '"') != f'"{hashlib.md5(up["parts"][int(n)]).hexdigest()}"' for n, e in listed):
                return self._send(400, xml_error("InvalidPart", "One or more of the specified parts could not be found or the ETag does not match."))
            data = b"".join(up["parts"][n] for n in nums)
            OBJECTS[up["key"]] = (datetime.datetime.now(datetime.timezone.utc), data)
            state["puts"].append((up["key"], len(data), f"multipart:{len(nums)}"))
            del UPLOADS[q["uploadId"]]
            return self._send(200, f"<CompleteMultipartUploadResult><Key>{up['key']}</Key></CompleteMultipartUploadResult>".encode())
        if method == "DELETE" and "uploadId" in q:
            UPLOADS.pop(q["uploadId"], None)
            state["aborted"] += 1
            return self._send(204, b"")
        if method == "DELETE":
            OBJECTS.pop(key, None)
            state["deleted"].append(key)
            return self._send(204, b"")
        self._send(405, xml_error("MethodNotAllowed", "no"))

    def list(self, q):
        state["list_calls"] += 1
        prefix, delim = q.get("prefix", ""), q.get("delimiter")
        keys = sorted(k for k in OBJECTS if k.startswith(prefix))
        if delim:
            keys = [k for k in keys if delim not in k[len(prefix):]]
        start = int(q["continuation-token"].removeprefix("tok")) if "continuation-token" in q else 0
        page, more = keys[start:start + 3], start + 3 < len(keys)
        items = "".join(
            f"<Contents><Key>{k.replace('&', '&amp;')}</Key><LastModified>{OBJECTS[k][0].strftime('%Y-%m-%dT%H:%M:%S.000Z')}</LastModified><ETag>&quot;e&quot;</ETag><Size>{len(OBJECTS[k][1])}</Size></Contents>"
            for k in page
        )
        nxt = f"<NextContinuationToken>tok{start + 3}</NextContinuationToken>" if more else ""
        self._send(200, f"<?xml version='1.0'?><ListBucketResult><Name>{BUCKET}</Name><IsTruncated>{'true' if more else 'false'}</IsTruncated>{nxt}{items}</ListBucketResult>".encode())


def access_ok(auth):
    return f"Credential={ACCESS}/" in auth


def start(port=None, tls=False):
    """后台起服务；tls=True 时用 run.sh 生成的测试证书（LEDGER_E2E_TLS_DIR）提供 HTTPS"""
    srv = ThreadingHTTPServer(("127.0.0.1", port or PORT), H)
    if tls:
        d = os.environ["LEDGER_E2E_TLS_DIR"]
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(os.path.join(d, "server.pem"), os.path.join(d, "server.key"))
        srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
