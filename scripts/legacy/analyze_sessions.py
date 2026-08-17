# -*- coding: utf-8 -*-
"""分析 testhy 项目的全部会话，输出会话摘要与工具调用统计"""
import sqlite3, sys, io, json, datetime

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

DB = r'C:\Users\<USER>\AppData\Roaming\com.deveco-switch.app\deveco-switch.db'
conn = sqlite3.connect(DB)
cur = conn.cursor()

# 表结构
cur.execute("SELECT name FROM sqlite_master WHERE type='table'")
tables = [r[0] for r in cur.fetchall()]
print("=== 表清单 ===")
print(tables)

# 项目
cur.execute("SELECT id, name, path FROM projects")
projects = cur.fetchall()
print("\n=== 项目 ===")
for p in projects:
    print(p)

# testhy 项目 id
testhy = [p for p in projects if 'testhy' in p[2].lower()]
if not testhy:
    print("\n未找到 testhy 项目")
    sys.exit(0)
pid = testhy[0][0]
print(f"\ntesthy project id: {pid}")

# 会话列表
cur.execute("SELECT id, title, created_at, updated_at FROM conversations WHERE project_id=? ORDER BY created_at", (pid,))
convs = cur.fetchall()
print(f"\n=== testhy 会话 ({len(convs)}) ===")
for c in convs:
    print(f"id={c[0]}  title={c[1]}  created={datetime.datetime.fromtimestamp(c[2])}  updated={datetime.datetime.fromtimestamp(c[3])}")

# 每个会话的消息统计
print("\n=== 会话消息统计 ===")
for c in convs:
    cid = c[0]
    cur.execute("SELECT COUNT(*) FROM messages WHERE conversation_id=?", (cid,))
    n = cur.fetchone()[0]
    cur.execute("SELECT COUNT(*) FROM messages WHERE conversation_id=? AND role='assistant'", (cid,))
    na = cur.fetchone()[0]
    cur.execute("SELECT COUNT(*) FROM messages WHERE conversation_id=? AND role='tool'", (cid,))
    nt = cur.fetchone()[0]
    print(f"{cid[:8]} 消息总数={n} assistant={na} tool={nt}  {c[1]}")

# 工具调用统计（tool_runs 表？）
for tbl in tables:
    if 'tool' in tbl.lower():
        print(f"\n=== 表 {tbl} 结构 ===")
        cur.execute(f"PRAGMA table_info({tbl})")
        for col in cur.fetchall():
            print(col)

conn.close()
