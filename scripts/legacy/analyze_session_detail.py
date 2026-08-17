# -*- coding: utf-8 -*-
"""分析 testhy 会话：工具调用统计、失败分析、关键事件"""
import sqlite3, sys, io, json, datetime

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

DB = r'C:\Users\<USER>\AppData\Roaming\com.deveco-switch.app\deveco-switch.db'
CID = '49a2be7b-d6e7-455d-8d3c-2378bfa2aeba'
conn = sqlite3.connect(DB)
cur = conn.cursor()

# 1. 工具调用统计
print("=== 工具调用统计（tool_runs） ===")
cur.execute("""
SELECT tool_name, COUNT(*) total,
       SUM(CASE WHEN status='success' THEN 1 ELSE 0 END) ok,
       SUM(CASE WHEN status='error' THEN 1 ELSE 0 END) err,
       SUM(CASE WHEN status NOT IN ('success','error') THEN 1 ELSE 0 END) other,
       ROUND(AVG(duration_ms),0) avg_ms
FROM tool_runs WHERE conversation_id=? GROUP BY tool_name ORDER BY total DESC
""", (CID,))
for r in cur.fetchall():
    print(f"{r[0]:28s} total={r[1]:3d} ok={r[2]:3d} err={r[3]:2d} other={r[4]:2d} avg={r[5]}ms")

# 2. 失败的工具调用详情（含输入和错误）
print("\n=== 失败工具调用详情 ===")
cur.execute("""
SELECT tool_name, input_json, result_json, duration_ms, created_at
FROM tool_runs WHERE conversation_id=? AND status='error' ORDER BY created_at
""", (CID,))
errs = cur.fetchall()
print(f"失败次数: {len(errs)}")
for e in errs:
    t = datetime.datetime.fromtimestamp(e[4])
    inp = e[1][:200] if e[1] else ''
    res = e[2][:300] if e[2] else ''
    print(f"\n[{t:%H:%M}] {e[0]} ({e[3]}ms)")
    print(f"  IN: {inp}")
    print(f"  ERR: {res}")

# 3. 消息内容中的关键模式
print("\n=== assistant 消息时间线 ===")
cur.execute("SELECT role, content, created_at FROM messages WHERE conversation_id=? AND role='assistant' ORDER BY created_at", (CID,))
for r in cur.fetchall():
    t = datetime.datetime.fromtimestamp(r[2])
    content = r[1][:150].replace('\n', ' ')
    print(f"[{t:%H:%M:%S}] {content}")

conn.close()
