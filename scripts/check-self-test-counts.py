#!/usr/bin/env python3
"""第 14 道门禁：门禁自测条数表必须与真源一致。

## 为什么要有这道门

`docs/TECH_DEBT.md` 里有一张「13 道门禁各有多少条自测用例」的表。
这张表**只能靠人抄**，而本项目已经被这件事咬过：

- i18n 重复键的「zh 5 / en 4」在 §6.14 ~ §6.28 的**六个**批次小节里被抄了六遍，
  六遍都错（§6.29 实测真实值是 120 / 0 / 49）
- 「docs 自测 12 条」实际 11 条，**不报红、不报错**，因为自测只打印「全部通过」
- 第十六批又踩了一次：`total` 多算一次，输出「24/25」却一条 FAIL 都没打印

三次的共同点：**数字是手写的，真源不产出它**。

## 契约（2 条）

1. **台账表里的每个数字必须与实际自测一致**。
   解析器对各门现有的输出格式都做了容错——
   **不去改 13 个门禁的输出格式**。为了满足一份文档去重写 13 个能跑的自测，
   代价与风险都远大于收益；正确的做法是让**核对自动化**。
2. **每道门的自测必须能读出条数**。读不出的门禁，其数字无法被任何人核对——
   这比「数字对不上」更糟，因为它连被发现的机会都没有。
   因此：13 道门**全部**必须输出可解析的条数。

## 判据的已知边界

- 解析的是**输出文本**而不是内部状态。若某个门禁改了输出措辞，本门会报
  「读不出条数」——这是**故意的**：那意味着契约 2 被破坏了，该去修门禁而不是放宽解析。
- 本门**不校验**台账里非数字列的内容（性质、理由、阻塞条件），那不是机械能做的。
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEBT = REPO / "docs" / "TECH_DEBT.md"
WORKFLOWS = REPO / ".github" / "workflows"

#: 台账表格的形状：`| `check-xxx.py` | 说明 | 数字 |`
#: 扩展名**收 `.py` 与 `.mjs`**：第二十七批新增的 `check-i18n-key-usage.mjs`
#: 用 TypeScript 解析器读 TSX，手写词法分析做不到（实测丢 380/424 个真引用）。
#: 只认 `.py` 会让这道门把它当成「没登记」——而它明明已经登记了。
ROW_RE = re.compile(r"^\|\s*`check-([a-z0-9-]+)\.(py|mjs)`\s*\|[^|]*\|\s*(\d+)\s*\|", re.M)

#: 13 道门现有的自测输出格式，全部兼容。
#: 顺序有意义：带 `N/N` 的先试（分母才是「共多少条」），再试单数字，
#: 最后试**复合格式**（`A 个…用例 + B 个…用例`，需要相加）。
FRACTION_RE = re.compile(r"自测\s*\d+\s*/\s*(\d+)")
SINGLE_PATTERNS = [
    re.compile(r"自测通过[：:]\s*(\d+)\s*条"),
    re.compile(r"自测失败[：:]\s*\d+\s*/\s*(\d+)\s*条"),
    re.compile(r"全部通过[（(]\s*(\d+)\s*个用例"),
    re.compile(r"self-test\s+OK:[^\n]*?(\d+)\s*个合成用例"),
    re.compile(r"(\d+)\s*个合成用例"),
    re.compile(r"(\d+)\s*个用例"),
    re.compile(r"(\d+)\s*条合成样例"),
]
#: 复合格式：一行里 `5 个声明用例 + 4 个矩阵用例` / `7 个仓库用例 + 10 个 SPDX 解析用例 + 9 个 GPL 判定用例`
#:
#: 段内用 `[^+\n]{0,20}?` 而不是 `\S*?`——后者跨不过空格，
#: `10 个 SPDX 解析用例` 里 SPDX 与 解析 之间有空格，导致第三段数不出来、
#: licenses 的 26 被读成 7。
COMPOUND_RE = re.compile(r"self-test\s*OK\s*[：:]?\s*([^\n]+)")
SEGMENT_RE = re.compile(r"(\d+)\s*个[^+\n]{0,20}?用例")


def parse_count(text: str) -> int | None:
    """从自测输出里取出用例条数。取不到返回 None。

    `N/N` 取**分母**——分子是「通过数」，分母才是「共多少条」。
    第一版写成 `m.groups() == 2`（拿元组比整数，恒为假），
    于是复合格式的两段只取到了第一段，`ui-states` 的 9 被读成 5。
    """
    m = FRACTION_RE.search(text)
    if m:
        return int(m.group(1))
    for pat in SINGLE_PATTERNS:
        m = pat.search(text)
        if m:
            return int(m.group(1))
    m = COMPOUND_RE.search(text)
    if m:
        parts = [int(x) for x in SEGMENT_RE.findall(m.group(1))]
        if parts:
            return sum(parts)
    return None


def gates_from_workflow() -> dict[str, str]:
    """quality.yml 里已接 CI 的门禁 -> 扩展名（`.py` / `.mjs`）。"""
    q = (WORKFLOWS / "quality.yml").read_text(encoding="utf-8")
    exts: dict[str, str] = {}
    for name, ext in re.findall(r"check-([a-z0-9-]+)\.(py|mjs)", q):
        if name != "warnings":
            exts.setdefault(name, ext)
    return exts


def run_self_test(gate: str, ext: str = "py") -> tuple[int, str]:
    """跑一道门的自测并取回它的输出。

    `ext` 决定解释器：`.py` 走 `python`，`.mjs` 走 `node`
    （`node` 是 `.exe`，不带 shell 也能找到——不像 `npx`/`npm` 那些 `.cmd`）。

    ## `PYTHONIOENCODING` 不是洁癖，是本门能不能用的前提（2026-10-08 实测）

    子门禁的 stdout 是**管道**，Python 按 locale 编码写——中文 Windows 上是 **GBK**。
    而下面按 **UTF-8** 解码 ⇒ 乱码 ⇒ `parse_count` 读不出数字 ⇒
    **6 道门同时报「读不出用例条数」**。

    真实 A/B（同一份代码、同一台机器）：

    | 环境 | 退出码 |
    |---|---:|
    | 不设 `PYTHONIOENCODING` | **1**（假红，6 项） |
    | 设 `PYTHONIOENCODING=utf-8` | **0** |

    **所以此前这个门在中文 Windows 上一直是坏的**，只有经
    `scripts/deveco-verify-all.py` 跑才对——因为**那个验收器**给子进程注入了
    变量，注释里写的就是「免得再出现一个符号的编码问题伪装成一道门坏了」。
    同一个补丁打在了验收器上，**没打在门上**；而 CI（Linux/UTF-8）永远是绿的，
    于是没人发现。

    教训：**修「症状出现的地方」不够，要问「谁让它出现」**。
    这里症状在解析结果，成因在 spawn 时没带环境。
    """
    path = str(REPO / "scripts" / f"check-{gate}.{ext}")
    runner = ["node"] if ext == "mjs" else ["python"]
    r = subprocess.run(
        runner + [path, "--self-test"],
        capture_output=True, cwd=REPO,
        # stdout 是管道 ⇒ 子进程按 locale（中文 Windows=GBK）写中文 ⇒
        # 与下面固定的 utf-8 解码对不上。必须显式钉死子进程输出编码。
        env={**os.environ, "PYTHONIOENCODING": "utf-8"},
    )
    return r.returncode, (r.stdout + r.stderr).decode("utf-8", "replace")


def check(claims: dict[str, int], runner) -> list[str]:
    problems: list[str] = []
    if not claims:
        return [
            "扫描范围失效：TECH_DEBT.md 里没有读到任何门禁表格行——"
            "「全部通过」和「没东西可查」在退出码上长得一模一样"
        ]
    for gate, claimed in sorted(claims.items()):
        code, out = runner(gate)
        if code != 0:
            problems.append(f"{gate}：自测本身没通过（exit {code}）→ {out.strip()[:160]}")
            continue
        got = parse_count(out)
        if got is None:
            problems.append(
                f"{gate}：自测输出里读不出用例条数，表格中的 {claimed} 无法被任何人核对"
                f"（输出：{out.strip()[-120:]}）"
            )
        elif got != claimed:
            problems.append(f"{gate}：台账写 {claimed} 条，实际 {got} 条")
    return problems


# ============================ 自测 ============================

GOOD_SELF_TEST = {
    "docs": "self-test: 全部通过（15 个用例，含 CHANGELOG 最新版本段 3 个用例）",
    "css-vars": "自测通过：20 条合成样例全部符合预期",
    "website": "自测 33/33",
    "ui-states": "self-test OK: 5 个声明用例 + 4 个矩阵用例全部符合预期",
    "telemetry": "self-test OK: 9 个合成用例全部符合预期",
    "licenses": "self-test OK: 7 个仓库用例 + 10 个 SPDX 解析用例 + 9 个 GPL 判定用例",
}


def self_test() -> int:
    parse_cases = [
        ("N/N 取分母", "自测 33/33", 33),
        ("N/N 失败分支", "自测失败：30/33 条不符预期", 33),
        ("中文条数格式", "自测通过：20 条合成样例全部符合预期", 20),
        ("docs 格式", "self-test: 全部通过（11 个用例，含 CHANGELOG 3 个用例）", 11),
        ("ui-states 两段相加", "self-test OK: 5 个声明用例 + 4 个矩阵用例全部符合预期", 9),
        ("telemetry 格式", "self-test OK: 9 个合成用例全部符合预期", 9),
        ("licenses 三段相加", "self-test OK: 7 个仓库用例 + 10 个 SPDX 解析用例 + 9 个 GPL 判定用例", 26),
        ("无数字 → None", "自测通过：全部符合预期", None),
    ]
    passed, failures = 0, []
    for name, text, expect in parse_cases:
        got = parse_count(text)
        if got != expect:
            failures.append(f"解析 {name}: 期望 {expect}，实得 {got}（{text!r}）")
            continue
        passed += 1

    # 判据级用例：数字不符 / 读不出 / 自测没通过
    check_cases = [
        (
            "数字与真源不符被拦",
            {"website": 99},
            {"website": (0, "自测 33/33")},
            "实际 33",
        ),
        (
            "读不出条数被拦",
            {"website": 33},
            {"website": (0, "全部通过")},
            "读不出用例条数",
        ),
        (
            "自测自身失败要报出来",
            {"website": 33},
            {"website": (1, "自测 30/33")},
            "自测本身没通过",
        ),
        (
            "一致时不得误报",
            {"website": 33},
            {"website": (0, "自测 33/33")},
            "",
        ),
    ]
    for name, claims, table, keyword in check_cases:
        runner = lambda g, _t=table: _t[g]  # noqa: E731
        problems = check(claims, runner)
        if keyword and not any(keyword in p for p in problems):
            failures.append(f"{name}: 期望提到「{keyword}」-> {problems}")
            continue
        if not keyword and problems:
            failures.append(f"{name}: 期望无问题 -> {problems}")
            continue
        passed += 1

    # 台账表格解析（第三行是 `.mjs` 门禁：扩展名不收的话会整行漏掉）
    fake = (
        "| 门 | 覆盖面 | 自测 |\n|---|---|---:|\n"
        "| `check-docs.py` | 说明 | 15 |\n"
        "| `check-website.py` | 说明 | 33 |\n"
        "| `check-i18n-key-usage.mjs` | 说明 | 12 |\n"
    )
    rows = dict((m.group(1), int(m.group(3))) for m in ROW_RE.finditer(fake))
    if rows == {"docs": 15, "website": 33, "i18n-key-usage": 12}:
        passed += 1
    else:
        failures.append(f"台账表格解析: 期望 3 行（含 .mjs），实得 {rows}")

    # 空表格必须报「扫描范围失效」
    problems = check({}, lambda g: (0, "自测 1/1"))
    if any("扫描范围失效" in p for p in problems):
        passed += 1
    else:
        failures.append("空表格：未报「扫描范围失效」")

    # 端到端：**真跑**一道门，自测输出里的条数必须读得出来。
    # 这条钉的是 `run_self_test` 注入 `PYTHONIOENCODING`。
    # ⚠️ **如实标注局限**：它只在**非 UTF-8 locale** 上有区分力——
    # 删掉那行 env，Linux/UTF-8 的 CI 依然全绿，只有中文 Windows 会红。
    # 写清楚比让人以为它在任何机器上都守着强。
    code, out = run_self_test("docs")
    if code == 0 and parse_count(out) is not None:
        passed += 1
    else:
        failures.append(
            f"真跑 check-docs.py 自测读不出条数（exit {code}）：{out.strip()[-120:]}"
        )

    total = len(parse_cases) + len(check_cases) + 3
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

    if "--self-test" in sys.argv:
        return self_test()

    if not DEBT.exists():
        print(f"找不到 {DEBT.relative_to(REPO)}", file=sys.stderr)
        return 1
    text = DEBT.read_text(encoding="utf-8")
    exts_doc = {m.group(1): m.group(2) for m in ROW_RE.finditer(text)}
    claims = {m.group(1): int(m.group(3)) for m in ROW_RE.finditer(text)}

    exts = gates_from_workflow()
    missing_from_doc = sorted(set(exts) - set(claims))
    if missing_from_doc:
        print(
            f"台账缺 {len(missing_from_doc)} 道已接 CI 的门禁：{missing_from_doc}"
            "——新门禁必须登记，否则它的条数无人核对"
        )
        return 1

    def runner(gate: str):
        return run_self_test(gate, exts_doc.get(gate, "py"))

    problems = check(claims, runner)
    if problems:
        print(f"门禁自测条数核对失败：{len(problems)} 项")
        for p in problems:
            print(f"- {p}")
        return 1
    print(f"门禁自测条数核对通过：{len(claims)} 道门的台账数字与真源一致")
    return 0


if __name__ == "__main__":
    sys.exit(main())
