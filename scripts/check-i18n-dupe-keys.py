#!/usr/bin/env python3
"""第 17 道门禁：i18n 词库里不得有同名重复键。

## 现象

JSON 规范允许重复键，而所有解析器（浏览器 `JSON.parse`、Python `json`、
Rust `serde_json`）都**保留最后一个并静默丢掉前面的**。于是 `src/i18n/zh.json`
里 `home.timeline` 写了两次——「时间线」与「时间线（回到历史点）」——
而**界面上永远只可能出现后者**。

## 为什么它**完全静默**

- 词库是合法 JSON，`tsc` / `eslint` / `vitest` / `npm run build` 全绿；
- `check-i18n-wording.py` 也**看不见它**：那道门用裸 `json.loads(raw)`，
  解析器已经替它把重复键丢掉了（实测该脚本第 183 / 512 / 513 行三处都是裸 loads）；
- 连带后果：文档里那个「i18n 键 **2119**」本身就是**建立在「丢弃被遮蔽键」之上**的数。

## 判据与已知边界

- 判据只管「**同一个对象内**出现两次以上的键」。**跨对象同名不算**——
  `searching` 在 `home` 与 `perf` 里各有一份是正常写法，误判它只会让门禁变成噪声源。
- 值比较用**规范化的 JSON 文本**（`sort_keys=True`），因此对象字面量里键序不同
  不会被误判成分歧；`"已复制"` 与 `"已复制 "` 则判为分歧（后者多一个空格）。
- 现存的分歧项走 `ALLOWED` 白名单，每条写清理由。
  **「删掉被遮蔽的那条」是零行为变化**（运行时只认最后一条），
  但**「留哪条」是措辞决策**，本门不替人做。
  **当前该表为空**——凡是被登记过的分歧都已处理完，
  空表也意味着本门现在对**任何**新增重复键零豁免。
- 白名单必须与真源**双向**对齐：**未登记的重复键要拦，已失效的白名单条目也要拦**。
  只做单向的话，「修好了但忘了删白名单」会让**同一个键将来再写重复时被静默放行**
  ——白名单就从豁免变成了漏洞。
  同值重复**不得进白名单**：它们是纯冗余，删掉零行为变化。
- ⚠️ **真源已无重复键 ⇒ 判据弱化类变异在真源上打不出红。**
  证明「真源上新增重复会被抓住」要靠**向真源注入重复**，
  这是与「判据是否准确」**不同**的一件事，两者都要验。
- 本门**不校验** zh/en 两种语言的措辞是否互相对应（那是 `check-i18n-wording.py` 的事）。
  ⚠️ 实测两者互补而非重叠：重复键那道门不看翻译，措辞那道门看不到重复。
- 词库文件缺失或 JSON 损坏时，本门会**抛 traceback**（退出码仍非 0，仍然是红的）。
  **刻意不加护栏**：那种情况下 `vite build` / `tsc` 会同时失败、而且响得多，
  门禁自己那段 stack 只会淹没真正的原因。要的是「一眼看出是谁坏了」，不是「报得好看」。
- 哨兵用 `tuple` 而 JSON 数组解析成 `list` ⇒ **两者不可能混淆**，
  所以 `isinstance(value, tuple)` 判定安全；反之若哨兵也用 `list` 就会误判。

用法：python scripts/check-i18n-dupe-keys.py            # 查，退出码 1 = 有重复键
      python scripts/check-i18n-dupe-keys.py --self-test # 合成用例
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
I18N_DIR = ROOT / "src" / "i18n"
FILES = ("zh.json", "en.json")

#: object_pairs_hook 的哨兵：让每个对象都保留原始键序，以便发现重复键
MARK = "__pairs__"

#: 已知分歧项（值不同的重复键）。每条都必须写理由，否则白名单就变成遮羞布。
#: 这里只登记「值分歧」——同值重复是纯冗余，删掉零行为变化，不该进白名单。
#:
#: ⚠️ **当前为空，且这是刻意留空的。** 第二十六批普查出来的 9 处分歧已全部处理
#: （其中 `timeline` / `timelineEmpty` 的根因不是措辞冲突，而是**一个键被两个面板共用**，
#: 已拆成 `home.timelineEvents` / `home.timelineEventsEmpty`），
#: 白名单不该再背任何一条。
#: 空表也带来一个**必须如实标注的后果**：真源已没有重复键可供比对，
#: 于是「判据被弱化」这类变异在真源上**打不出红**——
#: 证明「真源上新增重复会被抓住」靠的是**向真源注入重复**（变异 N1 / N2），
#: 而不是靠真源自带重复。两种证法守的不是同一件事。
ALLOWED: dict[tuple[str, str], str] = {}


def _hook(pairs):
    """把每个 JSON 对象转成 (哨兵, 键值对序列)，保住顺序与重复。"""
    return (MARK, pairs)


def _plain(value):
    """把哨兵包装的对象还原成普通 dict/list。

    必须先还原再比较：哨兵是 tuple，`json.dumps` 会把它序列化成**数组**，
    于是「键序不同的两个等价对象」又会被算成分歧——第一版就栽在这里，
    是自测第 8 条抓出来的（预期同值、实得分歧）。
    """
    if isinstance(value, tuple) and value and value[0] == MARK:
        return {k: _plain(v) for k, v in value[1]}
    if isinstance(value, list):
        return [_plain(v) for v in value]
    return value


def _canon(value) -> str:
    """值的规范化文本：用于判断两条重复键是否「同值」。"""
    return json.dumps(_plain(value), ensure_ascii=False, sort_keys=True)


def find_duplicates(text: str) -> list[tuple[str, list[str]]]:
    """返回 [(键的完整路径, 各次出现的值文本), ...]，按路径排序。

    只认**同一个对象内**的重复键；跨对象同名不算（`searching` 在 `home`
    与 `perf` 里各有一份是正常写法）。
    """
    out: list[tuple[str, list[str]]] = []

    def walk(node, path: str) -> None:
        _, pairs = node
        seen: dict[str, list[str]] = {}
        for key, value in pairs:
            seen.setdefault(key, []).append(_canon(value))
        for key, values in seen.items():
            if len(values) > 1:
                out.append((f"{path}.{key}" if path else key, values))
        for key, value in pairs:
            if isinstance(value, tuple) and value and value[0] == MARK:
                walk(value, f"{path}.{key}" if path else key)

    walk(json.loads(text, object_pairs_hook=_hook), "")
    out.sort(key=lambda x: x[0])
    return out


def is_same_value(values: list[str]) -> bool:
    """两条重复键是否文本完全一致（同值 ⇒ 纯冗余，可直接删）。"""
    return len(set(values)) == 1


def scan() -> list[tuple[str, str, list[str]]]:
    """扫描全部词库文件，返回 [(文件名, 路径, 值列表), ...]。"""
    found = []
    for name in FILES:
        text = (I18N_DIR / name).read_text(encoding="utf-8")
        for path, values in find_duplicates(text):
            found.append((name, path, values))
    return found


def audit(found, allowed) -> tuple[list, list]:
    """返回 (bad, stale)。

    `bad`   —— 不在白名单里的重复键（即真缺陷）。
    `stale` —— **已失效的白名单条目**：登记的位置现在根本没有重复键了。

    ## 为什么 stale 也要判红（反向审查时发现的真实漏洞）

    白名单只为「值分歧」而设。假设某天有人修好了 `zh.json` 的 `home.timeline`
    却忘了删白名单条目，那么**同一个键将来再写一次重复，会被静默放行**——
    白名单就从「豁免」变成了「漏洞」，而且没有任何一处会报红。
    ⇒ 白名单必须与真源**双向**对齐：既不能放过未登记的，也不能留下已失效的。
    """
    keys = {(f, p) for f, p, _v in found}
    bad = [d for d in found if (d[0], d[1]) not in allowed]
    stale = [k for k in allowed if k not in keys]
    return bad, stale


def self_test() -> int:
    """合成用例。前四条是**真源的实际写法**，不是想象出来的形状。"""
    cases = [
        # (词库文本, 期望重复键数, 期望首条的路径, 期望分类 或 None 表示不关心)
        ('{"a": "1"}', 0, None, None),
        # 真源写法：home.timeline 的形状——同对象同名、值不同
        ('{"home": {"timeline": "时间线", "x": "1", "timeline": "时间线（回到历史点）"}}',
         1, "home.timeline", "分歧"),
        # 真源写法：home.copied 的形状——同对象同名、值完全一致
        ('{"home": {"copied": "已复制", "copied": "已复制"}}', 1, "home.copied", "同值"),
        # 真源写法：searching 分散在不同顶层块，那**不是**重复
        ('{"home": {"searching": "搜索中…"}, "perf": {"searching": "搜索中…"},'
         ' "nav": {"searching": "搜索中…"}}', 0, None, None),
        # 真源写法：真源全部集中在顶层 `home` 块内，路径必须带块名
        ('{"home": {"copied": "a", "searching": "b", "copied": "a", "searching": "b",'
         ' "forkFromHere": "c", "forkFromHere": "d"}}',
         3, "home.copied", "同值"),
        # 三次重复算一组，但要三个值都列出来
        ('{"k": "1", "k": "2", "k": "3"}', 1, "k", "分歧"),
        # 深层嵌套里的重复，路径要逐段拼对
        ('{"a": {"b": {"c": 1, "c": 1}}}', 1, "a.b.c", "同值"),
        # 值为对象时，键序不同**不算**分歧（按 sort_keys 规范化）
        ('{"k": {"x": 1, "y": 2}, "k": {"y": 2, "x": 1}}', 1, "k", "同值"),
    ]
    passed = 0
    for text, want_n, want_path, want_class in cases:
        dups = find_duplicates(text)
        if len(dups) != want_n:
            print(f"  ✗ 期望 {want_n} 组重复键，实得 {len(dups)}：{text[:60]}")
            continue
        if want_path is not None and dups[0][0] != want_path:
            print(f"  ✗ 期望路径 {want_path}，实得 {dups[0][0]}")
            continue
        if want_class is not None:
            got = "同值" if is_same_value(dups[0][1]) else "分歧"
            if got != want_class:
                print(f"  ✗ 期望分类 {want_class}，实得 {got}：{dups[0][0]}")
                continue
        passed += 1
    # 白名单的键必须与真源的文件名对得上，否则登记了也永远不会命中
    passed -= sum(
        1 for (fname, _path) in ALLOWED if fname not in FILES
        for _ in (print(f"  ✗ 白名单文件名 {fname} 不在扫描范围 {FILES}"),)
    )
    # 白名单双向对齐：未登记的要拦，已失效的也要拦
    allow_cases = [
        ([("zh.json", "home.x", ["a", "b"])], {("zh.json", "home.x")},
         (0, 0), "已登记的分歧项放行"),
        ([("zh.json", "home.x", ["a", "b"])], {},
         (1, 0), "未登记的重复键报红"),
        ([], {("zh.json", "home.gone")},
         (0, 1), "白名单条目已失效要报红（否则同名新重复会被静默放行）"),
        ([("zh.json", "home.x", ["a", "a"])], {("zh.json", "home.gone")},
         (1, 1), "一红一失效同时报出（home.x 未登记 + home.gone 白名单已失效）"),
        ([], {}, (0, 0), "两空不得误报"),
    ]
    for found, allowed, (want_bad, want_stale), name in allow_cases:
        bad, stale = audit(found, allowed)
        if (len(bad), len(stale)) != (want_bad, want_stale):
            print(f"  ✗ {name}：期望 bad/stale = {want_bad}/{want_stale}，"
                  f"实得 {len(bad)}/{len(stale)}")
            continue
        passed += 1
    # 输出格式必须与其余 16 道门一致（`自测 N/N`）：
    # `check-self-test-counts.py` 解析的是**输出文本**，
    # 这里改成英文就会报「读不出用例条数」——契约被破坏时该改门禁，不是放宽解析。
    print(f"自测 {passed}/{len(cases) + len(allow_cases)}")
    return 0 if passed == len(cases) + len(allow_cases) else 1


def main() -> int:
    # ⚠️ 必须在 `--self-test` 分支**之前**设好编码：
    # 中文 Windows 控制台是 GBK，print 一个 `✗` 就抛 UnicodeEncodeError，
    # 退出码 1 与「自测发现真问题」完全一样。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in sys.argv:
        return self_test()

    found = scan()
    bad, stale = audit(found, ALLOWED)
    allowed_hit = sum(1 for f, p, _v in found if (f, p) in ALLOWED)

    if stale:
        print(f"发现 {len(stale)} 条**已失效的白名单条目**（对应位置已没有重复键）：")
        for fname, path in stale:
            print(f"- {fname}  {path}")
        print("  → 连同 ALLOWED 里那一条一起删掉。")
        print("     留着它 = 同一个键将来再写重复会被**静默放行**，白名单就从豁免变成了漏洞。")
    if not bad:
        n = len(found)
        print(
            f"词库无重复键（{'、'.join(FILES)}）。"
            + (f"另有 {n} 处分歧项按白名单登记，待拍板。" if n else "")
        )
        return 1 if stale else 0
    print(f"发现 {len(bad)} 处重复键（另有 {allowed_hit} 处按白名单登记）：")
    for fname, path, values in bad:
        kind = "同值" if is_same_value(values) else "分歧"
        print(f"- {fname}  {path}  [{kind}]  出现 {len(values)} 次")
        for i, v in enumerate(values):
            mark = "  ← 运行时生效" if i == len(values) - 1 else ""
            print(f"    #{i} {v[:100]}{mark}")
        if kind == "同值":
            print("    两条文本完全一致 ⇒ 删掉任意一条都是零行为变化，直接删即可")
        else:
            print("    删掉被遮蔽的那条是零行为变化，但「留哪条」是措辞决策，需拍板后登记进 ALLOWED")
    return 1


if __name__ == "__main__":
    sys.exit(main())
