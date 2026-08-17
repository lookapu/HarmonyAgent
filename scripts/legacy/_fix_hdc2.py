import re
with open("src-tauri/src/agent/tools/quality_tools.rs", "r", encoding="utf-8") as f:
    s = f.read()
# 找所有 output_blocking 行
for m in re.finditer(r'output_blocking', s):
    start = max(0, m.start() - 10)
    end = min(len(s), m.end() + 80)
    print(repr(s[start:end]))
    print("---")
