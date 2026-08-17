"""用更稳健的 brace 计数：忽略字符串和注释中的 {/}."""
import os
import re

FILES = [
    r"<PROJECT_ROOT>\src-tauri\src\agent\tools\quality_metrics.rs",
    r"<PROJECT_ROOT>\src-tauri\src\agent\tools\quality_security.rs",
    r"<PROJECT_ROOT>\src-tauri\src\agent\tools\quality_runtime.rs",
    r"<PROJECT_ROOT>\src-tauri\src\agent\tools\quality_media.rs",
]

SIG_RE = re.compile(r'^\s*(pub(?:\([^)]*\))?\s+)?(async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b')

def find_functions(path):
    with open(path, 'r', encoding='utf-8') as f:
        text = f.read()
    lines = text.split('\n')
    n = len(lines)
    results = []
    i = 0
    while i < n:
        m = SIG_RE.match(lines[i])
        if m:
            name = m.group(3)
            sig_line = i + 1
            j = i
            depth = 0
            found_brace = False
            in_string = None  # None / '"' / '\''
            in_block_comment = False
            in_line_comment = False
            while j < n:
                line = lines[j]
                k = 0
                while k < len(line):
                    ch = line[k]
                    nxt = line[k+1] if k+1 < len(line) else ''
                    if in_block_comment:
                        if ch == '*' and nxt == '/':
                            in_block_comment = False
                            k += 1
                    elif in_line_comment:
                        pass  # skip till end of line
                    elif in_string:
                        if ch == '\\':
                            k += 1  # skip next
                        elif ch == in_string:
                            in_string = None
                    else:
                        if ch == '/' and nxt == '/':
                            in_line_comment = True
                            k += 1
                        elif ch == '/' and nxt == '*':
                            in_block_comment = True
                            k += 1
                        elif ch == '"':
                            in_string = '"'
                        elif ch == "'":
                            in_string = "'"
                        elif ch == '{':
                            depth += 1
                            found_brace = True
                        elif ch == '}':
                            depth -= 1
                            if found_brace and depth == 0:
                                results.append((name, sig_line, j + 1))
                                i = j + 1
                                break
                    k += 1
                if found_brace and depth == 0:
                    break
                # 行结束：清除行注释状态
                in_line_comment = False
                j += 1
            else:
                print(f"  NO_CLOSE for {name} at L{sig_line}")
                i += 1
            continue
        i += 1
    return results

all_ok = True
total = 0
for f in FILES:
    if not os.path.exists(f):
        print(f"  MISS: {f}")
        all_ok = False
        continue
    fns = find_functions(f)
    total += len(fns)
    name = os.path.basename(f)
    print(f"\n=== {name} ({len(fns)} funcs) ===")
    for fn_name, start, end in fns:
        print(f"  L{start:4d}-L{end:4d} : {fn_name:<30s} ({end-start+1:3d} lines)")

print()
print(f"Total functions: {total}")
