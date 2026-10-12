# 亿级规模压测（N12）

在 8 核 16 GB 的 Mac 上实测过，结论见 `web/docs/platform-high-value-requirements.md` 的「亿级规模实测」。复现步骤（全程用临时目录，不碰正式库；需要约 10 GB 空闲磁盘，跑完删掉 `$DIR`）：

```bash
DIR=/tmp/n12 && mkdir -p $DIR/data && cd server
cargo build --release && cargo build --release --example inflate
(cd $DIR && python3 $OLDPWD/tests/bench/gen_base.py 5000000)            # 500 万行基础数据
LEDGER_ADDR=127.0.0.1:18950 LEDGER_DB=$DIR/data/ledger.duckdb LEDGER_BACKUP_AT=off target/release/gridlake &
BENCH_CSV=$DIR/base.csv python3 tests/bench/import_base.py              # 走真实导入路径，记下返回的 datasetId
kill %1                                                                  # 放大前必须停服务
target/release/examples/inflate $DIR/data/ledger.duckdb <数据集表名 ds_...> 100000000
LEDGER_ADDR=127.0.0.1:18950 LEDGER_DB=$DIR/data/ledger.duckdb LEDGER_BACKUP_AT=off target/release/gridlake &
LEDGER_E2E_TIMEOUT=900 python3 tests/bench/bench_scale.py $(pgrep -x gridlake)   # 全套：游标、筛选、排序、透视、公式、SQL、导出、备份、并发
python3 tests/bench/bench_cursor.py $(pgrep -x gridlake) sort_money   # 排序 / 筛选游标的建表耗时、内存峰值、取行延迟（每个场景前重启服务）；bench_storm.py 是并发风暴
LEDGER_MEMORY_LIMIT=2GB ...                                              # 受限内存对比用 bench_heavy.py
```

逐笔对账的规模测试（不依赖上面的亿级数据）：先起一个服务，再 `python3 tests/bench/bench_recon.py 1000000 keys`（`nokeys` 为没有对账键、大量相同金额的最坏情况），每次换一个新库；结果见平台需求文档的「逐笔对账与银行余额调节表」。

注意：`inflate` 放大出的行没有关键字搜索文本（search_text 为空），关键字搜索只覆盖原来的基础行。

## 夜间浸泡测试（Linux CI）

`tests/bench/nightly.sh` 把上面的流程自动化：造数据 → 导入 → 放大到 2000 万行 → `soak.py` 用多个线程持续发透视、公式汇总、SQL、游标、列值请求 15 分钟，记录各类请求的 p50 / p95 / 最大延迟、错误数和进程内存；`compare.py` 与上一次的结果对比，某类请求慢 2 倍以上（且多出 200 ms 以上）就算退步。
GitHub Actions 里由 `.github/workflows/nightly.yml` 每天运行（也可手动触发并调整行数和时长），结果存成构件，上一次成功的结果缓存起来当基线。

通过条件：服务一直存活；没有 5xx（503「服务繁忙」排队超时不算错误，但会计数）；预热后内存没有持续增长；没有明显的性能退步。

本机快速试跑（debug 版，几分钟）：

```bash
cargo build --example inflate
BIN=target/debug/gridlake INFLATE=target/debug/examples/inflate \
  BASE_ROWS=100000 TARGET_ROWS=300000 SOAK_MINUTES=1 tests/bench/nightly.sh
```
