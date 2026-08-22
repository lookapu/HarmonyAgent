#!/usr/bin/env python3
"""UI 状态覆盖门禁（ROADMAP Q-04）。

规则（详见 docs/UI_STATE_COVERAGE.md）：
1. src/pages/*.tsx 每个页面文件必须在头部（前 30 行）声明 @ui-states 状态覆盖清单。
2. 合法状态名：loading / empty / partial / failed / retry / permission（逗号分隔）。
3. 声明的每个状态必须在文件代码中有对应证据（模式匹配，见 STATE_PATTERNS）。
4. 纯容器页面（无 useState/useEffect，状态由子组件承载）必须声明为 @ui-states: delegated。
5. 未声明、声明非法状态、声明与代码证据不符均视为失败（退出非零）。

用法：
  python scripts/check-ui-states.py            # 校验全部页面（CI 门禁）
  python scripts/check-ui-states.py --report   # 输出完整状态矩阵报告（不失败）

自测：python scripts/check-ui-states.py --self-test
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PAGES_DIR = REPO / "src" / "pages"

VALID_STATES = {"loading", "empty", "partial", "failed", "retry", "permission"}

# 声明状态 -> 文件内代码证据模式（宽松语义匹配；只读证据，不要求全部状态都出现 UI）
STATE_PATTERNS = {
    "loading": re.compile(r"loading|isLoading|busy|saving", re.IGNORECASE),
    "empty": re.compile(r"empty|暂无|没有数据|无数据|noData", re.IGNORECASE),
    "partial": re.compile(r"partial|部分成功|部分失败|成功.{0,12}失败|失败.{0,12}成功|失败不影响|不影响主面板", re.IGNORECASE),
    "failed": re.compile(r"error|Error|失败|错误", re.IGNORECASE),
    "retry": re.compile(r"retry|重试|refresh|重新加载", re.IGNORECASE),
    "permission": re.compile(r"unauthorized|403|未授权|无权限|permission|授权", re.IGNORECASE),
}

DECL_RE = re.compile(r"@ui-states\s*:\s*([A-Za-z, ]+)")


def extract_declaration(text: str) -> list[str] | None:
    """提取页面头部（前 30 行）的 @ui-states 声明；无声明返回 None。"""
    head = "\n".join(text.splitlines()[:30])
    m = DECL_RE.search(head)
    if not m:
        return None
    states = [s.strip().lower() for s in m.group(1).split(",") if s.strip()]
    return states


def check_page(path: Path) -> dict:
    """校验单个页面，返回 {name, declared, problems, evidence}。"""
    text = path.read_text(encoding="utf-8")
    name = path.name
    declared = extract_declaration(text)
    problems: list[str] = []

    if declared is None:
        problems.append("缺少 @ui-states 声明（头部前 30 行内）")
        return {"name": name, "declared": None, "problems": problems, "evidence": {}}

    invalid = [s for s in declared if s not in VALID_STATES and s != "delegated"]
    if invalid:
        problems.append(f"非法状态名: {', '.join(invalid)}（合法: {', '.join(sorted(VALID_STATES))}）")

    if "delegated" in declared:
        if len(declared) != 1:
            problems.append("delegated 必须单独声明（容器页面状态由子组件承载）")
        if re.search(r"useState|useEffect", text):
            problems.append("声明 delegated 但文件含 useState/useEffect（非纯容器页面）")
        return {"name": name, "declared": declared, "problems": problems, "evidence": {}}

    evidence = {}
    # 声明行本身可能包含状态名（如 @ui-states: empty），证据搜索需排除声明行
    decl_lines = {ln for ln, line in enumerate(text.splitlines(), 1) if DECL_RE.search(line)}
    for state in declared:
        if state not in VALID_STATES:
            continue  # 非法状态已在上方报错，跳过一致性检查
        pattern = STATE_PATTERNS[state]
        hits = [ln for ln, line in enumerate(text.splitlines(), 1)
                if ln not in decl_lines and pattern.search(line)]
        evidence[state] = hits
        if not hits:
            problems.append(f"声明 {state} 但文件中无对应代码证据（模式: {pattern.pattern}）")

    return {"name": name, "declared": declared, "problems": problems, "evidence": evidence}


def run(pages: list[Path]) -> list[dict]:
    results = [check_page(p) for p in sorted(pages)]
    for r in results:
        evidence_summary = {s: len(hits) for s, hits in r["evidence"].items()}
        print(f"{r['name']}: {', '.join(r['declared']) if r['declared'] else '(未声明)'}"
              + (f" | 证据: {evidence_summary}" if evidence_summary else ""))
        for problem in r["problems"]:
            print(f"  ✗ {problem}")
    return results


def self_test() -> None:
    """合成样例回归：非法状态、声明无证据、delegated 误用、正常声明。"""
    import tempfile

    cases = [
        ("bad_state.tsx", "/** @ui-states: loading, unknown */\nexport default function A() { return null }", "非法状态名"),
        ("no_evidence.tsx", "/** @ui-states: empty */\nexport default function A() { const [x] = useState(0); return null }", "声明 empty 但文件中无对应代码证据"),
        ("delegated_abuse.tsx", "/** @ui-states: delegated */\nimport { useState } from 'react'\nexport default function A() { const [x] = useState(0); return null }", "声明 delegated 但文件含 useState/useEffect"),
        ("delegated_ok.tsx", "/** @ui-states: delegated */\nexport default function A() { return <div/> }", None),
        ("good.tsx", "/** @ui-states: loading, empty, failed, retry */\nexport default function A() { const [loading] = useState(false); const [error] = useState(null); const retry = () => {}; if (error) return <div>失败</div>; return <div>{loading ? 'loading' : 'empty 暂无'}</div> }", None),
    ]
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        for name, content, expected in cases:
            p = Path(tmp) / name
            p.write_text(content, encoding="utf-8")
            result = check_page(p)
            if expected is None:
                if result["problems"]:
                    print(f"  ✗ {name}: 期望通过，实际失败: {result['problems']}")
                    failed += 1
            elif not any(expected in prob for prob in result["problems"]):
                print(f"  ✗ {name}: 期望问题 '{expected}' 未出现: {result['problems']}")
                failed += 1
    if failed:
        print(f"self-test FAILED: {failed} 个用例失败")
        sys.exit(1)
    print("self-test OK: 5 个合成用例全部符合预期")


def main() -> int:
    args = sys.argv[1:]
    if "--self-test" in args:
        self_test()
        return 0
    pages = sorted(PAGES_DIR.glob("*.tsx"))
    results = run(pages)
    total = len(results)
    bad = [r for r in results if r["problems"]]
    declared = [r for r in results if r["declared"]]
    print(f"\n页面总数: {total} | 已声明: {len(declared)} | 未声明: {total - len(declared)} | 不通过: {len(bad)}")
    if bad:
        print("存在未通过页面（新增/修改页面必须显式声明并满足状态覆盖校验，见 docs/UI_STATE_COVERAGE.md）")
        return 1
    if "--report" not in args:
        print("UI 状态覆盖门禁通过：全部页面显式声明并满足一致性校验")
    return 0


if __name__ == "__main__":
    sys.exit(main())
