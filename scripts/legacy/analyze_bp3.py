# -*- coding: utf-8 -*-
"""从 tool_runs 查 build-profile 写入内容"""
import sqlite3, sys, io, json, datetime

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

DB = r'C:\Users\<USER>\AppData\Roaming\com.deveco-switch.app\deveco-switch.db'
CID = '49a2be7b-d6e7-455d-8d3c-2378bfa2aeba'
conn = sqlite3.connect(DB)
cur = conn.cursor()

cur.execute("""
SELECT tool_name, input_json, result_json, created_at FROM tool_runs
WHERE conversation_id=? AND input_json LIKE '%build-profile%' ORDER BY created_at
""", (CID,))
rows = cur.fetchall()
print(f"build-profile 相关工具调用: {len(rows)}")
for tool, inp, res, ts in rows:
    t = datetime.datetime.fromtimestamp(ts)
    print(f"\n{'='*90}\n[{t:%H:%M:%S}] {tool}")
    print(f"IN : {inp[:2500]}")
    print(f"OUT: {res[:400]}")

conn.close()
