#!/usr/bin/env python3
"""字号必须在 7 档阶梯内：拦住「随手写个 text-[12.5px]」的漂移

对应 docs/UI_REFINEMENT_PLAN.md 的 P1-1。

**为什么要有这道门**：字号是全库出现最多的视觉决策，也是最容易漂移的。
本项目一度有 12 种任意字号并存（8 / 8.5 / 9 / 9.5 / 10 / 10.5 / 11 / 11.5 /
12 / 12.5 / 13 / 13.5 / 14 / 15 / 18px），而 index.css 里那套 6 档
--app-text-* 阶梯只被 src/components/ui/ 的 8 个文件消费，主体代码全是裸字面量。
两套并存 = 阶梯形同虚设。

**契约**（改动前先确认不违反）：
1. **允许值从 src/index.css 的 --app-text-* 声明里解析**，不在本脚本里另抄一份。
   两份实现必然分叉，且分叉后没人会发现——门禁自己就成了漂移源。
   声明值形如 `--app-text-xs: calc(11px * var(--ui-font-scale))`（缩放包装），
   也兼容裸 `--app-text-xs: 11px`；两种都解析。
2. 只管**任意值**形式 text-[Npx]。Tailwind 命名档（text-xs/sm/base/lg/xl/2xl）
   有语义（正文/标题），不在收敛范围，放行。
3. token 形式 text-[length:var(--app-text-*)] 放行——它按定义就在阶梯内。
4. 只约束**字号**。line-height 的 leading-[14px]、max-w-[110px] 等一律不管。
5. **反向也查：引用的档名必须在 index.css 里真有定义。** 这是第 1 条的对偶。
   全库 1300 余处都走 token 引用，一旦有人删掉某一档而漏改引用点，
   `var(--app-text-2xs)` 解析失败 → font-size 声明作废 → **静默回退继承字号**，
   界面上看不出报错，只是那一层突然变大了。字面量检查对此完全无感，
   所以这条必须独立存在。

用法
  python scripts/check-font-sizes.py            # 校验全库（CI 门禁）
  python scripts/check-font-sizes.py --report   # 打印分布，便于看收敛情况

自测：python scripts/check-font-sizes.py --self-test
"""
from __future__ import annotations

import argparse
import re
import sys
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "src"
CSS_FILE = SRC / "index.css"

# index.css 里的阶梯声明。值可能带缩放包装：
#   --app-text-micro: 9px;
#   --app-text-micro: calc(9px * var(--ui-font-scale));
# 两种都要认，否则门禁会在有人加缩放的那天突然全线报「扫描范围失效」。
LADDER_RE = re.compile(
    r"--app-(text|lh)-([a-z0-9-]+)\s*:\s*(?:calc\(\s*)?([0-9]+(?:\.[0-9]+)?)px"
)

# 任意字号：text-[12.5px]；token 形式 text-[length:var(--app-text-sm)] 不匹配
# （含 'length:' 前缀与 var(），故天然落在放行侧。
ARBITRARY_RE = re.compile(r"(?<![\w-])text-\[([0-9]+(?:\.[0-9]+)?)px\]")

# token 引用：var(--app-text-xs) / var(--app-lh-sm)，含 text-[length:var(...)] 形式
TOKEN_REF_RE = re.compile(r"(?<![\w-])--app-(text|lh)-([a-z0-9-]+)")

# Tailwind 命名档：有意放行（正文/标题语义），仅用于 --report 区分展示
NAMED_RE = re.compile(r"(?<![\w-])text-(xs|sm|base|lg|xl|2xl|3xl)\b")


def ladder(css_text: str) -> dict[str, str]:
    """从 index.css 解析阶梯。**这是唯一真源**，不在脚本里硬编码。

    返回 {"text-xs": "11", "lh-xs": "16", ...}；档名带前缀以免字号与行高同名。
    """
    return {f"{m.group(1)}-{m.group(2)}": m.group(3) for m in LADDER_RE.finditer(css_text)}


def allowed_sizes(ladder_map: dict[str, str]) -> set[str]:
    """阶梯里的字号像素值（不含 lh-*）。"""
    return {v for k, v in ladder_map.items() if k.startswith("text-")}


def source_files() -> list[Path]:
    return sorted(p for p in SRC.rglob("*") if p.suffix in (".ts", ".tsx") and p.is_file())


def scan(files: list[Path]) -> tuple[Counter, list[tuple[str, str, str]], list[tuple[str, str]]]:
    """返回 (字面量分布, [(文件:行, 离档值, 空)], [(文件:行, 未定义档名)])。"""
    dist: Counter = Counter()
    bad: list[tuple[str, str, str]] = []
    refs: list[tuple[str, str]] = []
    for path in files:
        rel = path.relative_to(REPO).as_posix()
        for lineno, line in enumerate(
            path.read_text(encoding="utf-8", errors="replace").splitlines(), 1
        ):
            for m in ARBITRARY_RE.finditer(line):
                val = m.group(1)
                dist[val] += 1
                bad.append((f"{rel}:{lineno}", val, ""))
            for m in TOKEN_REF_RE.finditer(line):
                refs.append((f"{rel}:{lineno}", f"{m.group(1)}-{m.group(2)}"))
    return dist, bad, refs


def check(files: list[Path], css_text: str) -> list[str]:
    ladder_map = ladder(css_text)
    allowed = allowed_sizes(ladder_map)
    problems: list[str] = []
    if not allowed:
        return ["index.css 里没解析到任何 --app-text-* 阶梯声明，扫描范围可能已失效"]
    _, bad, refs = scan(files)
    for site, val, _ in bad:
        if val not in allowed:
            problems.append(f"{site}: text-[{val}px] 不在阶梯 {sorted(allowed, key=float)} 内")

    defined = set(ladder_map)
    for site, name in refs:
        if name not in defined:
            problems.append(
                f"{site}: 引用了 --app-{name}，但 index.css 里没有这一档定义。"
                "\n         未定义的自定义属性会让 font-size 整条声明作废并回退继承，"
                "界面不会报错、只会悄悄变大——所以必须先补定义或改引已存在的档。"
            )
    return problems


class _FakeFile:
    def __init__(self, text: str) -> None:
        self._text = text

    def read_text(self, encoding: str = "utf-8", errors: str = "strict") -> str:
        return self._text

    def relative_to(self, other: Path) -> Path:
        return Path("<selftest>")


CSS = "--app-text-a:9px;--app-text-b:11px;--app-text-c:13px;--app-lh-b:16px;"
# 加了缩放包装的形态：解析必须同样认得出，否则有人加缩放那天门禁全线失效
CSS_SCALED = (
    ":root{--ui-font-scale:1;"
    "--app-text-a:calc(9px * var(--ui-font-scale));"
    "--app-text-b:calc(11px * var(--ui-font-scale));"
    "--app-text-c:calc(13px * var(--ui-font-scale));"
    "--app-lh-b:calc(16px * var(--ui-font-scale));}"
)


def self_test() -> int:
    cases = [
        (CSS, "x text-[11px]", 0, "在阶梯内 → 放行"),
        (CSS, "x text-[12.5px]", 1, "离档 → 拦截"),
        (CSS, "x text-[length:var(--app-text-a)]", 0, "token 形式 → 放行"),
        (CSS, "x text-xs text-sm text-lg text-2xl", 0, "Tailwind 命名档 → 放行"),
        (CSS, "x leading-[14px] max-w-[110px]", 0, "非字号 → 不管"),
        (CSS, "x text-[9px] text-[13px]", 0, "多处在阶梯内 → 放行"),
        ("", "x text-[11px]", 1, "阶梯解析为空 → 报扫描范围失效"),
        (CSS, "x hover:text-[12.5px] focus:text-[12.5px]", 2, "变体前缀不豁免"),
        # 缩放包装形态：允许值仍要解析得出来
        (CSS_SCALED, "x text-[11px] text-[13px]", 0, "calc() 包装的阶梯 → 允许值照常解析"),
        (CSS_SCALED, "x text-[12.5px]", 1, "calc() 包装的阶梯 → 离档仍拦"),
        (CSS_SCALED, "x text-[length:var(--app-text-b)]", 0, "calc() 包装下 token 引用照常放行"),
        # 反向检查：档名没定义
        (CSS, "x text-[length:var(--app-text-huge)]", 1, "引用未定义档 → 拦"),
        (CSS, "x var(--app-lh-z)", 1, "引用未定义行高档 → 拦"),
        (CSS, "x var(--app-lh-b)", 0, "引用已定义行高档 → 放行"),
        (CSS, "x text-[length:var(--app-text-a)] var(--app-text-c)", 0, "多处引用已定义档 → 放行"),
    ]
    failures = 0
    for css, tsx, expect, desc in cases:
        got = len(check([_FakeFile(tsx)], css))
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
    ap = argparse.ArgumentParser(description="字号必须在阶梯内")
    ap.add_argument("--self-test", action="store_true", help="跑合成样例回归")
    ap.add_argument("--report", action="store_true", help="打印字号分布")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    if not CSS_FILE.exists():
        print(f"找不到 {CSS_FILE}", file=sys.stderr)
        return 1
    css_text = CSS_FILE.read_text(encoding="utf-8", errors="replace")
    ladder_map = ladder(css_text)
    allowed = allowed_sizes(ladder_map)
    files = source_files()
    if not files:
        print("src/ 下没有 .ts/.tsx，扫描范围可能已失效", file=sys.stderr)
        return 1

    dist, _bad, refs = scan(files)
    if args.report:
        print(f"阶梯档名（从 index.css 解析）：{sorted(ladder_map)}")
        print(f"阶梯允许值：{sorted(allowed, key=float)}")
        print("任意字号分布：")
        for val, n in sorted(dist.items(), key=lambda kv: float(kv[0])):
            mark = "OK " if val in allowed else "离档"
            print(f"  [{mark}] {val}px: {n} 处")
        named = 0
        for path in files:
            named += len(NAMED_RE.findall(
                path.read_text(encoding="utf-8", errors="replace")))
        print(f"Tailwind 命名档（放行）：{named} 处")
        print(f"token 引用：{len(refs)} 处，全部命中已定义档 {len(ladder_map)} 个")

    problems = check(files, css_text)
    if problems:
        shown = problems[:20]
        print("字号阶梯门禁失败：\n- " + "\n- ".join(shown), file=sys.stderr)
        if len(problems) > len(shown):
            print(f"- …另有 {len(problems) - len(shown)} 处", file=sys.stderr)
        print(
            "\n提示：离档字号不会报错，只会让同一层级出现肉眼难辨的粗细/行高差异。\n"
            "      要么并回阶梯里最接近的一档，要么在 index.css 的 --app-text-* "
            "里显式加一档并说明用途。",
            file=sys.stderr,
        )
        return 1

    print(f"字号阶梯门禁通过：{len(files)} 个文件、{len(dist)} 处任意字号、"
          f"{len(refs)} 处 token 引用，全部落在 {len(ladder_map)} 档阶梯内"
          f"（允许值 {sorted(allowed, key=float)}）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
