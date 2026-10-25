# Gridlake highlights

> **An open-source, self-hosted spreadsheet for 100M-row datasets — Excel-style formulas and pivot tables on top of DuckDB.**

Chinese version with more detail: [项目亮点.md](项目亮点.md). User manual (Chinese): [用户手册.md](用户手册.md). Use cases (Chinese): [应用场景.md](应用场景.md).

## In one sentence

**The feel of Excel, the capacity of a database.** The UI is an Excel-style workbook (built on Univer); data lives in DuckDB on the server and the browser only fetches the rows on screen — a 100-million-row table opens in about 50 ms.

## Ten highlights

1. **Fast on 100M rows.** Measured on 100M rows × 8 columns (8-core laptop, release build): open the whole table 52 ms, filter by shop (330k hits) ~0.1 s, pivot shop × month 1.8 s, SQL group-by 0.5 s, a two-condition `DSUMIFS` 0.17 s. The slow cases are published too (cold full-table sort 12–18 s, exact distinct count of 100M unique values 17–44 s), and the benchmark is reproducible.
2. **Nothing new to learn.** Hundreds of Excel functions, conditional formatting, outlines and named ranges, xlsx import/export. Formulas that reference big datasets (`SUMIFS(Orders[Amount], …)`, `DSUMIFS`) are pushed down to the server and converted back to native Excel formulas on export. Pivot tables support calculated values and items, YoY / MoM and approximate quantiles, and **every cell drills down to the detail rows with an automatic reconciliation check**.
3. **Spreadsheet features that scale.** Totals row, **multi-level collapsible subtotals** (kept as an Excel outline on export), column profiles (histograms, top values), filtering, sorting, search, selection statistics, split / merge columns, computed columns and lookups — all computed over the whole table on the server, not just the rows on screen.
4. **Data quality end to end.** Profile every column in one scan; validation rules (required, unique, range, phone / email / regex, allowed values) re-checked after every import or sync, with one click to show only the failing rows and a webhook on failure; cleaning (dedupe, trim, find & replace, fill blanks, delete rows) with a before / after preview and **undo**.
5. **Data that updates itself.** Connectors (MySQL / PostgreSQL, ClickHouse, HTTP APIs, S3, folders) on a schedule → SQL-derived datasets that recompute when their source changes → scheduled reports to WeCom / DingTalk / Feishu / email → threshold alerts (ratios, period-over-period drops, stale data) → event webhooks. No code required.
6. **Collaboration without merging 50 spreadsheets.** Real-time co-editing, comments with @mentions, version history, approvals; **collection tasks** let dozens of stores fill the same dataset, each seeing only their own rows, with a progress board, reminders and review.
7. **An AI assistant you stay in control of.** Bring your own OpenAI-compatible model (it can be self-hosted). Every number in an answer comes from a query tool. Anything that changes data or workbooks is a **proposal card** you preview and apply, and can undo. On a dataset page the assistant knows which table and filters you are looking at. A read-only **MCP** server lets Claude Desktop, Cursor and others query authorized datasets.
8. **Governance.** Built-in accounts plus OIDC SSO, gateway or reverse-proxy login; **row-level permissions** by shop / region / department applied consistently to pivots, formulas, SQL, export and AI (allow-list design); audit log; personal access tokens (read-only, dataset-scoped); read-only share links with password, expiry and revocation.
9. **Trivial to deploy, data stays home.** One binary and one data file, or one Docker command. `gridlake demo` creates a 1M-row sample with three ready-made workbooks in seconds; `gridlake open file.parquet` opens your own files locally. Backups with restore drills, Prometheus metrics and slow-request logs are built in.
10. **Serious engineering.** ~48k lines of Rust and ~53k lines of Vue 3 / TypeScript; 266 backend unit tests, 70 end-to-end test files, 258 frontend unit tests, 22 browser test files (93 cases), all in CI along with license and secret checks. Read-only SQL is validated with DuckDB's own parser, secrets are AES-GCM encrypted, tokens are stored as hashes and user input is always parameterized.

## Where it fits

Gridlake does not replace BI tools or data warehouses. It fills the gap between **"too big for Excel"** and **"too ad hoc for a BI dashboard"**. It is not meant for OLTP workloads, clusters beyond a billion rows, or second-level real-time dashboards.

| | Excel | BI tools | Airtable-style databases | Notebooks | **Gridlake** |
|---|---|---|---|---|---|
| Data size | 1,048,576 rows | Large | Usually < 1M rows | Large (memory-bound) | **100M+ rows** |
| Free-form formulas and layout | ✅ | ❌ | Partly | ❌ (code) | ✅ |
| Pivot tables | ✅ | ✅ | Partly | Code | ✅ with drill-down checks |
| Dashboards and scheduled reports | ❌ | ✅ | Partly | ❌ | ✅ |
| Data collection from many people | Email | ❌ | ✅ | ❌ | ✅ |
| Validation and cleaning | Manual | Partly | Partly | Code | ✅ preview and undo |
| Row-level permissions | ❌ | ✅ | Partly | ❌ | ✅ |
| Open source, self-hosted | — | Some | Some | ✅ | ✅ Apache-2.0 |

## Try it

```bash
bin/gridlake demo                 # 1M-row sample and three ready-made workbooks
bin/gridlake open orders.parquet  # open your own file in the spreadsheet UI
docker compose -f deploy/docker-compose.yml up -d
```

Stars, issues and pull requests are welcome — see [CONTRIBUTING.md](../CONTRIBUTING.md) and [ROADMAP.md](../ROADMAP.md).
