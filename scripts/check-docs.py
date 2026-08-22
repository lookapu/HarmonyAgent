#!/usr/bin/env python3
"""Q-08 文档漂移校验：数量、路径、接口与状态必须与代码真源一致。

从代码真源提取工具数、迁移数、IPC 入口、模块数等数量，再逐模式校验
README / ARCHITECTURE / TOOL_ENHANCEMENTS / TOOLCHAIN_ACCEPTANCE /
TOOL_RESULT_V2 / CHANGELOG / VERSION_COMPATIBILITY 中的对应数字；
校验 ROADMAP 与 docs 内相对链接目标存在、ROADMAP 反引号路径存在；
校验 quality.yml / release.yml 引用的 Rust 测试、集成测试与脚本存在。

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


def count_tool_specs(repo: Path) -> int:
    source = (repo / "src-tauri/src/agent/tools/mod.rs").read_text(errors="replace")
    return len(TOOL_SPEC_RE.findall(source))


def count_migrations(repo: Path) -> tuple[int, int]:
    """返回 (文件数, 注册数)；两者不一致本身即漂移。"""
    files = len(list((repo / "src-tauri/migrations").glob("*.sql")))
    text = (repo / "src-tauri/src/db/mod.rs").read_text(errors="replace")
    registered = len(re.findall(r"include_str!\(\"\.\./\.\./migrations/", text))
    return files, registered


def count_pages(repo: Path) -> int:
    return len(list((repo / "src/pages").glob("*.tsx")))


def count_files(repo: Path, rel_dir: str, exclude: tuple[str, ...]) -> int:
    return len(
        [p for p in (repo / rel_dir).glob("*.rs") if p.name not in exclude]
    )


def count_ipc_entries(repo: Path) -> int:
    text = (repo / "src-tauri/src/lib.rs").read_text(errors="replace")
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
    ]
    for doc, pattern, expected, label in checks:
        base = repo if doc in ("README.md", "CHANGELOG.md") else repo / "docs"
        text = (base / doc).read_text(errors="replace")
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
    """ROADMAP 与 docs/*.md 中的相对链接目标必须存在。"""
    problems = []
    docs_dir = repo / "docs"

    def resolve(base_dir: Path, target: str) -> Path | None:
        if target.startswith(("http://", "https://", "mailto:", "#")) \
                or target.startswith("<") or target.endswith((".png", ".jpg", ".jpeg")):
            return None
        if target.startswith("../"):
            return (base_dir / target).resolve()
        if target.startswith("/"):
            return None
        return (base_dir / target).resolve()

    for doc in sorted(docs_dir.glob("*.md")):
        text = doc.read_text(errors="replace")
        for match in re.finditer(r"\]\(([^)]+)\)", text):
            target = match.group(1).strip()
            resolved = resolve(docs_dir, target)
            if resolved is None:
                continue
            if not resolved.exists():
                problems.append(f"{doc.name} 链接目标不存在：{target}")
    return problems


def check_path_refs(repo: Path) -> list[str]:
    """ROADMAP 中反引号内的代码/脚本/迁移路径必须存在。"""
    problems = []
    text = (repo / "docs/ROADMAP.md").read_text(errors="replace")
    for match in re.finditer(r"`((?:src-tauri|scripts|src|docs)/[^`]+)`", text):
        ref = match.group(1).strip().rstrip("/")
        if not ref:
            continue
        if ref.endswith((".rs", ".py", ".sql", ".json", ".tsx", ".ts", ".md")):
            if not (repo / ref).exists():
                problems.append(f"ROADMAP 引用路径不存在：{ref}")
    return problems


def check_ci_interfaces(repo: Path) -> list[str]:
    """quality.yml/release.yml 引用的测试与脚本必须存在。"""
    problems = []
    quality = (repo / ".github/workflows/quality.yml").read_text(errors="replace")
    for match in re.finditer(r"agent::evals::tests::([a-z_]+)", quality):
        name = match.group(1)
        source = (repo / "src-tauri/src/agent/evals.rs").read_text(errors="replace")
        if not re.search(rf"fn {name}\b", source):
            problems.append(f"quality.yml 引用不存在的测试：agent::evals::tests::{name}")
    for match in re.finditer(r"--test ([a-zA-Z0-9_]+)", quality):
        name = match.group(1)
        if not (repo / f"src-tauri/tests/{name}.rs").exists():
            problems.append(f"quality.yml 引用不存在的集成测试：{name}")
    release = (repo / ".github/workflows/release.yml").read_text(errors="replace")
    for match in re.finditer(r"python3? scripts/([a-zA-Z0-9_.-]+\.py)", release):
        name = match.group(1)
        if not (repo / f"scripts/{name}").exists():
            problems.append(f"release.yml 引用不存在的脚本：{name}")
    return problems


def check_repo(repo: Path) -> list[str]:
    problems = []
    problems += check_counts(repo)
    problems += check_links(repo)
    problems += check_path_refs(repo)
    problems += check_ci_interfaces(repo)
    return problems


def self_test() -> None:
    """合成仓库：干净全绿；篡改数字、删除链接目标、改名测试均必须被检出。"""
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)
        for rel in ("docs", "src-tauri/src/agent/tools", "src-tauri/src/agent",
                    "src-tauri/src/commands", "src-tauri/src/services",
                    "src-tauri/src/db", "src-tauri/migrations", "src-tauri/tests",
                    "src/pages", ".github/workflows", "scripts"):
            (repo / rel).mkdir(parents=True, exist_ok=True)
        (repo / "src-tauri/src/agent/tools/mod.rs").write_text(
            "pub const TOOL_SPECS: &[ToolSpec] = &[\n    ToolSpec {},\n    ToolSpec {},\n];"
        )
        (repo / "src-tauri/migrations/001_a.sql").write_text("-- x\n")
        (repo / "src-tauri/src/db/mod.rs").write_text(
            "pub static MIGRATIONS: &[(i64, &str, &str)] = &[\n"
            "(1, \"001_a\", include_str!(\"../../migrations/001_a.sql\")),\n];"
        )
        (repo / "src-tauri/src/lib.rs").write_text(
            ".invoke_handler(tauri::generate_handler![\n"
            "    commands::command_palette::list_palette_commands,\n"
            "    commands::project::list_projects,\n"
            "]);"
        )
        (repo / "src-tauri/src/agent/evals.rs").write_text(
            "pub fn ci_baseline_gate() {}\npub fn reliability_gate() {}"
        )
        (repo / "src-tauri/tests/worker_crash_e2e.rs").write_text("")
        (repo / "src-tauri/src/commands/a.rs").write_text("")
        (repo / "src-tauri/src/commands/b.rs").write_text("")
        (repo / "src-tauri/src/services/s.rs").write_text("")
        (repo / "src/pages/P.tsx").write_text("")
        (repo / ".github/workflows/quality.yml").write_text(
            "run: cargo test agent::evals::tests::ci_baseline_gate\n"
            "run: cargo test --test worker_crash_e2e\n"
        )
        (repo / ".github/workflows/release.yml").write_text(
            "python3 scripts/gen-release-notes.py --out notes.md"
        )
        (repo / "scripts/gen-release-notes.py").write_text("")
        (repo / "docs/OTHER.md").write_text("# other")
        (repo / "docs/ROADMAP.md").write_text(
            "- [x] 任务引用 [文档](OTHER.md)，实现为 `src-tauri/src/agent/evals.rs`。\n"
        )
        (repo / "docs/ARCHITECTURE.md").write_text(
            "| Agent 对外工具 | 2 |\n| 数据库迁移 | 1 |\n"
            "| Tauri IPC 注册入口 | 2 |\n| React 页面 | 1 |\n"
            "| `commands/` 命令模块（不含 `mod.rs`） | 2 |\n"
            "| `services/` 服务模块（不含 `mod.rs`） | 1 |\n"
            "| `agent/` 顶层模块（不含 `mod.rs`） | 1 |\n"
            "| `agent/tools/` Rust 文件（含 `mod.rs`） | 1 |\n"
            "SQLite（1 个迁移）\n当前 1 个迁移\n2 工具 / 审批流水线\n"
        )
        (repo / "README.md").write_text(
            "**2 个 Agent 工具**\n## 2 个 Agent 工具按域分组\n"
            "2 个 Tauri IPC 入口 · 1 个 service 模块\n"
            "agent/ 1 个顶层模块 · tools/ 0 文件 · 2 工具\n"
            "2 个 Agent 工具（0 文件）\n2 个命令模块\n业务服务（1 个）\n"
            "SQLite + 1 个迁移\n"
        )
        (repo / "CHANGELOG.md").write_text("`TOOL_SPECS` 达到 **2**\n数据库迁移总数达到 **1**\n")
        (repo / "docs/TOOL_ENHANCEMENTS.md").write_text(
            "| 对外 Agent 工具 | 2 |\n| 工具实现文件 | 0 |\n不属于当前 2 工具\n"
        )
        (repo / "docs/TOOLCHAIN_ACCEPTANCE.md").write_text("2 个注册工具共享契约真源\n")
        (repo / "docs/TOOL_RESULT_V2.md").write_text("2 个注册工具均产生完整稳定字段\n")
        (repo / "docs/VERSION_COMPATIBILITY.md").write_text("迁移数（当前 1）\n")

        assert not check_repo(repo), f"干净仓库应全绿：{check_repo(repo)}"

        # 篡改工具数 → 必须检出
        (repo / "README.md").write_text(
            (repo / "README.md").read_text().replace("**2 个 Agent 工具**", "**3 个 Agent 工具**")
        )
        assert any("工具数" in p for p in check_repo(repo)), "篡改工具数未被检出"
        (repo / "README.md").write_text(
            (repo / "README.md").read_text().replace("**3 个 Agent 工具**", "**2 个 Agent 工具**")
        )

        # 删除链接目标 → 必须检出
        (repo / "docs/OTHER.md").unlink()
        assert any("链接目标不存在" in p for p in check_repo(repo)), "删除链接目标未被检出"
        (repo / "docs/OTHER.md").write_text("# other")

        # 改坏 CI 测试名 → 必须检出
        (repo / ".github/workflows/quality.yml").write_text(
            (repo / ".github/workflows/quality.yml").read_text().replace(
                "ci_baseline_gate", "no_such_gate"
            )
        )
        assert any("不存在的测试" in p for p in check_repo(repo)), "CI 测试名漂移未被检出"

        # ROADMAP 引用不存在的路径 → 必须检出
        (repo / "docs/ROADMAP.md").write_text(
            "- [x] 实现为 `src-tauri/src/agent/missing.rs`。\n"
        )
        assert any("引用路径不存在" in p for p in check_repo(repo)), "路径引用漂移未被检出"
    print("self-test: 全部通过")


def main() -> int:
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
    print("文档漂移校验通过：数量、路径、接口与代码真源一致")
    return 0


if __name__ == "__main__":
    sys.exit(main())
