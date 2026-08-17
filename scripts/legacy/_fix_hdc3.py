import re
with open("src-tauri/src/agent/tools/quality_tools.rs", "r", encoding="utf-8") as f:
    s = f.read()
# 模式：output_blocking(\s*"hdc",\s*&\[[^\]]*\])\s*);
# 让 ] 在数组内部允许：用 depth 跟踪
def replace_call(text):
    out = []
    i = 0
    while i < len(text):
        idx = text.find("output_blocking(", i)
        if idx == -1:
            out.append(text[i:])
            break
        out.append(text[i:idx])
        # 找到匹配的 )
        j = idx + len("output_blocking(")
        depth = 1
        while j < len(text) and depth > 0:
            c = text[j]
            if c == '(': depth += 1
            elif c == ')': depth -= 1
            j += 1
        call_text = text[idx+len("output_blocking("):j-1]
        # 解析参数：必须是 "hdc", &array
        m = re.match(r'\s*"hdc",\s*&(\[[^\]]+\])\s*', call_text)
        if m:
            arr = m.group(1)
            out.append("hdc_shell(&" + arr + ")")
        else:
            out.append("output_blocking(" + call_text + ")")
        i = j
    return "".join(out)

new_s = replace_call(s)
with open("src-tauri/src/agent/tools/quality_tools.rs", "w", encoding="utf-8") as f:
    f.write(new_s)
print("done")
