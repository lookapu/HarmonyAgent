# -*- coding: utf-8 -*-
"""查找会话中 build-profile.json5 的写入/读取内容"""
import sqlite3, sys, io, json, datetime

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

DB = r'C:\Users\<USER>\AppData\Roaming\com.deveco-switch.app\deveco-switch.db'
CID = '49a2be7b-d6e7-455d-8d3c-2378bfa2aeba'
conn = sqlite3.connect(DB)
cur = conn.cursor()

# tool 消息中与 build-profile 相关的
cur.execute("SELECT role, content, created_at FROM messages WHERE conversation_id=? ORDER BY created_at", (CID,))
rows = cur.fetchall()
print(f"总消息数: {len(rows)}")
for role, content, ts in rows:
    c = (content or '').strip()
    if 'build-profile' in c and ('write_file' in c or 'tool_result' in c or 'TOOL|' in c):
        t = datetime.datetime.fromtimestamp(ts)
        print(f"\n{'='*80}\n[{t:%H:%M:%S}] role={role}")
        print(c[:3000])
        print("..." if len(c) > 3000 else "")

conn.close()
