with open(r'src-tauri\src\agent\tools\quality_tools.rs', 'r', encoding='utf-8') as f:
    lines = f.read().splitlines()

# 用更严谨的 find_block_end：扫每行计算 depth
def find_block_end(start):
    depth = 0
    found_open = False
    for i in range(start, len(lines)):
        line = lines[i]
        in_str = False
        quote = None
        idx = 0
        while idx < len(line):
            c = line[idx]
            if in_str:
                if c == '\\' and idx + 1 < len(line):
                    idx += 2
                    continue
                if c == quote:
                    in_str = False
                idx += 1
                continue
            if c == '/' and idx + 1 < len(line) and line[idx + 1] == '/':
                # 行注释
                break
            if c == '"' or c == "'" or c == '`':
                in_str = True
                quote = c
                idx += 1
                continue
            if c == '{':
                depth += 1
                found_open = True
            elif c == '}':
                depth -= 1
                if found_open and depth == 0:
                    return i
            idx += 1
    return -1

# 测试 collect_source_files
end = find_block_end(278)  # 0-based 278 = line 279
print(f"collect_source_files: 0-based end = {end}, 1-based = {end+1}")
print(f"line at end: {lines[end]!r}")

# 测试所有 blocks
import re
blocks = []
i = 0
while i < len(lines):
    line = lines[i]
    m = re.match(r'^(pub\(super\)\s+async\s+fn|pub(?:\(crate\))?\s+fn|fn)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(', line)
    if m:
        s = i
        e = find_block_end(s)
        kind = "pub_async" if "async" in m.group(1) else "helper"
        blocks.append((kind, m.group(2), s, e))
        if e < 0:
            print(f"  FAILED: {kind} {m.group(2)} at {s+1}")
            break
        i = e + 1
    else:
        i += 1

print(f"\ntotal blocks: {len(blocks)}")
print(f"pub_async: {sum(1 for b in blocks if b[0] == 'pub_async')}")
print(f"helper: {sum(1 for b in blocks if b[0] == 'helper')}")
for b in blocks:
    print(f"  {b[0]:9} {b[1]:30} lines {b[2]+1}-{b[3]+1}")
