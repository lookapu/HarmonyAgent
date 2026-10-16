#!/usr/bin/env python3
"""CSS 变量引用必须有定义：拦住「代码看起来做了，界面上什么都没发生」

对应 docs/UI_REFINEMENT_PLAN.md 的 P0-1。

**为什么要有这道门**：自定义属性在 CSS 里查不到定义时**不报错**，浏览器静默按
「未设置的属性」处理——`background-color: var(--bg-muted)` 若 `--bg-muted` 不存在，
整条声明作废，元素退回透明。既没有构建错误，也没有测试失败，只有用户看到
「hover 没反应」。

本项目真实踩过：--bg-muted 被 4 个文件引用 10 处，index.css 从未定义它。
那 10 处 hover 态全部静默失效，且没有任何一道既有门禁会红。

**契约**（改动前先确认不违反）：
1. 只扫 src/ 下的 .ts/.tsx 里 `var(--x)` 形式的引用；
2. 引用到的名字必须在 src/index.css 里有**至少一处声明**（含 :root / 主题块）；
3. Tailwind 私有变量（--tw-*）与运行时注入的变量不在本门职责内，前者直接排除；
4. **本门不检查值是否「对」，只检查「有没有」**——--bg-muted 曾经「有名字、没定义」，
   而 --bg-card 被误用成 hover 色是另一类问题，由 code review 负责。

## 反向检查：定义了但没人用（第十二批补上另一半）

P0-1 只做了「引用了但没定义」这一个方向。同一种病还有另一半——
**定义了却没有任何消费点**，那它对界面的作用是零，而且是零信号：
没有构建错误、没有测试失败，`index.css` 里那一行白纸黑字摆着，
读代码的人会以为它生效了。

**同一道门做两个方向**，不另起新脚本：判据是同一个（一个自定义属性该不该有效果），
解析、扫描面、豁免机制全都一样，拆成两个脚本只会让两边的扫描范围各自漂移。

反向的**豁免表**是 `scripts/css-dead-token-allowlist.json`，每条必须写 `reason`。
**豁免也受审**：token 一旦被接上，条目即过期并报红——否则豁免表会变成垃圾场，
而没人知道哪一条已经名存实亡。

**反向检查有四个消费通道，漏掉一个就会误报**（详见 `check_dead` 的 docstring）：

| # | 通道 | 漏掉的后果 |
|---|---|---|
| 1 | 源码里的 `var(--x)` | 常规 |
| 2 | 源码里的 `var(--x, fallback)` | 11 个 `--hljs-*` 被误报 |
| 3 | `@theme` 注册项（值被内联进 utility，产物里没有 `var()` 引用） | 14 个 `--radius-*`/`--shadow-*` 被误报 |
| 4 | 构建产物 `dist/assets/*.css` 里的 Tailwind 生成引用 | 源码里看不到的引用被误报 |

> 通道 2 暴露了一个只有加反向检查才会看见的**不对称**：
> 正向检查刻意跳过带回退的引用（缺失会自愈，不算缺陷），
> 而反向判断必须把它算作消费。**两个方向需要两套口径**，用同一套必然错一边。

用法
  python scripts/check-css-vars.py            # 校验全库（CI 门禁）
  python scripts/check-css-vars.py --report   # 打印引用统计，便于看收敛情况

自测：python scripts/check-css-vars.py --self-test
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "src"
CSS_FILE = SRC / "index.css"
ALLOWLIST = REPO / "scripts" / "css-dead-token-allowlist.json"
DIST_ASSETS = REPO / "dist" / "assets"

# var(--x) 引用。允许 --a-z0-9 与 -_，不含后续字符组，避免吃到 --tw-shadow 之类。
# **带逗号回退的引用要单独识别**：`var(--x, red)` 在 --x 未定义时仍然成立（自愈），
# 不是缺陷；若一并按「必须有定义」判，会对合法的防御性写法误报。
VAR_REF_RE = re.compile(r"var\(\s*(--[A-Za-z0-9_-]+)\s*(,|\))")

# CSS 里的声明。**不锚定行首**：紧凑写法 `:root{--a:1px}` 里的声明同样要认，
# 否则门禁会把合法 token 误报成缺失（自测第 1/4/6 条就是这条）。
# 代价是注释里的「--x: 说明」也会被当成声明——本项目 index.css 无此类写法，
# 且误判方向是「多一个已定义」而非「少一个」，不会放过真正的缺失。
VAR_DECL_RE = re.compile(r"(--[A-Za-z0-9_-]+)\s*:")

# 任何形式的 var() 引用（带或不带回退）都算**消费**。
# 正向检查只用 VAR_REF_RE 且跳过带回退的那些（带回退时缺失会自愈，不产生视觉缺陷）；
# 反向检查必须用这个——两个方向的口径不同，详见 check_dead 的 docstring。
ANY_VAR_RE = re.compile(r"var\(\s*(--[A-Za-z0-9_-]+)\s*[,)]")

# Tailwind / 浏览器 / 运行时注入，本门不负责
EXEMPT_PREFIXES = ("--tw-",)
EXEMPT_EXACT = {
    # 浏览器 UA 与内联 style 运行时注入
    "--x", "--y",
}


# CSS 注释。**必须先剥再解析**：注释里写 `--tabular-nums: 原来那行是…`
# 会被声明正则当成真声明，于是「已经删掉了」的 token 继续被报成零消费。
# 同一段注释里的 `var(--x)` 同理会变成假的「有消费点」。
# 用等长空白替换，保留字符位置（行号类输出不会错位）。
CSS_COMMENT_RE = re.compile(r"/\*.*?\*/|//[^\n]*", re.S)
# .ts/.tsx 只剥块注释：`//` 在 JS 里会命中 URL 与正则，误剥的风险大于收益；
# 而 `/* */` 两种语言通用，剥掉是安全的。
BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", re.S)


def strip_css_comments(text: str) -> str:
    return CSS_COMMENT_RE.sub(lambda m: " " * len(m.group(0)), text)


def strip_comments_for(path_suffix: str, text: str) -> str:
    return strip_css_comments(text) if path_suffix == ".css" else BLOCK_COMMENT_RE.sub(
        lambda m: " " * len(m.group(0)), text
    )


def defined_vars(css_text: str) -> set[str]:
    """收集 index.css 里声明过的自定义属性名。

    刻意**不**区分声明所在的选择器：同一个名字在 :root 与 [data-theme="light"]
    各声明一次是本项目的正常做法（亮色覆盖），只按「至少存在一次」判定。
    """
    return {m.group(1) for m in VAR_DECL_RE.finditer(strip_css_comments(css_text))}


def source_files() -> list[Path]:
    """src/ 下的 .ts/.tsx，按路径排序保证输出稳定。"""
    return sorted(
        [p for p in SRC.rglob("*") if p.suffix in (".ts", ".tsx") and p.is_file()]
    )


def scan_refs(files: list[Path]) -> dict[str, list[str]]:
    """返回 {变量名: ["相对路径:行号", ...]}，只含**无回退**的引用。

    `var(--x, fallback)` 里 --x 未定义时浏览器用 fallback，声明缺失不产生
    视觉缺陷，因此不进本门（见 VAR_REF_RE 注释）。
    """
    hits: dict[str, list[str]] = defaultdict(list)
    for path in files:
        rel = path.relative_to(REPO).as_posix()
        for lineno, line in enumerate(
            path.read_text(encoding="utf-8", errors="replace").splitlines(), 1
        ):
            for m in VAR_REF_RE.finditer(line):
                if m.group(2) == ",":  # 带回退，自愈
                    continue
                name = m.group(1)
                if name.startswith(EXEMPT_PREFIXES) or name in EXEMPT_EXACT:
                    continue
                hits[name].append(f"{rel}:{lineno}")
    return hits


def check(files: list[Path], css_text: str) -> list[str]:
    """返回问题清单（空列表 = 通过）。"""
    defined = defined_vars(css_text)
    problems: list[str] = []
    for name, sites in sorted(scan_refs(files).items()):
        if name in defined:
            continue
        shown = "、".join(sites[:4])
        more = f"（共 {len(sites)} 处）" if len(sites) > 4 else ""
        problems.append(
            f"{name}：index.css 未定义，但有 {len(sites)} 处引用 → {shown}{more}"
        )
    return problems


# ---------- 反向检查：定义了但没人用 ----------

# `@theme` / `@theme inline` 块内的声明是**给 Tailwind 代码生成器读的注册项**，
# 不是普通的自定义属性。`@theme inline` 会把值**内联**进 utility：
#   .rounded-lg { border-radius: var(--app-radius-lg) }
# 于是产物里**根本没有 `var(--radius-lg)` 这个引用**。
# 本项目的 14 个 --radius-* / --shadow-* / --ease-* 全是这个形状。
# 不把它们从反向检查里排除，就会得到 14 条**假警报**——
# 而假警报会让人不再相信这一页，比漏报更糟。
#
# `inline` 后的 `\s*`（不是 `\s+`）：`@theme inline{` 紧贴花括号是合法写法，
# 写成 `\s+` 会让这种形式整块漏掉——自测第 12 条就是它。
THEME_BLOCK_RE = re.compile(r"@theme\s+(?:inline\s*)?\{", re.I)


def theme_registered_vars(css_text: str) -> set[str]:
    """收集 `@theme` 块内声明的自定义属性名。

    用**花括号配对**而不是正则截到下一个 `}`：嵌套规则与 calc() 里都可能出现花括号。
    """
    out: set[str] = set()
    for m in THEME_BLOCK_RE.finditer(css_text):
        i = m.end() - 1
        depth = 0
        for j in range(i, len(css_text)):
            if css_text[j] == "{":
                depth += 1
            elif css_text[j] == "}":
                depth -= 1
                if depth == 0:
                    out |= {d.group(1) for d in VAR_DECL_RE.finditer(css_text[i + 1 : j])}
                    break
    return out


def load_allowlist() -> dict[str, str]:
    """读死 token 豁免表：{token: reason}。文件缺失时返回空（等价于无豁免）。

    `_` 开头的键是**说明块**（本表顶部的 _comment），不是豁免条目——
    不过滤的话它会被当成「已过期的豁免」而报红，报错信息还完全指不到问题上。
    """
    if not ALLOWLIST.exists():
        return {}
    data = json.loads(ALLOWLIST.read_text(encoding="utf-8"))
    out: dict[str, str] = {}
    for k, v in data.items():
        if k.startswith("_"):
            continue
        if not (isinstance(v, dict) and str(v.get("reason", "")).strip()):
            raise SystemExit(
                f"{ALLOWLIST.relative_to(REPO)} 里的 `{k}` 缺少非空 reason —— "
                "豁免必须写明理由，否则这张表和没写一样没用"
            )
        out[k] = str(v["reason"])
    return out


def check_dead(
    files: list[Path],
    css_text: str,
    allowlist: dict[str, str],
    product_css: list[Path] | None = None,
) -> list[str]:
    """返回「定义了但没有任何消费点」的问题清单。

    ## 四个消费通道，缺一个就会误报

    1. **源码里的 `var(--x)`**（不带回退）—— 常规引用
    2. **源码里的 `var(--x, fallback)`** —— 带回退**也是消费**。
       正向检查刻意跳过它（带回退时变量缺失会自愈，不产生视觉缺陷），
       但反向判断不能跳过：`color: var(--hljs-keyword, #ff7b72)` 明显用到了它。
       **两个方向需要两套口径**——这是加反向检查时才暴露出来的不对称。
    3. **`@theme` 注册项** —— 见 `theme_registered_vars` 的注释
    4. **构建产物里 Tailwind 生成的引用** —— 产物才是最终形态，
       源码里看不到（与 check-font-sizes 的产物层校验同源）

    通道 1~2 在源码与 index.css 里找，通道 4 额外扫 `dist/assets/*.css`。
    """
    defined = defined_vars(css_text)
    theme = theme_registered_vars(css_text)

    consumed: set[str] = set()
    surfaces = list(files)
    if product_css:
        surfaces += product_css
    for path in surfaces:
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        # .css 剥两种注释；.ts/.tsx 只剥块注释（`//` 会命中 URL 与正则）
        text = strip_comments_for(path.suffix, text)
        for match in ANY_VAR_RE.finditer(text):
            consumed.add(match.group(1))

    dead = defined - consumed - theme
    problems: list[str] = []

    for name in sorted(dead - set(allowlist)):
        problems.append(
            f"{name}：index.css 里定义了，但源码、构建产物与 @theme 注册中都没有任何消费点"
            " → 它对界面没有任何作用。要么接上，要么删掉，"
            f"确认要保留就在 {ALLOWLIST.relative_to(REPO)} 里写明理由"
        )

    # 豁免也不能是永久通行证：token 一旦被接上，条目就该退休。
    # 不报这一条的话，豁免表会慢慢变成垃圾场，而没人知道哪条已经过时。
    for name in sorted(set(allowlist) - dead):
        problems.append(
            f"{name}：{ALLOWLIST.relative_to(REPO)} 里有豁免条目，但它现在**已经被消费**了"
            " → 豁免已过期，请删掉该条目（否则豁免表会变成垃圾场）"
        )
    return problems


class _FakeFile:
    """让 check() 能直接吃字符串，便于自测不落盘。"""

    def __init__(self, text: str, suffix: str = ".tsx") -> None:
        self._text = text
        self.suffix = suffix

    def read_text(self, encoding: str = "utf-8", errors: str = "strict") -> str:
        return self._text

    def relative_to(self, other: Path) -> Path:
        return Path("<selftest>")


def self_test() -> int:
    """合成样例回归：正向（未定义）与反向（零消费）两个方向都要验。"""
    failures = 0
    total = 0

    print("— 正向：引用了但没定义 —")
    cases = [
        # (css, tsx, 期望问题数, 说明)
        (":root{--a:1px}", "x bg-[var(--a)]", 0, "已定义 → 放行"),
        (":root{--b:1px}", "x bg-[var(--c)]", 1, "未定义 → 拦截"),
        (":root{}", "x var(--tw-ring-shadow)", 0, "Tailwind 私有变量 → 豁免"),
        (":root{--d:1px}", "x var(--d) var(--e) var(--d)", 1, "多处引用只报一次"),
        ("", "x color-mix(in srgb, var(--f) 60%, transparent)", 1, "color-mix 里的引用"),
        (":root{--g:1px}", "x var( --g )", 0, "括号内空格容忍"),
        (":root{--danger:red}", "x border-[var(--missing,var(--danger))]", 0,
         "外层带回退豁免，回退自身有定义"),
        (":root{}", "x border-[var(--missing,var(--danger))]", 1,
         "**回退里**的引用仍要查——回退自己也可能失效"),
        ("", "x var(--h,#fff)", 0, "回退是字面量也放行"),
    ]
    for css, tsx, expect, desc in cases:
        total += 1
        got = len(check([_FakeFile(tsx)], css))
        if got == expect:
            print(f"  [通过] {desc}")
        else:
            print(f"  [失败] {desc}：期望 {expect} 个问题，实得 {got}")
            failures += 1

    print("— 反向：定义了但没人用 —")
    # (css, src 文本, 产物文本|None, 豁免表, 期望问题数, 说明)
    dead_cases = [
        (":root{--used:1px}", "x var(--used)", None, {}, 0, "有 var() 引用 → 不是死 token"),
        (":root{--dead:1px}", "x", None, {}, 1, "零消费 → 报红"),
        # 通道 2：带回退的 var() 也是消费。漏掉这条会把 11 个 --hljs-* 全部误报
        (":root{--c:1px}", "x color: var(--c, #fff)", None, {}, 0, "带回退的 var() 也算消费"),
        # 通道 3：@theme 注册项被内联进 utility，产物里没有 var() 引用
        ("@theme inline{--radius-lg:var(--app-radius-lg)}:root{--app-radius-lg:8px}",
         "x", ".rounded-lg{border-radius:var(--app-radius-lg)}", {}, 0,
         "@theme 注册项 → 不是死 token"),
        # 通道 4：只有构建产物里才有引用
        (":root{--x:1px}", "x", ".btn{color:var(--x)}", {}, 0,
         "产物里的 Tailwind 生成引用也算消费"),
        (":root{--keep:1px}", "x", None, {"--keep": "理由"}, 0, "豁免表覆盖 → 放行"),
        # 豁免过期：token 已经接上了，条目还在
        (":root{--wired:1px}", "x var(--wired)", None, {"--wired": "理由"}, 1,
         "豁免过期（已接上）→ 报红，防止豁免表变垃圾场"),
        # 豁免表里有一个 index.css 里根本不存在的名字
        (":root{}", "x", None, {"--ghost": "理由"}, 1,
         "豁免表指向不存在的 token → 报红"),
        # 嵌套花括号不能把 @theme 块的结束位置判错
        ("@theme{--a:1px}@theme{--b:calc(1px + 2px)}:root{}", "x", None, {}, 2,
         "多个 @theme 块各自成对"),
        # 回归：注释里写「原先这里还有 --tabular-nums: …」，被声明正则当成真声明，
        # 于是**已经删掉**的 token 继续被报成零消费。踩过一次，且报错完全指不到根因。
        (":root{--keep:1px}/* 原先还有 --tabular-nums: xxx; */", "x", None,
         {"--keep": "理由"}, 0, "注释里的声明不算数（已删 token 不再被报死）"),
        # 回归：注释里的 var() 也不能伪造出「有消费点」
        (":root{--real:1px}", "/* 曾经是 color: var(--real) */", None, {}, 1,
         "注释里的引用不算消费"),
    ]
    for css, src, prod, allow, expect, desc in dead_cases:
        total += 1
        files = [_FakeFile(src)] + ([_FakeFile(prod)] if prod else [])
        got = len(check_dead(files, css, allow))
        if got == expect:
            print(f"  [通过] {desc}")
        else:
            print(f"  [失败] {desc}：期望 {expect} 个问题，实得 {got}")
            failures += 1

    if failures:
        print(f"\n自测失败：{failures}/{total} 条不符预期")
        return 1
    print(f"\n自测通过：{total} 条合成样例全部符合预期")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="CSS 变量引用必须有定义")
    ap.add_argument("--self-test", action="store_true", help="跑合成样例回归")
    ap.add_argument("--report", action="store_true", help="打印引用统计")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    if not CSS_FILE.exists():
        print(f"找不到 {CSS_FILE}，无法取自定义属性定义", file=sys.stderr)
        return 1

    files = source_files()
    if not files:
        print("src/ 下没有找到 .ts/.tsx 源文件，扫描范围可能已失效", file=sys.stderr)
        return 1

    css_text = CSS_FILE.read_text(encoding="utf-8", errors="replace")
    defined = defined_vars(css_text)
    refs = scan_refs(files)

    # 反向检查需要 index.css 自身 + 构建产物。产物是**最终形态**：
    # Tailwind 生成的引用只在 dist 里出现，源码里看不到（与 check-font-sizes
    # 的产物层校验同源，理由见 §6.9.6）。产物缺失时明确降级，不静默当成「无引用」。
    dead_surface = [CSS_FILE] + files
    product_css = sorted(DIST_ASSETS.glob("*.css")) if DIST_ASSETS.is_dir() else []
    allowlist = load_allowlist()

    if args.report:
        theme = theme_registered_vars(css_text)
        print(f"index.css 声明 {len(defined)} 个自定义属性；"
              f"src/ 引用 {len(refs)} 个不同名字，共 {sum(len(v) for v in refs.values())} 处；"
              f"@theme 注册 {len(theme)} 个；"
              f"构建产物 {len(product_css)} 个 CSS"
              + ("（**未找到 dist，反向检查覆盖面不全**）" if not product_css else ""))
        for name, sites in sorted(refs.items(), key=lambda kv: -len(kv[1])):
            mark = "OK " if name in defined else "缺失"
            print(f"  [{mark}] {name}: {len(sites)} 处")

    problems = check(files, css_text)
    problems += check_dead(dead_surface, css_text, allowlist, product_css)
    if problems:
        print("CSS 变量门禁失败：\n- " + "\n- ".join(problems), file=sys.stderr)
        print(
            "\n提示（正向）：自定义属性未定义时浏览器**静默作废**整条声明，"
            "元素退回默认态——没有构建错误也没有测试失败。\n"
            "      要么在 index.css 补定义，要么改用已有的 token。\n"
            "提示（反向）：定义了却没人用的 token 对界面的作用是零，"
            "而且是零信号——index.css 里那行白纸黑字，读代码的人会以为它生效了。\n"
            f"      确认要保留就在 {ALLOWLIST.relative_to(REPO)} 里写明 reason。",
            file=sys.stderr,
        )
        return 1

    print(f"CSS 变量门禁通过：{len(files)} 个源文件、{sum(len(v) for v in refs.values())} 处引用，"
          f"全部有定义；{len(defined)} 个声明全部有消费点"
          f"（豁免 {len(allowlist)} 条）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
