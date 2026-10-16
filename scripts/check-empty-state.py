#!/usr/bin/env python3
"""空态必须走 EmptyState：拦住「每个页面各写各的」

对应 docs/UI_REFINEMENT_PLAN.md 的 P0-4。

**为什么要有这道门**：空态是用户「点进来却什么都没有」时唯一看到的东西，
也是最容易被随手写掉的地方。2026-10-01 普查时，16 个页面里 11 处空态是各写各的：
有的居中大字、有的左对齐、有的带图标有的不带、有的给下一步按钮有的什么都不给。
视觉不统一只是表象，**更要紧的是有 6 处「暂无 X」之后没有任何下一步**——
用户看到「暂无 Provider」却不知道该点哪里。

组件化之后仍会回流，所以要有门禁。

**契约**（改动前先确认不违反）：
1. 扫描 src/pages 与 src/components 下的 .tsx，找「空态文案 + 非 EmptyState 容器」；
2. **表格式例外白名单**（见 TABLE_CELL_OK）：空态必须是 <td>/<tr>/<p> 里的单行文字，
   或必须嵌在带标题的 <section>/<h3> 里——这些位置换成 EmptyState 会破坏表格或章节结构；
3. 白名单**按文件+行锚点**记，锚点移动时要手工更新（行号漂移会让门禁误报，
   故报错信息里会同时给出文件与文案，便于重新定位）；
4. 本门只管**呈现形态**不管文案对错。

用法
  python scripts/check-empty-state.py            # 校验（CI 门禁）
  python scripts/check-empty-state.py --report   # 打印全部命中点与白名单命中

自测：python scripts/check-empty-state.py --self-test
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "src"
SCAN_DIRS = ("pages", "components")

# 空态文案的判据：**只认 i18n 键名**。
# 刻意不匹配「暂无」字面量——那会命中正文散文（例如会话摘要模板里的
# 「（该会话暂无内容）」）与组件自身的文档注释。键名是本项目唯一可靠信号。
EMPTY_KEY_RE = re.compile(
    r"t\(\s*'([A-Za-z0-9_.]*(?:\.empty|\.noData|\.noEvents|\.noCalls|\.emptyTemplateResult))'"
)

# EmptyState 的多行用法：<EmptyState 与 title={...} 往往不在同一行，
# 只看当前行会把它们误判成「自写空态」。故向上回看几行。
LOOKBACK = 6


def _in_empty_state_block(lines: list[str], idx: int) -> bool:
    for k in range(max(0, idx - LOOKBACK), idx + 1):
        if "<EmptyState" in lines[k]:
            return True
    return False


# 结构性例外：这些容器里空态只能是单行文字，换成 EmptyState 会破坏表格/章节骨架
# 形式是 (文件相对路径, 该行必须同时含有的结构特征正则)
STRUCTURAL_OK = (
    (r"pages/CostPage\.tsx", r"colSpan|<td|<p "),
    (r"pages/ReproductionBundlesPage\.tsx", r"<section"),
    (r"pages/TeamSharingPage\.tsx", r"<section"),
    (r"components/NotificationBell\.tsx", r"notification\.empty"),
    # 统计块里的一句旁注，不是列表空态（没有「列表是空的」这层语义）
    (r"pages/SkillsPage\.tsx", r"skillStats\.noCalls"),
    # 模板搜索结果为空：在一张模板卡片网格里，grid 的子项高度不能被撑开
    (r"pages/McpPage\.tsx", r"mcp\.emptyTemplateResult"),
)


def files_to_scan() -> list[Path]:
    out: list[Path] = []
    for d in SCAN_DIRS:
        out.extend(p for p in (SRC / d).rglob("*.tsx") if p.is_file())
    # EmptyState 组件自身必然含大量「空态」字样（含文档注释），排除掉
    return sorted(p for p in out if p.name != "EmptyState.tsx")


def is_structural_exception(rel: str, line: str) -> bool:
    for pattern, needs in STRUCTURAL_OK:
        if re.search(pattern, rel) and re.search(needs, line):
            return True
    return False


def scan(per_file: dict[str, list[str]]) -> tuple[list[str], list[str]]:
    """返回 (违规, 白名单命中)。per_file = {相对路径: [行文本, ...]}"""
    bad: list[str] = []
    allowed: list[str] = []
    for rel, lines in per_file.items():
        for i, line in enumerate(lines):
            if not EMPTY_KEY_RE.search(line):
                continue
            if _in_empty_state_block(lines, i):
                continue  # 已走组件（含 <EmptyState> 在上几行的情况）
            site = f"{rel}:{i + 1}"
            if is_structural_exception(rel, line):
                allowed.append(site)
                continue
            bad.append(f"{site}  {line.strip()[:110]}")
    return bad, allowed


def read_per_file(files: list[Path]) -> dict[str, list[str]]:
    out: dict[str, list[str]] = {}
    for p in files:
        out[p.relative_to(REPO).as_posix()] = p.read_text(
            encoding="utf-8", errors="replace"
        ).splitlines()
    return out


class _Fake:
    def __init__(self, rel: str) -> None:
        self._rel = rel

    def relative_to(self, other):  # noqa: ANN001
        return Path(self._rel)

    def as_posix(self) -> str:
        return self._rel

    def is_file(self) -> bool:
        return True

    def rglob(self, _p):  # noqa: ANN001
        return iter(())


def self_test() -> int:
    cases = [
        ("<p className='x'>{t('cost.noData')}</p>", "pages/CostPage.tsx", 0, "表格式例外放行"),
        ("<section><h3>{t('repro.empty')}</h3></section>", "pages/ReproductionBundlesPage.tsx", 0,
         "章节内例外放行"),
        ("<EmptyState compact title={t('version.empty')} />", "pages/VersionsPage.tsx", 0,
         "单行 EmptyState → 不算"),
        # 多行：<EmptyState 在上一行，title 在本行
        ("<EmptyState\n  icon='bolt'\n  title={t('provider.empty')}", "pages/ProvidersPage.tsx", 0,
         "多行 EmptyState → 也不算（第一版漏了这条，误报了 2 处）"),
        ("<p>{t('foo.empty')}</p>", "pages/FooPage.tsx", 1, "自写空态 → 拦截"),
        ("<div>（该会话暂无内容）</div>", "pages/Home.tsx", 0, "正文散文含「暂无」→ 不管"),
        ("<p>{t('foo.title')}</p>", "pages/BazPage.tsx", 0, "非空态文案 → 不管"),
        # 白名单按文件锚定：同一个 key 换个文件就不该再豁免
        ("<p>{t('cost.noData')}</p>", "pages/OtherPage.tsx", 1, "白名单不跨文件"),
    ]
    failures = 0
    for text, rel, expect, desc in cases:
        got = len(scan({rel: text.split("\n")})[0])
        if got == expect:
            print(f"  [通过] {desc}")
        else:
            print(f"  [失败] {desc}：期望 {expect}，实得 {got}")
            failures += 1
    if failures:
        print(f"自测失败：{failures}/{len(cases)} 条不符预期")
        return 1
    print(f"自测通过：{len(cases)} 条合成样例全部符合预期")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="空态必须走 EmptyState")
    ap.add_argument("--self-test", action="store_true", help="跑合成样例回归")
    ap.add_argument("--report", action="store_true", help="打印命中点与白名单")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    files = files_to_scan()
    if not files:
        print("没扫到 .tsx，扫描范围可能已失效", file=sys.stderr)
        return 1

    bad, allowed = scan(read_per_file(files))

    if args.report:
        print(f"白名单例外 {len(allowed)} 处（表格单元格 / 带标题的章节 / 通知下拉）：")
        for a in allowed:
            print(f"  {a}")
        print(f"\n自写空态 {len(bad)} 处：")
        for b in bad:
            print(f"  {b}")

    if bad:
        print("空态一致性门禁失败：\n- " + "\n- ".join(bad), file=sys.stderr)
        print(
            "\n提示：空态是用户「什么都没看到」时唯一看到的东西，也是唯一告诉他下一步的机会。\n"
            "      请改用 <EmptyState>（compact 用于面板内联，非 compact 用于整页）。\n"
            "      确实只能放单行文字（表格单元格 / 带标题的章节）时，\n"
            "      把该文件与结构特征补进脚本 STRUCTURAL_OK 并写明理由。",
            file=sys.stderr,
        )
        return 1

    print(f"空态一致性门禁通过：{len(files)} 个文件，{len(allowed)} 处结构例外，无自写空态")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
