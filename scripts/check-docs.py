#!/usr/bin/env python3
"""Q-08 文档漂移校验：数量、路径、接口与状态必须与代码真源一致。

从代码真源提取工具数、迁移数、IPC 入口、模块数等数量，再逐模式校验
中英文 README / ARCHITECTURE / TOOL_ENHANCEMENTS / TOOLCHAIN_ACCEPTANCE /
TOOL_RESULT_V2 / CHANGELOG / VERSION_COMPATIBILITY 中的对应数字；
校验 ROADMAP 与 docs 内相对链接目标存在、ROADMAP 反引号路径存在；
校验 quality.yml / release.yml 引用的 Rust 测试、集成测试与脚本存在；
校验 `##`/`###` 章节**必须有正文**（标题下面只剩标题 = 空章节），
其中代码围栏内的 `#` 不算标题、`####` 子标题不算「无正文」。

CHANGELOG 的特殊约定：只在**最新版本段**（第一个 `## ` 到下一个 `## ` 之前）
匹配当前规模声明。历史版本段里的旧数字在当时是对的，改它就是篡改历史。

用法：
  python3 scripts/check-docs.py                    # 校验真实仓库
  python3 scripts/check-docs.py --self-test        # 合成仓库自测（篡改检测）
退出码：0=全部通过；1=存在漂移（CI 阻断）。
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
TOOL_SPEC_RE = re.compile(r"^    ToolSpec \{", re.M)


def read_doc(repo: Path, rel: str) -> str:
    """按 UTF-8 读取仓库内文档。

    校验侧一律按 UTF-8 读，而 Windows 上 write_text/read_text 默认用本地编码
    （cp936/cp1252）：不显式指定时，中文内容会被写成/读成 mojibake，自检的合成仓库
    即使"干净"也会报一堆未找到模式（CI 是 UTF-8 locale，碰不到这个坑）。
    """
    return (repo / rel).read_text(encoding="utf-8", errors="replace")


def write_doc(repo: Path, rel: str, text: str) -> None:
    (repo / rel).write_text(text, encoding="utf-8")


def count_tool_specs(repo: Path) -> int:
    source = read_doc(repo, "src-tauri/src/agent/tools/mod.rs")
    return len(TOOL_SPEC_RE.findall(source))


def count_migrations(repo: Path) -> tuple[int, int]:
    """返回 (文件数, 注册数)；两者不一致本身即漂移。"""
    files = len(list((repo / "src-tauri/migrations").glob("*.sql")))
    text = read_doc(repo, "src-tauri/src/db/mod.rs")
    registered = len(re.findall(r"include_str!\(\"\.\./\.\./migrations/", text))
    return files, registered


def count_pages(repo: Path) -> int:
    """页面数。**排除 *.test.tsx**——同目录下的页面测试也是 .tsx。

    排除规则与 scripts/check-ui-states.py 的页面枚举保持一致：两处若不同步，
    加一个页面测试就会同时触发「文档页数对不上」与「页面未声明 @ui-states」，
    同一个改动两种错法，排查时容易误判成两个独立问题。
    """
    return len(
        [p for p in (repo / "src/pages").glob("*.tsx") if not p.name.endswith(".test.tsx")]
    )


def count_files(repo: Path, rel_dir: str, exclude: tuple[str, ...]) -> int:
    return len(
        [p for p in (repo / rel_dir).glob("*.rs") if p.name not in exclude]
    )


def count_ipc_entries(repo: Path) -> int:
    text = read_doc(repo, "src-tauri/src/lib.rs")
    lines = text.splitlines()
    inside = False
    count = 0
    for line in lines:
        if "generate_handler![" in line:
            inside = True
            continue
        if inside and re.match(r"^\s*\]\)", line):
            inside = False
            continue
        if inside and "commands::" in line:
            count += 1
    return count


def check_counts(repo: Path) -> list[str]:
    """文档中的数量模式必须与代码真源一致。"""
    problems = []
    tools = count_tool_specs(repo)
    files, registered = count_migrations(repo)
    if files != registered:
        problems.append(
            f"迁移不一致：migrations/ 目录 {files} 个文件，db/mod.rs 注册 {registered} 个"
        )
    migrations = files
    pages = count_pages(repo)
    commands = count_files(repo, "src-tauri/src/commands", ("mod.rs",))
    services = count_files(repo, "src-tauri/src/services", ("mod.rs",))
    agent_modules = count_files(repo, "src-tauri/src/agent", ("mod.rs",))
    tools_files = len(list((repo / "src-tauri/src/agent/tools").glob("*.rs")))
    ipc = count_ipc_entries(repo)
    checks = [
        ("README.md", r"\*\*(\d+) 个 Agent 工具\*\*", tools, "工具数"),
        ("README.md", r"(\d+) 个 Tauri IPC 入口", ipc, "IPC 入口"),
        ("README.md", r"agent/ (\d+) 个顶层模块", agent_modules, "agent 模块数"),
        ("README.md", r"tools/ (\d+) 文件", tools_files - 1, "tools 文件数(不含注册表)"),
        ("README.md", r"个 Agent 工具（(\d+) 文件）", tools_files - 1, "tools 文件数(括号)"),
        ("README.md", r"(\d+) 个命令模块", commands, "commands 模块数"),
        ("README.md", r"业务服务（(\d+) 个）", services, "services 模块数"),
        ("README.md", r"(\d+) 个 service 模块", services, "services 模块数(简写)"),
        ("README.md", r"SQLite \+ (\d+) 个迁移", migrations, "迁移数"),
        ("README.md", r"## (\d+) 个 Agent 工具按域分组", tools, "工具数(分组标题)"),
        ("ARCHITECTURE.md", r"\| Agent 对外工具 \| (\d+) \|", tools, "工具数"),
        ("ARCHITECTURE.md", r"\| `agent/` 顶层模块（不含 `mod.rs`） \| (\d+) \|", agent_modules, "agent 模块数"),
        ("ARCHITECTURE.md", r"\| `agent/tools/` Rust 文件（含 `mod.rs`） \| (\d+) \|", tools_files, "tools 文件数(含注册表)"),
        ("ARCHITECTURE.md", r"\| `commands/` 命令模块（不含 `mod.rs`） \| (\d+) \|", commands, "commands 模块数"),
        ("ARCHITECTURE.md", r"\| `services/` 服务模块（不含 `mod.rs`） \| (\d+) \|", services, "services 模块数"),
        ("ARCHITECTURE.md", r"\| Tauri IPC 注册入口 \| (\d+) \|", ipc, "IPC 入口"),
        ("ARCHITECTURE.md", r"\| 数据库迁移 \| (\d+) \|", migrations, "迁移数"),
        ("ARCHITECTURE.md", r"\| React 页面 \| (\d+) \|", pages, "页面数"),
        ("ARCHITECTURE.md", r"SQLite（(\d+) 个迁移）", migrations, "迁移数(图)"),
        ("ARCHITECTURE.md", r"当前 (\d+) 个迁移", migrations, "迁移数(正文)"),
        ("ARCHITECTURE.md", r"(\d+) 工具 / 审批流水线", tools, "工具数(图)"),
        ("TOOL_ENHANCEMENTS.md", r"\| 对外 Agent 工具 \| (\d+) \|", tools, "工具数"),
        ("TOOL_ENHANCEMENTS.md", r"不属于当前 (\d+) 工具", tools, "工具数(暂缓)"),
        ("TOOL_ENHANCEMENTS.md", r"\| 工具实现文件 \| (\d+) \|", tools_files - 1, "工具实现文件数"),
        ("TOOLCHAIN_ACCEPTANCE.md", r"(\d+) 个注册工具共享契约真源", tools, "工具数"),
        ("TOOL_RESULT_V2.md", r"(\d+) 个注册工具均产生完整稳定字段", tools, "工具数"),
        ("CHANGELOG.md", r"`TOOL_SPECS` 达到 \*\*(\d+)\*\*", tools, "工具数"),
        ("CHANGELOG.md", r"数据库迁移总数达到 \*\*(\d+)\*\*", migrations, "迁移数"),
        ("VERSION_COMPATIBILITY.md", r"迁移数（当前 (\d+)）", migrations, "迁移数"),
        ("README.en.md", r"\*\*(\d+) Agent tools\*\*", tools, "Tool count"),
        ("README.en.md", r"(\d+) Tauri IPC entry points", ipc, "IPC entry count"),
        ("README.en.md", r"agent/ (\d+) top-level modules", agent_modules, "Agent module count"),
        ("README.en.md", r"tools/ (\d+) files", tools_files - 1, "Tool file count"),
        ("README.en.md", r"(\d+) command modules", commands, "Command module count"),
        ("README.en.md", r"Business services \((\d+)\)", services, "Service module count"),
        ("README.en.md", r"(\d+) service modules", services, "Service module count (summary)"),
        ("README.en.md", r"SQLite \+ (\d+) migrations", migrations, "Migration count"),
        ("README.en.md", r"## The (\d+) Agent Tools Grouped", tools, "Tool count (heading)"),
        ("ARCHITECTURE.en.md", r"\| Agent-facing tools \| (\d+) \|", tools, "Tool count"),
        ("ARCHITECTURE.en.md", r"\| `agent/` top-level modules \(excluding `mod.rs`\) \| (\d+) \|", agent_modules, "Agent module count"),
        ("ARCHITECTURE.en.md", r"\| `agent/tools/` Rust files \(incl\. `mod.rs`\) \| (\d+) \|", tools_files, "Tool file count"),
        ("ARCHITECTURE.en.md", r"\| `commands/` command modules \(excluding `mod.rs`\) \| (\d+) \|", commands, "Command module count"),
        ("ARCHITECTURE.en.md", r"\| `services/` service modules \(excluding `mod.rs`\) \| (\d+) \|", services, "Service module count"),
        ("ARCHITECTURE.en.md", r"\| Tauri IPC registration entry points \| (\d+) \|", ipc, "IPC entry count"),
        ("ARCHITECTURE.en.md", r"\| Database migrations \| (\d+) \|", migrations, "Migration count"),
        ("ARCHITECTURE.en.md", r"\| React pages \| (\d+) \|", pages, "Page count"),
        ("ARCHITECTURE.en.md", r"SQLite \((\d+) migrations\)", migrations, "Migration count (diagram)"),
        ("ARCHITECTURE.en.md", r"current (\d+) migrations", migrations, "Migration count (body)"),
        ("ARCHITECTURE.en.md", r"(\d+) tools / approval pipeline", tools, "Tool count (diagram)"),
        ("TOOL_ENHANCEMENTS.en.md", r"\| External Agent tools \| (\d+) \|", tools, "Tool count"),
        ("TOOL_ENHANCEMENTS.en.md", r"current (\d+) tools", tools, "Tool count (deferred)"),
        ("TOOL_ENHANCEMENTS.en.md", r"\| Tool implementation files \| (\d+) \|", tools_files - 1, "Tool implementation file count"),
        ("CHANGELOG.en.md", r"`TOOL_SPECS` to \*\*(\d+)\*\*", tools, "Tool count"),
        ("CHANGELOG.en.md", r"migration count reaches \*\*(\d+)\*\*", migrations, "Migration count"),
    ]
    for doc, pattern, expected, label in checks:
        path = repo / doc
        if not path.exists():
            path = repo / "docs" / doc
        if not path.exists():
            problems.append(f"文档不存在：{doc}")
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        # CHANGELOG 是**按版本分节的历史记录**：v2.1.0 里写的「迁移总数达到 **84**」
        # 在当时是对的，改成 85 就是篡改历史。所以只在**最新版本段**里找当前规模声明
        # ——从第一个 `## ` 到**下一个** `## ` 之前（只取一段，不是从第一个到文末，
        # 否则后面历史段里的旧数字仍会被匹配到，那正是第一版写法的 bug）。
        # 2026-10-01 加这条：加第 85 个迁移时门禁把 v2.1.0 的历史数字判成漂移。
        if doc.startswith("CHANGELOG"):
            head = re.search(r"(?m)^## ", text)
            if head:
                nxt = re.search(r"(?m)^## ", text[head.end():])
                text = text[head.start():head.end() + nxt.start()] if nxt else text[head.start():]
        match = re.search(pattern, text)
        if not match:
            problems.append(f"{label}：{doc} 未找到模式 {pattern!r}")
            continue
        actual = int(match.group(1))
        if actual != expected:
            problems.append(
                f"{label}：{doc} 文档写 {actual}，代码真源为 {expected}"
            )
    return problems


def check_links(repo: Path) -> list[str]:
    """中英文 README/CHANGELOG 与 docs/*.md 中的相对链接目标必须存在。"""
    problems = []
    docs_dir = repo / "docs"

    def resolve(base_dir: Path, target: str) -> Path | None:
        if target.startswith(("http://", "https://", "mailto:", "#")) \
                or target.startswith("<") or target.endswith((".png", ".jpg", ".jpeg")):
            return None
        # 锚点片段不是文件路径（约定“锚点除外”）：先剥离 #fragment 再判定目标存在，
        # 否则 ./x.md#section 这类合法链接会被误判成漂移。
        path_part = target.split("#", 1)[0]
        if not path_part:
            return None
        if path_part.startswith("../"):
            return (base_dir / path_part).resolve()
        if path_part.startswith("/"):
            return None
        return (base_dir / path_part).resolve()

    root_docs = [
        path for pattern in ("README*.md", "CHANGELOG*.md")
        for path in repo.glob(pattern)
    ]
    for doc in sorted([*docs_dir.glob("*.md"), *root_docs]):
        text = doc.read_text(encoding="utf-8", errors="replace")
        for match in re.finditer(r"\]\(([^)]+)\)", text):
            target = match.group(1).strip()
            resolved = resolve(doc.parent, target)
            if resolved is None:
                continue
            if not resolved.exists():
                problems.append(f"{doc.name} 链接目标不存在：{target}")
    return problems


def check_path_refs(repo: Path) -> list[str]:
    """中英文 ROADMAP 中反引号内的代码/脚本/迁移路径必须存在。"""
    problems = []
    for roadmap in (repo / "docs/ROADMAP.md", repo / "docs/ROADMAP.en.md"):
        if not roadmap.exists():
            continue
        text = roadmap.read_text(encoding="utf-8", errors="replace")
        for match in re.finditer(r"`((?:src-tauri|scripts|src|docs)/[^`]+)`", text):
            ref = match.group(1).strip().rstrip("/")
            if not ref:
                continue
            if ref.endswith((".rs", ".py", ".sql", ".json", ".tsx", ".ts", ".md")):
                if not (repo / ref).exists():
                    problems.append(f"{roadmap.name} 引用路径不存在：{ref}")
    return problems


def check_ci_interfaces(repo: Path) -> list[str]:
    """quality.yml/release.yml 引用的测试与脚本必须存在。"""
    problems = []
    quality = read_doc(repo, ".github/workflows/quality.yml")
    for match in re.finditer(r"agent::evals::tests::([a-z_]+)", quality):
        name = match.group(1)
        source = read_doc(repo, "src-tauri/src/agent/evals.rs")
        if not re.search(rf"fn {name}\b", source):
            problems.append(f"quality.yml 引用不存在的测试：agent::evals::tests::{name}")
    for match in re.finditer(r"--test ([a-zA-Z0-9_]+)", quality):
        name = match.group(1)
        if not (repo / f"src-tauri/tests/{name}.rs").exists():
            problems.append(f"quality.yml 引用不存在的集成测试：{name}")
    release = read_doc(repo, ".github/workflows/release.yml")
    for match in re.finditer(r"python3? scripts/([a-zA-Z0-9_.-]+\.py)", release):
        name = match.group(1)
        if not (repo / f"scripts/{name}").exists():
            problems.append(f"release.yml 引用不存在的脚本：{name}")
    return problems


def check_empty_sections(repo: Path) -> list[str]:
    """`##`/`###` 章节必须有正文：标题下面只剩标题 = 空章节。

    ## 为什么要有这条（2026-10-08，第二十四批自查时真的撞上）

    `docs/TECH_DEBT.md` 里当时同时存在两个 `### P-6` 标题：旧的写着「等拍板」，
    新的写着「撤回重开」，而**旧的那个下面什么都没有**。后果不是「多了个字」：

    - 台账顶部声明「产品判断 **6** 条待决」，而文件里实际有 **7** 个 `### P-`
      标题 ⇒ **声明与事实对不上，且没有任何一处会报红**；
    - 更要紧的是：扫台账的人在「等拍板」处就停了，**永远读不到下面那句
      「原拍板已作废，不要按它实现」**——而那半句才是危险所在。

    成因不是粗心，是**新旧并存**：想「给旧条目加个状态标注」时，实际动作是
    **新加一个标题**。这类改法在「按状态维护」的台账里会反复出现，所以值得判。

    ## 判据边界（三条都实测过，不是推测）

    - **只看 `##`/`###` 及更深，不看 `#`**：H1 后面紧跟 `##` 是正常结构
      （CHANGELOG 就是「# 标题」+「## Unreleased」），把 H1 判进去全是误报。
      ⚠️ **如实标注承重方**：变异实测，把 H1 纳进来在**真源语料**上命中 **0 条**
      （现有文档的 H1 恰好都有正文）——**真正钉住它的是自测合成仓库**里
      `# other` 这种单标题文件。删掉这行守卫，真源不会立刻变红，但自测会。
    - **子标题不算正文**：`### X` / `#### 子标题` / 正文 ⇒ `### X` **不算空**。
      所以判据是「到下一个**同级或更高级**标题为止，有没有非标题的正文」，
      **不是**「下一行是不是标题」——后者会把上面这种合法结构全判成空。
      这条**承重于真源**：削弱成「下一行」后真源立刻误报 **63 条**。
    - **代码围栏内的 `#` 不是标题**：否则文档里贴一段 `## foo` 的 shell 命令
      就会被当成章节。
      ✅ **这条在真源上是承重的**，而且是被**本项目自己的记录**逼出来的：
      把这个缺陷写进文档时，惯用的是 ` ```markdown ` 围栏 + 里面两行 `### P-6`，
      **那正是这条规则要抓的形状**。实测去掉围栏感知 ⇒ 真源立刻 **4 条误报**
      （`CHANGELOG.md` / `CHANGELOG.en.md` / `docs/TECH_DEBT.md` /
      `docs/UI_REFINEMENT_PLAN.md`，各一处，全部是记录本缺陷的示例块）。
      误报比漏报更致命：门禁一旦只会喊狼，早晚被人关掉。
    """
    problems: list[str] = []
    docs_dir = repo / "docs"
    targets = sorted(
        [*(docs_dir.glob("*.md") if docs_dir.exists() else []),
         *repo.glob("README*.md"), *repo.glob("CHANGELOG*.md")]
    )
    for doc in targets:
        lines = doc.read_text(encoding="utf-8", errors="replace").splitlines()
        # 先标出「真标题」：跳过代码围栏内的 # 注释
        headings: list[tuple[int, int, str]] = []
        heading_idx: set[int] = set()
        in_fence = False
        for i, line in enumerate(lines):
            if line.lstrip().startswith("```"):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            m = re.match(r"^(#{1,6})\s+(.*)$", line)
            if m:
                headings.append((i, len(m.group(1)), m.group(2).strip()))
                heading_idx.add(i)
        for idx, (start, level, title) in enumerate(headings):
            if level < 2:
                continue
            end = len(lines)
            for j, lv2, _t in headings[idx + 1:]:
                if lv2 <= level:
                    end = j
                    break
            has_body = any(
                ln.strip() and (start + 1 + k) not in heading_idx
                for k, ln in enumerate(lines[start + 1:end])
            )
            if not has_body:
                problems.append(
                    f"{doc.name}:{start + 1} 空章节：[{level} 级] {title}（标题下没有任何正文）"
                )
    return problems


def check_repo(repo: Path) -> list[str]:
    problems = []
    problems += check_counts(repo)
    problems += check_links(repo)
    problems += check_path_refs(repo)
    problems += check_ci_interfaces(repo)
    problems += check_empty_sections(repo)
    return problems


def self_test() -> None:
    """合成仓库：干净全绿；篡改数字、删除链接目标、改名测试均必须被检出。"""
    import tempfile

    # 用例数 = 本函数里的 assert 条数，**扫源码数出来**而不是手写常量。
    # 手写的数字会随自测增删用例而静默过期，而过期数字比没有数字更糟——
    # 文档里写着「docs 自测 12 条」、实际 11 条时，没有任何一处会报红。
    #
    # 静态计数只在「所有 assert 都在函数体顶层顺序执行」时等于实际执行条数。
    # 一旦有人把某个 assert 挪进 for/if，静态数就会**少报**，所以下面那条
    # 护栏断言会让这种改动当场报红，而不是悄悄让计数失真。
    # （曾试过 sys.settrace 统计实际执行行：它不追溯已开始的当前帧，实测恒为 0。）
    _src = Path(__file__).read_text(encoding="utf-8").splitlines()
    _start = self_test.__code__.co_firstlineno
    _end = next(
        (i for i in range(_start, len(_src)) if _src[i] and not _src[i][0].isspace()),
        len(_src),
    )
    _assert_lines = [
        (i + 1, _src[i]) for i in range(_start - 1, _end) if _src[i].lstrip().startswith("assert")
    ]
    # 真实用例都在 `with` 块内（缩进 8）；比 8 更深的才是「被挪进循环或分支」。
    # 本函数自己的那条护栏断言在缩进 4，天然被排除在两边之外。
    _cases = [n for n, s in _assert_lines if len(s) - len(s.lstrip()) == 8]
    _nested = [n for n, s in _assert_lines if len(s) - len(s.lstrip()) > 8]
    assert not _nested, (
        f"有用例被挪进循环或分支（行 {_nested}），静态计数会少报；"
        "请把它们提到顶层，或改用可执行计数"
    )

    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)

        for rel in ("docs", "src-tauri/src/agent/tools", "src-tauri/src/agent",
                    "src-tauri/src/commands", "src-tauri/src/services",
                    "src-tauri/src/db", "src-tauri/migrations", "src-tauri/tests",
                    "src/pages", ".github/workflows", "scripts"):
            (repo / rel).mkdir(parents=True, exist_ok=True)
        write_doc(repo, "src-tauri/src/agent/tools/mod.rs",
            "pub const TOOL_SPECS: &[ToolSpec] = &[\n    ToolSpec {},\n    ToolSpec {},\n];"
        )
        write_doc(repo, "src-tauri/migrations/001_a.sql", "-- x\n")
        write_doc(repo, "src-tauri/src/db/mod.rs",
            "pub static MIGRATIONS: &[(i64, &str, &str)] = &[\n"
            "(1, \"001_a\", include_str!(\"../../migrations/001_a.sql\")),\n];"
        )
        write_doc(repo, "src-tauri/src/lib.rs",
            ".invoke_handler(tauri::generate_handler![\n"
            "    commands::command_palette::list_palette_commands,\n"
            "    commands::project::list_projects,\n"
            "]);"
        )
        write_doc(repo, "src-tauri/src/agent/evals.rs",
            "pub fn ci_baseline_gate() {}\npub fn reliability_gate() {}"
        )
        write_doc(repo, "src-tauri/tests/worker_crash_e2e.rs", "")
        write_doc(repo, "src-tauri/src/commands/a.rs", "")
        write_doc(repo, "src-tauri/src/commands/b.rs", "")
        write_doc(repo, "src-tauri/src/services/s.rs", "")
        write_doc(repo, "src/pages/P.tsx", "")
        write_doc(repo, ".github/workflows/quality.yml",
            "run: cargo test agent::evals::tests::ci_baseline_gate\n"
            "run: cargo test --test worker_crash_e2e\n"
        )
        write_doc(repo, ".github/workflows/release.yml",
            "python3 scripts/gen-release-notes.py --out notes.md"
        )
        write_doc(repo, "scripts/gen-release-notes.py", "")
        write_doc(repo, "docs/OTHER.md", "# other")
        write_doc(repo, "docs/ROADMAP.md",
            "- [x] 任务引用 [文档](OTHER.md)，实现为 `src-tauri/src/agent/evals.rs`。\n"
        )
        write_doc(repo, "docs/ARCHITECTURE.md",
            "| Agent 对外工具 | 2 |\n| 数据库迁移 | 1 |\n"
            "| Tauri IPC 注册入口 | 2 |\n| React 页面 | 1 |\n"
            "| `commands/` 命令模块（不含 `mod.rs`） | 2 |\n"
            "| `services/` 服务模块（不含 `mod.rs`） | 1 |\n"
            "| `agent/` 顶层模块（不含 `mod.rs`） | 1 |\n"
            "| `agent/tools/` Rust 文件（含 `mod.rs`） | 1 |\n"
            "SQLite（1 个迁移）\n当前 1 个迁移\n2 工具 / 审批流水线\n"
        )
        write_doc(repo, "docs/ARCHITECTURE.en.md",
            "| Agent-facing tools | 2 |\n| Database migrations | 1 |\n"
            "| Tauri IPC registration entry points | 2 |\n| React pages | 1 |\n"
            "| `commands/` command modules (excluding `mod.rs`) | 2 |\n"
            "| `services/` service modules (excluding `mod.rs`) | 1 |\n"
            "| `agent/` top-level modules (excluding `mod.rs`) | 1 |\n"
            "| `agent/tools/` Rust files (incl. `mod.rs`) | 1 |\n"
            "SQLite (1 migrations)\nthe current 1 migrations\n2 tools / approval pipeline\n"
        )
        write_doc(repo, "README.md",
            "**2 个 Agent 工具**\n## 2 个 Agent 工具按域分组\n"
            "2 个 Tauri IPC 入口 · 1 个 service 模块\n"
            "agent/ 1 个顶层模块 · tools/ 0 文件 · 2 工具\n"
            "2 个 Agent 工具（0 文件）\n2 个命令模块\n业务服务（1 个）\n"
            "SQLite + 1 个迁移\n"
        )
        write_doc(repo, "README.en.md",
            "**2 Agent tools**\n## The 2 Agent Tools Grouped\n"
            "2 Tauri IPC entry points · 1 service modules\n"
            "agent/ 1 top-level modules · tools/ 0 files\n"
            "2 command modules\nBusiness services (1)\nSQLite + 1 migrations\n"
        )
        write_doc(repo, "CHANGELOG.md", "`TOOL_SPECS` 达到 **2**\n数据库迁移总数达到 **1**\n")
        write_doc(repo, "CHANGELOG.en.md",
            "migration count reaches **1**; `TOOL_SPECS` to **2**.\n"
        )
        write_doc(repo, "docs/TOOL_ENHANCEMENTS.md",
            "| 对外 Agent 工具 | 2 |\n| 工具实现文件 | 0 |\n不属于当前 2 工具\n"
        )
        write_doc(repo, "docs/TOOL_ENHANCEMENTS.en.md",
            "| External Agent tools | 2 |\n| Tool implementation files | 0 |\n"
            "not part of the current 2 tools\n"
        )
        write_doc(repo, "docs/TOOLCHAIN_ACCEPTANCE.md", "2 个注册工具共享契约真源\n")
        write_doc(repo, "docs/TOOL_RESULT_V2.md", "2 个注册工具均产生完整稳定字段\n")
        write_doc(repo, "docs/VERSION_COMPATIBILITY.md", "迁移数（当前 1）\n")

        assert not check_repo(repo), f"干净仓库应全绿：{check_repo(repo)}"

        # ── CHANGELOG 只查最新版本段（2026-10-01 加）─────────────────────────
        # 放在这里而不是末尾：后面几步会故意把 CI 测试名 / ROADMAP 路径弄脏且**不还原**，
        # 断言写在它们后面就只能写成「过滤出 CHANGELOG 相关问题」，那会顺带把
        # 本组用例降级成弱断言。放在干净基线上，整组断言可以用全量强度。
        changelog_single = read_doc(repo, "CHANGELOG.md")
        changelog_en_single = read_doc(repo, "CHANGELOG.en.md")

        # 1) 历史段写旧数字、最新段写当前数字 → 必须全绿。
        #    这就是加第 85 个迁移时 v2.1.0 的「迁移总数达到 **84**」把门禁弄红的场景。
        write_doc(repo, "CHANGELOG.md",
            "## v2.3.0\n\n`TOOL_SPECS` 达到 **2**\n数据库迁移总数达到 **1**\n\n"
            "## v2.1.0\n\n数据库迁移总数达到 **99**\n`TOOL_SPECS` 达到 **77**\n"
        )
        write_doc(repo, "CHANGELOG.en.md",
            "## v2.3.0\n\nmigration count reaches **1**; `TOOL_SPECS` to **2**.\n\n"
            "## v2.1.0\n\nmigration count reaches **99**; `TOOL_SPECS` to **77**.\n"
        )
        assert not check_repo(repo), f"CHANGELOG 历史段的旧数字被误判为漂移：{check_repo(repo)}"

        # 2) 最新版本段本身写错 → 必须检出（切段不能把检查一并切没了）
        write_doc(repo, "CHANGELOG.md",
            "## v2.3.0\n\n`TOOL_SPECS` 达到 **2**\n数据库迁移总数达到 **99**\n\n"
            "## v2.1.0\n\n数据库迁移总数达到 **1**\n"
        )
        assert any("迁移数" in p and "CHANGELOG.md" in p for p in check_repo(repo)), \
            "最新版本段的迁移数漂移未被检出"

        # 3) 只有最新段有声明、更早的段完全没有 → 仍必须报红：
        #    「最新段没写当前规模」是真缺口，不能因为后面某段碰巧有数字就放行
        write_doc(repo, "CHANGELOG.md",
            "## v2.3.0\n\n本次没有改动规模。\n\n## v2.1.0\n\n数据库迁移总数达到 **1**\n"
        )
        assert any("未找到模式" in p and "CHANGELOG.md" in p for p in check_repo(repo)), \
            "最新版本段缺声明时未报红（等于把门禁切没了）"

        write_doc(repo, "CHANGELOG.md", changelog_single)
        write_doc(repo, "CHANGELOG.en.md", changelog_en_single)
        assert not check_repo(repo), "CHANGELOG 恢复后应重新全绿"

        # ── 空章节：标题下面只剩标题（2026-10-08 加）─────────────────────────
        # 用例 1 **照抄真源里真实出现过的写法**（docs/TECH_DEBT.md 的两个 ### P-6）。
        # 合成用例若与真源写法不同，这条判据就等于只测了假想情形。
        write_doc(repo, "docs/TECH_DEBT.md",
            "## 三、产品判断\n\n"
            "### P-6 语义检索要不要版本感知（第二十三批提出，**数据已备好，等拍板**）\n\n"
            "### P-6 语义检索要不要版本感知（第二十三批提出 → 第二十四批撤回重开）\n\n"
            "> **原拍板已作废，不要按它实现。**\n\n正文。\n"
        )
        assert any("空章节" in p and "TECH_DEBT.md" in p for p in check_repo(repo)), \
            "标题下只剩标题（空章节）未被检出"
        write_doc(repo, "docs/TECH_DEBT.md", "")

        # 用例 2：`####` 子标题**不算**「无正文」。判据若写成「下一行是不是标题」，
        #         会把这种合法结构全判成空——那是误报，必须保持全绿。
        write_doc(repo, "docs/TECH_DEBT.md",
            "### T-1 某某问题\n\n#### 已普查：某方向\n\n正文段落。\n"
        )
        assert not any("空章节" in p for p in check_repo(repo)), \
            "`####` 子标题被误判为空章节"
        write_doc(repo, "docs/TECH_DEBT.md", "")

        # 用例 3：代码围栏里的 `## foo` / `# bar` 不是标题，是命令
        write_doc(repo, "docs/TECH_DEBT.md",
            "### T-2 某某问题\n\n```bash\n## 这不是章节，是命令\n# 也不是\n```\n"
        )
        assert not any("空章节" in p for p in check_repo(repo)), \
            "代码围栏内的 # 被误判为标题"
        write_doc(repo, "docs/TECH_DEBT.md", "")

        # 用例 4：H1 后面紧跟 `##` 是正常结构（CHANGELOG 就是「# 标题」+「## Unreleased」），
        #         把 H1 判进去全是误报
        write_doc(repo, "docs/TECH_DEBT.md", "# 台账\n\n## 一、技术债\n\n正文。\n")
        assert not any("空章节" in p for p in check_repo(repo)), \
            "H1 紧跟 ## 被误判为空章节"
        write_doc(repo, "docs/TECH_DEBT.md", "")

        # 篡改工具数 → 必须检出
        write_doc(repo, "README.md",
            read_doc(repo, "README.md").replace("**2 个 Agent 工具**", "**3 个 Agent 工具**")
        )
        assert any("工具数" in p for p in check_repo(repo)), "篡改工具数未被检出"
        write_doc(repo, "README.md",
            read_doc(repo, "README.md").replace("**3 个 Agent 工具**", "**2 个 Agent 工具**")
        )

        # 英文文档篡改同样必须被检出
        write_doc(repo, "README.en.md",
            read_doc(repo, "README.en.md").replace("**2 Agent tools**", "**3 Agent tools**")
        )
        assert any("Tool count" in p for p in check_repo(repo)), "英文工具数漂移未被检出"
        write_doc(repo, "README.en.md",
            read_doc(repo, "README.en.md").replace("**3 Agent tools**", "**2 Agent tools**")
        )

        # 删除链接目标 → 必须检出
        (repo / "docs/OTHER.md").unlink()
        assert any("链接目标不存在" in p for p in check_repo(repo)), "删除链接目标未被检出"
        write_doc(repo, "docs/OTHER.md", "# other")

        # 带锚点的相对链接是合法 Markdown，不得误判为漂移
        write_doc(repo, "docs/ROADMAP.md",
            "- [x] 引用 [文档](OTHER.md#section)，实现为 `src-tauri/src/agent/evals.rs`。\n"
        )
        assert not any("链接目标不存在" in p for p in check_repo(repo)), \
            "带锚点的相对链接被误判为漂移"
        write_doc(repo, "docs/ROADMAP.md",
            "- [x] 任务引用 [文档](OTHER.md)，实现为 `src-tauri/src/agent/evals.rs`。\n"
        )

        # 改坏 CI 测试名 → 必须检出
        write_doc(repo, ".github/workflows/quality.yml",
            read_doc(repo, ".github/workflows/quality.yml").replace(
                "ci_baseline_gate", "no_such_gate"
            )
        )
        assert any("不存在的测试" in p for p in check_repo(repo)), "CI 测试名漂移未被检出"

        # ROADMAP 引用不存在的路径 → 必须检出
        write_doc(repo, "docs/ROADMAP.md",
            "- [x] 实现为 `src-tauri/src/agent/missing.rs`。\n"
        )
        assert any("引用路径不存在" in p for p in check_repo(repo)), "路径引用漂移未被检出"
    # 用例数从**实际执行到的 assert 语句**里数，不手写常量：
    # 手写的数字会随自测增删用例而静默过期，而过期数字比没有数字更糟——
    # 文档里写着「docs 自测 13 条」，实际是 12 条时，没有任何一处会报红
    print(f"self-test: 全部通过（{len(_cases)} 个用例，含 CHANGELOG 最新版本段 3 个用例）")


def main() -> int:
    # Windows 控制台默认 cp1252：直接 print 中文会抛 UnicodeEncodeError，CI 上因此看不到
    # 真正的漂移/告警明细（只看到编码崩溃）。这里显式改用 UTF-8 输出。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")

    parser = argparse.ArgumentParser(description="文档漂移校验（Q-08）")
    parser.add_argument("--repo", default=str(ROOT), help="仓库路径")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0
    problems = check_repo(Path(args.repo))
    if problems:
        print(f"文档漂移校验失败：{len(problems)} 项")
        for problem in problems:
            print(f"- {problem}")
        return 1
    print("文档漂移校验通过：数量、路径、接口、章节结构与代码真源一致")
    return 0


if __name__ == "__main__":
    sys.exit(main())
