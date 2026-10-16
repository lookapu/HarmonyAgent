#!/usr/bin/env python3
"""UI 状态覆盖门禁（ROADMAP Q-04）。

规则（详见 docs/UI_STATE_COVERAGE.md）：
1. src/pages/*.tsx 每个页面文件必须在头部（前 30 行）声明 @ui-states 状态覆盖清单。
2. 合法状态名：loading / empty / partial / failed / retry / permission（逗号分隔）。
3. 声明的每个状态必须在文件代码中有对应证据（模式匹配，见 STATE_PATTERNS）。
4. 纯容器页面（无 useState/useEffect，状态由子组件承载）必须声明为 @ui-states: delegated。
5. 未声明、声明非法状态、声明与代码证据不符均视为失败（退出非零）。
6. docs/UI_STATE_COVERAGE.md 的现状审计矩阵必须覆盖**每一个**页面文件。

第 6 条是补上的：矩阵原先纯手写、没有任何门禁盯着，于是新增 UsagePage（P2-1）时
它被静默漏掉，而读者拿它回答「某页有没有 permission 态」时得到的是
「表里没这一行」——听起来像结论，实际是没数据。**产出方（页面声明）有门禁、
消费方（文档矩阵）没有，就是典型的「只改一端」**：每加一个页面漏一行，
半年后这张表就不再可信，而它失效的过程没有任何一步会报错。

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
COVERAGE_DOC = REPO / "docs" / "UI_STATE_COVERAGE.md"

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


def doc_matrix_pages(doc_path: Path) -> set[str]:
    """解析现状审计矩阵的页面名集合（不含 .tsx 后缀）。

    只在「## 现状审计矩阵」这一节里找表格行：文档里还有六类状态的定义表，
    那是**规则**不是**现状**，混进来会让 Home 之类的页面名被误判为已登记。
    """
    if not doc_path.exists():
        return set()
    names: set[str] = set()
    in_matrix = False
    for line in doc_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("## "):
            in_matrix = "现状审计矩阵" in line
            continue
        if not in_matrix or not line.startswith("|"):
            continue
        first = line.strip().strip("|").split("|")[0].strip()
        if not first or first == "页面" or set(first) <= set("-: "):
            continue  # 空行 / 表头 / 分隔行
        names.add(first)
    return names


def check_matrix_coverage(pages: list[Path], doc_path: Path | None = None) -> list[str]:
    """矩阵与页面文件集必须一致（两个方向都要查）。"""
    doc_path = doc_path or COVERAGE_DOC
    actual = {p.stem for p in pages}
    documented = doc_matrix_pages(doc_path)
    problems: list[str] = []
    if missing := sorted(actual - documented):
        problems.append(f"现状审计矩阵缺少这些页面行: {', '.join(missing)}（见 {doc_path.name}）")
    if stale := sorted(documented - actual):
        problems.append(f"现状审计矩阵仍列着已删除的页面: {', '.join(stale)}（删页面时忘了删行）")
    return problems


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

        # 矩阵覆盖：漏行、残留行、以及「别把规则表当现状表」这三类
        doc_ok = (
            "## 现状审计矩阵\n\n| 页面 | loading |\n|---|---|\n| APage | ✓ |\n| BPage | ✓ |\n"
        )
        # 规则表里出现的 APage 不应被算作「已登记」——只有矩阵节内的行才算
        doc_decoy = (
            "| 状态 | 含义 |\n|---|---|\n| APage | 误导行 |\n\n"
            "## 现状审计矩阵\n\n| 页面 | loading |\n|---|---|\n| APage | ✓ |\n"
        )
        matrix_cases = [
            ("矩阵齐", [Path("APage.tsx"), Path("BPage.tsx")], doc_ok, None),
            ("漏一行", [Path("APage.tsx"), Path("CPage.tsx")], doc_ok, "缺少这些页面行"),
            ("删页残留", [Path("APage.tsx")], doc_ok, "已删除的页面"),
            ("规则表不算现状", [Path("APage.tsx")], doc_decoy, None),
        ]
        for label, page_names, doc_text, expected in matrix_cases:
            d = Path(tmp) / f"doc_{abs(hash(label))}.md"
            d.write_text(doc_text, encoding="utf-8")
            probs = check_matrix_coverage([Path(n) for n in page_names], d)
            if expected is None:
                if probs:
                    print(f"  ✗ 矩阵[{label}]: 期望通过，实际失败: {probs}")
                    failed += 1
            elif not any(expected in p for p in probs):
                print(f"  ✗ 矩阵[{label}]: 期望问题 '{expected}' 未出现: {probs}")
                failed += 1

    if failed:
        print(f"self-test FAILED: {failed} 个用例失败")
        sys.exit(1)
    print(f"self-test OK: 5 个声明用例 + {len(matrix_cases)} 个矩阵用例全部符合预期")


def main() -> int:
    # Windows 控制台默认 cp1252：直接 print 中文会抛 UnicodeEncodeError，CI 上只会看到
    # 编码崩溃而看不到真正的失败明细。显式改用 UTF-8 输出。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")

    args = sys.argv[1:]
    if "--self-test" in args:
        self_test()
        return 0
    # **必须排除 *.test.tsx**：同目录下的页面测试也是 .tsx，一并 glob 进来会被
    # 当成一个「没声明 @ui-states 的页面」，页面总数也跟着虚增 1。
    # 同一份排除规则也用在 scripts/check-docs.py 的 count_pages()——两处若不一致，
    # 一个报「页面数对不上文档」、另一个报「页面未声明」，同一个改动触发两种错法。
    pages = sorted(p for p in PAGES_DIR.glob("*.tsx") if not p.name.endswith(".test.tsx"))
    results = run(pages)
    total = len(results)
    bad = [r for r in results if r["problems"]]
    declared = [r for r in results if r["declared"]]
    print(f"\n页面总数: {total} | 已声明: {len(declared)} | 未声明: {total - len(declared)} | 不通过: {len(bad)}")
    matrix_problems = check_matrix_coverage(pages)
    for p in matrix_problems:
        print(f"  ✗ {p}")
    if bad or matrix_problems:
        print("存在未通过页面（新增/修改页面必须显式声明并满足状态覆盖校验，见 docs/UI_STATE_COVERAGE.md）")
        return 1
    if "--report" not in args:
        print("UI 状态覆盖门禁通过：全部页面显式声明并满足一致性校验")
    return 0


if __name__ == "__main__":
    sys.exit(main())
