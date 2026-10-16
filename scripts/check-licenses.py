#!/usr/bin/env python3
"""第三方许可合规门禁（P2-4）。

对应 docs/UI_REFINEMENT_PLAN.md 的 P2-4（§6.17）。

**契约**（改动前先确认不违反）：

1. **`src/generated/thirdParty.json` 必须与 lockfile 一致**。
   它是 About 页的**唯一数据来源**，由 `scripts/gen-license-manifest.py` 生成。
   任何人改了 lockfile 却没重新生成，页面就会展示一份**已经不准的**许可清单——
   而且**页面照常渲染，看不出任何异常**。这与「文档矩阵纯手写」、
   「事件名三处各抄一份」是同一族问题，所以必须有门禁。
2. **非宽松依赖（copyleft / unknown）必须在 `scripts/license-allowlist.json`
   里有显式条目**，且每条要有 `reason` 与 `obligation`。
   新增一个 GPL 依赖会让门禁报红——这是刻意的：它必须是一个**显式决定**，
   而不是某次 `cargo add` 的副作用。
   反过来，豁免清单里有、实际依赖里没有的条目也报红（防止豁免变成永久通行证）。
3. **GPL / AGPL / SSPL 一律阻断**，不接受豁免。
   这类许可与闭源商用分发不兼容，属于技术选型问题，不是清单问题。
4. **本项目自身必须声明许可**：根目录有 `LICENSE`、`package.json` 有 `license`、
   `author`、`repository`，且 README 声明的许可与之**一致**。
   曾经的状态是「README 徽章和正文都写 MIT，但仓库里根本没有 LICENSE 文件」——
   徽章不等于授权。

用法
  python scripts/check-licenses.py             # 校验（CI 门禁）
  python scripts/check-licenses.py --print     # 打印非宽松依赖明细

自测：python scripts/check-licenses.py --self-test
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MANIFEST = REPO / "src" / "generated" / "thirdParty.json"
ALLOWLIST = REPO / "scripts" / "license-allowlist.json"
LICENSE_FILE = REPO / "LICENSE"
GEN = REPO / "scripts" / "gen-license-manifest.py"

# 硬阻断的判定在 gen-license-manifest.is_gpl_unavoidable() 里，
# 本脚本**不自己重写一遍**——两份 SPDX 解析必然分叉（见 check_allowlist 的注释）。
NON_PERMISSIVE = ("copyleft", "unknown")


def _regenerate() -> str:
    """在内存里跑一遍生成器，拿到「当前应有」的内容。

    刻意复用生成器而不是重写一遍解析逻辑：**两份实现必然分叉**，
    门禁与生成器各写一套 SPDX 解析，判据就会在某次修改后不再一致，
    而这种不一致的表现是「门禁绿了但没在检查该检查的东西」。
    """
    import importlib.util
    spec = importlib.util.spec_from_file_location("gen_license_manifest", GEN)
    if spec is None or spec.loader is None:
        raise SystemExit(f"无法加载生成器：{GEN}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.render(mod.build())


def check_manifest_fresh(problems: list[str]) -> None:
    if not MANIFEST.exists():
        problems.append(
            f"{MANIFEST.relative_to(REPO)} 不存在，请运行 python scripts/gen-license-manifest.py"
        )
        return
    if MANIFEST.read_text(encoding="utf-8") != _regenerate():
        problems.append(
            f"{MANIFEST.relative_to(REPO)} 与 lockfile 不一致（lockfile 变了），"
            "请重新运行 python scripts/gen-license-manifest.py"
        )


def check_allowlist(repo: Path, manifest: dict, problems: list[str]) -> None:
    # **所有路径都从 repo 推导**。用模块级常量的话，函数签名里的 repo 就是摆设：
    # 自测时读到的是真实仓库的豁免清单，合成样例永远测不到自己写的分支——
    # 而自测「通过」会让人以为这几条规则有人管。
    allow_path = repo / ALLOWLIST.relative_to(REPO)
    if not allow_path.exists():
        problems.append(f"缺少豁免清单：{ALLOWLIST.relative_to(REPO)}")
        return
    try:
        data = json.loads(allow_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        problems.append(f"{ALLOWLIST.relative_to(REPO)} 解析失败：{e}")
        return

    allowed: set[tuple[str, str]] = set()
    for i, e in enumerate(data.get("entries", [])):
        eco, name = e.get("ecosystem"), e.get("name")
        if not eco or not name:
            problems.append(f"豁免清单第 {i + 1} 条缺 ecosystem / name")
            continue
        for field in ("reason", "obligation", "lastReviewed"):
            if not (e.get(field) or "").strip():
                problems.append(
                    f"豁免条目 {eco}/{name} 缺 {field}：豁免必须写清「为什么放行」与「要履什么义务」，"
                    "不能只是一行备注"
                )
        allowed.add((eco, name))

    actual: set[tuple[str, str]] = set()
    for g in manifest.get("groups", []):
        if g.get("tier") not in NON_PERMISSIVE:
            continue
        for raw in g.get("names", []):
            actual.add((g["ecosystem"], raw.rsplit(" ", 1)[0]))

    for eco, name in sorted(actual - allowed):
        problems.append(
            f"新增未登记的非宽松依赖：{eco}/{name}（档位 {next(g['tier'] for g in manifest['groups'] if g['ecosystem'] == eco and any(n.startswith(name + ' ') or n == name for n in g['names']))}）。"
            f"请先确认商用义务，确认后在 {ALLOWLIST.relative_to(REPO)} 加一条带 reason / obligation 的豁免。"
        )
    for eco, name in sorted(allowed - actual):
        problems.append(
            f"豁免清单里的 {eco}/{name} 已不再是依赖：删掉这条豁免。"
            "留着它会让豁免变成**永久通行证**——依赖删了，义务描述还挂着。"
        )

    # 硬阻断：**无法回避**的 GPL 系。刻意不自己写子串匹配——
    # 那样会把 `MIT OR Apache-2.0 OR LGPL-2.1-or-later` 里的 LGPL 当成强制 copyleft，
    # 而 SPDX 里 OR 意味着你选 MIT 就没有任何 LGPL 义务（本项目真实踩过）。
    gen = _gen_module()
    for g in manifest.get("groups", []):
        if gen.is_gpl_unavoidable(g.get("license", "")):
            problems.append(
                f"出现无法回避的 GPL 系许可：{g['ecosystem']} {g['license']} x{g['count']}。"
                "此类许可与闭源商用分发不兼容，**不接受豁免**，需换掉该依赖。"
            )


def check_self_license(repo: Path = REPO, problems: list[str] | None = None) -> None:
    """本项目自身的许可声明：LICENSE 文件 + package.json + README 三处一致。"""
    problems = problems if problems is not None else []
    license_path = repo / LICENSE_FILE.relative_to(REPO)
    if not license_path.exists():
        problems.append("缺少根目录 LICENSE 文件（README 徽章不等于授权）")
    else:
        text = license_path.read_text(encoding="utf-8", errors="replace")
        if "MIT License" not in text:
            problems.append("LICENSE 文件不是 MIT 正文，与 package.json 声明可能不一致")
        if "Copyright (c)" not in text:
            problems.append("LICENSE 文件缺 Copyright 行")

    pkg_path = repo / "package.json"
    pkg = json.loads(pkg_path.read_text(encoding="utf-8"))
    if not pkg.get("license"):
        problems.append("package.json 缺 license 字段")
    for field in ("author", "repository"):
        if not pkg.get(field):
            problems.append(f"package.json 缺 {field} 字段")

    lic = (pkg.get("license") or "").strip()
    if lic:
        for readme in ("README.md", "README.en.md"):
            p = repo / readme
            if not p.exists():
                continue
            m = re.search(r"^##\s*(?:License|许可).*?$(.*?)(?=^##\s|\Z)",
                          p.read_text(encoding="utf-8", errors="replace"),
                          re.M | re.S)
            body = m.group(1) if m else ""
            if body.strip() and lic.lower() not in body.lower():
                problems.append(f"{readme} 声明的许可与 package.json 的 {lic} 不一致")


def check(repo: Path = REPO) -> list[str]:
    problems: list[str] = []
    check_manifest_fresh(problems)
    if MANIFEST.exists():
        try:
            manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
        except json.JSONDecodeError as e:
            problems.append(f"{MANIFEST.name} 解析失败：{e}")
            manifest = None
        if manifest:
            check_allowlist(repo, manifest, problems)
    check_self_license(repo, problems)
    return problems


def self_test() -> None:
    """合成样例回归：清单过时、新增非宽松、豁免缺 reason、豁免失效、GPL 硬阻断、自身许可缺失。"""
    import tempfile

    gen = _gen_module()

    classify_cases = [
        ("MIT", "permissive", None),
        ("MIT OR Apache-2.0 OR LGPL-2.1-or-later", "permissive", None),
        ("MIT/Apache-2.0", "permissive", None),
        ("(Apache-2.0 OR MIT) AND BSD-3-Clause", "notice", None),
        ("(MIT OR Apache-2.0) AND Unicode-3.0", "permissive", None),
        ("Apache-2.0 AND ISC", "notice", None),
        ("MPL-2.0", "copyleft", None),
        ("Apache-2.0 WITH LLVM-exception", "notice", None),
        ("SEE LICENSE IN LICENSE.txt", "unknown", None),
        ("GPL-3.0", "copyleft", None),
    ]
    # 「无法回避 GPL」的判定：这组是本门最贵的判据，错了会误杀 200+ 个 crate
    gpl_cases = [
        ("MIT OR Apache-2.0 OR LGPL-2.1-or-later", False),
        ("MIT/Apache-2.0", False),
        ("(MIT OR Apache-2.0) AND Unicode-3.0", False),
        ("GPL-3.0", True),
        ("GPL-3.0 AND MIT", True),
        ("MIT OR GPL-3.0", False),          # 可以选 MIT → 躲得开
        ("(MIT OR GPL-3.0) AND Apache-2.0", False),
        ("LGPL-2.1-only", True),
        ("MPL-2.0", False),                 # copyleft 但不是 GPL 系
    ]

    def make(root: Path, *, groups, allow, pkg_license="MIT", has_license=True,
            readme_body="MIT", manifest_text=None):
        (root / "src" / "generated").mkdir(parents=True, exist_ok=True)
        (root / "scripts").mkdir(parents=True, exist_ok=True)
        data = {"generatedFrom": [], "totals": {"all": 0}, "groups": groups}
        (root / MANIFEST.relative_to(REPO)).write_text(
            manifest_text if manifest_text is not None else json.dumps(data, ensure_ascii=False),
            encoding="utf-8")
        (root / ALLOWLIST.relative_to(REPO)).write_text(
            json.dumps({"entries": allow}, ensure_ascii=False), encoding="utf-8")
        (root / "package.json").write_text(
            json.dumps({"license": pkg_license, "author": "a", "repository": "r"}), encoding="utf-8")
        if has_license:
            (root / "LICENSE").write_text("MIT License\n\nCopyright (c) 2026 lookapu\n", encoding="utf-8")
        (root / "README.md").write_text(f"## License\n\n{readme_body}\n", encoding="utf-8")
        (root / "README.en.md").write_text(f"## License\n\n{readme_body}\n", encoding="utf-8")
        return data

    perm = [{"ecosystem": "npm", "license": "MIT", "tier": "permissive",
             "nonSpdx": False, "count": 1, "names": ["react 19.0.0"]}]
    mpl = {"ecosystem": "cargo", "license": "MPL-2.0", "tier": "copyleft",
           "nonSpdx": False, "count": 1, "names": ["cssparser 0.36.0"]}
    mpl_ok = [{"ecosystem": "npm", "license": "MIT", "tier": "permissive", "nonSpdx": False,
               "count": 1, "names": ["react 19.0.0"]},
              {**mpl, "count": 1}]

    def run(root: Path):
        """在合成仓库上跑 3/4/5 三项（1 项要 cargo metadata，跳过）。"""
        ps: list[str] = []
        manifest = json.loads((root / MANIFEST.relative_to(REPO)).read_text(encoding="utf-8"))
        check_allowlist(root, manifest, ps)
        check_self_license(root, ps)
        return ps

    cases = [
        ("全绿", dict(groups=mpl_ok, allow=[{"ecosystem": "cargo", "name": "cssparser",
                                          "tier": "copyleft", "reason": "r", "obligation": "o",
                                          "lastReviewed": "2026-10-01"}]), None),
        ("新增未登记的 MPL 依赖", dict(groups=perm + [mpl], allow=[]), "新增未登记的非宽松依赖"),
        ("豁免缺 reason", dict(groups=mpl_ok, allow=[{"ecosystem": "cargo", "name": "cssparser",
                                                 "tier": "copyleft", "obligation": "o",
                                                 "lastReviewed": "2026-10-01"}]),
         "缺 reason"),
        ("豁免已失效（依赖删了条目还在）", dict(groups=perm, allow=[
            {"ecosystem": "cargo", "name": "cssparser", "tier": "copyleft",
             "reason": "r", "obligation": "o", "lastReviewed": "2026-10-01"}]),
         "不再是依赖"),
        ("GPL 硬阻断且不接受豁免", dict(groups=perm + [
            {"ecosystem": "cargo", "license": "GPL-3.0", "tier": "copyleft", "nonSpdx": False,
             "count": 1, "names": ["bad 1.0.0"]}],
            allow=[{"ecosystem": "cargo", "name": "bad", "tier": "copyleft",
                    "reason": "r", "obligation": "o", "lastReviewed": "2026-10-01"}]),
         "不接受豁免"),
        ("缺 LICENSE 文件", dict(groups=perm, allow=[], has_license=False), "缺少根目录 LICENSE 文件"),
        ("package.json 与 README 许可不一致",
         dict(groups=perm, allow=[], pkg_license="Apache-2.0", readme_body="MIT"), "不一致"),
    ]

    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        for label, kwargs, expected in cases:
            root = Path(tmp) / label.replace(" ", "_")
            root.mkdir()
            make(root, **kwargs)
            probs = run(root)
            if expected is None:
                if probs:
                    print(f"  ✗ {label}: 期望通过，实际失败: {probs}")
                    failed += 1
            elif not any(expected in p for p in probs):
                print(f"  ✗ {label}: 期望问题 '{expected}' 未出现: {probs}")
                failed += 1

    for expr, want, _ in classify_cases:
        got = gen.classify(expr)
        if got != want:
            print(f"  ✗ classify({expr!r}) = {got}，期望 {want}")
            failed += 1

    for expr, want in gpl_cases:
        got = gen.is_gpl_unavoidable(expr)
        if got != want:
            print(f"  ✗ is_gpl_unavoidable({expr!r}) = {got}，期望 {want}")
            failed += 1

    if failed:
        print(f"self-test FAILED: {failed} 个用例失败")
        sys.exit(1)
    print(f"self-test OK: {len(cases)} 个仓库用例 + {len(classify_cases)} 个 SPDX 解析用例 "
          f"+ {len(gpl_cases)} 个 GPL 回避判定用例全部符合预期")


def _gen_module():
    import importlib.util
    spec = importlib.util.spec_from_file_location("gen_license_manifest", GEN)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")

    ap = argparse.ArgumentParser(description="第三方许可合规校验（P2-4）")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--print", dest="show", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        self_test()
        return 0

    if args.show and MANIFEST.exists():
        data = json.loads(MANIFEST.read_text(encoding="utf-8"))
        t = data["totals"]
        print(f"第三方组件 {t['all']} 个：宽松 {t.get('tier_permissive', 0)} / "
              f"需附许可 {t.get('tier_notice', 0)} / copyleft {t.get('tier_copyleft', 0)} / "
              f"无法判定 {t.get('tier_unknown', 0)}")
        for g in data["groups"]:
            if g["tier"] in NON_PERMISSIVE:
                print(f"\n[{g['tier']}] {g['ecosystem']} {g['license']} × {g['count']}")
                print("  " + ", ".join(g["names"]))
        return 0

    problems = check()
    if problems:
        print(f"第三方许可校验失败：{len(problems)} 项")
        for p in problems:
            print(f"- {p}")
        return 1
    data = json.loads(MANIFEST.read_text(encoding="utf-8"))
    t = data["totals"]
    print(f"第三方许可校验通过：{t['all']} 个组件（宽松 {t.get('tier_permissive', 0)} / "
          f"需附许可 {t.get('tier_notice', 0)} / copyleft {t.get('tier_copyleft', 0)} / "
          f"无法判定 {t.get('tier_unknown', 0)}），无 GPL 系，豁免均有据")
    return 0


if __name__ == "__main__":
    sys.exit(main())
