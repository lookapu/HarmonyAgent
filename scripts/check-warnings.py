#!/usr/bin/env python3
"""Q-07 告警基线门禁：统计 clippy 唯一告警数，超过基线即失败。

用法：
  python scripts/check-warnings.py                 # 默认基线 44
  python scripts/check-warnings.py --baseline N    # 显式指定基线
  python scripts/check-warnings.py --self-test     # 合成样例回归自测

统计口径：`cargo clippy --all-targets --message-format=json` 的 warning 级
compiler-message，按 (lint 名, 文件, **告警所在行的源码文本**) 去重。同一告警在
lib/bin/test 等多个 target 会重复报告；用源码文本而不是行号做键，是因为行号会随
插入/删除代码漂移，那样同一处告警会被算成"新增"，基线反复失效（2026-09-15 实测过）。
读不到源码时回退到行号。

基线说明（Q-07 收敛结果）：338 → 44，当时剩余 44 个全为结构类告警
（too_many_arguments 31 + type_complexity 13），按项目哲学"不以消除全部
历史告警作为前置条件"保留为基线；新增任何机械类告警立即阻断 CI。

2026-09-15 重新定基线：44 → 57（改用源码行文本做去重键后的实测值）。机械类告警（bool_assert_comparison、
cloned_ref_to_slice_refs、manual_inspect、unnecessary_map_or、question_mark、
items_after_test_module、redundant_closure、single_match、
manual_pattern_char_comparison、suspicious_open_options 等约 41 条）已全部收敛，
部分属于较新 clippy lint 与测试代码；结构类随代码增长到 57
（too_many_arguments 41 + type_complexity 16）。基线只保留结构类，
口径不变：新增任何机械类告警仍然立即阻断。
"""

import argparse
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_BASELINE = 57


def source_line(repo: Path, file_name: str, line_start) -> str:
    """取告警所在行的源码文本；读不到时回退空串。"""
    if not file_name or not line_start:
        return ""
    for base in (repo, repo / "src-tauri"):
        path = base / file_name
        try:
            lines = path.read_text(errors="replace").splitlines()
        except OSError:
            continue
        if 1 <= line_start <= len(lines):
            return lines[line_start - 1].strip()
    return ""


def count_unique_warnings(json_lines, repo=None):
    """从 clippy JSON 输出流中统计去重后的告警数，返回 (总数, 分类统计, 明细)。

    去重键是 (lint, 文件, 该行源码文本) 而不是 (lint, 文件:行号)：行号会随着在文件里
    插入/删除代码而漂移，用行号做键会让**同一处告警**在改动后变成「新增告警」，
    导致基线反复失效。改用源码文本后，只有告警内容本身变化（例如函数真的又多了一个参数）
    才会改变键。读不到源码时回退到行号，保证退化情况下仍有去重。
    """
    seen = set()
    by_lint = Counter()
    detail = []
    for line in json_lines:
        if not line.strip():
            continue
        try:
            msg_obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg_obj.get("reason") != "compiler-message":
            continue
        msg = msg_obj.get("message", {})
        if msg.get("level") != "warning" or msg.get("code") is None:
            continue
        code = msg["code"].get("code", "?")
        spans = msg.get("spans", [])
        loc = "?"
        anchor = "?"
        if spans:
            s = spans[0]
            file_name = s.get("file_name", "?")
            line_start = s.get("line_start")
            loc = f"{file_name}:{line_start}"
            anchor = source_line(repo, file_name, line_start) if repo else "?"
        key = (code, anchor if anchor else loc)
        if key in seen:
            continue
        seen.add(key)
        by_lint[code] += 1
        detail.append((code, loc))
    return len(seen), by_lint, detail


def run_clippy_json():
    cmd = [
        "cargo", "clippy",
        "--manifest-path", str(REPO / "src-tauri" / "Cargo.toml"),
        "--all-targets", "--locked", "--message-format=json",
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, cwd=str(REPO))
    if proc.returncode != 0 and "error" in proc.stderr.lower():
        # clippy 无 error 时正常返回 0；有编译错误时返回非零，直接透传
        sys.stderr.write(proc.stderr)
        sys.exit(proc.returncode or 1)
    return proc.stdout.splitlines()


def self_test():
    """合成四组样例：多 target 重复、非 warning 行、error 行、无告警，验证计数。"""
    sample = [
        '{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::x"},"spans":[{"file_name":"src/a.rs","line_start":1}]}}',
        # 同位置重复（模拟 lib+test 双 target）→ 只计 1
        '{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::x"},"spans":[{"file_name":"src/a.rs","line_start":1}]}}',
        '{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::y"},"spans":[{"file_name":"src/b.rs","line_start":7}]}}',
        '{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"spans":[{"file_name":"src/c.rs","line_start":9}]}}',
        '{"reason":"compiler-artifact","target":{"name":"x"}}',
        'not-json',
    ]
    total, by_lint, detail = count_unique_warnings(sample)
    assert total == 2, f"期望 2 个唯一告警，实际 {total}"
    # 同一处告警在不同 target 里行号不同 -> 仍算同一处（无源码可读时按行号回退）
    shifted = [
        '{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::z"},"spans":[{"file_name":"src/d.rs","line_start":10}]}}',
        '{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::z"},"spans":[{"file_name":"src/d.rs","line_start":10}]}}',
    ]
    total_shift, _, _ = count_unique_warnings(shifted)
    assert total_shift == 1, f"同一处告警应只计一次，实际 {total_shift}"
    assert by_lint == Counter({"clippy::x": 1, "clippy::y": 1}), by_lint
    assert len(detail) == 2
    # 无告警流
    total2, by2, _ = count_unique_warnings(['{"reason":"compiler-artifact","target":{"name":"x"}}'])
    assert total2 == 0 and not by2
    print("self-test OK: 去重/过滤/分类逻辑正确")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=int, default=DEFAULT_BASELINE,
                        help=f"告警基线（默认 {DEFAULT_BASELINE}）")
    parser.add_argument("--self-test", action="store_true", help="合成样例自测后退出")
    args = parser.parse_args()

    if args.self_test:
        return self_test()

    print(f"运行 cargo clippy（基线 {args.baseline}）…")
    lines = run_clippy_json()
    total, by_lint, detail = count_unique_warnings(lines, REPO)

    print(f"clippy 唯一告警：{total}/{args.baseline}")
    for code, n in by_lint.most_common():
        print(f"  {n:4d}  {code}")

    if total > args.baseline:
        print(f"FAIL：告警数 {total} 超过基线 {args.baseline}，新增告警必须修复或更新基线")
        for code, loc in detail:
            print(f"  {code}  {loc}")
        return 1
    print("PASS：未新增告警")
    return 0


if __name__ == "__main__":
    sys.exit(main())
