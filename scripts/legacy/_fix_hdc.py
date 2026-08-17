import re
with open("src-tauri/src/agent/tools/quality_tools.rs", "r", encoding="utf-8") as f:
    s = f.read()
# 多行替换：output_blocking(\n  "hdc",\n  &[...]\n)
pat = re.compile(r'crate::utils::process::output_blocking\(\s*"hdc",\s*&(\[[^\]]+\])\s*\)', re.DOTALL)
print("match count:", len(pat.findall(s)))
s2 = pat.sub(r'hdc_shell(&\1)', s)
with open("src-tauri/src/agent/tools/quality_tools.rs", "w", encoding="utf-8") as f:
    f.write(s2)
print("done")
