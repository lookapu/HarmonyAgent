#!/usr/bin/env python3
"""埋点契约门禁：前端事件名、Rust 白名单、i18n 展示名、IPC 注册四方必须一致。

对应 docs/UI_REFINEMENT_PLAN.md 的 P2-1（§6.15）。

**为什么要有这道门**：事件名在三个地方各存一份——
`src/utils/telemetry.ts` 的 `EVENT_NAMES`、`queries.rs` 的 `EVENT_NAMES`、
中英文 i18n 的 `usage.event.*`。三份一旦漂移，后果是**静默的**：

- 前端多一个、Rust 少一个 → `record_usage_event` 抛 `InvalidParameterName`，
  而正常路径**吞掉错误**（埋点不该影响用户操作），于是事件永远不出现，
  面板上少一行，**没有任何地方报错**。
- Rust 多一个、前端少一个 → 白名单里躺着一个死事件名，面板上永远为 0。
- i18n 缺展示名 → 面板上显示成原始事件名（`context_compacted` 而不是「上下文已压缩」），
  同样不报错。

`UsagePage` 的自检按钮是**运行时**的人工验证手段；本门是**构建期**的那一道。

**契约**（改动前先确认不违反）：
1. 前后端 `EVENT_NAMES` 必须**逐字同序**——不只是集合相同。顺序是面板里
   条形分布的展示顺序，按集合比对会让「谁在前」变成没人管的事。
2. 每个事件名在 zh / en 都必须有 `usage.event.*` 展示名。
3. `usage.event.*` 不得有多余项（事件删了忘删 i18n）。
4. `telemetry.rs` 里注册的每个 command 都必须有前端调用点：
   注册了没人调 = 死代码，且会让 check-docs 的 IPC 数字虚增；
   反过来前端调了没注册 = 运行时才炸。

用法
  python scripts/check-telemetry-contract.py             # 校验（CI 门禁）
  python scripts/check-telemetry-contract.py --self-test  # 合成样例回归

自测：python scripts/check-telemetry-contract.py --self-test
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TS_FILE = "src/utils/telemetry.ts"
RS_FILE = "src-tauri/src/db/queries.rs"
CMD_FILE = "src-tauri/src/commands/telemetry.rs"
LIB_FILE = "src-tauri/src/lib.rs"
API_FILE = "src/api/telemetry.ts"

RUST_NAMES_RE = re.compile(r"pub const EVENT_NAMES:.*?\[&str\]\s*=\s*&\[(.*?)\];", re.S)
TS_NAMES_RE = re.compile(r"export const EVENT_NAMES = \[(.*?)\]", re.S)
INVOKE_RE = re.compile(r"invokeWithError<[^>]*>\(\s*'([a-z_]+)'")


def read(repo: Path, rel: str) -> str:
    path = repo / rel
    return path.read_text(encoding="utf-8") if path.exists() else ""


def event_names(repo: Path) -> tuple[list[str], list[str]]:
    """返回 (前端事件名, Rust 白名单)。任一侧解析失败时返回空表，由调用方报错。"""
    ts_block = TS_NAMES_RE.search(read(repo, TS_FILE))
    rust_block = RUST_NAMES_RE.search(read(repo, RS_FILE))
    ts = re.findall(r"'([a-z_]+)'", ts_block.group(1)) if ts_block else []
    rust = re.findall(r'"([a-z_]+)"', rust_block.group(1)) if rust_block else []
    return ts, rust


def check(repo: Path) -> list[str]:
    problems: list[str] = []

    ts, rust = event_names(repo)
    if not ts:
        problems.append(f"解析不到前端事件名：{TS_FILE}")
    if not rust:
        problems.append(f"解析不到 Rust 白名单：{RS_FILE}")
    if not ts or not rust:
        return problems

    if ts != rust:
        only_ts = [n for n in ts if n not in rust]
        only_rust = [n for n in rust if n not in ts]
        detail = []
        if only_ts:
            detail.append(f"仅前端有（后端会静默丢弃）: {', '.join(only_ts)}")
        if only_rust:
            detail.append(f"仅后端有（永远为 0 的死事件名）: {', '.join(only_rust)}")
        if not detail:
            detail.append("内容相同但顺序不同")
        problems.append(f"前后端事件名不一致 —— {'；'.join(detail)}")

    for dup in {n for n in ts if ts.count(n) > 1}:
        problems.append(f"前端事件名重复：{dup}")

    # i18n 展示名（缺了不报错，只是面板上显示成原始事件名）
    for lang in ("zh", "en"):
        path = repo / f"src/i18n/{lang}.json"
        if not path.exists():
            problems.append(f"缺少 i18n 文件：src/i18n/{lang}.json")
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as e:
            problems.append(f"src/i18n/{lang}.json 解析失败：{e}")
            continue
        shown = data.get("usage", {}).get("event", {})
        for name in ts:
            if name not in shown:
                problems.append(f"{lang} 缺展示名：usage.event.{name}")
        for name in shown:
            if name not in ts:
                problems.append(f"{lang} 有多余展示名：usage.event.{name}")

    # command 注册 <-> 前端调用
    lib = read(repo, LIB_FILE)
    api = read(repo, API_FILE)
    cmd_src = read(repo, CMD_FILE)
    registered = set(re.findall(r"commands::telemetry::([a-z_]+)", lib))
    if not registered:
        problems.append(f"{LIB_FILE} 里没有注册任何 telemetry command")
    for name in sorted(registered):
        if f"'{name}'" not in api:
            problems.append(f"command {name} 已注册但前端无调用点（死代码，且 IPC 数字虚增）")
    for called in set(INVOKE_RE.findall(api)):
        if called not in registered:
            problems.append(f"前端调用了未注册的 command：{called}")
    # usage_event_names 必须返回的就是同一份白名单，否则运行时自检会误报漂移
    if registered and "EVENT_NAMES" not in cmd_src:
        problems.append(f"{CMD_FILE} 未引用 EVENT_NAMES，usage_event_names 可能返回的不是白名单")

    return problems


def self_test() -> None:
    """合成样例回归：漂移、缺展示名、多余展示名、死 command、未注册调用，各自必须被检出。"""
    import tempfile

    def make_repo(root: Path, *, ts_names, rust_names, lang_event, registered, api_calls, cmd_src=""):
        for rel in ("src/utils", "src/i18n", "src/api", "src-tauri/src/db", "src-tauri/src/commands"):
            (root / rel).mkdir(parents=True, exist_ok=True)
        (root / TS_FILE).write_text(
            "export const EVENT_NAMES = [\n"
            + "".join(f"  '{n}',\n" for n in ts_names) + "]\n", encoding="utf-8")
        (root / RS_FILE).write_text(
            'pub const EVENT_NAMES: &[&str] = &[\n'
            + "".join(f'    "{n}",\n' for n in rust_names) + "];\n", encoding="utf-8")
        for lang in ("zh", "en"):
            (root / f"src/i18n/{lang}.json").write_text(
                json.dumps({"usage": {"event": lang_event}}, ensure_ascii=False), encoding="utf-8")
        (root / LIB_FILE).write_text(
            "\n".join(f"            commands::telemetry::{c}," for c in registered) + "\n",
            encoding="utf-8")
        (root / API_FILE).write_text(
            "import { invokeWithError } from './invoke'\n"
            + "".join(f"export const f_{c} = () => invokeWithError<boolean>('{c}')\n" for c in api_calls),
            encoding="utf-8")
        (root / CMD_FILE).write_text(cmd_src or "// uses EVENT_NAMES\n", encoding="utf-8")

    good = dict(ts_names=["a", "b"], rust_names=["a", "b"], lang_event={"a": "甲", "b": "乙"},
                registered=["usage_stats_total"], api_calls=["usage_stats_total"])
    cases = [
        ("全绿", good, None),
        ("前端多一个（后端静默丢弃）",
         {**good, "ts_names": ["a", "b", "c"], "lang_event": {"a": "甲", "b": "乙", "c": "丙"}},
         "仅前端有"),
        ("后端多一个（死事件名）",
         {**good, "rust_names": ["a", "b", "c"]},
         "仅后端有"),
        ("同集合但顺序不同",
         {**good, "rust_names": ["b", "a"]},
         "顺序不同"),
        ("i18n 缺展示名",
         {**good, "lang_event": {"a": "甲"}},
         "缺展示名"),
        ("i18n 多余展示名",
         {**good, "lang_event": {"a": "甲", "b": "乙", "z": "旧"}},
         "多余展示名"),
        ("command 注册了没人调",
         {**good, "registered": ["usage_stats_total", "clear_usage_stats"]},
         "无调用点"),
        ("前端调了没注册",
         {**good, "api_calls": ["usage_stats_total", "ghost_cmd"]},
         "未注册的 command"),
        ("命令没返回白名单",
         {**good, "cmd_src": "// 空实现\n"},
         "usage_event_names 可能返回的不是白名单"),
    ]

    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        for label, kwargs, expected in cases:
            repo = Path(tmp) / label.replace(" ", "_").replace("/", "_")
            repo.mkdir()
            make_repo(repo, **kwargs)
            problems = check(repo)
            if expected is None:
                if problems:
                    print(f"  ✗ {label}: 期望通过，实际失败: {problems}")
                    failed += 1
            elif not any(expected in p for p in problems):
                print(f"  ✗ {label}: 期望问题 '{expected}' 未出现: {problems}")
                failed += 1
    if failed:
        print(f"self-test FAILED: {failed} 个用例失败")
        sys.exit(1)
    print(f"self-test OK: {len(cases)} 个合成用例全部符合预期")


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")

    parser = argparse.ArgumentParser(description="埋点契约校验（前后端事件名 / i18n / IPC）")
    parser.add_argument("--repo", default=str(REPO))
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0

    problems = check(Path(args.repo))
    if problems:
        print(f"埋点契约校验失败：{len(problems)} 项")
        for p in problems:
            print(f"- {p}")
        return 1
    ts, _ = event_names(Path(args.repo))
    print(f"埋点契约校验通过：{len(ts)} 个事件名在前后端 / i18n / IPC 四方一致")
    return 0


if __name__ == "__main__":
    sys.exit(main())
