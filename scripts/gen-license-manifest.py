#!/usr/bin/env python3
"""第三方许可清单生成器：从两份 lockfile 生成应用内展示与分发的清单。

对应 docs/UI_REFINEMENT_PLAN.md 的 P2-4（§6.17）。

## 为什么要有这个脚本

About 页要展示「用了哪些第三方组件、各自什么许可」。若手写这份清单，
它一定会在第一次 `npm install` / `cargo update` 之后失真——
而**失真是静默的**：页面照常显示，只是不再完整。
这与本项目此前处理过的「文档矩阵纯手写」「事件名三处各抄一份」是同一族问题。

所以：**真源 = lockfile，本脚本 = 产出方，About 页 = 消费方，
`check-licenses.py` = 保证产出方与真源不脱节的判定方。**

## 许可信息从哪来

- **Rust**：`Cargo.lock` **只有版本号，没有许可**（Cargo.lock 的 schema 不含
  license 字段）。许可要从 `cargo metadata` 读——它带每一个 crate 的
  `license` 表达式，来源是各 crate 的 Cargo.toml，比任何手抄都可靠。
- **npm**：`package-lock.json` 同样**不含 license**（npm 刻意不把它写进 lockfile，
  避免 lock 体积膨胀）。许可从 `node_modules/<pkg>/package.json` 的
  `license` 字段读。**这意味着本脚本必须在装了依赖的环境里跑**，
  CI 与开发机都满足；未装依赖时会明确报错而不是静默产出空清单。

## 分类口径

按 SPDX 表达式**结构**分档，而不是把原子词堆在一起看：

- `A OR B` → **二选一**，取最宽松的那一支
- `A AND B` → **同时适用**，取最严格的那一支
- `A WITH exception` → 宽松度由 A 决定

分四档：`permissive`（闭源商用无附加义务）/ `notice`（需随分发件附许可文本）/
`copyleft`（文件级 copyleft 或更严格）/ `unknown`（拿不到可判定的 SPDX 标识）。

**刻意不做的事**：不判断某个依赖「能不能用」——那是法务判断。
本脚本只如实报告，让 `check-licenses.py` 按显式豁免清单卡住新增的非预期项。

用法
  python scripts/gen-license-manifest.py            # 写入 src/generated/thirdParty.json
  python scripts/gen-license-manifest.py --check    # 只校验是否为最新（不写）
  python scripts/gen-license-manifest.py --print    # 打印摘要

注意：--check 与 --print 都需要 cargo metadata，
      因此在没装 Rust 工具链的环境里无法运行（会明确报错，不静默跳过）。
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "src" / "generated" / "thirdParty.json"
CARGO_LOCK = REPO / "src-tauri" / "Cargo.lock"
PKG_LOCK = REPO / "package-lock.json"

# ---- 许可分类 ----------------------------------------------------------------
# 档位语义（**从严方向不是单调的**，所以必须真解析 OR/AND 结构，见下）：
#   permissive 闭源商用分发无附加义务
#   notice     需随分发件附带许可文本 / NOTICE
#   copyleft   文件级 copyleft（MPL-2.0）或更严格，需人工确认
#   unknown    拿不到可判定的 SPDX 标识

RANK = {"permissive": 0, "notice": 1, "unknown": 2, "copyleft": 3}

# 宽松许可的原子名（小写）
PERMISSIVE_ATOMS = {
    "mit", "apache-2.0", "bsd-2-clause", "bsd-3-clause", "isc", "zlib",
    "0bsd", "unlicense", "cc0-1.0", "bsl-1.0", "unicode-3.0",
    "cdla-permissive-2.0", "cdla-permissive-2.0-or-later",
}
# 需要随分发件附带许可文本 / NOTICE
NOTICE_ATOMS = {"apache-2.0", "bsd-2-clause", "bsd-3-clause", "isc", "mpl-2.0"}


def _split_top(expr: str, op: str) -> list[str]:
    """按顶层 ` op ` 切分。**只看括号外的连接词**——
    `Apache-2.0 OR (MIT AND BSD-3-Clause)` 里括号内的 OR 不该参与顶层切分。

    括号计数必须**按 token 内的字符数**算，不能比 `tok == "("`：
    空白分词后 `(Apache-2.0` 是**一个** token，字符串相等比较永远不成立，
    深度会一直是 0，于是括号内的 OR 被误当成顶层连接词、
    外层的 AND 反而切不出来（表现为「无法判定」）。
    """
    tokens = expr.split()
    parts: list[str] = []
    buf: list[str] = []
    depth = 0
    for tok in tokens:
        if depth == 0 and tok.upper() == op:
            parts.append(" ".join(buf))
            buf = []
            continue
        buf.append(tok)
        depth += tok.count("(") - tok.count(")")
    parts.append(" ".join(buf))
    return [p.strip() for p in parts if p.strip()]


def _strip_outer_parens(expr: str) -> str:
    """剥掉包住整个表达式的一层括号。

    必须剥：`(Apache-2.0 OR MIT) AND BSD-3-Clause` 里那个 OR 不是**顶层**连接词，
    不剥的话递归会把它当成一个原子，最终报成「无法判定」——而它其实
    等价于 `Apache-2.0 OR MIT AND BSD-3-Clause` 这种可判定的组合。
    """
    s = expr.strip()
    while s.startswith("(") and s.endswith(")"):
        depth = 0
        for i, ch in enumerate(s):
            depth += 1 if ch == "(" else (-1 if ch == ")" else 0)
            if depth == 0 and i < len(s) - 1:
                return s  # 括号没包住整个串，停止
        s = s[1:-1].strip()
    return s


def _normalize(expr: str) -> str:
    """把 SPDX 表达式的写法差异归一：/ → OR、去 -or-later、压空白、去外层括号。"""
    e = (expr or "").strip()
    e = re.sub(r"\s*/\s*", " OR ", e)
    e = e.replace("-or-later", "")
    e = re.sub(r"\s+", " ", e).strip()
    return _strip_outer_parens(e)


def is_gpl_unavoidable(expr: str) -> bool:
    """该许可表达式是否**无法回避** GPL 系（用于闭源商用分发的硬阻断判断）。

    供 `check-licenses.py` 复用——**门禁绝不能自己重写一遍 SPDX 解析**，
    两份实现必然在某次修改后分叉，而分叉的表现是「门禁绿了但没在检查该检查的东西」。
    本例真实踩过：门禁用子串匹配 raw 字符串，把
    `MIT OR Apache-2.0 OR LGPL-2.1-or-later` 里的 LGPL 当成强制 copyleft，
    而 SPDX 里 `OR` 意味着**你选 MIT 就没有任何 LGPL 义务**。
    生成器已正确判成 permissive，门禁却报红——门禁在**比生成器更严格**，
    而两者用的是同一份真源。

    规则：
    - `A OR B` → 只有**每一支都躲不开** GPL 才算躲不开（选另一支即可）
    - `A AND B` → 任何一边躲不开就躲不开（两者都要满足）
    """
    e = _normalize(expr)
    or_parts = _split_top(e, "OR")
    if len(or_parts) > 1:
        return all(is_gpl_unavoidable(p) for p in or_parts)
    and_parts = _split_top(e, "AND")
    if len(and_parts) > 1:
        return any(is_gpl_unavoidable(p) for p in and_parts)
    atom = _split_top(e, "WITH")[0].strip().lower()
    return re.match(r"^(a?gpl|lgpl|sspl|eupl|osl-|cc-by-nc)", atom) is not None


def classify(expr: str) -> str:
    """返回 permissive / notice / copyleft / unknown。

    **必须区分 OR 与 AND**，这是本函数唯一容易写错的地方：

    - `A OR B` = **二选一**，取**最宽松**的那一支。
      `MIT OR Apache-2.0 OR LGPL-2.1-or-later` 允许你直接选 MIT，
      **不产生任何 copyleft 义务**。把它判成 copyleft 会让读者以为
      这两个 crate 带来了 LGPL 义务——**这是误报，而且是会误导法务判断的误报**。
    - `A AND B` = **同时适用**，取**最严格**的那一支。
      `Apache-2.0 AND ISC` 必须同时满足两者的通知义务，故不低于 notice。
    - `A WITH exception` = A 加上例外条款，宽松度由 A 决定
      （`Apache-2.0 WITH LLVM-exception` 仍是宽松的）。

    这不是「保守起见」的问题，是**算错了**的问题。
    """
    e = (expr or "").strip()
    if not e or e.upper() == "UNKNOWN":
        return "unknown"
    if re.search(r"SEE LICENSE IN|LICENSE\.txt|\bsee\b|UNLICENSED", e, re.I):
        return "unknown"

    # Cargo 把 `/` 当作 `OR` 的旧写法（`MIT/Apache-2.0`、`MIT / Apache-2.0`、
    # `Apache-2.0/MIT` 都是同一个意思）。**不归一的话这 53 个 crate 会被
    # 报成「无法判定」**——而它们清清楚楚是 MIT/Apache 双许可。
    # 假警报会让人不信任整页，比漏报更糟。
    # `-or-later` 变体与基础许可同档（MIT 与 MIT OR-later 义务相同）
    e = _normalize(e)

    or_parts = _split_top(e, "OR")
    if len(or_parts) > 1:
        # 二选一 → 选最宽松
        return min((classify(p) for p in or_parts), key=lambda t: RANK[t])

    and_parts = _split_top(e, "AND")
    if len(and_parts) > 1:
        # 同时适用 → 取最严格
        return max((classify(p) for p in and_parts), key=lambda t: RANK[t])

    # 单个原子（可能带 WITH exception）
    parts = _split_top(e, "WITH")
    atom = parts[0].strip().lower()
    if atom in ("gpl", "agpl") or re.match(r"^(a?gpl|lgpl|sspl|eupl|osl|cc-by-nc)", atom):
        return "copyleft"
    if re.fullmatch(r"mpl-\d+\.\d+", atom):
        return "copyleft"          # 文件级 copyleft：分发义务 + 修改过的文件须开放
    if atom in PERMISSIVE_ATOMS:
        return "notice" if atom in NOTICE_ATOMS else "permissive"
    return "unknown"


# ---- 采集 ---------------------------------------------------------------------

def collect_rust() -> list[dict]:
    if not CARGO_LOCK.exists():
        raise SystemExit(f"缺少 {CARGO_LOCK}，无法生成 Rust 侧清单")
    proc = subprocess.run(
        ["cargo", "metadata", "--manifest-path", str(REPO / "src-tauri" / "Cargo.toml"),
         "--format-version", "1", "--locked"],
        capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    if proc.returncode != 0:
        raise SystemExit(
            "cargo metadata 执行失败（多半是没装 Rust 工具链或 lock 与 manifest 不一致）。\n"
            "本脚本**不会**在拿不到真源时静默产出一份空清单。\n"
            f"stderr: {proc.stderr[:500]}"
        )
    meta = json.loads(proc.stdout)
    out = []
    for p in meta["packages"]:
        if not p.get("source"):
            continue  # 本地 workspace 成员，不是第三方
        expr = p.get("license") or ""
        out.append({
            "ecosystem": "cargo",
            "name": p["name"],
            "version": p["version"],
            "license": expr or "UNKNOWN",
            "tier": classify(expr),
        })
    return out


def collect_npm() -> list[dict]:
    if not PKG_LOCK.exists():
        raise SystemExit(f"缺少 {PKG_LOCK}，无法生成 npm 侧清单")
    pkg = json.loads((REPO / "package.json").read_text(encoding="utf-8"))
    # 只取**直接**依赖：传递依赖已经在各自 package.json 里带自己的许可，
    # 而它们的完整闭包需要遍历 node_modules（几千个），对 About 页没有价值。
    direct = sorted(set(pkg.get("dependencies", {})) | set(pkg.get("devDependencies", {})))
    out = []
    missing = []
    for name in direct:
        p = REPO / "node_modules" / name / "package.json"
        if not p.exists():
            missing.append(name)
            continue
        j = json.loads(p.read_text(encoding="utf-8", errors="replace"))
        expr = j.get("license") or (j.get("licenses") or [{}])[0].get("type") or "UNKNOWN"
        out.append({
            "ecosystem": "npm",
            "name": name,
            "version": j.get("version", ""),
            "license": expr,
            "tier": classify(expr),
            # 部分包用「见某个文件」代替 SPDX 标识（@ohos/hvigor 就是这样），
            # 这类必须原样标出来：它**不是**可判定的宽松许可
            "nonSpdx": not _is_spdxish(expr),
        })
    if missing:
        raise SystemExit(
            f"这些直接依赖在 node_modules 里找不到，无法取许可：{missing}\n"
            "请先 `npm install`。**刻意不跳过**——跳过会让清单静默变短。"
        )
    return out


def _is_spdxish(expr: str) -> bool:
    e = (expr or "").strip()
    if not e or e.upper() == "UNKNOWN":
        return False
    return not re.search(r"SEE LICENSE IN|LICENSE\.txt|\bsee\b", e, re.I)


def build() -> dict:
    items = collect_rust() + collect_npm()
    by_tier: dict[str, list[dict]] = {}
    for it in items:
        by_tier.setdefault(it["tier"], []).append(it)
    for tier in by_tier:
        by_tier[tier].sort(key=lambda x: (x["ecosystem"], x["name"]))

    # 按「许可表达式」聚合：用户看的是「有哪些类型的许可」，不是 671 个名字
    groups: dict[str, dict] = {}
    for it in items:
        key = f"{it['ecosystem']}:{it['license']}"
        g = groups.setdefault(key, {
            "ecosystem": it["ecosystem"],
            "license": it["license"],
            "tier": it["tier"],
            "nonSpdx": it.get("nonSpdx", False),
            "count": 0,
            "names": [],
        })
        g["count"] += 1
        g["names"].append(f"{it['name']} {it['version']}".strip())

    return {
        "generatedFrom": ["src-tauri/Cargo.lock (via cargo metadata)", "package-lock.json + node_modules/*/package.json"],
        "totals": {
            "all": len(items),
            "cargo": sum(1 for i in items if i["ecosystem"] == "cargo"),
            "npm": sum(1 for i in items if i["ecosystem"] == "npm"),
            **{f"tier_{k}": len(v) for k, v in sorted(by_tier.items())},
        },
        "groups": sorted(groups.values(), key=lambda g: (
            {"copyleft": 0, "notice": 1, "unknown": 2, "permissive": 3}[g["tier"]],
            g["ecosystem"], g["license"],
        )),
    }


def render(data: dict) -> str:
    return json.dumps(data, ensure_ascii=False, indent=2, sort_keys=False) + "\n"


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")

    ap = argparse.ArgumentParser(description="第三方许可清单生成器")
    ap.add_argument("--check", action="store_true", help="只校验是否为最新，不写文件")
    ap.add_argument("--print", dest="show", action="store_true", help="打印摘要")
    args = ap.parse_args()

    data = build()
    text = render(data)

    if args.show:
        t = data["totals"]
        print(f"第三方组件共 {t['all']} 个（cargo {t['cargo']} / npm {t['npm']}）")
        for tier in ("copyleft", "notice", "unknown", "permissive"):
            n = t.get(f"tier_{tier}", 0)
            if n:
                print(f"  {tier:11} {n}")
        print()
        for g in data["groups"]:
            if g["tier"] in ("copyleft", "unknown"):
                flag = " [非 SPDX 标识]" if g["nonSpdx"] else ""
                print(f"  [{g['tier']}] {g['ecosystem']} {g['license']} x{g['count']}{flag}")
        return 0

    if args.check:
        if not OUT.exists():
            print(f"{OUT.relative_to(REPO)} 不存在，请运行 gen-license-manifest.py")
            return 1
        if OUT.read_text(encoding="utf-8") != text:
            print(f"{OUT.relative_to(REPO)} 已过时（lockfile 变了），请重新运行 gen-license-manifest.py")
            return 1
        print(f"第三方许可清单是最新的：{data['totals']['all']} 个组件")
        return 0

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"已写入 {OUT.relative_to(REPO)}：{data['totals']['all']} 个第三方组件，"
          f"{len(data['groups'])} 组许可")
    return 0


if __name__ == "__main__":
    sys.exit(main())
