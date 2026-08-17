# -*- coding: utf-8 -*-
import sqlite3, sys, io
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
conn = sqlite3.connect(r'C:\Users\<USER>\AppData\Roaming\com.deveco-switch.app\deveco-switch.db')
cur = conn.cursor()
cur.execute("SELECT tool_name, input_json, result_json, created_at FROM tool_runs WHERE conversation_id='49a2be7b-d6e7-455d-8d3c-2378bfa2aeba' AND tool_name IN ('todo_write','ohpm_search') ORDER BY created_at")
for r in cur.fetchall():
    print('===', r[0], r[3])
    print('IN :', r[1][:1800])
    print('OUT:', r[2][:400])
    print()
conn.close()
