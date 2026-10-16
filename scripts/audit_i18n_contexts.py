#!/usr/bin/env python3
"""逐组输出 49 组冲突的**可靠使用点**（只认 t() 直线调用）。

上一版加了「裸名兜底」想覆盖 labelKey / 模板字面量，
结果把 JS 变量（`projectPath`）与 TS 类型（`RefreshProgress`）也当成使用点——
**误报比漏报更贵**：它会让人以为某个键「到处都在用」而放弃核对。
兜底只用于**一个 t() 都没有**的键，且单独标注来源。
"""
import collections
import json
import re
import sys
from pathlib import Path

ROOT = Path(r"H:\work\code\Deveco-code-swich")
SRC = ROOT / "src"


def flat(d, p=""):
    for k, v in d.items():
        nk = f"{p}.{k}" if p else k
        if isinstance(v, dict):
            yield from flat(v, nk)
        else:
            yield nk, v


zh = dict(flat(json.loads((ROOT / "src/i18n/zh.json").read_text(encoding="utf-8"))))
en = dict(flat(json.loads((ROOT / "src/i18n/en.json").read_text(encoding="utf-8"))))

groups = collections.defaultdict(list)
for k, v in zh.items():
    if isinstance(v, str) and v.strip():
        groups[v.strip()].append(k)

conflicts = []
for zv, keys in groups.items():
    if len(keys) < 2:
        continue
    vals = [en.get(k, "") for k in keys]
    if any(v == "" for v in vals):
        continue
    if len(set(vals)) == 1 or len(set(v.lower() for v in vals)) == 1:
        continue
    conflicts.append((zv, keys, vals))

ALL = {k for _z, keys, _v in conflicts for k in keys}
sources = {}
for p in sorted(SRC.rglob("*.ts*")):
    if p.name.endswith(".d.ts") or "i18n" in p.parts:
        continue
    try:
        sources[p.relative_to(ROOT).as_posix()] = p.read_text(encoding="utf-8")
    except Exception:
        pass

USE_RE = re.compile(r"""\bt\(\s*['"]([a-zA-Z0-9_.]+)['"]""")
t_sites = collections.defaultdict(list)
other_refs = collections.defaultdict(list)
#: 非 t() 的引用形态：labelKey / i18nKey / 模板字面量里的叶名
REF_RE = re.compile(r"""(?:labelKey|i18nKey)\s*:\s*['"]([a-zA-Z0-9_.]+)['"]""")
TMPL_RE = re.compile(r"""\bt\(\s*`([^`]+)`""")

for rel, text in sources.items():
    lines = text.splitlines()
    for m in USE_RE.finditer(text):
        k = m.group(1)
        if k not in ALL:
            continue
        ln = text[: m.start()].count("\n") + 1
        t_sites[k].append(f"{rel}:{ln} {lines[ln - 1].strip()[:110]}")
    for m in REF_RE.finditer(text):
        k = m.group(1)
        if k in ALL:
            ln = text[: m.start()].count("\n") + 1
            other_refs[k].append(f"{rel}:{ln} ({m.group(0)})")
    for m in TMPL_RE.finditer(text):
        ln = text[: m.start()].count("\n") + 1
        snippet = lines[ln - 1].strip()[:110]
        for k in ALL:
            if k.rsplit(".", 1)[-1] in snippet and k not in t_sites:
                other_refs[k].append(f"{rel}:{ln} (模板字面量) {snippet}")

rows = []
for i, (zv, keys, vals) in enumerate(sorted(conflicts, key=lambda x: x[1][0]), 1):
    rows.append(f"\n[{i:02d}] 中文「{zv}」")
    for k, v in zip(keys, vals):
        sites = t_sites.get(k, [])
        if sites:
            rows.append(f"   {k:32s} = {v!r}")
            for s in sites[:2]:
                rows.append(f"        {s}")
            if len(sites) > 2:
                rows.append(f"        …共 {len(sites)} 处")
        else:
            refs = other_refs.get(k, [])
            rows.append(f"   {k:32s} = {v!r}   [无 t() 调用]")
            for s in refs[:2]:
                rows.append(f"        {s}")

Path(ROOT / "docs" / ".i18n-contexts.txt").write_text(
    "\n".join(rows), encoding="utf-8", newline=""
)
no_t = sum(1 for _z, keys, _v in conflicts for k in keys if not t_sites.get(k))
print(f"{len(conflicts)} 组，共 {len(ALL)} 个键；其中无 t() 调用的 {no_t} 个")
print(f"已写入 docs/.i18n-contexts.txt（{len(rows)} 行）")
