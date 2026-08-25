#!/usr/bin/env python3
"""EC-17 发布说明自动汇总。

从 git 提交范围与 CHANGELOG 自动生成发布说明：用户可见变化（Unreleased 段）、
数据库迁移清单、工具协议变更、资产版本与回滚方式。供 release.yml 的
Create Release 步骤使用，也可本地预览。

用法：
  python3 scripts/gen-release-notes.py                    # 输出到 stdout
  python3 scripts/gen-release-notes.py --out notes.md     # 写入文件
  python3 scripts/gen-release-notes.py --prev-tag v2.0.0  # 指定基线标签
  python3 scripts/gen-release-notes.py --self-test        # 合成仓库自测
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from datetime import date
from pathlib import Path

PROTOCOL_PATHS = [
    "docs/TOOL_RESULT_V2.md",
    "docs/TOOL_CONTRACTS.md",
    "src-tauri/src/agent/structured_result.rs",
    "src-tauri/src/agent/tools/contracts.rs",
]
MIGRATIONS_DIR = "src-tauri/migrations"
VERSION_DOC = "docs/VERSION_COMPATIBILITY.md"
ROLLBACK_TEMPLATE = """## 回滚方式

- 数据库：迁移只前滚不回滚。回退应用版本时，旧代码按 schema 0 或字段默认值兼容新表；如确需数据回退，先备份数据库再操作。
- 应用：从本 Release 页下载上一版本安装包即可回退，不影响本地会话、项目与设备配置。
- 工具协议：协议变化向后兼容；降级后新协议字段由旧读取器忽略，不会导致读取失败。"""


def git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=False
    )


def count_tools(repo: Path, ref: str | None) -> int:
    """统计工具注册表条目数（排除 ToolSpec struct 定义本身）；ref 为 None 时读工作区。"""
    path = "src-tauri/src/agent/tools/mod.rs"
    if ref:
        proc = git(repo, "show", f"{ref}:{path}")
        if proc.returncode != 0:
            return 0
        source = proc.stdout
    else:
        source = (repo / path).read_text(errors="replace")
    return len(re.findall(r"^    ToolSpec \{", source, re.M))


def migrations_between(repo: Path, prev: str) -> list[dict]:
    """返回 prev..HEAD 之间新增的迁移清单（编号、文件名、首行注释）。"""
    proc = git(repo, "diff", "--name-only", prev, "HEAD", "--", MIGRATIONS_DIR)
    items = []
    for rel in sorted(proc.stdout.splitlines()):
        name = Path(rel).name
        match = re.match(r"(\d+)_(.+)\.sql", name)
        number = match.group(1) if match else "?"
        comment = ""
        current = repo / rel
        if current.exists():
            for line in current.read_text(errors="replace").splitlines():
                stripped = line.strip()
                if stripped.startswith("--"):
                    comment = stripped.lstrip("-").strip()
                    break
        items.append({"number": number, "file": name, "comment": comment})
    return items


def protocol_changes(repo: Path, prev: str) -> list[str]:
    proc = git(repo, "diff", "--name-only", prev, "HEAD", "--", *PROTOCOL_PATHS)
    return [line for line in proc.stdout.splitlines() if line.strip()]


def changelog_changes(repo: Path, release_tag: str | None) -> str:
    text = (repo / "CHANGELOG.md").read_text(errors="replace")
    if release_tag:
        match = re.search(
            rf"^##\s+{re.escape(release_tag)}(?:\s+[^\n]*)?\n(.*?)(?=^##\s+|\Z)",
            text,
            re.M | re.S,
        )
        if match:
            return match.group(1).strip()
    match = re.search(r"## Unreleased[^\n]*\n(.*?)(?=\n## v|\Z)", text, re.S)
    return match.group(1).strip() if match else ""


def asset_version_table(repo: Path) -> str:
    text = (repo / VERSION_DOC).read_text(errors="replace")
    lines = []
    for line in text.splitlines():
        if line.startswith("| 数据库 |") or line.startswith("| 工具协议 |") \
                or line.startswith("| 评测运行快照 |") or line.startswith("| 评测 CI 基线 |"):
            lines.append(line)
    return "\n".join(lines) if lines else "见 docs/VERSION_COMPATIBILITY.md"


def generate(repo: Path, prev_tag: str | None) -> str:
    head = git(repo, "rev-parse", "HEAD").stdout.strip()
    release_tags = [
        tag
        for tag in git(repo, "tag", "--points-at", "HEAD", "--sort=-version:refname").stdout.splitlines()
        if tag.startswith("v")
    ]
    release_tag = release_tags[0] if release_tags else None
    if prev_tag:
        prev = prev_tag
    elif release_tag:
        # 发布工作流运行在当前 tag 上；基线必须取父提交可达的上一 tag，
        # 否则 git describe 会返回当前 tag，导致差异被错误汇总为 0。
        prev = git(repo, "describe", "--tags", "--abbrev=0", "HEAD^").stdout.strip()
    else:
        prev = git(repo, "describe", "--tags", "--abbrev=0").stdout.strip()
    if not prev:
        prev = git(repo, "rev-list", "--max-parents=0", "HEAD").stdout.strip() or "HEAD~1"
        prev_label = "首次提交"
    else:
        prev_label = prev
    commit_count = git(repo, "rev-list", "--count", f"{prev}..HEAD").stdout.strip() or "0"
    migrations = migrations_between(repo, prev)
    protocol = protocol_changes(repo, prev)
    tools_before = count_tools(repo, prev)
    tools_now = count_tools(repo, None)
    changes = changelog_changes(repo, release_tag)
    assets = asset_version_table(repo)

    lines = [
        f"# HarmonyAgent 发布说明",
        "",
        f"- 生成时间：{date.today().isoformat()}",
        f"- 提交范围：{prev_label}..{head[:8]}（{commit_count} 个提交）",
        f"- 数据库迁移：当前 {len(list((repo / MIGRATIONS_DIR).glob('*.sql')))} 个（本次新增 {len(migrations)} 个）",
        f"- 工具注册表：{tools_before} → {tools_now}",
        "",
        "## 用户可见变化",
        "",
        changes or "（CHANGELOG 无当前版本或 Unreleased 段）",
        "",
        "## 数据库迁移清单",
        "",
    ]
    if migrations:
        lines += [
            f"- `{item['number']}` `{item['file']}`：{item['comment']}"
            for item in migrations
        ]
    else:
        lines.append("- 本次无新增迁移。")
    lines += ["", "## 工具协议"]
    if protocol:
        lines += ["以下文件在本版本发生变化，发布前必须确认无破坏性变更："]
        lines += [f"- {path}" for path in protocol]
    else:
        lines.append("- 工具协议无变更，schema 保持向后兼容。")
    lines += [
        "",
        "## 资产版本",
        "",
        assets,
        "",
        ROLLBACK_TEMPLATE,
        "",
    ]
    return "\n".join(lines)


def self_test() -> None:
    """在临时 git 仓库中验证迁移清单、工具计数与 CHANGELOG 提取。"""
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)
        git(repo, "init", "-q")
        git(repo, "config", "user.email", "test@example.com")
        git(repo, "config", "user.name", "test")
        (repo / "src-tauri").mkdir(parents=True)
        (repo / "src-tauri/migrations").mkdir()
        (repo / "src-tauri/src/agent/tools").mkdir(parents=True)
        (repo / "docs").mkdir()
        tools = "pub const TOOL_SPECS: &[ToolSpec] = &[\n    ToolSpec {},\n    ToolSpec {},\n];"
        (repo / "src-tauri/src/agent/tools/mod.rs").write_text(tools)
        (repo / "CHANGELOG.md").write_text("## Unreleased — 测试\n\n- 用户可见变化 A。\n\n## v2.2 — 旧版本\n")
        (repo / "docs/VERSION_COMPATIBILITY.md").write_text(
            "| 数据库 | 迁移数（当前 2） | stable | x | y |\n| 工具协议 | schema 2 | stable | x | y |\n"
        )
        git(repo, "add", ".")
        git(repo, "commit", "-qm", "base")
        git(repo, "tag", "v0.0.1")
        # 新增迁移与工具
        (repo / "src-tauri/migrations/001_first.sql").write_text("-- 首个迁移：测试表。\nCREATE TABLE t(id INTEGER);\n")
        (repo / "src-tauri/migrations/002_second.sql").write_text("-- 第二个迁移。\n")
        tools2 = tools.replace("];", "    ToolSpec {},\n];")
        (repo / "src-tauri/src/agent/tools/mod.rs").write_text(tools2)
        (repo / "CHANGELOG.md").write_text("## Unreleased — 测试\n\n- 用户可见变化 B。\n\n## v2.2 — 旧版本\n")
        git(repo, "add", ".")
        git(repo, "commit", "-qm", "second")

        notes = generate(repo, "v0.0.1")
        assert "用户可见变化 B" in notes
        assert "`001` `001_first.sql`：首个迁移：测试表。" in notes
        assert "`002`" in notes
        assert "2 → 3" in notes
        assert "回滚方式" in notes
        assert "schema 2" in notes
        assert "工具协议无变更" in notes

        # 发布工作流在当前 tag 上运行：应对比上一 tag，并读取当前版本段落。
        (repo / "CHANGELOG.md").write_text(
            "## v0.0.2 — 测试版本\n\n- 当前版本可见变化。\n\n## v0.0.1 — 旧版本\n"
        )
        git(repo, "add", "CHANGELOG.md")
        git(repo, "commit", "-qm", "release")
        git(repo, "tag", "v0.0.2")
        tagged_notes = generate(repo, None)
        assert "v0.0.1.." in tagged_notes
        assert "当前版本可见变化" in tagged_notes
        assert "（0 个提交）" not in tagged_notes
    print("self-test: 全部通过")


def main() -> int:
    parser = argparse.ArgumentParser(description="发布说明自动汇总（EC-17）")
    parser.add_argument("--repo", default=".", help="仓库路径")
    parser.add_argument("--prev-tag", help="基线标签（默认 git describe）")
    parser.add_argument("--out", help="输出文件路径（默认 stdout）")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0
    notes = generate(Path(args.repo), args.prev_tag)
    if args.out:
        Path(args.out).write_text(notes)
    else:
        print(notes)
    return 0


if __name__ == "__main__":
    sys.exit(main())
