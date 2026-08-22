#!/usr/bin/env python3
"""Q-07 告警基线门禁：统计 clippy 唯一告警数，超过基线即失败。

用法：
  python scripts/check-warnings.py                 # 默认基线 44
  python scripts/check-warnings.py --baseline N    # 显式指定基线
  python scripts/check-warnings.py --self-test     # 合成样例回归自测

统计口径：`cargo clippy --all-targets --message-format=json` 的 warning 级
compiler-message，按 (lint 名, 文件:行) 去重。同一告警在 lib/bin/test 等
多个 target 会重复报告，去重后才是真实告警数。

基线说明（Q-07 收敛结果）：338 → 44，剩余 44 个全为结构类告警
（too_many_arguments 31 + type_complexity 13），按项目哲学"不以消除全部
历史告警作为前置条件"保留为基线；新增任何机械类告警立即阻断 CI。
"""

import argparse
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_BASELINE = 44


def count_unique_warnings(json_lines):
    """从 clippy JSON 输出流中统计去重后的告警数，返回 (总数, 分类统计, 明细)。"""
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
        if spans:
            s = spans[0]
            loc = f"{s.get('file_name', '?')}:{s.get('line_start')}"
        key = (code, loc)
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
    total, by_lint, detail = count_unique_warnings(lines)

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
