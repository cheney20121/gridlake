# Gridlake

[中文](README.md) · **English**

**An open-source, self-hosted spreadsheet for 100M-row datasets.** Excel-style formulas, pivot tables and free-form layout in the browser; the big data lives in a server-side [DuckDB](https://github.com/duckdb/duckdb) and the browser only fetches the screen you are looking at. The spreadsheet UI is built on [Univer](https://github.com/dream-num/univer).

> **Language note:** the user interface is available in **Chinese and English** (use the language switch in the top bar, or `?lang=en-US`; the choice is remembered). Most screens and the server's error messages are translated; the e-commerce ledger scenario and some runtime messages are still Chinese, and code comments are Chinese. See [ROADMAP.md](ROADMAP.md). The server, API and deployment docs are Chinese as well; this page is a short English overview.

![A 2M-row dataset opened in the big-data grid](docs/images/big-grid.png)

![Pivot table over 2M rows, computed on the server and written into a normal sheet](docs/images/pivot.png)

## Why

- Excel stops at 1,048,576 rows.
- BI tools are great for fixed dashboards but not for ad-hoc formulas, pivots and free-form layout.
- Database-style tables (Airtable, NocoDB, Teable…) are one-type-per-column record stores, not free-form spreadsheets.

Gridlake keeps the **Excel-style interface** and pushes everything that touches a large dataset — pivots, formulas, data tables, charts — down to DuckDB on the server.

## Performance

Measured on 100M rows × 8 columns of synthetic order data (MacBook, 8 cores, 16 GB, release build, over HTTP). Slow cases are listed too; how to reproduce is in `server/tests/bench/README.md`.

| Operation | Time |
|---|---|
| Open the whole table | 52 ms |
| Filter by shop (330k matching rows) | ~0.1 s |
| Pivot: shop × month | 1.8 s |
| SQL group-by | 0.5 s |
| `DSUMIFS` formula, two conditions | 0.17 s |
| Export 330k rows to CSV | 1.8 s |
| *Cold* full-table sort | 11.6 – 18 s (3 ms afterwards) |
| Exact distinct count over 100M unique values | 17 – 44 s (use the approximate variant) |

## Features

- **Large datasets** — import CSV / Excel / Parquet / JSON; connect ClickHouse, MySQL / PostgreSQL, HTTP APIs; filter, sort, search, computed columns, joined views, trash.
- **Pivot tables** — computed on the server and written as normal sheets; calculated values and items, share / YoY / MoM, approximate quantiles, drill-down.
- **Formula push-down** — `=SUMIFS(Orders[Amount], Orders[Shop], B2)` runs on the server; platform functions such as `DSUMIFS`; exported to native Excel formulas.
- **SQL workbench** — read-only SQL (checked against a parser-based allow-list), typed parameters, save results as datasets, scheduled refresh.
- **Dashboards** — slicers, KPI cards, linked charts, presentation mode.
- **AI assistant** — bring your own OpenAI-compatible model; proposals are applied only after confirmation and can be undone; read-only MCP server for external AI clients.
- **Collaboration & governance** — real-time editing, comments, four login modes, audit log, row-level data scope, approvals, in-app notifications.
- **Other** — xlsx import/export, templates, scheduled reports (WeCom / DingTalk / Feishu / email), version history, backups.

## Quick start

```bash
git clone <this repository> && cd gridlake
mkdir -p data && sudo chown -R 10001:10001 data
docker compose -f deploy/docker-compose.yml up -d
docker logs gridlake 2>&1 | grep 初始密码      # the generated initial admin password
```

Open `http://localhost:8787/excel` and sign in as `admin`. To try it with sample data, log in via the API to get a token and run
`scripts/seed-demo.py --url http://localhost:8787 --token "$TOKEN" --rows 2000000` (see the Chinese README for the exact commands).

Build from source: Rust 1.85+, Node 20+ with pnpm, a C++ compiler (DuckDB is bundled; the first build takes 5–15 minutes).

```bash
cd server && cargo run                 # backend on http://127.0.0.1:8787
cd web && pnpm install && pnpm dev  # frontend on http://localhost:5173/excel/home
```

## Security defaults

`LEDGER_AUTH_MODE` is `off` by default, which is meant for local trials only: the server **refuses to start** if login is off while it listens on a non-loopback address (or `LEDGER_CORS=*`), unless you explicitly set `LEDGER_ALLOW_NO_AUTH=1`. For real deployments use `local` (built-in accounts), `gateway` or `proxy`. See [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## Status and limitations

- Single-node DuckDB, no distributed mode; a billion rows has not been tested.
- The English UI is partial: parts of the e-commerce ledger scenario and some runtime messages are still Chinese.
- AI assistant quality depends on your model and has not been systematically evaluated.
- Not yet run long-term in Linux production — feedback is welcome.

## Contributing & license

See [CONTRIBUTING.md](CONTRIBUTING.md), [ROADMAP.md](ROADMAP.md) and [CHANGELOG.md](CHANGELOG.md). Licensed under [Apache-2.0](LICENSE); third-party licenses are listed in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
