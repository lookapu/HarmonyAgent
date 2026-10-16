#!/usr/bin/env python3
"""第 15 道门禁：懒加载必须真的懒。

## 为什么要这道门

2026-10-01 真实踩到过一个**静默**缺陷（§6.35.1）：`Home.tsx` 里三块面板各写了一行
`lazy(() => import('.../devicePanels'))`，看着是「分别懒加载」，
而 Rollup 对**同一模块**的多次 `import()` 只产出一个 chunk（63.6KB）。
于是那三行 lazy 全部白写——打开设备面板会把分析面板和符号面板一起下载。

那个缺陷 `tsc` / `eslint` / `vitest` / `build` / 体积门禁**全绿**：
「同一个模块」在类型和语义上都没错，只是**没分包**。

## 契约（2 条）

1. **每个 `lazy(() => import('X'))` 的 X 必须解析得到一个真实文件。**
   拼错的路径不会让任何一道现有门禁报红——它在构建时可能仍能解析，
   却让面板在运行时**永远不出现**。
2. **X 不得从 `main.tsx` 出发被**静态** import 触达。**
   静态可达的模块已经进了首屏包，`lazy()` 是**空操作**：
   既没有省下载量，还多了一层 Suspense 复杂度。

## 判据的已知边界（本门**不**判的一件事）

**「N 行 lazy 指向 N 个不同模块」本门不判。** 多个组件共用一个懒加载模块
是**合法**的写法——有时你就是要它们一起下载。
`Home.tsx` 那三行的问题不在于「指向同一个模块」，而在于**写的时候以为它们会分包**。
那是**意图与事实的偏差**，机械判不出来：把三块面板合回一个文件在语义上完全正确。

所以本门只拦「客观为空操作」的情况。要表达「这几块必须各自成块」，
靠的是模块边界（拆文件），以及 §6.35.1 那条注释里写明的理由。
**门禁可要求「书面理由」，不可硬指定统一答案**——
硬指定会逼人把合法的共用写法也改掉。

## 扫描范围失效必须报红

一个 `lazy()` 都扫不到时，本门**报「扫描范围失效」并退出非零**，
而不是安静地通过——「全部通过」和「没东西可查」在退出码上完全一样。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path, PurePosixPath
from posixpath import normpath

REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "src"
ENTRY = SRC / "main.tsx"
EXTS = (".ts", ".tsx", ".js", ".jsx")

# `lazy(() => import('X'))` / `lazy(() => import('X').then(m => ...))`
LAZY_RE = re.compile(r"\blazy\s*\(\s*\(\s*\)\s*=>\s*import\s*\(\s*(['\"])([^'\"]+)\1\s*\)")
# 动态 import 但**不是** lazy 的（预取、命名导入等）——不参与契约
DYNAMIC_RE = re.compile(r"\bimport\s*\(\s*(['\"])([^'\"]+)\1\s*\)")
# 静态 import：`import X from 'p'` / `import type X from 'p'` / `import 'p'`
IMPORT_RE = re.compile(
    r"^[ \t]*import\b(?P<body>[^;]*?)\bfrom\s*['\"](?P<path>[^'\"]+)['\"]",
    re.M,
)
BARE_IMPORT_RE = re.compile(r"^[ \t]*import\s*['\"](?P<path>[^'\"]+)['\"]", re.M)
EXPORT_FROM_RE = re.compile(
    r"^[ \t]*export\b[^;\n]*?\bfrom\s*['\"](?P<path>[^'\"]+)['\"]", re.M
)

RESOLVE_EXT = (".tsx", ".ts", ".jsx", ".js", "/index.tsx", "/index.ts", "/index.jsx", "/index.js")


def iter_sources() -> list[Path]:
    return sorted(p for p in SRC.rglob("*") if p.suffix in EXTS and p.is_file())


def resolve_map(spec: str, importer: Path, files: dict[Path, str]) -> Path | None:
    """把 import 说明符解析成与 importer 同基准的、存在于 `files` 里的文件。

    **纯内存，不读磁盘、不调 `.resolve()`**：自测用的是虚拟根下的合成路径，
    一旦 resolve 成「相对 CWD 的绝对路径」，合成图就会和真实仓库同处一个命名空间，
    可达性结论被真源污染。自测的图必须与真源**完全隔离**。
    """
    if not spec.startswith("."):
        return None  # 本项目没有路径别名；裸包名不归本门管
    base = Path(normpath(str(importer.parent).replace("\\", "/") + "/" + spec))
    if base.suffix in EXTS and base in files:
        return base
    for ext in RESOLVE_EXT:
        cand = Path(normpath(str(base) + ext))
        if cand in files:
            return cand
    return None


def static_targets(text: str, importer: Path, files: dict[Path, str]) -> set[Path]:
    """本文件静态 import 到的模块（`import type` 整体不计——它不进产物）。"""
    out: set[Path] = set()
    for m in IMPORT_RE.finditer(text):
        if re.match(r"\s*type\s", m.group("body")):
            continue
        r = resolve_map(m.group("path"), importer, files)
        if r:
            out.add(r)
    for rx in (BARE_IMPORT_RE, EXPORT_FROM_RE):
        for m in rx.finditer(text):
            r = resolve_map(m.group("path"), importer, files)
            if r:
                out.add(r)
    return out


def lazy_targets(text: str, importer: Path, files: dict[Path, str]) -> list[tuple[int, str, Path | None]]:
    """返回 [(行号, 说明符, 解析结果 or None)]。"""
    out = []
    for m in LAZY_RE.finditer(text):
        line = text.count("\n", 0, m.start()) + 1
        out.append((line, m.group(2), resolve_map(m.group(2), importer, files)))
    return out


def reachable_from(entry: Path, files: dict[Path, str]) -> set[Path]:
    """从入口出发沿静态 import 做可达性闭包。**纯内存**，不读磁盘。

    自测必须能注入自己的图：第一版这里直接读真实仓库，于是合成用例的
    可达性被真源污染——「应该报红」的用例全都不红，而门禁看起来是绿的。
    **判据里读外部世界的步骤必须可注入**，这条规矩自己也得守。
    """
    seen: set[Path] = set()
    stack = [entry]
    while stack:
        cur = stack.pop()
        if cur in seen or cur not in files:
            continue
        seen.add(cur)
        stack.extend(static_targets(files[cur], cur, files) - seen)
    return seen


def strip_comments(text: str) -> str:
    """粗略去块注释与行注释：注释里写的 import()/lazy 不能算真代码。

    注意这里只做**行数对齐**用的替换（用等长空格保住行号），
    不追求正确解析——真正的判据是语法级的 `^import`，注释里的不影响。
    """
    def blank(m: re.Match) -> str:
        return re.sub(r"[^\n]", " ", m.group(0))
    text = re.sub(r"/\*.*?\*/", blank, text, flags=re.S)
    text = re.sub(r"//[^\n]*", blank, text)
    return text


def rel_of(path: Path) -> str:
    """尽量给仓库相对路径；自测用的是合成相对路径，会抛 ValueError。"""
    try:
        return path.relative_to(REPO).as_posix()
    except ValueError:
        return path.as_posix()


def run(files: dict[Path, str], entry: Path) -> list[str]:
    problems: list[str] = []
    reach = reachable_from(entry, files)
    total_lazy = 0
    for path, raw in sorted(files.items()):
        text = strip_comments(raw)
        for line, spec, target in lazy_targets(text, path, files):
            total_lazy += 1
            rel = rel_of(path)
            if target is None:
                problems.append(
                    f"{rel}:{line} 懒加载目标解析不到文件：{spec!r}"
                    f"（面板会永远不出现，且现有门禁都不会报红）"
                )
            elif target in reach:
                problems.append(
                    f"{rel}:{line} 懒加载目标 {spec!r} 已被静态 import 触达，"
                    f"lazy() 是空操作：它已经在首屏包里，既没省下载量又多一层 Suspense"
                )
    if total_lazy == 0:
        problems.append(
            "扫描范围失效：src/ 下一个 lazy(() => import(...)) 都没有——"
            "「没东西可查」和「全部通过」在退出码上长得一模一样"
        )
    return problems


# ============================ 自测 ============================
#: 合成图。`main.tsx` 是入口；`reached/` 是从入口静态可达的，`free/` 不是。
CASES: list[tuple[str, dict[str, str], bool, str]] = [
    (
        "懒加载目标不在静态图里 → 通过",
        {
            "src/main.tsx": "import { A } from './reached/a';\nA();\n",
            "src/reached/a.tsx": "export const A = 1;\n",
            "src/panels/x.tsx": "const X = lazy(() => import('../free/x'))\n",
            "src/free/x.tsx": "export default function X() {}\n",
        },
        True, "通过",
    ),
    (
        "懒加载目标被静态 import 触达 → 报红（契约 2）",
        {
            "src/main.tsx": "import { X } from './free/x';\nX();\n",
            "src/free/x.tsx": "export const X = 1;\n",
            "src/panels/x.tsx": "const X = lazy(() => import('../free/x'))\n",
        },
        False, "空操作",
    ),
    (
        "间接静态触达（经两层）也算触达",
        {
            "src/main.tsx": "import './reached/a';\n",
            "src/reached/a.tsx": "import './b';\nexport const a = 1;\n",
            "src/reached/b.ts": "export const b = 2;\n",
            "src/panels/x.tsx": "const X = lazy(() => import('../reached/b'))\n",
        },
        False, "空操作",
    ),
    (
        "路径拼错 → 报红（契约 1）",
        {
            "src/main.tsx": "import './reached/a';\n",
            "src/reached/a.tsx": "export const a = 1;\n",
            "src/panels/x.tsx": "const X = lazy(() => import('../nope/missing'))\n",
        },
        False, "解析不到文件",
    ),
    (
        "目录形式（有 index）能解析",
        {
            "src/main.tsx": "import './reached/a';\n",
            "src/reached/a.tsx": "export const a = 1;\n",
            "src/panels/x.tsx": "const X = lazy(() => import('../free/dir'))\n",
            "src/free/dir/index.tsx": "export default function D() {}\n",
        },
        True, "通过",
    ),
    (
        "`import type` 不算静态触达（它不进产物）",
        {
            "src/main.tsx": "import type { T } from './free/x';\nexport type { T };\n",
            "src/free/x.tsx": "export type T = number;\n",
            "src/panels/x.tsx": "const X = lazy(() => import('../free/x'))\n",
        },
        True, "通过",
    ),
    (
        "`export ... from` 也算静态触达",
        {
            "src/main.tsx": "export * from './free/x';\n",
            "src/free/x.tsx": "export const x = 1;\n",
            "src/panels/x.tsx": "const X = lazy(() => import('../free/x'))\n",
        },
        False, "空操作",
    ),
    (
        "注释里的 lazy 不算（行数对齐不塌）",
        {
            "src/main.tsx": "import './reached/a';\n",
            "src/reached/a.tsx": "export const a = 1;\n",
            "src/panels/x.tsx": (
                "/*\n * const Y = lazy(() => import('../free/y'))\n */\n"
                "const X = lazy(() => import('../free/x'))\n"
            ),
            "src/free/x.tsx": "export default function X() {}\n",
        },
        True, "通过",
    ),
    (
        "一个 lazy 都没有 → 报「扫描范围失效」",
        {"src/main.tsx": "import './reached/a';\n", "src/reached/a.tsx": "export const a = 1;\n"},
        False, "扫描范围失效",
    ),
    (
        "非 lazy 的动态 import（预取）不参与契约",
        {
            "src/main.tsx": "import { X } from './free/x';\nvoid import('./free/x');\nX();\n",
            "src/free/x.tsx": "export const X = 1;\n",
            # 必须**另有**一处合法 lazy：否则「扫描范围失效」护栏会正确地报红，
            # 而那条护栏报红与「预取被误判」在期望值上撞车。
            "src/panels/y.tsx": "const Y = lazy(() => import('../free/y'))\n",
            "src/free/y.tsx": "export default function Y() {}\n",
        },
        True, "通过",
    ),
]


#: 自测专用虚拟根。磁盘上不存在，但路径是**绝对**的，
#: 因此合成图与真实仓库不可能撞上同一个命名空间。
VROOT = REPO / "__lazygraph_self_test__"


def mk(tree: dict[str, str]) -> tuple[dict[Path, str], Path]:
    files = {VROOT / k: v for k, v in tree.items()}
    return files, VROOT / "src" / "main.tsx"


def self_test() -> int:
    passed, failures = 0, []
    for name, tree, should_pass, keyword in CASES:
        files, entry = mk(tree)
        problems = run(files, entry)
        got_pass = not problems
        if got_pass != should_pass:
            failures.append(f"{name}: 期望{'通过' if should_pass else '报红'}，实得{problems}")
            continue
        if not should_pass and keyword not in " ".join(problems):
            failures.append(f"{name}: 报错信息里应提到 {keyword!r}，实得 {problems}")
            continue
        passed += 1

    # 账目护栏：任何「输出 N/N」都必须对得上
    total = len(CASES)
    if passed + len(failures) != total:
        failures.append(f"用例账目不平：passed {passed} + failures {len(failures)} != total {total}")

    print(f"自测 {passed}/{total}")
    for f in failures:
        print(f"  [FAIL] {f}")
    return 0 if passed == total and not failures else 1


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8")
        except (AttributeError, ValueError):
            pass

    if "--self-test" in sys.argv:
        return self_test()

    if not SRC.is_dir():
        print(f"找不到 {SRC.relative_to(REPO)}", file=sys.stderr)
        return 1

    files = {}
    for p in iter_sources():
        try:
            rp = p.resolve()
        except OSError:
            continue
        try:
            files[rp] = rp.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue

    entry = ENTRY.resolve()
    if entry not in files:
        print(f"找不到入口 {ENTRY.relative_to(REPO)}，无法判定静态可达性", file=sys.stderr)
        return 1

    problems = run(files, entry)
    if problems:
        print(f"懒加载门禁失败：{len(problems)} 项")
        for p in problems:
            print(f"- {p}")
        return 1
    n = sum(len(lazy_targets(strip_comments(t), f, files)) for f, t in files.items())
    print(f"懒加载门禁通过：{n} 处 lazy() 目标均解析得到且未被静态 import 触达")
    return 0


if __name__ == "__main__":
    sys.exit(main())
