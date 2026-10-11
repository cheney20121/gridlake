"""生成亿级压测的基础数据：python3 gen_base.py 5000000  → base.csv（约 420 MB，8 列：订单号、店铺 300 个、下单时间、商品 2 万个、渠道、状态、数量、金额）"""
import random, datetime, sys
random.seed(7)
n = int(sys.argv[1])
shops = [f"店铺{i:03d}" for i in range(300)]
goods = [f"商品{i:05d}" for i in range(20000)]
chan = ["直播", "商品卡", "短视频", "自然", "达人"]
status = ["已完成", "已发货", "待发货", "已退款", "部分退款", "已取消"]
w = [60, 15, 8, 8, 5, 4]
start = datetime.datetime(2024, 1, 1).timestamp()
span = (datetime.datetime(2026, 9, 30).timestamp() - start)
r = random.random; ri = random.randint
with open("base.csv", "w", encoding="utf-8") as f:
    f.write("订单号,店铺,下单时间,商品,渠道,状态,数量,金额\n")
    for i in range(n):
        t = datetime.datetime.fromtimestamp(start + r() * span).strftime("%Y-%m-%d %H:%M:%S")
        q = ri(1, 5)
        f.write(f"DY{i:012d},{random.choice(shops)},{t},{random.choice(goods)},{random.choice(chan)},{random.choices(status, w)[0]},{q},{round(q * (10 + r() * 290), 2)}\n")
