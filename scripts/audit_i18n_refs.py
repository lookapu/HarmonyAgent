#!/usr/bin/env python3
"""i18n 引用扫描器：判定每个键是「被引用」还是「**可证明为死**」。

## 为什么需要它

搜字面量零命中 **不等于**死键。本仓既有
`t(\`home.cat_${e.category}\`)` 这样的**模板字面量调用**，
也有 `t(item.labelKey)` / `t(failKey)` 这样的**完全动态取键**。
只搜字面量会同时犯两个方向的错：把活键报成死的（更误导），把死键留在原地。

## 「可证明为死」的三条判据（必须同时满足）

1. 没有任何**完整点号字面量**引用它
2. 不在任何一个**已枚举的模板族**里（见下）
3. 全仓的**动态取键出口全部已解析**（`RESOLVED_DYNAMIC`）

## 三处最容易写错的地方（本脚本都踩过）

- **模板前缀不能靠「含点」判**：Tailwind 类名里到处是点
  （`px-2 py-0.5 rounded ` 的 `0.5`），第一版因此把 50 多个类名串当成 i18n 前缀，
  248 个键被误判成「无法证死」，包括明显在用的 `home.projectName`。
  现在要求前缀**只由标识符/点/下划线组成**且**以 `.` 或 `_` 收尾**。
- **单字母前缀是假阳性**：`a${Date.now()}` 是 ID 前缀不是 i18n 前缀，
  而 `a` 是 `about.*` / `apiKb.*` 的前缀。要求前缀**必须含点**。
- **模板族要枚举，不要前缀匹配**：`t(\`home.${tp.i18nKey}\`)` 只能产生
  `home.ruleTemplate.*` 这几十个具体键，**不是**全部 `home.*`。
  用前缀 `home.` 会把 `home.switchModel` 这类真死键一并藏起来。

## 动态出口的处置

`RESOLVED_DYNAMIC` 是一份**人工核验过的出口清单**，每条写明「实参取值从哪来」。
**清单之外只要多出一个 `t(<裸变量>)`，本脚本 exit 2**——
因为那意味着此前的死键结论全部失效，而退出码是 0 的话没人会发现。

新增动态出口时的正确做法：先追到它的值来自哪个字面量集合，
把那条集合里**没被字面量扫描覆盖到**的值补进 `EXTRA_COVERED`（或直接确认它们已被覆盖），
再把出口登记进来。**不要为了让脚本变绿而删掉一个出口。**
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKIP_DIRS = {"node_modules", "dist", "target", ".git", "generated", "__snapshots__"}

#: 模板族：把「前缀匹配」换成「枚举取值」。
#: 键 = 产生该族的代码位置，值 = (静态前缀, 枚举来源文件, 枚举用的正则, 后缀模板列表)
TEMPLATE_FAMILIES = {
    "src/chat/components/dialogs.tsx": (
        "home.",
        "src/data/ruleTemplates.ts",
        r"""i18nKey:\s*['"]([\w.]+)['"]""",
        ["", ".desc"],
    ),
}

#: 已核验的动态取键出口：t(<实参>) -> 值的来源说明。
#: **新出口必须先进这里**，否则脚本 exit 2。
RESOLVED_DYNAMIC = {
    "t(item.labelKey)": "各页数据数组里的 `labelKey: 'x.y'` 字面量，已被字面量扫描覆盖",
    "t(k.labelKey)": "devicePanels 符号种类表的 `labelKey` 字面量",
    "t(it.descKey)": "HomeOverlays 空态项表的 `descKey` 字面量",
    "t(g.titleKey)": "HomeOverlays 分组表的 `titleKey` 字面量",
    "t(failKey)": "run(fn, 'lan.xxxFailed') 的**第二实参**是字面量",
    "t(reversibilityKey)": "impactDisplay() 返回对象里的字面量",
    "t(scopeKey)": "impactDisplay() 返回对象里的字面量",
    "t(row.labelKey)": "LicensePage 权益行的 `labelKey` 字面量",
    "t(f.labelKey)": "LimitsPage 限流项表的 `labelKey` 字面量",
    "t(f.descKey)": "LimitsPage 限流项表的 `descKey` 字面量",
    "t(f.unit)": "LimitsPage 限流项表的 `unit` 字面量",
    "t(group.groupKey)": "LimitsPage 分组表的 `groupKey` 字面量",
    "t(cat.labelKey)": "ProvidersPage 分类表的 `labelKey` 字面量",
    "t(label)": "ReproductionBundlesPage 的 `[['includeMessages','repro.messages'], …]` 元组数组",
}

LITERAL_RE = re.compile(r"""['"`]([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)['"`]""")
TPL_DOLLAR_RE = re.compile(r"`([^`]*\$\{[^`]*)`")
PREFIX_SHAPE_RE = re.compile(r"^[A-Za-z0-9_]+\.[A-Za-z0-9_.]*$")
T_ARG_RE = re.compile(r"\bt\(\s*([A-Za-z_$][\w$]*(?:\s*\[[^\]]*\])?(?:\.[\w$]+)*)\s*[,)]")
#: i18next 复数后缀。`foo` / `foo_other` 并存时，**基键可以不存在**——
#: i18next 会按 count 落到 `_one` / `_other` 上。这类键**永远不会被字面量引用**，
#: 却被「搜不到引用」的判据判成死键。删了它，界面上会直接显示键名本身。
PLURAL_RE = re.compile(r"_(zero|one|two|few|many|other)$")
#: `t(...)` 的实参里，**字符串字面量作为 `+` 的操作数** ⇒ 键是拼出来的。
#: 必须限定在实参**内部**：整行的 `t(busy ? 'a' : 'b') + ': ' + e` 拼的是**译文**不是键，
#: 第一版对整行判，于是三个三元表达式被误报成动态出口。
CONCAT_IN_ARG_RE = re.compile(r"""['"`][^'"`]*['"`]\s*\+\s*|\+\s*['"`][^'"`]*['"`]\s*\+\s*""")
#: 实参里是三元：`cond ? 'a' : 'b'` —— 两支都是字面量，**不是**动态
TERNARY_RE = re.compile(r"\?.*:")


def flatten(node, prefix=""):
    out = {}
    for k, v in node.items():
        key = f"{prefix}.{k}" if prefix else k
        if isinstance(v, dict):
            out.update(flatten(v, key))
        elif isinstance(v, str):
            out[key] = v
    return out


def t_arg(code: str) -> str:
    """切出 `t(...)` 的实参文本（跟括号、跟引号）。

    正则 `\\bt\\(([^)]*)\\)` 在 `t('a', { x: f(1) })` 上会截断，
    截断后再判拼接/三元都会得到错误结论——**实参切错，后面全错**。
    """
    m = re.search(r"\bt\(", code)
    if not m:
        return ""
    i = m.end() - 1
    depth = 0
    in_str: str | None = None
    esc = False
    for j in range(i, len(code)):
        c = code[j]
        if in_str:
            if esc:
                esc = False
            elif c == "\\":
                esc = True
            elif c == in_str:
                in_str = None
            continue
        if c in "'\"`":
            in_str = c
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return code[i + 1 : j]
    return ""


def t_key_arg(arg: str) -> str:
    """从实参里切出**键**那一段：第一个顶层逗号之前。

    i18next 的实参形态是 `t(键, { 插值 })`。插值里可以有任意表达式，
    包括 `'#' + tk.id`——那是**拼插值**，不是拼键。
    不切这一刀，`t('config.lanTokenRevokeConfirm', { name: tk.name || '#' + tk.id })`
    会被判成动态出口（第三次栽在「表达式作用域切错」这一族）。
    """
    depth = 0
    in_str: str | None = None
    esc = False
    for j, c in enumerate(arg):
        if in_str:
            if esc:
                esc = False
            elif c == "\\":
                esc = True
            elif c == in_str:
                in_str = None
            continue
        if c in "'\"`":
            in_str = c
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            return arg[:j]
    return arg


def sources():
    out = []
    for pat in ("**/*.ts", "**/*.tsx", "**/*.js", "**/*.mjs"):
        for p in sorted(ROOT.glob(f"src/{pat}")):
            if any(part in SKIP_DIRS for part in p.parts) or ".test." in p.name:
                continue
            out.append(p)
    return out


def main() -> int:
    out_json = None
    args = [a for a in sys.argv[1:]]
    if "--json" in args:
        i = args.index("--json")
        out_json = Path(args[i + 1]) if i + 1 < len(args) else None
    en = flatten(json.loads((ROOT / "src/i18n/en.json").read_text(encoding="utf-8")))
    zh = flatten(json.loads((ROOT / "src/i18n/zh.json").read_text(encoding="utf-8")))
    keys = set(en) | set(zh)

    referenced: set[str] = set()
    unresolved_dynamic: list[str] = []
    unresolved_tpl: list[str] = []
    unresolved_tpl_prefixes: set[str] = set()

    for p in sources():
        rel = p.relative_to(ROOT).as_posix()
        for i, line in enumerate(p.read_text(encoding="utf-8").splitlines(), 1):
            code = line.split("//")[0]
            if not code.strip():
                continue
            for m in LITERAL_RE.finditer(code):
                if m.group(1) in keys:
                    referenced.add(m.group(1))
            for m in TPL_DOLLAR_RE.finditer(code):
                static = m.group(1).split("${")[0]
                if not PREFIX_SHAPE_RE.match(static):
                    continue
                if not any(k.startswith(static) for k in keys):
                    continue
                if rel in TEMPLATE_FAMILIES and TEMPLATE_FAMILIES[rel][0] == static:
                    continue  # 走枚举分支，不进保守覆盖
                unresolved_tpl_prefixes.add(static)
            for m in T_ARG_RE.finditer(code):
                arg = m.group(1)
                if any(q in arg for q in "'\"`"):
                    continue
                # 报**这个正则实际匹配到的**实参，不要报行内第一个 `t(` 的键——
                # `setError(t('a') + \`: ${t(f.labelKey)}\`)` 一行有两个 t()，
                # 取行内第一个会把 `f.labelKey` 报成 `t('a')`。
                unresolved_dynamic.append(f"{rel}:{i}  t({arg})")
            if CONCAT_IN_ARG_RE.search(code):
                keypart = t_key_arg(t_arg(code))
                if keypart and not TERNARY_RE.search(keypart) and CONCAT_IN_ARG_RE.search(keypart):
                    unresolved_dynamic.append(f"{rel}:{i}  实参拼接 t({keypart[:60].strip()})")

    # ---- 模板族两档处置 ----
    # ① 已枚举的族用**枚举**（更紧，能让族外的键掉进死键候选）
    # ② 未枚举的族用**前缀覆盖**（保守、零维护——宁可漏判不可错判）
    #    本仓有 40+ 个模板族，逐个枚举是独立工程；在枚举之前，
    #    前缀覆盖是唯一**不会把活键报成死的**做法。
    enumerated: set[str] = set()
    for _rel, (prefix, src_rel, rx, suffixes) in TEMPLATE_FAMILIES.items():
        text = (ROOT / src_rel).read_text(encoding="utf-8")
        for m in re.finditer(rx, text):
            for suf in suffixes:
                enumerated.add(prefix + m.group(1) + suf)
    enumerated &= keys

    prefixed: set[str] = set()
    for k in keys:
        for _site in unresolved_tpl_prefixes:
            if k.startswith(_site):
                prefixed.add(k)
                break

    # ---- 复数键保护：永远不进死键候选 ----
    plural_keys = {k for k in keys if PLURAL_RE.search(k)}

    provably_dead = sorted(keys - referenced - enumerated - prefixed - plural_keys)
    undecided = sorted((keys & (enumerated | prefixed | plural_keys)) - referenced)
    dangling = sorted(referenced - keys)

    print(f"i18n 键 {len(keys)}（en {len(en)} / zh {len(zh)}）")
    print(f"字面量引用 {len(referenced)}")
    print(f"模板族：已枚举 {len(enumerated)} 键 / 未枚举按前缀保守覆盖 {len(prefixed)} 键")
    print(f"复数键保护（_one/_other 等，永远无字面量引用）{len(plural_keys)} 个")
    print(f"未被覆盖（死键候选）{len(provably_dead)}")
    print(f"只靠模板族/复数约定存活（不可删）{len(undecided)}")
    print(f"代码引用但 i18n 无此键 {len(dangling)}: {dangling[:10]}")

    # ---- 动态出口必须全部登记 ----
    seen_dyn = set()
    for site in unresolved_dynamic:
        expr = re.search(r"t\(([^)]*)\)", site)
        if expr is None:
            continue
        seen_dyn.add(f"t({expr.group(1).strip()})")
    unlisted = sorted(seen_dyn - set(RESOLVED_DYNAMIC))
    stale = sorted(set(RESOLVED_DYNAMIC) - seen_dyn)
    if unlisted or stale or unresolved_tpl:
        print("\n[FAIL] 动态取键出口与登记清单对不上——死键结论失效，工具拒绝输出")
        for s in unlisted:
            print(f"  未登记的新出口：{s}  ← 追清它的值来源后再登记，不要删掉它来让脚本变绿")
        for s in stale:
            print(f"  清单里已不存在的出口：{s}  ← 代码已改，请删掉这条")
        for s in sorted(set(unresolved_tpl)):
            print(f"  模板：{s}")
        return 2

    print(
        f"\n已核验的动态出口 {len(RESOLVED_DYNAMIC)} 条，全部登记且全部仍存在"
    )

    print(f"\n### 死键候选（未被任何字面量或模板族覆盖）{len(provably_dead)} 个")
    for k in provably_dead:
        print(f"  {k}")
    print(f"\n### 只靠模板族/复数约定存活（**不可删**）{len(undecided)} 个")
    for k in undecided:
        print(f"  {k}")

    if out_json is not None:
        out_json.write_text(
            json.dumps(
                {
                    "total_keys": len(keys),
                    "literal_referenced": len(referenced),
                    "enumerated_family": len(enumerated),
                    "prefix_covered": len(prefixed),
                    "plural_protected": sorted(plural_keys),
                    "dead_candidates": provably_dead,
                    "undecided": undecided,
                },
                ensure_ascii=False,
                indent=2,
            ),
            encoding="utf-8",
            newline="",
        )
        print(f"\n机器可读结果已写入 {out_json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
