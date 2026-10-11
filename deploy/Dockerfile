# WebExcel 镜像：前端页面 + 表格服务（单进程，数据在 /data 卷）
#
#   docker build -f deploy/Dockerfile -t webexcel:0.1.0 .        # 在 webexcel 仓库根目录执行
#   docker compose -f deploy/docker-compose.yml up -d
#
# 换依赖源（如国内镜像）：--build-arg NPM_REGISTRY=https://registry.npmmirror.com --build-arg CARGO_REGISTRY=sparse+https://mirrors.ustc.edu.cn/crates.io-index/

FROM node:22-bookworm-slim AS web
ARG NPM_REGISTRY=https://registry.npmjs.org
WORKDIR /src
RUN corepack enable
COPY demo-web-vue3/package.json demo-web-vue3/pnpm-lock.yaml ./
RUN pnpm config set registry "$NPM_REGISTRY" && pnpm install --frozen-lockfile
COPY demo-web-vue3/ ./
RUN node_modules/.bin/vite build --outDir /out/web --emptyOutDir \
 && find /out/web -type f \( -name '*.js' -o -name '*.css' -o -name '*.html' -o -name '*.svg' -o -name '*.json' -o -name '*.wasm' \) \
      -size +1k ! -path '*/config/*' -exec gzip -k -9 -f {} \;

FROM rust:1.98-bookworm AS server
ARG CARGO_REGISTRY=
WORKDIR /src
COPY ledger-server/ ./
RUN if [ -n "$CARGO_REGISTRY" ]; then \
      mkdir -p .cargo && printf '[source.crates-io]\nreplace-with = "mirror"\n[source.mirror]\nregistry = "%s"\n' "$CARGO_REGISTRY" > .cargo/config.toml; \
    fi \
 && cargo build --release --locked \
 && strip target/release/ledger-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl tzdata \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /data webexcel \
 && mkdir -p /data && chown webexcel:webexcel /data
COPY --from=server /src/target/release/ledger-server /app/bin/ledger-server
COPY --from=web /out/web /app/web
COPY LICENSE NOTICE THIRD_PARTY_LICENSES.md /app/licenses/
ENV TZ=Asia/Shanghai \
    LEDGER_ADDR=0.0.0.0:8787 \
    LEDGER_DB=/data/ledger.duckdb \
    LEDGER_WEB_DIR=/app/web \
    LEDGER_BACKUP_DIR=/data/backups \
    RUST_LOG=ledger_server=info,tower_http=warn
USER webexcel
WORKDIR /data
VOLUME /data
EXPOSE 8787
# 停止时 SIGTERM：等进行中的请求完成并写回主文件（docker stop 默认只等 10 秒，compose 里已调大）
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=60s --retries=3 CMD curl -fsS http://127.0.0.1:8787/api/health || exit 1
ENTRYPOINT ["/app/bin/ledger-server"]
