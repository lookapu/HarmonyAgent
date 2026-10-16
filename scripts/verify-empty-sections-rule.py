#!/usr/bin/env python3
"""验证 `check-docs.py::check_empty_sections` 这条判据**是不是真的承重**。

## 这道工具解决什么

静态判据最常见的两种死法：

1. **写了但没人用**（永远绿）——合成用例全过，真源上却一条都抓不到。
2. **看着承重其实没人承**——文档里声称它守住 X，真去掉 X 才发现反应是 0。

第 2 种更危险：它会让人把一条**真正承重**的守卫写成「实测不承重」再删掉。
本项目的真实经历：围栏感知那条守卫，第一次用临时命令测出来是
「去掉后真源 0 条误报」，差点据此判定它是装饰——
**真因是那条临时命令的转义没匹配上，`.replace()` 是空操作，两版代码一模一样。**

## 因此本工具的硬约束

- 每个变异都必须**先断言真的变了**：锚点在源码中出现且仅出现 1 次，
  且 `mutated != original`。**断言不过直接判失败，绝不静默跳过。**
- 每条守卫报**两个维度**，因为它们可以只有其一：
  - **真源**（`docs/*.md` + `README*.md` + `CHANGELOG*.md`）
  - **合成用例**（`self_test()`）
  只报一个维度就会得出偏的结论：只报真源，会把「仅由合成用例承重」
  误判成「不承重」；只报合成，会漏掉「真源上其实有 4 处」这种关键事实。
- 真源注入用**字节级备份/还原**并核对 sha256，绝不用 git 还原
  （工作区有未提交改动，`git checkout` 会把别人的编辑一起抹掉）。

## 用法

    python3 scripts/verify-empty-sections-rule.py
退出码：0=全部承重；1=有守卫无人承重 / 变异没生效 / 还原不一致。
"""
from __future__ import annotations

import hashlib
import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-docs.py"
LEDGER = ROOT / "docs" / "TECH_DEBT.md"

#: 真源注入用的「真缺陷」：必须是**真源里真实出现过**的写法。
#: 合成写法与真源写法不同，这条判据就只测了假想情形。
REAL_DEFECT_ANCHOR = "### P-6 语义检索要不要**版本感知**（第二十三批提出 → **第二十四批撤回重开**）"
REAL_DEFECT_INJECT = (
    "### P-6 语义检索要不要**版本感知**（第二十三批提出，**数据已备好，等拍板**）\n\n"
    + REAL_DEFECT_ANCHOR
)


def load_module(source: str):
    spec = importlib.util.spec_from_loader("check_docs_variant", loader=None)
    mod = importlib.util.module_from_spec(spec)
    # check-docs.py 顶层用 __file__ 算 ROOT，exec 时必须先塞进命名空间
    mod.__dict__["__file__"] = str(SCRIPT)
    exec(compile(source, str(SCRIPT), "exec"), mod.__dict__)  # noqa: S102
    return mod


def mutate(original: str, old: str, new: str) -> str | None:
    """做变异。锚点不唯一/不存在、或变异后没变化 ⇒ 返回 None（= 没生效）。"""
    if original.count(old) != 1:
        return None
    out = original.replace(old, new, 1)
    return out if out != original else None


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")

    original = SCRIPT.read_text(encoding="utf-8")
    mod = load_module(original)
    problems: list[str] = []

    # ── M0 基线：当前真源必须全绿，否则下面的变异无从谈起 ──────────────────
    base = mod.check_empty_sections(ROOT)
    print(f"[M0] 基线（当前真源）空章节数 = {len(base)}")
    for p in base[:5]:
        print("     " + p)
    if base:
        problems.append("M0 基线本该为 0：真源已存在空章节，先修再谈判据")

    # ── M1 真源注入：塞回真实缺陷，必须报红，且还原后字节一致 ─────────────
    backup = LEDGER.read_bytes()
    orig_sha = hashlib.sha256(backup).hexdigest()
    text = backup.decode("utf-8")
    if REAL_DEFECT_ANCHOR not in text:
        problems.append("M1 锚点未命中：台账结构变了，先更新 REAL_DEFECT_ANCHOR")
    else:
        LEDGER.write_text(
            text.replace(REAL_DEFECT_ANCHOR, REAL_DEFECT_INJECT, 1),
            encoding="utf-8",
            newline="",
        )
        try:
            found = mod.check_empty_sections(ROOT)
        finally:
            LEDGER.write_bytes(backup)
        print(f"[M1] 注入真实缺陷后命中数 = {len(found)}")
        for p in found[:3]:
            print("     " + p)
        if not any("空章节" in p and LEDGER.name in p for p in found):
            problems.append("M1 注入真源缺陷未报红")
        restored = hashlib.sha256(LEDGER.read_bytes()).hexdigest()
        print(f"[M1] 还原后 sha256 一致 = {restored == orig_sha}")
        if restored != orig_sha:
            problems.append("M1 还原后字节不一致")

    # ── 反向变异：削弱判据，必须在**至少一个**维度上开始报错 ────────────────
    variants = [
        ("M2", "子标题判定改成「下一行是标题就收尾」",
         "                if lv2 <= level:", "                if True:",
         "「#### 子标题不算正文」"),
        ("M3", "把 H1 也纳入判定",
         "            if level < 2:", "            if level < 1:",
         "「H1 不参与」"),
        ("M4", "代码围栏不再跳过",
         '            if line.lstrip().startswith("```"):', "            if False:",
         "「围栏内的 # 不是标题」"),
    ]
    for tag, desc, old, new, guard in variants:
        mutated = mutate(original, old, new)
        if mutated is None:
            problems.append(f"{tag} 变异没生效（锚点不唯一/不存在，或替换为空操作）")
            print(f"[{tag}] 变异没生效：{desc}")
            continue
        m = load_module(mutated)
        real_hits = m.check_empty_sections(ROOT)
        try:
            m.self_test()
            red, why = False, "self_test() 仍全绿"
        except AssertionError as e:
            red, why = True, str(e).splitlines()[0][:70]
        except Exception as e:  # noqa: BLE001
            red, why = True, f"{type(e).__name__}: {e}"[:70]
        print(f"[{tag}] 削弱守卫：{guard}（{desc}）")
        print(f"     合成用例：{'报红 ✓' if red else '仍全绿 ✗'}（{why}）")
        print(f"     真源误报：{len(real_hits)} 条")
        for p in real_hits[:2]:
            print("       " + p)
        if not red and not real_hits:
            problems.append(f"{tag}「{guard}」自测未红且真源零误报 ⇒ 无人承重")
        elif not red:
            print("     └ 仅真源承重：合成用例没覆盖到它")
        elif not real_hits:
            print(f"     └ 仅合成用例承重：真源语料里目前没有这种形状，"
                  f"它防的是将来出现该形状")
        else:
            print(f"     └ 真源也承重")

    print("---")
    if problems:
        print(f"判据未全部承重（{len(problems)} 项）：")
        for p in problems:
            print(f"- {p}")
        return 1
    print("三条边界守卫均承重（真源 / 合成 两个维度分别标注）")
    return 0


if __name__ == "__main__":
    sys.exit(main())