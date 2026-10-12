#!/usr/bin/env bash
# Gridlake 安装 / 升级（Linux + systemd），在解压后的安装包目录里以 root 执行：
#
#   sudo ./install.sh
#
# 首次安装：创建 gridlake 用户与目录、写入配置模板、注册并启动服务
# 升级：停服务 → 离线备份数据 → 替换程序与页面（保留 /etc/gridlake/ledger.env 与 web/config/index.js）→ 启动 → 健康检查
set -euo pipefail

PKG="$(cd "$(dirname "$0")" && pwd)"
APP=/opt/gridlake
DATA=/var/lib/gridlake
ETC=/etc/gridlake
ENV_FILE="$ETC/ledger.env"
UNIT=/etc/systemd/system/gridlake.service

[[ $EUID -eq 0 ]] || { echo "请用 root 执行：sudo ./install.sh"; exit 1; }
if [[ -f /etc/systemd/system/webexcel.service && ! -f "$UNIT" ]]; then
  echo "检测到旧版本（项目原名 WebExcel）的安装：服务 webexcel、目录 /opt/webexcel、/var/lib/webexcel、/etc/webexcel。"
  echo "新版本改用 gridlake 这个名字，直接安装会再起一个全新的服务，不会沿用旧数据。迁移步骤见《部署手册》的「从 WebExcel 迁移」。"
  exit 1
fi
command -v systemctl >/dev/null || { echo "需要 systemd；容器环境请使用 Dockerfile"; exit 1; }

echo "==> 安装 Gridlake $(cat "$PKG/VERSION")"
id gridlake >/dev/null 2>&1 || useradd --system --home "$DATA" --shell /usr/sbin/nologin gridlake
mkdir -p "$APP/bin" "$DATA" "$ETC"

if [[ ! -f "$ENV_FILE" ]]; then
  cp "$PKG/ledger.env.example" "$ENV_FILE"
  chmod 640 "$ENV_FILE" && chown root:gridlake "$ENV_FILE"
  echo "    已生成配置 $ENV_FILE（内存上限等按服务器调整后 systemctl restart gridlake）"
fi

# 读取配置里的数据文件与端口（健康检查用）
set -a; source "$ENV_FILE"; set +a
PORT="${LEDGER_ADDR##*:}"

# 新版程序先放到 .new：升级前的备份用新版执行（旧版本可能没有 backup 命令），备份成功后再替换
install -m 755 "$PKG/bin/gridlake" "$APP/bin/gridlake.new"
"$APP/bin/gridlake.new" version >/dev/null || { echo "程序无法运行（安装包与服务器的系统 / 架构不匹配？）"; rm -f "$APP/bin/gridlake.new"; exit 1; }

UPGRADE=0
if [[ -f "$UNIT" ]]; then
  UPGRADE=1
  echo "==> 停止服务（等待进行中的请求完成）"
  systemctl stop gridlake
  if [[ -f "${LEDGER_DB:-$DATA/ledger.duckdb}" ]]; then
    echo "==> 升级前备份数据"
    # runuser 保留当前环境变量（上面从 ledger.env 读入的 LEDGER_DB / LEDGER_BACKUP_DIR）
    if ! runuser -u gridlake -- "$APP/bin/gridlake.new" backup; then
      echo "!! 备份失败，已取消升级并恢复启动原服务"
      rm -f "$APP/bin/gridlake.new"
      systemctl start gridlake
      exit 1
    fi
  fi
fi

echo "==> 复制程序与页面"
mv -f "$APP/bin/gridlake.new" "$APP/bin/gridlake"
# 页面整体替换，但保留现场改过的运行时配置 config/index.js
KEEP_CONFIG=""
if [[ -f "$APP/web/config/index.js" ]]; then
  KEEP_CONFIG="$(mktemp)" && cp "$APP/web/config/index.js" "$KEEP_CONFIG"
fi
rm -rf "$APP/web.new" && cp -r "$PKG/web" "$APP/web.new"
if [[ -n "$KEEP_CONFIG" ]]; then
  cp "$KEEP_CONFIG" "$APP/web.new/config/index.js" && rm -f "$KEEP_CONFIG"
  echo "    保留了原有的 web/config/index.js"
fi
rm -rf "$APP/web" && mv "$APP/web.new" "$APP/web"
cp "$PKG/部署手册.md" "$PKG/VERSION" "$APP/"
# 验收测试：cd /opt/gridlake && tests/e2e/run.sh（临时数据库，不影响正在运行的服务与数据）
if [[ -d "$PKG/tests" ]]; then rm -rf "$APP/tests" && cp -r "$PKG/tests" "$APP/tests"; fi
chown -R gridlake:gridlake "$DATA"

cp "$PKG/gridlake.service" "$UNIT"
systemctl daemon-reload
systemctl enable gridlake >/dev/null

echo "==> 启动服务"
systemctl start gridlake
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1; then
    echo "==> 服务已就绪：$(curl -fsS "http://127.0.0.1:$PORT/api/health")"
    [[ $UPGRADE -eq 1 ]] && echo "    升级完成；数据检查：curl http://127.0.0.1:$PORT/api/admin/status"
    [[ $UPGRADE -eq 0 ]] && echo "    首次安装完成：浏览器访问 http://<服务器地址>:$PORT/excel；登录方式见 /etc/gridlake/ledger.env 的 LEDGER_AUTH_MODE"
    exit 0
  fi
  sleep 1
done
echo "!! 服务 60 秒内没有就绪，查看日志：journalctl -u gridlake -n 100 --no-pager"
exit 1
