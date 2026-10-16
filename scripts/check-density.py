#!/usr/bin/env python3
"""布局密度门禁（P1-4 的验收网）。

对应 docs/UI_REFINEMENT_PLAN.md 的 P1-4（§6.19）。

**为什么是「量计数」而不是「比截图」**

P1-4 的判据原文是「默认 1100×720 窗口多显示约 15% 内容」——
这句话本身就是一个**可计数**的量。直接量它比像素 diff 更准也更稳：
结论是「多显示 1 行会话（共 4 → 5，+25%）」，而不是「17.3% 的像素变了」。

像素方案的三个问题都是硬的：
  1. 字体渲染 / 抗锯齿 / GPU 光栅跨机器不一致，阈值只能调宽到失去意义；
  2. 二进制基线进 git，每次改动都是一堆不可读的 diff；
  3. 阈值一宽，「真回归」和「换台机器」就再也分不开了。

**契约**（改动前先确认不违反）：

1. **`src/generated/layoutBaseline.json` 的 `targets` 是下限，`measured` 是上次实测。**
   `measured <= targets` 恒成立——若不成立，说明承诺从未达成过（手改了目标，
   或改了 CSS 没重跑探针）。反过来 `measured > targets`（密度变好了）是**安全**的：
   探针的 `--update` 会**自动抬高**目标，把改进锁住。
   **脚本只许自动抬高，不许自动下调**——下调必须显式 `--accept-regression`，
   而那次下调会留在一个文本文件的 git diff 里，看得见、审得动。
2. **判据指标必须是非负整数。** `null` 的含义是「探针没找到那个元素」，
   绝不能被当成 0 通过——那会让一次定位点失效悄悄变成「密度达标」。
3. **视口必须钉死 1100×720。** 判据是相对这个尺寸说的，换尺寸等于换了一道题。
4. **探针用到的每个 `[data-layout="..."]` 定位点都必须在 src/ 里真实存在。**
   探针的「量不到就返回 null」是刻意的（见 layout-probe.mjs），
   于是删掉一个定位点的后果是**报 null 而不是报错**——必须有人兜住这一半。
5. **fixture 的 Conversation 字段必须与 `src/api/project.ts` 的 interface 逐字一致。**
   fixture 是 `.mjs`，**不参与 TypeScript 类型检查**：字段名写错时编译是绿的，
   而 UI 读到的是 `undefined`。这不是假设——本门禁建立之前，fixture 里
   写的就是 `pinned` / `message_count`，而 interface 里是 `is_pinned` / `messages_count`。
6. **fixture 的会话年龄必须覆盖 4 个时间分组 + 2 档归档徽章。**
   曾经 40 个会话全是「273 天前」，每行都挂归档徽章、被挤成两行，
   行高从 50px 涨到 66.5px——基线量到的是**最坏态而不是常态**，
   而数字「看着稳定」，没有任何外部信号能发现。
7. **shim 里每个 `localStorage.setItem` 的键都必须是 `STORAGE_KEYS` 里真实存在的值。**
   「写错了键名」是**零信号**的：页面照常渲染、测量照样字节一致，
   只有在核对「这个 preset 到底生效没有」时才会发现。本门禁建立之前，
   shim 写的是 `lang` / `deveco-switch:theme`，而真实键是
   `deveco-switch-lang` / `deveco-switch-theme`——两个 preset 全都没生效。

**不检查**（刻意）：

- 不比对像素、不做截图 diff（理由见上）
- 高度类字段（`sessionRowHeight*` / `*ScrollablePx`）**不作判据**：
  它们受字体与 DPR 影响，跨机器会飘；只用于解释「为什么多了一行」

用法
  python scripts/check-density.py                     # 只校验基线契约（无需浏览器）
  python scripts/check-density.py --fresh <path>      # 拿探针本轮实测判回归
  python scripts/check-density.py --print            # 打印数字明细

自测：python scripts/check-density.py --self-test
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# 本门会往 stdout 打 `✓`（14 道门里只有它打）。stdout 是管道时 Python 按
# **locale 编码**写，中文 Windows 上就是 GBK —— 第一个 `✓` 直接
# UnicodeEncodeError 崩掉。崩掉的后果不止是自己红：`check-self-test-counts.py`
# 会把每道门拉起来数自测条数，它一崩，那道**台账门**也跟着红，
# 于是「一个符号的编码问题」伪装成「两道门坏了」。
# 判据自己也可能被当变量灌进输出，所以用 errors="replace" 而不是让它抛。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except AttributeError:  # 极老的解释器没有 reconfigure
    pass

REPO = Path(__file__).resolve().parent.parent
BASELINE = REPO / "src" / "generated" / "layoutBaseline.json"
PROBE = REPO / "scripts" / "layout-probe.mjs"
FIXTURES = REPO / "scripts" / "layout-fixtures.mjs"
PROJECT_API = REPO / "src" / "api" / "project.ts"
SRC = REPO / "src"

# P1-4 的判据窗口。必须是「默认窗口」——判据是相对这个尺寸说的
VIEWPORT = {"width": 1100, "height": 720}

# 判据指标。与 layout-probe.mjs 的 JUDGE_METRICS 是同一份约定，两边都要改。
JUDGE_METRICS = ("sessionsFullyVisible", "messagesFullyVisible")

# fixture 的会话年龄必须覆盖的五档。阈值取自 Home.tsx 的
# ARCHIVE_SUGGEST_DAYS=7 / ARCHIVE_STRONG_DAYS=30。
DAY = 86400
AGE_BANDS = (
    ("今天", 0, 1 * DAY),
    ("昨天", 1 * DAY, 2 * DAY),
    ("本周(<7 天，不挂徽章)", 2 * DAY, 7 * DAY),
    ("更早·建议归档(7~30 天)", 7 * DAY, 30 * DAY),
    ("更早·建议清理(>30 天)", 30 * DAY, float("inf")),
)

# 解析出来的字段少于这个数就认为解析失效，而不是「没发现问题」。
# 理由见 check_licenses 的教训：数量对不上比结论更值得先查。
MIN_PARSED_CONVERSATION_FIELDS = 8


# ---------- 工具 ----------


def _is_count(v) -> bool:
    """判据必须是**非负整数**。bool 是 int 的子类，显式排除。"""
    return isinstance(v, int) and not isinstance(v, bool) and v >= 0


def strip_js_comments(text: str) -> str:
    """去掉 JS 注释，保留字符位置（用空格替换）。

    必须做：fixture 里满是解释性注释（它们是有价值的文档，不该删），
    而 `// 更早·强提醒（12）：45d ~ 420d` 里的 12 / 45 / 420 会被年龄解析器
    当成真实数据——下标从此与实际位置错位，后面的判断全错，而且**不会报错**。
    """
    out = re.sub(r"/\*.*?\*/", lambda m: " " * len(m.group(0)), text, flags=re.S)
    out = re.sub(r"//[^\n]*", lambda m: " " * len(m.group(0)), out)
    return out


def _load_json(path: Path, problems: list[str], label: str):
    if not path.exists():
        problems.append(f"{label} 不存在：{path}")
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        problems.append(f"{label} 不是合法 JSON：{e}")
        return None


# ---------- 契约 1~3：基线文件自身 ----------


def check_baseline(baseline: dict, problems: list[str]) -> None:
    """校验入库基线的自洽性。**只看文件**，不碰探针，所以任何机器都能跑。"""
    if not isinstance(baseline, dict):
        problems.append("基线顶层不是对象")
        return

    viewport = baseline.get("viewport")
    if viewport != VIEWPORT:
        problems.append(
            f"视口必须是 {VIEWPORT['width']}×{VIEWPORT['height']}，实际 {viewport!r}——"
            "判据是相对默认窗口说的，换尺寸等于换了一道题"
        )

    measured = baseline.get("measured")
    if not isinstance(measured, dict) or not isinstance(measured.get("counts"), dict):
        problems.append("基线缺 `measured.counts`（由 scripts/layout-probe.mjs --update 生成）")
        return

    targets = baseline.get("targets")
    if not isinstance(targets, dict):
        problems.append("基线缺 `targets`——没有目标就没有判据，只剩一个记录")
        return

    counts = measured["counts"]
    for metric in JUDGE_METRICS:
        for src_name, src in (("measured.counts", counts), ("targets", targets)):
            if metric not in src:
                problems.append(f"基线的 `{src_name}` 缺指标 `{metric}`")
                continue
            val = src[metric]
            if not _is_count(val):
                problems.append(
                    f"`{src_name}.{metric}` = {val!r} 不是非负整数。"
                    + (
                        "null 的含义是「探针没找到那个元素」，绝不能当成 0 通过"
                        if val is None
                        else ""
                    )
                )

    # 契约 1：`targets` 是**下限**，`measured <= targets` 必须成立。
    # 目标高于实测 = 承诺从未达成过：要么有人手改了目标，要么基线没跟上 --update。
    # 反过来 `measured > targets`（密度变好了）是**安全**的，不报错——
    # 方向搞反会把「做得更好」判成错误。探针的 --update 会自动抬高目标。
    for metric in JUDGE_METRICS:
        t, m = targets.get(metric), counts.get(metric)
        if _is_count(t) and _is_count(m) and t > m:
            problems.append(
                f"目标 {metric}={t} **高于**实测 {m}——承诺从未达成过。"
                "多半是手改了 targets，或改了 CSS 之后没跑 --update"
            )


# ---------- 契约：fresh 测量 vs 目标 ----------


def check_fresh(baseline: dict, fresh: dict, problems: list[str]) -> None:
    """拿**本轮实测**与入库目标对照。

    为什么必须用 fresh 而不是只读基线：CSS 改了但没人重跑探针时，
    入库文件还是旧的，门禁会对着过期数字报绿——**一个永远绿的检查比没有检查更糟**。
    """
    targets = baseline.get("targets")
    if not isinstance(targets, dict):
        problems.append("基线缺 `targets`，无法判回归")
        return
    if fresh.get("hasErrorBoundary"):
        problems.append("探针测量时页面落进了 ErrorBoundary，本次数字不可信")
        return
    if fresh.get("missingFixtures"):
        problems.append(
            f"探针缺 {len(fresh['missingFixtures'])} 个命令的 fixture："
            + ", ".join(fresh["missingFixtures"][:5])
        )

    counts = fresh.get("counts")
    if not isinstance(counts, dict):
        problems.append("fresh 测量缺 `counts`")
        return

    # 虚拟列表自洽：应用报的总数不可能小于它自己挂载的条目数
    diag = fresh.get("diagnostics") or {}
    total, mounted = diag.get("messageTotal"), diag.get("messageMounted")
    if _is_count(total) and _is_count(mounted) and mounted > total:
        problems.append(
            f"自相矛盾：应用报 {total} 条消息，DOM 里却挂载了 {mounted} 个虚拟条目"
            "——多半是 data-total 与 data-layout 钩子不同步（只改了一端）"
        )

    for metric in JUDGE_METRICS:
        if metric not in counts:
            problems.append(f"fresh 测量缺指标 `{metric}`")
            continue
        val = counts[metric]
        if not _is_count(val):
            problems.append(
                f"fresh 的 `{metric}` = {val!r} 不是非负整数"
                "（null = 探针没找到元素，不能当成 0）"
            )
            continue
        target = targets.get(metric)
        if not _is_count(target):
            problems.append(f"基线 targets.{metric} = {target!r} 非法，无法判回归")
            continue
        if val < target:
            problems.append(
                f"密度回归：{metric} 实测 {val} < 目标 {target}"
                f"（少显示 {target - val} 行/条）"
            )


# ---------- 契约 4：定位点齐全 ----------


def check_layout_hooks(probe_text: str, src_files: dict[str, str], problems: list[str]) -> None:
    """探针用到的每个 `[data-layout="X"]` 都必须在 src/ 里真实存在。

    探针刻意「量不到就返回 null 而不是 0」，所以删掉一个定位点的后果
    不是报错而是**基线悄悄变成 0**。这一条替那个缺口兜底。
    """
    wanted = sorted(set(re.findall(r"""\[data-layout=["']([\w-]+)["']\]""", probe_text)))
    if not wanted:
        problems.append(
            "在 scripts/layout-probe.mjs 里没解析到任何 [data-layout=\"...\"] 定位点——"
            "解析规则已失效，这条门禁现在什么也没检查"
        )
        return

    all_src = "\n".join(src_files.values())
    for hook in wanted:
        if f'data-layout="{hook}"' not in all_src and f"data-layout='{hook}'" not in all_src:
            problems.append(
                f"探针要量 `[data-layout=\"{hook}\"]`，但 src/ 里没有任何元素带这个属性"
                "——钩子被删了，探针会返回 null 而不是报错"
            )


# ---------- 契约 5：fixture 字段 ↔ interface ----------


def parse_interface_fields(ts_text: str) -> tuple[set[str], set[str]]:
    """返回 (全部字段, 必填字段)。可选字段带 `?`。"""
    m = re.search(
        r"export\s+interface\s+Conversation\s*\{(.*?)\n\}", ts_text, re.S
    )
    if not m:
        return set(), set()
    all_fields: set[str] = set()
    required: set[str] = set()
    for line in m.group(1).splitlines():
        fm = re.match(r"\s*([A-Za-z_]\w*)(\?)?\s*:", line)
        if fm:
            all_fields.add(fm.group(1))
            if not fm.group(2):
                required.add(fm.group(1))
    return all_fields, required


def parse_fixture_conversation_keys(js_text: str) -> set[str]:
    """从 conversations() 里的对象字面量抽出实际提供的字段名。"""
    start = js_text.find("function conversations(")
    if start < 0:
        return set()
    end = js_text.find("\n}\n", start)
    body = js_text[start : end if end > 0 else len(js_text)]
    obj_start = body.find("return {")
    if obj_start < 0:
        return set()
    obj_end = body.find("\n    }", obj_start)
    literal = body[obj_start : obj_end if obj_end > 0 else len(body)]
    # 6 空格缩进 = 对象字面量的属性行（return { 在 4 空格）
    return set(re.findall(r"^ {6}([A-Za-z_]\w*)\s*:", strip_js_comments(literal), re.M))


def check_fixture_contract(api_text: str, js_text: str, problems: list[str]) -> None:
    if_fields, required = parse_interface_fields(api_text)
    if not if_fields:
        problems.append(
            "在 src/api/project.ts 里没解析到 `export interface Conversation`——"
            "解析规则已失效，这条门禁现在什么也没检查"
        )
        return

    found = parse_fixture_conversation_keys(js_text)
    if len(found) < MIN_PARSED_CONVERSATION_FIELDS:
        problems.append(
            f"从 fixture 只解析出 {len(found)} 个 Conversation 字段"
            f"（阈值 {MIN_PARSED_CONVERSATION_FIELDS}）——解析规则多半已失效，"
            "**数量对不上比结论更值得先查**，不要当成「没发现问题」"
        )
        return

    # fixture 里多出来的 = 拼错的字段名。UI 读到 undefined，而 tsc 是绿的。
    for name in sorted(found - if_fields):
        near = [f for f in if_fields if abs(len(f) - len(name)) <= 3 and f[:3] == name[:3]]
        hint = f"，interface 里相近的是 {', '.join(sorted(near))}" if near else ""
        problems.append(
            f"fixture 提供了 interface 里不存在的字段 `{name}`{hint}。"
            "fixture 是 .mjs，**不参与类型检查**：写错时编译是绿的，UI 读到 undefined"
        )
    # interface 必填但 fixture 没给
    for name in sorted(required - found):
        problems.append(f"fixture 缺 interface 的必填字段 `{name}`")


# ---------- 契约 6：fixture 年龄分布覆盖 ----------


def parse_conv_ages(js_text: str) -> list[float]:
    """从 CONV_AGES 里读出秒数。支持裸数字与 `d(3.5)` 两种写法。"""
    start = js_text.find("const CONV_AGES = [")
    if start < 0:
        return []
    end = js_text.find("\n]", start)
    body = js_text[start : end if end > 0 else len(js_text)]
    ages: list[float] = []
    for tok in re.findall(r"d\(([\d.]+)\)|(?<![\w.])(\d{2,})", strip_js_comments(body)):
        dpart, npart = tok
        if dpart:
            ages.append(float(dpart) * DAY)
        elif npart:
            ages.append(float(npart))
    return ages


def parse_pinned_idx(js_text: str) -> set[int]:
    # 注意 Set 的括号：`new Set\(` 里的 `\(` 是**转义**的，不参与配对，
    # 所以末尾那个 `)` 也必须转义，否则整个正则抛「unbalanced parenthesis」。
    m = re.search(r"PINNED_IDX\s*=\s*new Set\(\[(.*?)\]\)", js_text, re.S)
    if not m:
        return set()
    return {int(x) for x in re.findall(r"\d+", m.group(1))}


def parse_storage_keys(ts_text: str) -> set[str]:
    """抽出 `STORAGE_KEYS` 对象里所有键的**值**（它们才是 localStorage 里的键名）。

    只认对象字面量里的 `KEY: 'value'` 形式。正则扫全文件会把注释里的字符串也收进来，
    而注释里的键名恰好是最容易「看起来对」的那一种。
    """
    m = re.search(r"export\s+const\s+STORAGE_KEYS\s*=\s*\{(.*?)\n\}\s*as\s+const", ts_text, re.S)
    if not m:
        return set()
    return set(re.findall(r"^\s*[A-Z_0-9]+\s*:\s*'([^']+)'", m.group(1), re.M))


def check_shim_storage_keys(constants_text: str, shim_text: str, problems: list[str]) -> None:
    """shim preset 的每个 localStorage 键都必须真实存在。

    这是本门禁里唯一一条「防写了没生效」的契约。键名写错的表现是
    **零信号**：页面照常渲染、测量照样可复现，只有专门去核对时才发现 preset 没生效。
    """
    real = parse_storage_keys(constants_text)
    if not real:
        problems.append(
            "在 src/constants.ts 里没解析到 `STORAGE_KEYS` 的字面量键值——"
            "解析规则已失效，这条门禁现在什么也没检查"
        )
        return

    used = set(re.findall(r"localStorage\.setItem\(\s*'([^']+)'", strip_js_comments(shim_text)))
    if not used:
        problems.append(
            "在 scripts/layout-probe-shim.js 里没解析到任何 localStorage.setItem——"
            "解析规则已失效，shim 的 preset 是否生效已无人看守"
        )
        return

    for key in sorted(used - real):
        hint = [k for k in real if k.endswith(key.split(":")[-1].split("-")[-1])]
        suffix = f"，相近的真实键：{', '.join(sorted(hint)[:3])}" if hint else ""
        problems.append(
            f"shim preset 的键 `{key}` 不在 STORAGE_KEYS 里{suffix}——"
            "**写不进去且没有任何信号**：页面照常渲染、测量照样可复现"
        )


def check_fixture_coverage(js_text: str, problems: list[str]) -> None:
    """fixture 必须同时踩到 4 个时间分组与 2 档归档徽章。

    这条是为了防「基线量到最坏态」。曾经 fixture 的 40 个会话全是 273 天前，
    全部挂上归档徽章、被挤成两行，行高 50 → 66.5px：
    数字稳定得看不出任何问题，量的却不是常态布局。
    """
    ages = parse_conv_ages(js_text)
    if not ages:
        problems.append("在 scripts/layout-fixtures.mjs 里没解析到 CONV_AGES")
        return
    if len(ages) < 20:
        problems.append(
            f"CONV_AGES 只有 {len(ages)} 项——样本太小时「几行可见」这个计数没有意义"
        )

    for label, lo, hi in AGE_BANDS:
        if not any(lo <= a < hi for a in ages):
            problems.append(
                f"fixture 没有任何会话落在「{label}」区间——"
                "时间分组与归档徽章的多种形态没被覆盖，基线会只量到其中一种"
            )

    # 置顶豁免：至少一个「本该挂徽章」的位置被 pin 住，
    # 否则 pin 对 suggestArchive 的短路逻辑在基线里等于没被测过
    pinned = parse_pinned_idx(js_text)
    banded = [i for i, a in enumerate(ages) if a >= 7 * DAY]
    if not (pinned & set(banded)):
        problems.append(
            "PINNED_IDX 里没有任何落在 7 天以上（会挂归档徽章）的位置——"
            "「置顶豁免徽章」这条逻辑在基线里等于没被覆盖"
        )


# ---------- 装配 ----------


def _read(p: Path) -> str:
    return p.read_text(encoding="utf-8")


def _collect_src(root: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    for p in root.rglob("*.tsx"):
        out[str(p)] = _read(p)
    for p in root.rglob("*.ts"):
        out[str(p)] = _read(p)
    return out


def run_checks(repo: Path, fresh_path: Path | None) -> list[str]:
    """跑全部契约。**所有路径都从 repo 推**——模块级常量只放代码，不放数据。

    否则自测的合成用例会读到真实仓库的数据，失败时长得像「门禁在报真问题」。
    """
    problems: list[str] = []

    baseline_path = repo / "src" / "generated" / "layoutBaseline.json"
    baseline = _load_json(baseline_path, problems, "布局基线")
    if baseline is not None:
        check_baseline(baseline, problems)

    probe_text = _read(repo / "scripts" / "layout-probe.mjs")
    src_files = _collect_src(repo / "src")
    check_layout_hooks(probe_text, src_files, problems)

    api_text = _read(repo / "src" / "api" / "project.ts")
    js_text = _read(repo / "scripts" / "layout-fixtures.mjs")
    check_fixture_contract(api_text, js_text, problems)
    check_fixture_coverage(js_text, problems)
    check_shim_storage_keys(
        _read(repo / "src" / "constants.ts"),
        _read(repo / "scripts" / "layout-probe-shim.js"),
        problems,
    )

    if fresh_path is not None and baseline is not None:
        fresh = _load_json(fresh_path, problems, "fresh 测量")
        if fresh is not None:
            check_fresh(baseline, fresh, problems)

    return problems


# ---------- 自测 ----------

_GOOD_BASELINE = {
    "viewport": {"width": 1100, "height": 720},
    "measured": {
        "counts": {
            "sessionsFullyVisible": 7,
            "sessionTotal": 40,
            "messagesFullyVisible": 2,
            "messageTotal": 32,
        }
    },
    "targets": {"sessionsFullyVisible": 7, "messagesFullyVisible": 2},
}

_GOOD_API = """
export interface Conversation {
  id: string
  project_id: string
  title: string
  provider_id: string | null
  is_pinned: boolean
  archived: boolean
  tags?: string
  created_at: number
  updated_at: number
  messages_count?: number
}
"""

_GOOD_FIXTURE = """
const d = (n) => n * 86400
const CONV_AGES = [
  1800, 5400, 9000, 18000, 28800, 43200,
  90000, 108000,
  d(3), d(4), d(5), d(6),
  d(10), d(20), d(28),
  d(45), d(120), d(300), d(420), d(500), d(600), d(700), d(800),
]
const PINNED_IDX = new Set([0, 12, 13])
function conversations(n) {
  return {
      id: `conv`,
      title: `t`,
      project_id: `p`,
      provider_id: null,
      is_pinned: true,
      archived: false,
      tags: '',
      created_at: 1,
      updated_at: 2,
      messages_count: 3,
    }
}
"""


_GOOD_CONSTANTS = """
export const STORAGE_KEYS = {
  LANG: 'deveco-switch-lang',
  FIRST_RUN_SEEN: 'deveco-switch:first-run-seen',
  THEME: 'deveco-switch-theme',
  FONT_SCALE: 'deveco-switch:font-scale',
} as const
"""

_GOOD_SHIM = """
try {
  localStorage.setItem('deveco-switch:first-run-seen', '1')
  localStorage.setItem('deveco-switch-lang', 'zh')
  localStorage.setItem('deveco-switch-theme', 'dark')
} catch { }
"""


def _self_test() -> int:
    cases: list[tuple[str, bool, bool, list[str]]] = []

    def case(name: str, fn, expect_problems: bool = True) -> None:
        """`expect_problems=True` 表示这是个**负向**用例：必须报出问题才算通过。

        负向用例占多数，所以默认即为负向；正向用例显式传 False。
        早期版本这里默认「无问题 = 通过」，而大部分用例期望的是「有问题」——
        结果是自测全红，看起来像门禁坏了，其实是 harness 把期望写反了。
        """
        problems: list[str] = []
        try:
            fn(problems)
        except Exception as e:  # noqa: BLE001 - 自测要报出任何异常本身
            problems.append(f"抛异常：{e}")
        ok = bool(problems) == expect_problems
        cases.append((name, ok, expect_problems, problems))

    def _mutate(src: str, old: str, new: str) -> str:
        """改合成数据，并断言**真的改动了**。

        `.replace()` 命中不到时是空操作，负向用例于是「没报出问题」→ 假通过。
        这种失败形态最坏：测试绿着，而它守护的那条契约已经没人验证了。
        """
        out = src.replace(old, new)
        assert out != src, f"替换串在合成数据里不存在：{old!r}"
        return out

    def _sub(src: str, pattern: str, repl: str) -> str:
        out = re.sub(pattern, repl, src, flags=re.S)
        assert out != src, f"正则未命中合成数据：{pattern!r}"
        return out

    # --- 基线自洽 ---
    def t_baseline_ok(p):
        check_baseline(_GOOD_BASELINE, p)

    def t_baseline_above_target(p):
        # 实测 > 目标：密度比承诺的还好。**这是安全的方向，不许报错**——
        # 方向搞反会把「做得更好」判成错误（探针的 --update 会自动抬高目标）
        b = json.loads(json.dumps(_GOOD_BASELINE))
        b["measured"]["counts"]["sessionsFullyVisible"] = 9
        check_baseline(b, p)

    def t_baseline_target_above_measured(p):
        b = json.loads(json.dumps(_GOOD_BASELINE))
        b["targets"]["sessionsFullyVisible"] = 9
        check_baseline(b, p)

    def t_baseline_null_count(p):
        b = json.loads(json.dumps(_GOOD_BASELINE))
        b["measured"]["counts"]["sessionsFullyVisible"] = None
        check_baseline(b, p)

    def t_baseline_float_count(p):
        b = json.loads(json.dumps(_GOOD_BASELINE))
        b["measured"]["counts"]["sessionsFullyVisible"] = 7.5
        check_baseline(b, p)

    def t_baseline_wrong_viewport(p):
        b = json.loads(json.dumps(_GOOD_BASELINE))
        b["viewport"] = {"width": 1280, "height": 800}
        check_baseline(b, p)

    def t_baseline_missing_metric(p):
        b = json.loads(json.dumps(_GOOD_BASELINE))
        del b["targets"]["messagesFullyVisible"]
        check_baseline(b, p)

    def t_baseline_no_targets(p):
        b = json.loads(json.dumps(_GOOD_BASELINE))
        del b["targets"]
        check_baseline(b, p)

    case("基线合法 → 通过", t_baseline_ok, expect_problems=False)
    case("实测高于目标 → 通过（方向不能反）", t_baseline_above_target, expect_problems=False)
    case("目标高于实测 → 报错", t_baseline_target_above_measured)
    case("计数为 null → 报错（不是 0）", t_baseline_null_count)
    case("计数是小数 → 报错", t_baseline_float_count)
    case("视口不是 1100×720 → 报错", t_baseline_wrong_viewport)
    case("缺判据指标 → 报错", t_baseline_missing_metric)
    case("缺 targets → 报错", t_baseline_no_targets)

    # --- fresh vs target ---
    def _fresh(counts, **kw):
        f = {
            "viewport": {"width": 1100, "height": 720},
            "counts": counts,
            "diagnostics": {"messageTotal": 32, "messageMounted": 6},
            "missingFixtures": [],
            "hasErrorBoundary": False,
        }
        f.update(kw)
        return f

    def t_fresh_ok(p):
        check_fresh(_GOOD_BASELINE, _fresh({"sessionsFullyVisible": 7, "messagesFullyVisible": 2}), p)

    def t_fresh_above(p):
        check_fresh(_GOOD_BASELINE, _fresh({"sessionsFullyVisible": 8, "messagesFullyVisible": 3}), p)

    def t_fresh_regression(p):
        check_fresh(_GOOD_BASELINE, _fresh({"sessionsFullyVisible": 6, "messagesFullyVisible": 2}), p)

    def t_fresh_msg_regression(p):
        check_fresh(_GOOD_BASELINE, _fresh({"sessionsFullyVisible": 7, "messagesFullyVisible": 1}), p)

    def t_fresh_null(p):
        check_fresh(_GOOD_BASELINE, _fresh({"sessionsFullyVisible": None, "messagesFullyVisible": 2}), p)

    def t_fresh_error_boundary(p):
        check_fresh(_GOOD_BASELINE, _fresh({"sessionsFullyVisible": 7, "messagesFullyVisible": 2}, hasErrorBoundary=True), p)

    def t_fresh_missing_fixture(p):
        check_fresh(_GOOD_BASELINE, _fresh({"sessionsFullyVisible": 7, "messagesFullyVisible": 2}, missingFixtures=["list_x"]), p)

    def t_fresh_contradiction(p):
        f = _fresh({"sessionsFullyVisible": 7, "messagesFullyVisible": 2})
        f["diagnostics"] = {"messageTotal": 3, "messageMounted": 6}
        check_fresh(_GOOD_BASELINE, f, p)

    case("fresh 达标 → 通过", t_fresh_ok, expect_problems=False)
    case("fresh 优于目标 → 通过", t_fresh_above, expect_problems=False)
    case("fresh 会话行数退步 → 报错", t_fresh_regression)
    case("fresh 消息数退步 → 报错", t_fresh_msg_regression)
    case("fresh 计数为 null → 报错", t_fresh_null)
    case("fresh 落进 ErrorBoundary → 报错", t_fresh_error_boundary)
    case("fresh 缺 fixture → 报错", t_fresh_missing_fixture)
    case("挂载数 > 总数 → 报错（钩子只改了一端）", t_fresh_contradiction)

    # --- fixture 字段对账 ---
    def t_fixture_ok(p):
        check_fixture_contract(_GOOD_API, _GOOD_FIXTURE, p)

    def t_fixture_typo(p):
        # 复现真实缺陷：pinned vs is_pinned、message_count vs messages_count
        js = _mutate(_GOOD_FIXTURE, "is_pinned: true,", "pinned: true,")
        check_fixture_contract(_GOOD_API, js, p)

    def t_fixture_missing_required(p):
        js = _mutate(_GOOD_FIXTURE, "      archived: false,\n", "")
        check_fixture_contract(_GOOD_API, js, p)

    def t_fixture_parse_dead(p):
        # 解析失效时必须报错，而不是「没发现问题」
        check_fixture_contract("export const X = 1\n", _GOOD_FIXTURE, p)

    def t_fixture_too_few_keys(p):
        js = _mutate(_GOOD_FIXTURE, "      archived: false,\n", "")
        for frag in ("      tags: '',\n", "      messages_count: 3,\n", "      provider_id: null,\n"):
            js = _mutate(js, frag, "")
        check_fixture_contract(_GOOD_API, js, p)

    case("fixture 字段一致 → 通过", t_fixture_ok, expect_problems=False)
    case("fixture 字段名拼错 → 报错", t_fixture_typo)
    case("fixture 缺必填字段 → 报错", t_fixture_missing_required)
    case("interface 解析失效 → 报错", t_fixture_parse_dead)
    case("fixture 解析出的字段过少 → 报错", t_fixture_too_few_keys)

    # --- fixture 形态覆盖 ---
    def t_cov_ok(p):
        check_fixture_coverage(_GOOD_FIXTURE, p)

    def t_cov_all_stale(p):
        # 复现真实缺陷：40 个会话全是 273 天前
        js = _sub(
            _GOOD_FIXTURE,
            r"const CONV_AGES = \[.*?\n\]",
            "const CONV_AGES = [\n  d(273), d(273), d(273), d(273),\n"
            "  d(273), d(273), d(273), d(273),\n  d(273), d(273), d(273), d(273),\n]",
        )
        check_fixture_coverage(js, p)

    def t_cov_all_today(p):
        js = _sub(
            _GOOD_FIXTURE,
            r"const CONV_AGES = \[.*?\n\]",
            "const CONV_AGES = [\n  1800, 5400, 9000, 18000, 28800, 43200,\n"
            "  90000, 108000, 126000, 162000, 198000, 234000,\n  300000, 320000, 340000, 360000,\n"
            "  380000, 400000, 420000, 440000, 460000, 480000,\n]",
        )
        check_fixture_coverage(js, p)

    def t_cov_comment_digits(p):
        # 回归用例：注释里的数字曾经被当成真实年龄，下标整体错位而**不报错**。
        # 直接对解析结果断言，而不是绕 check_fixture_coverage——
        # 绕过去的话，修复生效后自然「不报问题」，负向用例反而变红。
        before = parse_conv_ages(_GOOD_FIXTURE)
        js = _mutate(
            _GOOD_FIXTURE,
            "  d(10), d(20), d(28),",
            "  // 更早·弱提醒（8）：10d ~ 28d\n  d(10), d(20), d(28),",
        )
        after = parse_conv_ages(js)
        if before != after:
            p.append(
                f"注释里的数字被当成了年龄：{len(before)} 项 → {len(after)} 项，"
                "下标已错位（这类错位不会报错，只会静默判错）"
            )
        if after[:3] != [1800.0, 5400.0, 9000.0]:
            p.append(f"前 3 项解析错误：{after[:3]}")

    def t_cov_pin_not_in_badge_band(p):
        # PINNED_IDX 只留今天两项：置顶豁免逻辑在基线里等于没被覆盖
        js = _mutate(_GOOD_FIXTURE, "new Set([0, 12, 13])", "new Set([0, 1])")
        check_fixture_coverage(js, p)

    case("fixture 覆盖 4 分组 + 2 档徽章 → 通过", t_cov_ok, expect_problems=False)
    case("fixture 全是陈旧会话 → 报错（量到最坏态）", t_cov_all_stale)
    case("fixture 全是今天/昨天 → 报错", t_cov_all_today)
    case("注释里的数字不被当成年龄（回归用例）", t_cov_comment_digits, expect_problems=False)
    case("置顶项都不在徽章区间 → 报错", t_cov_pin_not_in_badge_band)

    # --- 定位点 ---
    def t_hooks_ok(p):
        probe = "const a = pick('[data-layout=\"session-list\"]')\nconst b = pickAll('[data-layout=\"session-item\"]')\n"
        src = {'Home.tsx': 'data-layout="session-list"\ndata-layout="session-item"\n'}
        check_layout_hooks(probe, src, p)

    def t_hooks_missing(p):
        # 探针要两个钩子，src/ 只剩一个 → 少的那一个必须被报出来
        probe = (
            "const a = pick('[data-layout=\"session-list\"]')\n"
            "const b = pickAll('[data-layout=\"session-item\"]')\n"
        )
        src = {'Home.tsx': 'data-layout="session-list"\n'}
        check_layout_hooks(probe, src, p)

    def t_hooks_parse_dead(p):
        check_layout_hooks("const a = 1\n", {'Home.tsx': 'x\n'}, p)

    case("定位点齐全 → 通过", t_hooks_ok, expect_problems=False)
    case("定位点被删 → 报错", t_hooks_missing)
    case("探针里解析不到定位点 → 报错", t_hooks_parse_dead)

    # --- shim 的 localStorage 键 ---
    def t_keys_ok(p):
        check_shim_storage_keys(_GOOD_CONSTANTS, _GOOD_SHIM, p)

    def t_keys_wrong(p):
        # 复现真实缺陷：`lang` / `deveco-switch:theme` 都不是真实键名，两个 preset 全都没生效
        shim = _mutate(_GOOD_SHIM, "'deveco-switch-lang'", "'lang'")
        shim = _mutate(shim, "'deveco-switch-theme'", "'deveco-switch:theme'")
        check_shim_storage_keys(_GOOD_CONSTANTS, shim, p)

    def t_keys_invented(p):
        shim = _mutate(_GOOD_SHIM, "'deveco-switch-theme'", "'deveco-switch:update-dismissed-at'")
        check_shim_storage_keys(_GOOD_CONSTANTS, shim, p)

    def t_keys_constants_parse_dead(p):
        check_shim_storage_keys("export const X = 1\n", _GOOD_SHIM, p)

    def t_keys_shim_parse_dead(p):
        check_shim_storage_keys(_GOOD_CONSTANTS, "// 没有 setItem\n", p)

    case("shim 键名与 STORAGE_KEYS 一致 → 通过", t_keys_ok, expect_problems=False)
    case("shim 键名拼错（复现真实缺陷）→ 报错", t_keys_wrong)
    case("shim 编造了一个不存在的键 → 报错", t_keys_invented)
    case("STORAGE_KEYS 解析失效 → 报错", t_keys_constants_parse_dead)
    case("shim 里解析不到 setItem → 报错", t_keys_shim_parse_dead)

    # ---------- 跑 ----------
    failed = 0
    for name, ok, expect_problems, problems in cases:
        if ok:
            print(f"  ✓ {name}")
        else:
            failed += 1
            want = "应报出问题" if expect_problems else "不应报出问题"
            print(f"  ✗ {name}（{want}，实际报出 {len(problems)} 条）")
            for msg in problems:
                print(f"      → {msg}")

    print(f"\n自测 {len(cases) - failed}/{len(cases)} 通过")
    return 1 if failed else 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--fresh", type=Path, help="探针 --json-out 写出的本轮实测 JSON")
    ap.add_argument("--print", dest="do_print", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        return _self_test()

    problems = run_checks(REPO, args.fresh)

    if args.do_print and BASELINE.exists():
        data = json.loads(BASELINE.read_text(encoding="utf-8"))
        print("viewport:", data.get("viewport"))
        print("measured:", data.get("measured", {}).get("counts"))
        print("targets :", data.get("targets"))
        print("probe   :", data.get("probe"))
        print()

    if args.fresh is None:
        print("· 本次只校验基线契约（未重新测量）")
        print("  数字判据需要探针：node scripts/layout-probe.mjs --json-out <path>")

    if problems:
        print(f"✗ 布局密度门禁失败（{len(problems)} 项）")
        for p in problems:
            print(f"  - {p}")
        return 1

    print("✓ 布局密度门禁通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
