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
- **Subtotals** — group by a column with a subtotal row after each group (same functions as the totals row), even on tens of millions of rows.
- **Totals row** — a footer showing each column's sum, average, median, earliest / latest, distinct count and more over all rows matching the current filters, computed on the server in one query.
- **Split / merge columns** — split a column by a separator (with a preview of how many parts each row has) or join several columns into one, as computed columns that also apply to rows imported later.
- **Validation rules** — required, unique, range, format (mobile number / email / regex…) and allowed values; re-checked after every import or sync, with one click to show only the failing rows.
- **Data cleaning** — remove duplicate rows, trim whitespace, find and replace, fill empty values and delete filtered rows, with a before / after preview and undo for the last change.
- **Window calculations** — running totals, ranks, previous-row comparisons, moving averages, group totals / shares and top N per group, computed over the whole table on the server into a new dataset that recomputes when the source updates.
- **Column profiles** — for the current filters, one scan shows every column's empty values, distinct counts, ranges, mean / median and unrecognized dates; open a column for its histogram and most common values, and click a bar to filter by it (about 3 s on 10 million rows).
- **Pivot tables** — computed on the server and written as normal sheets; calculated values and items, share / YoY / MoM, approximate quantiles, drill-down.
- **Formula push-down** — `=SUMIFS(Orders[Amount], Orders[Shop], B2)` runs on the server; platform functions such as `DSUMIFS`; exported to native Excel formulas.
- **SQL workbench** — read-only SQL (checked against a parser-based allow-list), typed parameters, save results as datasets, scheduled refresh.
- **Dashboards** — slicers, KPI cards, linked charts, presentation mode.
- **AI assistant** — bring your own OpenAI-compatible model; proposals are applied only after confirmation and can be undone; anomaly detection, data-quality checks, and cleaning / window-calculation proposals with a before / after preview; read-only MCP server for external AI clients.
- **Collaboration & governance** — real-time editing, comments, four login modes, audit log, row-level data scope, approvals, in-app notifications.
- **Scenario packs** — the project started as an e-commerce finance tool; that part (shop cash-flow import, month-end close and approval, balance reconciliation, bank reconciliation, operations analysis) is now an optional pack, `LEDGER_PACKS=ecommerce`. It is auto-enabled only when the database already holds ledger data; a fresh install is a plain, general-purpose spreadsheet platform.
- **Multi-person data collection** — collection tasks: dozens of stores / departments fill in the same dataset, each only seeing and filling their own units (synced to data scope automatically), opened period by period, with a board showing who has and has not filled in.
- **Read-only share links** — send a link to a customer or partner without an account to view one workbook (charts, KPI cards and slicers stay live); no editing or export, never more data than the creator can see, optional expiry, revocable at any time.
- **Threshold alerts** — notify a WeCom / DingTalk / Feishu group or email when a metric (or a ratio such as refund rate) crosses a threshold, per group; change vs the previous period (sales down 30%), data-staleness checks (which shop has had no new orders for days); minimum-sample guard, one notification per new problem, optional recovery notices, and a dry-run before saving.
- **Other** — xlsx import/export, templates, scheduled reports (WeCom / DingTalk / Feishu / email), version history, backups.

## Quick start

**See it first** — download a release package, unpack it, and run:

```bash
bin/gridlake demo        # generates sample data and opens your browser a few seconds later
```

It generates 1 million orders (dated up to today), stock movements and receivables / payables, and builds three workbooks from the built-in templates — monthly operations, inventory, and AR / AP reconciliation — so slicers, pivots, charts and formulas work right away. Everything lives in a separate demo database (`~/.gridlake/demo.duckdb`); running it again just reopens it, `--reset` regenerates, and `--rows 10000000` tries a bigger one. Localhost only, no login.

**Open a file on your own machine** — download a release package, unpack it, and run:

```bash
bin/gridlake open orders.parquet customers.csv   # opens your browser
bin/gridlake open /data/orders/                  # every Parquet file below (Hive-style year=2026/ folders become columns)
```

Parquet files are **queried in place, not copied**; CSV / XLSX / JSON are imported (encoding, header row and column types detected). Data lives in a personal database at `~/.gridlake/open.duckdb`; opening the same file again refreshes it instead of creating a duplicate. It listens on localhost only and needs no login. Options: `--port`, `--db`, `--no-browser`.

**Run the server (Docker):**

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

## Trademarks and affiliation

Gridlake is an independent open-source project. It is **not affiliated with, endorsed by or sponsored by** Microsoft (Excel), ByteDance / Douyin (Douyin Shop, Qianchuan), the Univer team, the DuckDB Foundation, or any other product or company mentioned. Those names are used only to describe compatibility or data sources (for example "import Douyin Shop ledger exports" or "open the exported xlsx in Excel"); all trademarks belong to their respective owners.

## Contributing & license

See [CONTRIBUTING.md](CONTRIBUTING.md), [ROADMAP.md](ROADMAP.md) and [CHANGELOG.md](CHANGELOG.md). Licensed under [Apache-2.0](LICENSE); third-party licenses are listed in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
