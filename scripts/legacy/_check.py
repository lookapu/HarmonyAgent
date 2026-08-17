with open(r'src-tauri\src\agent\tools\quality_metrics.rs', 'r', encoding='utf-8') as f:
    lines = f.read().splitlines()
for i, line in enumerate(lines):
    if 'fn collect_source_files' in line:
        print(f'fn at 0-based {i}, 1-based {i+1}: {line!r}')
        depth = 0
        in_str = False
        quote = None
        for j in range(i, len(lines)):
            for c in lines[j]:
                if in_str:
                    if c == quote:
                        in_str = False
                    continue
                if c == '"' or c == "'" or c == '`':
                    in_str = True
                    quote = c
                    continue
                if c == '{':
                    depth += 1
                elif c == '}':
                    depth -= 1
                    if depth == 0:
                        print(f'  end at 0-based {j}, 1-based {j+1}: {lines[j]!r}')
                        raise SystemExit(0)
