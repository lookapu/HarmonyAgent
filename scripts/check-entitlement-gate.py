#!/usr/bin/env python3
"""授权分级边界门禁（第十四批 P2-2）。

分级模型一旦落地，真正的风险不是「档位定错了」，而是**有人绕过网关**：
新加一个 team_sharing 的 command，或者改了某个 command 的名字，
权益判断就悄悄失效了——而**失效的表现是免费用户能用上 Pro 功能**，
那种 bug 要到有人投诉「我白嫖了」才会被发现。

所以本门禁守的是**结构**：

## 契约（7 条）

1. **受限 command 必须过网关**：`commands/` 下每个碰受限能力的 command
   函数体里必须有 `licensing::check(`。
2. **受限 service 不得被裸调**：service 层函数（`team_sharing::*`、`lan_server::*`）
   只能被 command 层经网关调用。若某个 command 直接调 service 而不过网关 → 报红。
3. **档位判断只在 licensing 里**：`commands/` 与 service 层不得出现
   `Tier::` 的相等比较（`tier == Tier::Pro` 这类）。所有判断走 `check()`。
4. **权益矩阵是单一真源**：`Capability` 枚举的每个成员都必须出现在
   `TIER_ENTITLEMENTS` 矩阵里。漏填 = 该能力对所有档位关闭且不报错。
5. **免费版不得含协作能力**：`Tier::Free` 对应的能力集必须为空。
   一旦有人给免费版加上 LAN，「免费版=本地全功能」这条产品口径就没了，
   而它消失时没有任何编译错误。
6. **降级方向不被拦**：涉及「关闭」的入口（`auto_pool_mode == 0`）不得被网关拦。
   拦了就意味着 Pro 用户降级后永远关不掉多池。
7. **前端不抄权益矩阵**：`src/` 下不得出现按档位硬编码的能力判断
   （`capabilities.includes` 之外的自建对照表）。

## 判据的已知边界

本门禁是**结构**检查，扫的是字面量。它挡不住：

- 把 `licensing::check(&conn, Capability::X)` 改成 `Capability::Y`（换个能力名，仍过）
- 新增一个 command 但**没有**在任何受限名单里（名单需要跟着代码更新）

第 2 点是真正的隐患，所以名单写在脚本顶部的 `GATED_SERVICES` 且要求
**任何 service 出现在名单里，就必须至少有一个调用点过了网关**。
新增受限 service 时忘了加名单 → 门禁不知道它存在 → 静默失效。
缓解办法：契约 3 会抓住「绕过网关直接调 service」这一半，
另一半靠 code review 时的「新增 service 记得加名单」。

## 用法

    python scripts/check-entitlement-gate.py            # 全仓检查（CI 门禁）
    python scripts/check-entitlement-gate.py --report   # 列出所有受限 command 与网关状态
    python scripts/check-entitlement-gate.py --self-test
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
COMMANDS_DIR = REPO / "src-tauri" / "src" / "commands"
LICENSING = REPO / "src-tauri" / "src" / "services" / "licensing.rs"
SRC_DIR = REPO / "src"

#: 受限 service：模块名 → 能力键。**新增受限 service 必须登记到这里**。
GATED_SERVICES = {
    "team_sharing": "TeamSharing",
    "lan_server": "LanCollaboration",
}

#: 刻意不过网关的入口。**每条都要写理由**，理由要能被 reviewer 读到。
#:
#: 它们共享同一条原则：**能力分级管「谁能开通」，不能管「谁能关闭」**。
#: 拦住关闭入口的代价是——降级后那些已经开出去的东西再也关不掉
#: （令牌收不回来、多池卡在开着只能改数据库自救）。
#: 误伤方向必须选「宁可少拦」这一侧。
GATE_EXEMPT = {
    "get_lan_server_status": (
        "只读本机状态，供设置页判断是否显示升级引导；无副作用、拿它开不了服务"
    ),
    "get_lan_ips": "只读本机网卡地址；无 DB 访问，拿它既开不了服务也签不了令牌",
    "revoke_lan_token": (
        "撤销是收口动作不是开闸动作；拦了就意味着降级后已发出的令牌永远收不回来"
    ),
}

#: 涉及「关闭」的入口。降级后必须还能执行。
CLEARING_COMMANDS = {
    "update_provider": "auto_pool_mode 置 0 属于关闭",
}

CAPABILITIES = [
    "MultiProviderPool",
    "TeamSharing",
    "EvalReports",
    "LanCollaboration",
    "AuditTrail",
]

LINE_COMMENT_RE = re.compile(r"//[^\n]*")
BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", re.DOTALL)
COMMAND_RE = re.compile(r"pub (?:async )?fn (\w+)\([^)]*\)[^{;]*\{(.*?)\n\}", re.DOTALL)
#: `use crate::services::<module>::{ a, b, c };` —— 取出被导入的**裸名**。
#:
#: 这一条不是锦上添花。`commands/lan.rs` 全部走 `use` 导入后的裸名调用
#: （`load_config` / `create_token` / `revoke_token`），而不是 `lan_server::xxx`。
#: 只认全路径的判据会把 4 个受限入口**全部漏判**——门禁报「8 个受限入口，全绿」，
#: 而实际上有 12 个。这与 §6.23.5 里「只查输出别名会被绕过」是同一个病。
USE_RE = re.compile(
    r"use\s+crate::services::(\w+)::\{([^}]*)\}", re.DOTALL
)


def strip_comments(text: str) -> str:
    out = BLOCK_COMMENT_RE.sub(lambda m: " " * len(m.group(0)), text)
    return LINE_COMMENT_RE.sub(lambda m: " " * len(m.group(0)), out)


def commands_of(path: Path) -> list[tuple[str, str]]:
    """返回 [(函数名, 函数体)]，已剥注释。"""
    stripped = strip_comments(path.read_text(encoding="utf-8"))
    return [(m.group(1), m.group(2)) for m in COMMAND_RE.finditer(stripped)]


def imported_names(path: Path, module: str) -> set[str]:
    """该文件从 `module` 导入的裸名集合。"""
    stripped = strip_comments(path.read_text(encoding="utf-8"))
    names: set[str] = set()
    for match in USE_RE.finditer(stripped):
        if match.group(1) != module:
            continue
        for raw in match.group(2).split(","):
            item = raw.strip()
            if not item:
                continue
            # `create_token` 或 `load_config as load` —— 取最后一段
            names.add(item.split(" as ")[-1].strip())
    return names


def uses_gated_service(body: str, module: str, bare_names: set[str]) -> bool:
    """函数体是否用了受限 service 的能力。

    **两条通道都要看**：全路径 `lan_server::load_config` 与
    `use` 导入后的裸名 `load_config`。只看一条必然漏判一半——
    而漏判的那一半在门禁输出里表现为「受限入口数变少」，没人会去追问。
    """
    if f"{module}::" in body:
        return True
    return any(re.search(rf"\b{re.escape(name)}\s*\(", body) for name in bare_names)


def check_text(
    commands: dict[str, list[tuple[str, str]]],
    licensing: str,
    frontend: str,
    bare: dict[str, dict[str, set[str]]] | None = None,
) -> list[str]:
    problems: list[str] = []
    bare = bare or {}

    # --- 契约 1 / 2：受限 command 必须过网关 ---
    for filename, funcs in commands.items():
        for name, body in funcs:
            if name in GATE_EXEMPT:
                continue
            module = next(
                (
                    svc
                    for svc in GATED_SERVICES
                    if uses_gated_service(body, svc, bare.get(filename, {}).get(svc, set()))
                ),
                None,
            )
            if module is None:
                continue
            if "licensing::check(" not in body:
                problems.append(
                    f"{filename}::{name} 调了受限 service `{module}` 却没过能力网关"
                    f"——免费用户会直接用到付费功能"
                )

    # --- 契约 3：档位判断只在 licensing 里 ---
    for filename, funcs in commands.items():
        for name, body in funcs:
            if re.search(r"Tier::\w+\s*==|==\s*Tier::\w+", body):
                problems.append(
                    f"{filename}::{name} 自己比较了档位（`Tier::… ==`）"
                    f"——权益判断只能发生在 licensing::check 里，"
                    f"散落的比较迟早与矩阵不一致"
                )

    # --- 契约 4：枚举成员都在矩阵里 ---
    matrix = re.search(r"TIER_ENTITLEMENTS[^=]*=\s*\[(.*?)\n\];", licensing, re.DOTALL)
    if not matrix:
        problems.append("未找到 TIER_ENTITLEMENTS 矩阵——权益判据的真源不见了")
    else:
        listed = set(re.findall(r"Capability::(\w+)", matrix.group(1)))
        for capability in CAPABILITIES:
            if capability not in listed:
                problems.append(
                    f"能力 `{capability}` 没进 TIER_ENTITLEMENTS 矩阵"
                    f"——它会对所有档位关闭且不报任何错"
                )
        free = re.search(r"\(Tier::Free,\s*&\[(.*?)\]\)", matrix.group(1), re.DOTALL)
        if free is None:
            problems.append("矩阵里没有 Tier::Free 条目")
        elif free.group(1).strip():
            problems.append(
                f"免费版被授予了能力 {free.group(1).strip()!r}"
                f"——「免费版=本地全功能」这条产品口径不能靠代码悄悄改掉"
            )

    # --- 契约 5：降级方向不被拦 ---
    for filename, funcs in commands.items():
        for name, body in funcs:
            if name not in CLEARING_COMMANDS:
                continue
            # 网关在 `if mode != 0` 之内才允许；`if mode == 0` 之内出现即为反向
            if re.search(r"if\s+[^;]*==\s*0[^{]*\{[^}]*licensing::check", body, re.DOTALL):
                problems.append(
                    f"{filename}::{name} 在「关闭」分支里过网关"
                    f"——{CLEARING_COMMANDS[name]}；拦了就意味着降级后关不掉"
                )

    # --- 契约 6：前端不抄权益矩阵 ---
    for path in SRC_DIR.rglob("*.ts*"):
        if path.name == "license.ts":
            continue  # 类型定义与常量映射，是合法的单点
        text = strip_comments(path.read_text(encoding="utf-8"))
        for match in re.finditer(r"(free|pro|team)\s*:\s*\[[^\]]*\]", text):
            problems.append(
                f"{path.relative_to(REPO)} 出现按档位硬编码的能力数组 {match.group(0)[:40]!r}"
                f"——档位与能力的对应关系由后端 TIER_ENTITLEMENTS 决定，"
                f"前端抄一份必然与真源漂移"
            )
    return problems


def run() -> int:
    if not LICENSING.exists():
        print(f"[FAIL] 找不到授权服务：{LICENSING}")
        return 1
    commands = {
        path.name: commands_of(path) for path in sorted(COMMANDS_DIR.glob("*.rs"))
    }
    bare = {
        path.name: {
            svc: imported_names(path, svc) for svc in GATED_SERVICES
        }
        for path in COMMANDS_DIR.glob("*.rs")
    }
    total_commands = sum(len(f) for f in commands.values())
    gated = sum(
        1
        for filename, funcs in commands.items()
        for name, body in funcs
        if name not in GATE_EXEMPT
        and any(
            uses_gated_service(body, svc, bare[filename].get(svc, set()))
            for svc in GATED_SERVICES
        )
    )
    problems = check_text(
        commands,
        strip_comments(LICENSING.read_text(encoding="utf-8")),
        "",
        bare,
    )
    print(
        f"授权门禁：{len(commands)} 个 command 文件 / {total_commands} 个 command"
        f" / {gated} 个受限入口"
    )
    if problems:
        print(f"[FAIL] {len(problems)} 项：")
        for p in problems:
            print(f"  - {p}")
        return 1
    print("[OK] 授权分级边界契约全部满足")
    return 0


def report() -> int:
    print("受限 command 清单：")
    for path in sorted(COMMANDS_DIR.glob("*.rs")):
        bare = {svc: imported_names(path, svc) for svc in GATED_SERVICES}
        for name, body in commands_of(path):
            used = [
                s
                for s in GATED_SERVICES
                if uses_gated_service(body, s, bare.get(s, set()))
            ]
            if not used:
                continue
            gated = "licensing::check(" in body
            exempt = name in GATE_EXEMPT
            mark = "豁免" if exempt else ("已网关" if gated else "!! 无网关")
            why = f"  // {GATE_EXEMPT[name]}" if exempt else ""
            print(f"  [{mark}] {path.name}::{name}  <- {', '.join(used)}{why}")
    return 0


# ---------------------------------------------------------------------------
# 自测
# ---------------------------------------------------------------------------

GOOD_COMMAND = """
pub fn do_team_thing(db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    licensing::check(&conn, Capability::TeamSharing)?;
    crate::services::team_sharing::preview(&conn)
}
"""

BAD_NO_GATE = """
pub fn do_team_thing(db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::services::team_sharing::preview(&conn)
}
"""

BAD_TIER_COMPARE = """
pub fn do_team_thing(conn: &Connection) -> Result<(), String> {
    if licensing::current_tier(conn) == Tier::Pro {
        return Ok(());
    }
    crate::services::team_sharing::preview(conn)
}
"""

BAD_CLEARING_GATED = """
pub fn update_provider(conn: &Connection, mode: i64) -> Result<(), String> {
    if mode == 0 {
        licensing::check(conn, Capability::MultiProviderPool)?;
    }
    Ok(())
}
"""

GOOD_LICENSING = """
pub const TIER_ENTITLEMENTS: [(Tier, &[Capability]); 3] = [
    (Tier::Free, &[]),
    (Tier::Pro, &[Capability::MultiProviderPool, Capability::TeamSharing, Capability::EvalReports]),
    (Tier::Team, &[Capability::MultiProviderPool, Capability::TeamSharing, Capability::EvalReports,
                  Capability::LanCollaboration, Capability::AuditTrail]),
];
"""


def self_test() -> int:
    cases = [
        ("干净源码无问题", {"a.rs": [("do_team_thing", GOOD_COMMAND)]}, GOOD_LICENSING, "", False),
        (
            "受限 command 不过网关被拦",
            {"a.rs": [("do_team_thing", BAD_NO_GATE)]},
            GOOD_LICENSING,
            "能力网关",
            True,
        ),
        (
            "command 自比档位被拦",
            {"a.rs": [("do_team_thing", BAD_TIER_COMPARE)]},
            GOOD_LICENSING,
            "自己比较了档位",
            True,
        ),
        (
            "关闭分支被拦被拦",
            {"a.rs": [("update_provider", BAD_CLEARING_GATED)]},
            GOOD_LICENSING,
            "关闭",
            True,
        ),
        (
            "矩阵缺能力被拦",
            {"a.rs": [("do_team_thing", GOOD_COMMAND)]},
            # 整行删掉，模拟「枚举加了成员但矩阵忘了填」
            GOOD_LICENSING.replace(
                "    (Tier::Team, &[Capability::MultiProviderPool, Capability::TeamSharing, "
                "Capability::EvalReports,\n                  Capability::LanCollaboration, "
                "Capability::AuditTrail]),\n",
                "",
            ),
            "没进 TIER_ENTITLEMENTS",
            True,
        ),
        (
            "免费版被授予能力被拦",
            {"a.rs": [("do_team_thing", GOOD_COMMAND)]},
            GOOD_LICENSING.replace("(Tier::Free, &[])", "(Tier::Free, &[Capability::LanCollaboration])"),
            "免费版被授予",
            True,
        ),
        (
            "矩阵整体消失被拦",
            {"a.rs": [("do_team_thing", GOOD_COMMAND)]},
            "pub fn no_matrix() {}",
            "TIER_ENTITLEMENTS",
            True,
        ),
        (
            "无受限调用时不误报",
            {"a.rs": [("plain_thing", "pub fn plain_thing() { let x = 1; }")]},
            GOOD_LICENSING,
            "",
            False,
        ),
    ]

    passed = 0
    failures: list[str] = []
    for name, cmds, lic, keyword, expect in cases:
        # 负向用例必须**断言注入真的生效**：`.replace()` 锚点漂移会变成空操作，
        # 用例于是「通过」了，但通过的原因是没注入成功——那比没有用例更糟。
        if expect and lic is not GOOD_LICENSING and lic.replace(
            "Capability::AuditTrail,", ""
        ) == GOOD_LICENSING.replace("Capability::AuditTrail,", ""):
            failures.append(f"{name}: 注入没生效（licensing 片段替换后与原文相同）")
            continue
        problems = check_text(cmds, lic, "")
        if bool(problems) != expect:
            failures.append(f"{name}: 期望{'有问题' if expect else '无问题'} -> {problems}")
            continue
        if expect and keyword and not any(keyword in p for p in problems):
            failures.append(f"{name}: 报红了但没提到「{keyword}」-> {problems}")
            continue
        passed += 1

    total = len(cases)
    print(f"自测 {passed}/{total}")
    for f in failures:
        print(f"  [FAIL] {f}")
    return 0 if passed == total else 1


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    if "--report" in argv:
        return report()
    return run()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
