#!/usr/bin/env python3
"""i18n 措辞一致性门禁（第十六批建，第十八批扩到 7 条契约）。

`zh` 里同一句话对应多个键**是正常的**——导航项与页面标题本来就该用同一句话
（本仓库 120 组如此）。真正要管的是英文侧：同一个词在同一个界面里有两种写法。

漂移的症状是**纯视觉的**：没有编译错误、没有测试失败、没有任何地方报错，
用户只是看到按钮一会儿写 `Copy Path` 一会儿写 `Copy path`，
于是怀疑这个界面是不是没做完。

## 判据的出发点

**不是「同一个中文就该同一个英文词」，而是「两个键指的是不是同一界面里的同一个东西」。**

按钮 vs 状态、图例 vs 动作、面板 vs 表格列、集合 vs 单项——这些**本来就该不一样**。
中文过共享是**数据组织问题**，不是英文措辞问题；把两边硬拉平，
反而会在同一屏制造两个一模一样的标签去指两个不同的数。

实例：`cost.taskAvg` 与 `cost.toolAverageDuration` 中文都是「平均耗时」，
但前者取 `taskStats.avg_duration_ms`（任务级，同族 taskLegend/taskCost），
后者取 `reliability.tool_quality.average_duration_ms`（工具质量级，同族 toolSuccessRate）。
两个不同的数，英文的区分是对的。

所以本门禁**不追求「同中文必同英文」**，而是要求：
**每一个不同的，都必须写得出为什么不同。** 写在 `scripts/i18n-wording-explained.json`。

## 契约（7 条）

1. **仅大小写不同不算漂移**：当一组键的中文值相同、且它们的英文值
   **忽略大小写后完全相同**时，英文值必须**逐字符相同**。
   本仓库 2026-10-01 有 14 组 / 31 键违反，已统一到句首式。

2. **换词不算这里的事**：Delete / Removed、Revert / Revoke 这类**需要上下文**，
   交给契约 5 判。

3. **中英键集必须对齐**：任一侧缺的键，在界面上会**直接显示成键名本身**
   （i18next 的 fallback），排查起来像「翻译没加载」。
   唯一豁免是 i18next 的复数形式 `foo` / `foo_other`。

4. **英文值不得为空**：空字符串渲染出来是**一片空白**，
   而空白在界面上和「加载中」「已隐藏」长得一模一样。

5. **每一个冲突组都必须有书面理由**（第十八批新增）：中文值相同、英文不同的组，
   必须在 `scripts/i18n-wording-explained.json` 里有条目。
   **写不出理由的组就是待修的漂移**，报红。
   这一条把第十六批「清单在 docs/I18N_WORDING_AUDIT.md，靠人扫」变成机器可判的
   「0 组未解释」。

6. **解释表不得有失效条目**（第十八批新增）：`EXPLAINED` 里写了、但当前
   已经不存在对应冲突组的中文值，报红。
   允许清单不报错的后果是：组修好之后条目留在表里，
   于是后来人以为「这组已经解释过了」，其实解释的是已经不存在的现象。
   **判「还在」和判「不在」都要查**——只查一边时，「全绿」和「表已过期」长得一样。

7. **省略号必须是 U+2026**（第十八批新增）：全库 102 个键用 `…`（U+2026），
   只有 13 个「忙碌态」键用三个点 `...`，且集中在 `provider`/`version`/`health`
   三个文件的同一语义簇里——只改其中几个会在同文件内制造簇内不一致。
   白名单 `LITERAL_DOTS` 只收**字面三个点**（`sk-...` 是占位符里的省略，不是省略号），
   白名单条目失效同样报红。

## 复数键的取值口径

`home.batchLines` 在英文侧只以 `home.batchLines_other` 存在（i18next 复数约定）。
分类时按 `_other` 取值，于是「共 {{count}} 条」这一组是
`{{count}} lines` / `{{count}} messages` / `{{count}} entries` 三种——
它们指三类不同的实体，本来就该不同。

**第十八批之前，审计脚本在这组上显示「缺键」而门禁直接跳过整组**，
于是同一个数字在两处对不上（审计 50 组 / 门禁 49 组）。
根因是两份分类实现各跳各的。现在两份脚本共用 `classify_groups()`，
数字不再分叉。

## 判据的已知边界

- 判的是**同一句话的多个键之间**是否一致。**同一个键的措辞本身好不好，不判**——
  那需要母语者，且不是机械能做的。
- 契约 5 只要求「有书面理由」，**不判理由对不对**。理由写错（例如把真漂移
  说成「不同槽位」）门禁看不出来。理由本身在代码评审里过。
- 复数形式只认 `_other` 一种后缀。i18next 还支持 `_zero` / `_one` / `_few` 等，
  但中文语境下不会出现，需要时再扩。

## 用法

    python scripts/check-i18n-wording.py            # 全仓检查（CI 门禁）
    python scripts/check-i18n-wording.py --report   # 打印三档统计
    python scripts/check-i18n-wording.py --self-test
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ZH = REPO / "src" / "i18n" / "zh.json"
EN = REPO / "src" / "i18n" / "en.json"
EXPLAINED_JSON = REPO / "scripts" / "i18n-wording-explained.json"

#: i18next 英文复数后缀。中文无复数变化，故英文侧多出这些是**正确**的。
PLURAL_SUFFIX = "_other"

#: 英文值里允许出现**字面三个点**的键。这里的点不是省略号，是内容本身
#: （`sk-...` 是 API Key 占位符）。白名单条目失效时报红（契约 7）。
LITERAL_DOTS = frozenset({"provider.apiKeyPlaceholder"})

MISSING = "�缺键"


def flatten(node, prefix: str = "") -> dict[str, str]:
    out: dict[str, str] = {}
    for k, v in node.items():
        key = f"{prefix}.{k}" if prefix else k
        if isinstance(v, dict):
            out.update(flatten(v, key))
        elif isinstance(v, str):
            out[key] = v
    return out


def effective_en(en: dict[str, str], key: str) -> str:
    """取值口径：优先本键，否则按 i18next 复数约定取 `_other` 形式。

    合同：调用方传进来的 `en` 只读不改；缺键返回 MISSING 而不是空串，
    因为空串会被契约 4 抢走，措辞组就静默少了一组。
    """
    if key in en:
        return en[key]
    plural = key + PLURAL_SUFFIX
    if plural in en:
        return en[plural]
    return MISSING


def classify_groups(zh: dict[str, str], en: dict[str, str]):
    """把「同一中文值的多键组」分档。**唯一实现**——审计脚本 import 本函数。

    返回 (identical, case_only, diff_word, skipped)，每项是 (中文值, 键列表, 英文值列表)。
    `skipped` 是含空值的组（归契约 4，不在这里报两遍）。
    """
    groups: dict[str, list[str]] = {}
    for key, value in zh.items():
        if value.strip():
            groups.setdefault(value.strip(), []).append(key)

    identical: list = []
    case_only: list = []
    diff_word: list = []
    skipped: list = []
    for zv, keys in groups.items():
        if len(keys) < 2:
            continue
        vals = [effective_en(en, k) for k in keys]
        if any(v == "" or v == MISSING for v in vals):
            skipped.append((zv, keys, vals))
        elif len(set(vals)) == 1:
            identical.append((zv, keys, vals))
        elif len(set(v.lower() for v in vals)) == 1:
            case_only.append((zv, keys, vals))
        else:
            diff_word.append((zv, keys, vals))
    return identical, case_only, diff_word, skipped


class ExplainedTableError(RuntimeError):
    """解释表读不出来。**单独一类**，不与「i18n 漂移」混在同一个退出码里。"""


def load_explained(path: Path = EXPLAINED_JSON) -> dict[str, dict]:
    """读解释表。读不出来就抛 `ExplainedTableError`，**不要让 json 的 traceback 冒出去**。

    负向验证时撞出来的：往表里塞一条坏数据，门禁直接抛
    `JSONDecodeError: line 24 column 5`——退出码是红的（CI 照样拦住），
    但消息里既没有「哪个文件」、也没有「怎么修」，看起来像门禁自己坏了。
    配置文件坏了和判据坏了必须能一眼分开。
    """
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ExplainedTableError(f"解释表读不出来：{path}（{exc}）") from exc
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ExplainedTableError(
            f"解释表不是合法 JSON：{path} 第 {exc.lineno} 行第 {exc.colno} 列（{exc.msg}）"
        ) from exc
    groups = data.get("groups")
    if not isinstance(groups, dict):
        raise ExplainedTableError(f"解释表缺少顶层 groups 对象：{path}")
    bad = [k for k, v in groups.items() if not isinstance(v, dict) or "reason" not in v]
    if bad:
        raise ExplainedTableError(
            f"解释表里有条目缺 reason 字段：{path} 的 {bad[:3]}——"
            f"没有理由的条目等于没解释，门禁不该把它当已解释"
        )
    return groups


def check(
    zh: dict[str, str],
    en: dict[str, str],
    explained: dict[str, dict] | None = None,
    literal_dots: frozenset | None = None,
) -> list[str]:
    problems: list[str] = []

    # 元判据：判定面不能是空的
    if not zh or not en:
        return ["扫描范围失效：i18n 文件为空或结构变了——「全部通过」不等于「检查到了东西」"]

    if explained is None:
        explained = load_explained()
    if literal_dots is None:
        literal_dots = LITERAL_DOTS

    identical, case_only, diff_word, _skipped = classify_groups(zh, en)

    # 契约 1：仅大小写不同的组，英文必须逐字符相同
    for zv, keys, vals in case_only:
        problems.append(
            f"同一句话的英文只差大小写：{', '.join(keys)} -> {vals}（中文 {zv!r}）"
        )

    # 契约 5：每个冲突组都必须有书面理由
    live = {zv for zv, _keys, _vals in diff_word}
    for zv, keys, vals in diff_word:
        if zv not in explained:
            problems.append(
                f"同一句话的英文不一致且没有书面理由：{', '.join(keys)} -> {vals}（中文 {zv!r}）"
            )

    # 契约 6：解释表不得有失效条目
    for zv in sorted(set(explained) - live):
        problems.append(
            f"解释表条目已失效：{zv!r}——当前没有对应的英文不一致组。"
            f"要么 i18n 改好了（删掉这条），要么分组口径变了（重新判读）"
        )

    # 契约 7：省略号必须是 U+2026
    used_dots: set[str] = set()
    for key, value in en.items():
        if "..." in value:
            used_dots.add(key)
            if key not in literal_dots:
                problems.append(
                    f"英文值用了三个点而不是 U+2026 省略号：{key} = {value!r}"
                )
    for key in sorted(literal_dots - used_dots):
        problems.append(
            f"字面三点白名单条目已失效：{key}——它的值里已经没有 ... 了，"
            f"白名单留着会让真正的字面三点漏网"
        )

    # 契约 3：键集对齐，豁免英文侧的复数后缀
    def normalized(keys: set[str]) -> set[str]:
        return {re.sub(PLURAL_SUFFIX + r"$", "", k) for k in keys}

    zh_keys, en_keys = set(zh), set(en)
    only_zh = normalized(zh_keys) - normalized(en_keys)
    only_en = normalized(en_keys) - normalized(zh_keys)
    for key in sorted(only_zh)[:10]:
        problems.append(f"英文侧缺键：{key}（界面上会直接显示键名本身）")
    for key in sorted(only_en)[:10]:
        problems.append(f"中文侧缺键：{key}（界面会退回英文）")

    # 契约 4：英文值不得为空
    for key, value in en.items():
        if value == "":
            problems.append(f"英文值为空：{key}（渲染是一片空白，与「加载中」无法区分）")

    return problems


def stats(zh: dict[str, str], en: dict[str, str]) -> dict[str, int]:
    identical, case_only, diff_word, skipped = classify_groups(zh, en)
    explained = load_explained()
    live = {zv for zv, _k, _v in diff_word}
    return {
        "identical": len(identical),
        "case_only": len(case_only),
        "diff_word": len(diff_word),
        "skipped": len(skipped),
        "explained": len(live & set(explained)),
        "unexplained": len(live - set(explained)),
        "stale": len(set(explained) - live),
    }


# ============================ 自测 ============================

def self_test() -> int:
    # 干净基线：自测里每个「期望有问题」的用例都必须与它不同，
    # 否则注入没生效，用例会假通过。
    zh_a = {"nav.save": "保存", "home.save": "保存"}
    en_ok = {"nav.save": "Save", "home.save": "Save"}

    no_expl: dict = {}
    dots_none: frozenset = frozenset()

    cases: list[tuple[str, dict, dict, bool, str]] = [
        ("两侧完全一致不算问题", zh_a, en_ok, False, ""),
        ("仅大小写不同被拦", zh_a, {"nav.save": "Save", "home.save": "SAVE"}, True, "只差大小写"),
        ("英文侧缺键被拦", zh_a, {"nav.save": "Save"}, True, "英文侧缺键"),
        ("英文值为空被拦", {"a": "甲"}, {"a": ""}, True, "英文值为空"),
        ("扫描面为空被拦", {}, {}, True, "扫描范围失效"),
        ("单键不受重复判据影响", {"a": "甲", "b": "乙"}, {"a": "A", "b": "B"}, False, ""),
        # ---- 契约 5：冲突组必须有书面理由 ----
        (
            "换词且无理由被拦",
            zh_a,
            {"nav.save": "Save", "home.save": "Store"},
            True,
            "没有书面理由",
        ),
        (
            "换词但有理由则放行",
            zh_a,
            {"nav.save": "Save", "home.save": "Store"},
            False,
            "",
        ),
        # ---- 契约 6：解释表不得有失效条目 ----
        (
            "解释表有失效条目被拦",
            zh_a,
            {"nav.save": "Save", "home.save": "Store"},
            True,
            "解释表条目已失效",
        ),
        (
            "解释表条目全部在用则放行",
            {
                "nav.save": "保存",
                "home.save": "保存",
                "a.title": "标题",
                "b.title": "标题",
            },
            {
                "nav.save": "Save",
                "home.save": "Store",
                "a.title": "Title",
                "b.title": "Heading",
            },
            False,
            "",
        ),
        # ---- 契约 7：省略号 ----
        (
            "英文值用三个点被拦",
            {"a": "甲"},
            {"a": "Working..."},
            True,
            "U+2026",
        ),
        (
            "英文值用 U+2026 放行",
            {"a": "甲"},
            {"a": "Working…"},
            False,
            "",
        ),
        (
            "字面三点白名单放行",
            {"a": "甲"},
            {"a": "sk-... (optional)"},
            False,
            "",
        ),
        (
            "字面三点白名单失效被拦",
            {"a": "甲"},
            {"a": "Working…"},
            True,
            "白名单条目已失效",
        ),
        # ---- 复数取值口径 ----
        (
            "只以 _other 存在的键按 _other 取值，不算缺键",
            {"home.batchLines": "条 {{count}}", "home.auditCount": "条 {{count}}"},
            {"home.batchLines_other": "{{count}} lines", "home.auditCount": "{{count}} entries"},
            False,
            "",
        ),
    ]

    # 逐用例可注入的解释表 / 白名单：默认全空（最严格），个别用例给出条目。
    # 键是用例名——**不要靠用例名做隐式开关**：改一次用例名，
    # 下面这张表就静默失配，参数回到「最严格」，用例会以「期望无问题」失败。
    # 失败得响比悄悄改判据好，但更好的办法是让它根本没法悄悄改。
    per_case: dict[str, tuple[dict, frozenset]] = {
        "换词但有理由则放行": ({"保存": {"verdict": "different-slot", "reason": "x"}}, dots_none),
        "解释表有失效条目被拦": (
            {"已不存在的句子": {"verdict": "different-thing", "reason": "x"}},
            dots_none,
        ),
        "解释表条目全部在用则放行": (
            {
                "保存": {"verdict": "different-slot", "reason": "x"},
                "标题": {"verdict": "different-slot", "reason": "y"},
            },
            dots_none,
        ),
        "字面三点白名单放行": (no_expl, frozenset({"a"})),
        "字面三点白名单失效被拦": (no_expl, frozenset({"a"})),
        "只以 _other 存在的键按 _other 取值，不算缺键": (
            {"条 {{count}}": {"verdict": "different-thing", "reason": "x"}},
            dots_none,
        ),
    }

    passed = 0
    failures: list[str] = []
    seen: dict[str, str] = {}
    for name, zh, en, expect, keyword in cases:
        if name in per_case:
            kwargs = {"explained": per_case[name][0], "literal_dots": per_case[name][1]}
        else:
            kwargs = {"explained": no_expl, "literal_dots": dots_none}

        # 注入是否生效：判据只读 zh/en 时，「注入没生效」= 输入与干净样例相同；
        # 判据还读 explained/literal_dots 时，注入可能只落在那两个参数上，
        # 所以护栏必须看**整个输入**而不是只看 zh/en。
        # 反过来也要查「重复用例」：两条签名相同的用例里，
        # 后一条的注入百分之百没生效，而它可能照样「通过」。
        sig = json.dumps(
            [zh, en, kwargs["explained"], sorted(kwargs["literal_dots"])],
            sort_keys=True,
            ensure_ascii=False,
        )
        if sig in seen:
            failures.append(
                f"{name}: 输入与用例「{seen[sig]}」完全相同——本条注入不会生效，"
                f"结果与前一条同进同退"
            )
            continue
        seen[sig] = name
        if expect and kwargs["explained"] == no_expl and not kwargs["literal_dots"]:
            if zh == zh_a and en == en_ok:
                failures.append(f"{name}: 注入没生效（输入与干净样例相同）")
                continue

        problems = check(zh, en, **kwargs)
        if bool(problems) != expect:
            failures.append(f"{name}: 期望{'有问题' if expect else '无问题'} -> {problems}")
            continue
        if expect and keyword and not any(keyword in p for p in problems):
            failures.append(f"{name}: 报红了但没提到「{keyword}」-> {problems}")
            continue
        passed += 1

    # 解释表读不出来：必须抛本门自定义的错，且消息里要能看出是哪个文件、哪一行。
    # 这不是锦上添花——负向验证时真的撞到了：往表里塞一条坏数据，
    # 门禁抛的是 json 的 JSONDecodeError，退出码是红的（CI 照样拦住），
    # 但消息里既没有文件名也没有「怎么修」，看起来像门禁自己坏了。
    import tempfile

    explain_cases = [
        ("解释表不是合法 JSON", '{"groups": {\n  "x": \n}}', "第 3 行"),
        ("解释表缺 groups 顶层", '{"version": 1}', "缺少顶层 groups"),
        ("解释表条目缺 reason", '{"groups": {"\\u4e2d\\u6587": {"verdict": "x"}}}', "缺 reason 字段"),
        ("解释表文件不存在", None, "解释表读不出来"),
    ]
    for name, content, keyword in explain_cases:
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "explained.json"
            if content is not None:
                p.write_text(content, encoding="utf-8")
            try:
                load_explained(p)
            except ExplainedTableError as exc:
                if keyword in str(exc) and p.name in str(exc):
                    passed += 1
                else:
                    failures.append(
                        f"{name}: 抛了 ExplainedTableError 但消息没指向文件/位置"
                        f"（期望含 {keyword!r}）-> {exc}"
                    )
            except Exception as exc:  # noqa: BLE001
                failures.append(
                    f"{name}: 抛的是 {type(exc).__name__} 而不是 ExplainedTableError -> {exc}"
                )
            else:
                failures.append(f"{name}: 本该抛错却读成功了")

    total = len(cases) + len(explain_cases)
    if passed + len(failures) != total:
        failures.append(
            f"用例账目不平：passed {passed} + failures {len(failures)} != total {total}"
        )
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

    ap = argparse.ArgumentParser(description="i18n 措辞一致性门禁")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--report", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    zh = flatten(json.loads(ZH.read_text(encoding="utf-8")))
    en = flatten(json.loads(EN.read_text(encoding="utf-8")))
    try:
        st = stats(zh, en)
    except ExplainedTableError as exc:
        print(f"i18n 措辞门禁无法运行：{exc}")
        print("这一类是「配置坏了」，不是「措辞漂移了」——先修解释表，门禁才会重新开始判。")
        return 2

    if args.report:
        print(
            f"同中文值的多键组：完全一致 {st['identical']} 组 / "
            f"仅大小写不同 {st['case_only']} 组 / 英文不一致 {st['diff_word']} 组 / "
            f"含空值跳过 {st['skipped']} 组"
        )
        print(
            f"英文不一致的 {st['diff_word']} 组里：已书面解释 {st['explained']} 组，"
            f"**未解释 {st['unexplained']} 组**，解释表失效条目 {st['stale']} 条"
        )
        print("清单见 docs/I18N_WORDING_AUDIT.md（改 i18n 后请重跑 scripts/audit_i18n_wording.py）")
        return 0

    problems = check(zh, en)
    if problems:
        print(f"i18n 措辞一致性校验失败：{len(problems)} 项")
        for p in problems:
            print(f"- {p}")
        print(
            f"\n参考：英文不一致的 {st['diff_word']} 组里，"
            f"已书面解释 {st['explained']} 组、未解释 {st['unexplained']} 组。"
        )
        return 1
    print(
        f"i18n 措辞一致性校验通过：{len(en)} 个键；"
        f"仅大小写不同 0 组；英文不一致 {st['diff_word']} 组"
        f"**全部有书面理由**（未解释 0 组，解释表失效条目 0 条）"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
