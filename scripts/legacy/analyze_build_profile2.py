# -*- coding: utf-8 -*-
"""提取 assistant 消息中 build-profile 相关的 TOOL| 调用内容"""
import sqlite3, sys, io, json, datetime, re

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

DB = r'C:\Users\<USER>\AppData\Roaming\com.deveco-switch.app\deveco-switch.db'
CID = '49a2be7b-d6e7-455d-8d3c-2378bfa2aeba'
conn = sqlite3.connect(DB)
cur = conn.cursor()

cur.execute("SELECT content, created_at FROM messages WHERE conversation_id=? AND role='assistant' ORDER BY created_at", (CID,))
rows = cur.fetchall()
print(f"assistant 消息数: {len(rows)}")
for content, ts in rows:
    c = content or ''
    t = datetime.datetime.fromtimestamp(ts)
    # 找 build-profile 的 write_file 调用
    for m in re.finditer(r'【TOOL\|write_file\|[^\n]*', c):
        seg = m.group(0)
        if 'build-profile' in seg:
            print(f"\n[{t:%H:%M:%S}] {seg[:4000]}")
    # 也找多行 write_file（跨行 JSON）
    if 'build-profile.json5' in c and 'TOOL|' in c:
        idx = c.find('build-profile.json5')
        # 打印包含该文件名的前后上下文（800 字符）
        start = max(0, idx - 1500)
        print(f"\n[{t:%H:%M:%S}] === 上下文 ===\n{c[start:idx+2500]}")

conn.close()
