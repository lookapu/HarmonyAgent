#!/usr/bin/env python3
"""a11y 基线：图标按钮必须有可访问名，选择型控件必须有对应 role

对应 docs/UI_REFINEMENT_PLAN.md 的 P1-3。

**为什么要有这道门**：可访问性缺陷和死 CSS token 是同一族——
「代码看起来做了，界面上什么都没发生」。一个只有 `title` 的图标按钮在
读屏软件里可能读出名字、也可能读不出（title 作为可访问名的支持并不一致），
而**既没有 aria-label 也没有可见文字**的按钮，读屏用户完全不知道那是什么。
这两种都不会报任何构建错误。

**契约**（改动前先确认不违反）：
1. 图标独占按钮（子节点只有 <Icon、没有可见文字）必须有 aria-label；
   有可见文字的按钮名字来自内容，不在本门管辖；
2. `ui/IconButton` 组件自带 aria-label + title，天然满足，**不重复统计**；
3. role="tab" 必须与 role="tablist" / role="tabpanel" 成套出现——
   孤零零一个 tab 角色比没有角色更糟，读屏会按错误的语义播报；
4. role="option" 必须有配套的 role="listbox"；
5. 本门只管**语义存在性**，不管对比度与焦点顺序（那需要真实浏览器 + axe）。

用法
  python scripts/check-a11y.py            # 校验（CI 门禁）
  python scripts/check-a11y.py --report   # 打印统计

自测：python scripts/check-a11y.py --self-test
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "src"
SCAN_DIRS = ("pages", "components", "chat", "icons", "utils", "stores", "api")

BTN_OPEN = re.compile(r"<button\b([^>]*?)/?>", re.S)
# 需要成套出现的 role：role 值 -> 必须同时存在的其它 role
ROLE_COMPANIONS = {
    "tab": {"tablist", "tabpanel"},
    "option": {"listbox"},
}
ICON_TAG = re.compile(r"<Icon\b")


def files_to_scan() -> list[Path]:
    out: list[Path] = []
    for d in SCAN_DIRS:
        p = SRC / d
        if p.is_dir():
            out.extend(q for q in p.rglob("*.tsx") if q.is_file())
    return sorted(out)


def _is_icon_only(text: str, open_end: int) -> bool:
    """开标签之后到 </button> 之间是否只有图标、没有可见文字。"""
    tail = text[open_end : open_end + 400]
    close = tail.find("</button>")
    if close == -1:
        return False
    inner = tail[:close]
    if not ICON_TAG.search(inner):
        return False
    return not re.sub(r"<[^>]+>", "", inner).strip()


def scan(files: list[Path]) -> tuple[list[str], list[str]]:
    """返回 (问题, 统计行)。"""
    problems: list[str] = []
    total_btns = 0
    roles_seen: dict[str, list[str]] = {}
    role_files: dict[str, set[str]] = {}

    for p in files:
        if p.name.endswith(".test.tsx"):
            continue
        rel = p.relative_to(REPO).as_posix()
        text = p.read_text(encoding="utf-8", errors="replace")

        for m in BTN_OPEN.finditer(text):
            attrs = m.group(1)
            if "aria-label" in attrs:
                continue
            if not _is_icon_only(text, m.end()):
                continue
            total_btns += 1
            lineno = text[: m.start()].count("\n") + 1
            problems.append(
                f"{rel}:{lineno}  图标按钮无可访问名"
                + ("（只有 title）" if "title=" in attrs else "（title 也没有）")
            )

        for m in re.finditer(r'role="([a-z]+)"', text):
            r = m.group(1)
            roles_seen[r] = roles_seen.get(r, []) + [
                f"{rel}:{text[: m.start()].count(chr(10)) + 1}"
            ]
            role_files.setdefault(r, set()).add(rel)

    # role 成套检查：跨文件统计（tablist 与 tab 可能不在同一个文件里）
    present = set(roles_seen)
    for role, needs in ROLE_COMPANIONS.items():
        if role not in present:
            continue
        for need in needs:
            if need not in present:
                sites = "、".join(roles_seen[role][:3])
                problems.append(
                    f'有 role="{role}"（{sites}）但全库没有 role="{need}"——'
                    f"角色不成套，读屏会按错误语义播报"
                )

    stats = [f"图标独占按钮（无可见文字、无 aria-label）：{total_btns} 个"]
    for r in sorted(roles_seen):
        stats.append(f'role="{r}": {len(roles_seen[r])} 处')
    return problems, stats


class _Fake:
    def __init__(self, rel: str, text: str) -> None:
        self._rel, self._text = rel, text
        self.name = Path(rel).name

    def relative_to(self, other):  # noqa: ANN001
        return Path(self._rel)

    def read_text(self, encoding: str = "utf-8", errors: str = "strict") -> str:
        return self._text

    def is_file(self) -> bool:
        return True

    def rglob(self, _p):  # noqa: ANN001
        return iter(())


def self_test() -> int:
    cases = [
        ("<button aria-label='x'><Icon name='close'/></button>", "components/A.tsx", 0,
         "有 aria-label → 放行"),
        ("<button title='x'><Icon name='close'/></button>", "components/A.tsx", 1,
         "只有 title → 拦截"),
        ("<button><Icon name='close'/></button>", "components/A.tsx", 1,
         "title 也没有 → 拦截"),
        ("<button><Icon name='close'/>取消</button>", "components/A.tsx", 0,
         "有可见文字 → 名字来自内容，放行"),
    ]
    failures = 0
    for text, rel, expect, desc in cases:
        got = len(scan([_Fake(rel, text)])[0])
        if got == expect:
            print(f"  [通过] {desc}")
        else:
            print(f"  [失败] {desc}：期望 {expect}，实得 {got}")
            failures += 1

    # role 成套：tab 需要 tablist **和** tabpanel 两个，缺一个报一条 → 期望 2
    solo = _Fake("components/B.tsx", '<div role="tab"><button role="tab">x</button></div>')
    got = len(scan([solo])[0])
    if got == 2:
        print("  [通过] 有 tab 无 tablist/tabpanel → 两条都报")
    else:
        print(f"  [失败] role 成套检查：期望 2 个问题，实得 {got}")
        failures += 1

    # option 需要 listbox
    opt = _Fake("components/C.tsx", '<div role="listbox"><div role="option">a</div></div>')
    if len(scan([opt])[0]) == 0:
        print("  [通过] option + listbox 成套 → 放行")
    else:
        print("  [失败] option + listbox 成套却被误报")
        failures += 1

    if failures:
        print(f"自测失败：{failures}/{len(cases) + 2} 条不符预期")
        return 1
    print(f"自测通过：{len(cases) + 2} 条合成样例全部符合预期")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="a11y 基线门禁")
    ap.add_argument("--self-test", action="store_true", help="跑合成样例回归")
    ap.add_argument("--report", action="store_true", help="打印统计")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    files = files_to_scan()
    if not files:
        print("没扫到 .tsx，扫描范围可能已失效", file=sys.stderr)
        return 1

    problems, stats = scan(files)
    if args.report:
        for s in stats:
            print(f"  {s}")

    if problems:
        print("a11y 门禁失败：\n- " + "\n- ".join(problems), file=sys.stderr)
        print(
            "\n提示：图标按钮若无可见文字，必须显式 aria-label——\n"
            "      只有 title 在读屏软件里未必被读出，且不支持国际化文本以外的场景。\n"
            "      role= 必须成套：tab↔tablist/tabpanel，option↔listbox。",
            file=sys.stderr,
        )
        return 1

    print(f"a11y 门禁通过：{len(files)} 个文件，{len(stats) - 1} 类 role 均成套，无无名图标按钮")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
