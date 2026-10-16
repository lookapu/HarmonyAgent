#!/usr/bin/env python3
"""把 i18n 的「同一句话、两侧不一致」生成台账（第十八批重写）。

## 为什么这次重写

第十六批的版本按「能不能不看上下文就判定」分三档，把「换了词」的一档
**整体推给人判断**，清单写在 `docs/I18N_WORDING_AUDIT.md` 里靠人扫。
第十八批把那一档真正处理掉了：38 组逐组挖使用点，判定成

- 真的漂移 → 改英文（36 键）
- 本来就该不同 → **写个书面理由**存进 `scripts/i18n-wording-explained.json`

于是台账不再是一张「待办清单」，而是一张**判读记录**：
「0 组未解释」是一个机器能判的事实（门禁契约 5），不是一句承诺。

## 不重写分类逻辑

分类的唯一实现在 `scripts/check-i18n-wording.py` 的 `classify_groups()`。
本脚本 import 它，不自己分档。

**两份分类实现各跳各的**是第十六批的真实故障：审计报 50 组、门禁报 49 组，
差的那组含 i18next 复数键，根因是门禁跳过、审计不跳。
数字分叉之后，文档里那个数字就没有人核对得动了。
"""
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"

#: 门禁模块名带连字符，不能直接 import；按路径加载一次。
_spec = importlib.util.spec_from_file_location(
    "check_i18n_wording", SCRIPTS / "check-i18n-wording.py"
)
_gate = importlib.util.module_from_spec(_spec)
sys.modules["check_i18n_wording"] = _gate
_spec.loader.exec_module(_gate)

flatten = _gate.flatten
classify_groups = _gate.classify_groups
load_explained = _gate.load_explained


def main() -> int:
    zh = flatten(json.loads((ROOT / "src" / "i18n" / "zh.json").read_text(encoding="utf-8")))
    en = flatten(json.loads((ROOT / "src" / "i18n" / "en.json").read_text(encoding="utf-8")))
    explained = load_explained()

    identical, case_only, diff_word, skipped = classify_groups(zh, en)
    unexplained = [(z, k, v) for z, k, v in diff_word if z not in explained]

    out = ROOT / "docs" / "I18N_WORDING_AUDIT.md"
    lines = [
        "# i18n 措辞一致性判读记录",
        "",
        "> 本文件由 `scripts/audit_i18n_wording.py` 生成，**不要手改**。",
        "> 重跑：`python scripts/audit_i18n_wording.py`",
        "",
        "同一个中文句子对应多个 i18n 键时，中文侧相同是**正常的**",
        "（导航项与页面标题本来就该用同一句话，本仓 120 组如此）。",
        "要管的是英文侧：同一个词在同一屏界面里有两种写法。",
        "",
        "## 判据",
        "",
        "**不是「同一个中文就该同一个英文词」，而是",
        "「两个键指的是不是同一界面里的同一个东西」。**",
        "",
        "按钮 vs 状态、图例 vs 动作、面板 vs 表格列、集合 vs 单项——这些**本来就该不一样**。",
        "中文过共享是**数据组织问题**，不是英文措辞问题；把两边硬拉平，",
        "反而会在同一屏制造两个一模一样的标签去指两个不同的数。",
        "",
        "实例：`cost.taskAvg` 与 `cost.toolAverageDuration` 中文都是「平均耗时」，",
        "但前者取 `taskStats.avg_duration_ms`（任务级，同族 `taskLegend`/`taskCost`），",
        "后者取 `reliability.tool_quality.average_duration_ms`（工具质量级，同族 `toolSuccessRate`）。",
        "两个不同的数，英文的区分是对的。",
        "",
        "## 汇总",
        "",
        "| 档 | 组数 | 涉及键数 |",
        "|---|---:|---:|",
        f"| 完全一致（正常） | {len(identical)} | {sum(len(k) for _z, k, _v in identical)} |",
        f"| **仅大小写不同** | {len(case_only)} | {sum(len(k) for _z, k, _v in case_only)} |",
        f"| 英文不一致 · **已书面解释** | {len(diff_word) - len(unexplained)} | "
        f"{sum(len(k) for z, k, _v in diff_word if z in explained)} |",
        f"| 英文不一致 · **未解释** | {len(unexplained)} | "
        f"{sum(len(k) for z, k, _v in diff_word if z not in explained)} |",
        f"| 含空值跳过（归契约 4） | {len(skipped)} | {sum(len(k) for _z, k, _v in skipped)} |",
        "",
        f"**未解释 {len(unexplained)} 组**。门禁 `scripts/check-i18n-wording.py` 契约 5 会把"
        f"「未解释」判为红，",
        f"契约 6 会把「解释表里有、但当前已无对应冲突组」的**失效条目**也判为红——",
        "允许清单不报错的后果是组修好之后条目留在表里，",
        "后来人以为「这组解释过了」，其实解释的是已经不存在的现象。",
        "",
        "## 判读规则",
        "",
        "| verdict | 含义 |",
        "|---|---|",
        "| `different-slot` | 同一个东西在不同槽位（动作/状态/图例/列头/导航 vs 页面标题），本来就该不同 |",
        "| `different-thing` | 两个键指的根本不是同一个东西，中文侧过共享 |",
        "| `zh-side` | 疑似中文侧标注问题，需产品口径，不擅自动用户可见文案 |",
        "| `dead-key` | 该键在 src 里零引用，值已统一，键待清理 |",
        "",
        "---",
        "",
        "## 英文不一致的组（逐组附判读理由）",
        "",
        "| 中文 | 键 | 英文 | verdict | 理由 |",
        "|---|---|---|---|---|",
    ]

    for zv, keys, vals in sorted(diff_word, key=lambda x: x[1][0]):
        entry = explained.get(zv)
        verdict = entry["verdict"] if entry else "**未解释**"
        reason = (entry["reason"] if entry else "**门禁会报红：写不出理由的组就是待修的漂移**")
        reason = reason.replace("|", "\\|")
        lines.append(
            f"| {zv} | {', '.join(f'`{k}`' for k in keys)} | "
            f"{' / '.join(vals)} | {verdict} | {reason} |"
        )

    lines += [
        "",
        "---",
        "",
        "## 修正记录（第十八批）",
        "",
        "共改 36 键，全部**按 key 定点替换**（顶层段 + 叶键 + 旧值三重锚定，段内唯一），",
        "不是 `json.load` + `json.dump` 全量重写——后者会重排整个文件、产生几百行假 diff。",
        "",
        "| 段 | 键数 | 内容 |",
        "|---|---:|---|",
        "| A 真漂移 | 6 | 确认删除？/ 已删除 / 保存失败 / 成功率 / 调用次数 / 引用文件 |",
        "| B chat -> conversation | 7 | 「会话记录」这一个名词义项；全库 50 : 22 |",
        "| C 审计族内部一致 | 3 | `home.auditLabel*` 三个不带宾语的补上宾语 |",
        "| D 省略号 | 13 | 忙碌态语义簇的 `...` -> `…`（U+2026） |",
        "| E 第二轮 | 7 | 第一轮漏判的组：当前 / 切换模型 / 刷新 / 重试 / 读取失败 |",
        "",
        "### B 段的决定性证据",
        "",
        "同一句话里就混着两个词：",
        "`home.noConversation` = \"No **conversations** yet. Click New **Chat** to start\"，",
        "侧栏标题是 `home.conversations` = \"Conversations\"，",
        "而同一个右键菜单里是 \"Pin **chat** / Archive **chat** / Delete **chat**\"。",
        "",
        "但 22 个含 chat 的键里**有一大半不是漂移**，一律不动：",
        "`Send to chat`（动词短语）、`Chat settings`、`in the chat area`、",
        "`neither chat nor tools` —— 那些是「对话区域 / 对话行为」义项，",
        "改成 conversation 语法就错了（\"Send to conversation\"）。",
        "",
        "### 三条被证据推翻、因此不改",
        "",
        "- `repro.title`「Issue Reproduction Bundles」**不该**缩短：",
        "  全仓页面标题域的既定模式就是「nav 短、页面标题长」——",
        "  Cost / Cost Tracking、MCP / MCP Servers、Health / Health Monitor 同款。",
        "- `config.telemetry`「Usage stats」**不该**改成 Title Case：",
        "  它是设置项行标签（句首式），`usage.title` 是页面标题（Title Case），",
        "  两者本来就该不同；改了就与第十六批刚统一的规则相反。",
        "- `home.detailRam`「RAM」**不该**改成「Memory」：",
        "  `home.devicePerfMem` 是**百分比** PerfCard（`unit=\"%\"`），",
        "  `home.detailRam` 是 **GB 容量**行。两个不同的量。",
        "",
        "---",
        "",
        "## 完全一致的一档（抽样 15 组，说明这一档不是问题）",
        "",
        "| 中文 | 键 | 英文 |",
        "|---|---|---|",
    ]
    for zv, keys, vals in identical[:15]:
        lines.append(f"| {zv} | {', '.join(f'`{k}`' for k in keys)} | {vals[0]} |")

    out.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="")

    n_keys = sum(len(k) for _z, k, _v in diff_word)
    print(f"完全一致 {len(identical)} 组")
    print(f"仅大小写不同 {len(case_only)} 组，涉及 {sum(len(k) for _z, k, _v in case_only)} 个键")
    print(
        f"英文不一致 {len(diff_word)} 组，涉及 {n_keys} 个键："
        f"已书面解释 {len(diff_word) - len(unexplained)} 组，**未解释 {len(unexplained)} 组**"
    )
    if explained:
        by_verdict: dict[str, int] = {}
        for zv in explained:
            v = explained[zv]["verdict"]
            by_verdict[v] = by_verdict.get(v, 0) + 1
        print(f"解释表 verdict 分布：{by_verdict}")
    print(f"报告已写入 {out.relative_to(ROOT)}")
    for zv, keys, vals in case_only:
        print(f"  [大小写] {keys} -> {vals}")
    for zv, keys, vals in diff_word:
        if zv not in explained:
            print(f"  [未解释] {keys} -> {vals}")
    return 1 if unexplained else 0


if __name__ == "__main__":
    sys.exit(main())
