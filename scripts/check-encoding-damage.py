#!/usr/bin/env python3
"""扫仓库里的**写坏汉字**（内容被替换成 `U+FFFD` 的位置）。

## 现象

长中文经工具写入时偶发字节截断：一个汉字变成 2~3 个 `U+FFFD`。
每次都「看起来正常」——文件长度不变、行号不变、**类型检查与 lint 全绿**——
只有一个字没了，而它常常恰好在**语义最关键的位置**（主/辅、存/档、需/要、物/产）。

## 一个必须先纠正的误解（我自己写错过一次）

曾以为「文件里有**非法 UTF-8 字节**，所以 Edit 工具匹配不上，只能按字节替换」。
**实测是错的**：损坏位置存的是**字面的 `U+FFFD` 字符（`EF BF BD`）**，
文件本身**完全合法**，`decode("utf-8")` 不抛异常，
**Edit 可以直接匹配并替换**。那次「Edit 匹配不上」的真实原因是
**我把 `old_string` 和 `new_string` 写成了同一串**。

⇒ 先验证「文件到底是不是合法 UTF-8」，再决定修法与判据。

## 为什么值得立一条门禁

它是**静默**的：改坏了没有任何一处会报红，而内容往往已经进了发行说明与文档。
本项目在一天之内就撞到 4 次，其中一次出现在**当批新增的迁移文件**里。

## 判据与已知边界

- 只查「该行是否含 `U+FFFD`」。**刻意使用**的位置走 `ALLOWED` 白名单
  （例如 `check-i18n-wording.py` 的 `MISSING = "<U+FFFD>缺键"`
  是一个**哨兵值**，靠这个字符保证不与真实翻译撞上）。
- `references/` 是**外部 vendored 代码**，不归本项目维护，直接跳过。
- 写文档时**不要直接写 `U+FFFD` 字符**，一律写 `U+FFFD` 这六个字母——
  否则这条门禁会把自己写出来的说明当成损坏。

用法：python scripts/check-encoding-damage.py            # 查，退出码 1 = 有损坏
      python scripts/check-encoding-damage.py --self-test # 合成用例
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
#: 判据用**后缀**而不是 glob（一次遍历就够，不必 8 趟）
SUFFIXES = (".md", ".py", ".rs", ".ts", ".tsx", ".json", ".sql", ".yml", ".yaml")
#: 构建产物、依赖、以及外部 vendored 代码
SKIP_DIRS = {"node_modules", "target", "dist", ".git", "resources", "coverage",
             "references"}
#: 刻意含 U+FFFD 的行（子串匹配，命中即跳过）
ALLOWED = ("MISSING = \"",)


def damaged_lines(data: bytes) -> list[tuple[int, str]]:
    """返回含 U+FFFD 的 (行号, 内容)。跳过白名单行。

    源码里一律写转义 `\\ufffd` 而不是那个字符本身——
    否则这条门禁会把自己写出来的说明当成损坏（本项目已经因此误报过三次）。
    """
    text = data.decode("utf-8", errors="replace")
    return [
        (i, ln)
        for i, ln in enumerate(text.splitlines(), 1)
        if "\ufffd" in ln and not any(a in ln for a in ALLOWED)
    ]


def collect() -> list[tuple[Path, list]]:
    """遍历仓库，收集含 U+FFFD 的文件。

    ## 这里必须用 os.walk 而不是 rglob（实测 56 秒 → 不到 1 秒）

    `rglob` 是**先遍历完再过滤**：`.gitignore` / `SKIP_DIRS` 只在结果阶段判，
    而 `references/` 是个 vendored 的大仓，于是每次跑都要把它整个走一遍——
    对一个每次提交都跑的门禁来说，56 秒不可接受。
    `os.walk` 可以**在遍历中就改 `dirnames`**，真正不进那些目录。
    """
    bad = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        # 原地剪枝：把要跳过的目录从下一层遍历里删掉
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and not d.startswith(".")]
        for name in filenames:
            if not name.endswith(SUFFIXES):
                continue
            p = Path(dirpath) / name
            try:
                hits = damaged_lines(p.read_bytes())
            except OSError:
                continue
            if hits:
                bad.append((p, hits))
    bad.sort(key=lambda x: str(x[0]))
    return bad


def self_test() -> int:
    """合成用例：正常 / 被写坏 / 白名单 三类都要判对。"""
    cases = [
        (b"# ok\n\xe5\xa5\xbd\n", 0, "正常 UTF-8"),
        (b"# bad\n\xe4\xb8\xef\xbf\xbd\xba\x8b\n", 1, "被写坏的汉字"),
        (b'x = "\xef\xbf\xbd\xe7\xbc\xba\xe9\x94\xae"\n', 1, "U+FFFD 哨兵值未加白名单时报"),
        (b'MISSING = "\xef\xbf\xbd\xe7\xbc\xba\xe9\x94\xae"\n', 0, "白名单行放行"),
        (b"line1\nline2 \xff\xfe line3\n", 1, "裸非法字节（decode 后成替身）"),
        ("中文\n".encode("utf-8"), 0, "多行正常"),
    ]
    passed = 0
    for data, expect, name in cases:
        got = len(damaged_lines(data))
        if got == expect:
            passed += 1
        else:
            print(f"  ✗ {name}：期望 {expect} 处，实得 {got}")
    # 输出格式必须与其余 15 道门一致（`自测 N/N`）：
    # `check-self-test-counts.py` 解析的是**输出文本**，
    # 这里改成英文就会报「读不出用例条数」——契约被破坏时该改门禁，不是放宽解析。
    print(f"自测 {passed}/{len(cases)}")
    return 0 if passed == len(cases) else 1


def main() -> int:
    # ⚠️ 必须在 `--self-test` 分支**之前**设好编码：
    # 中文 Windows 控制台是 GBK，print 一个 `✗` 就抛 UnicodeEncodeError，
    # 退出码 1 与「自测发现真问题」完全一样。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in sys.argv:
        return self_test()

    bad = collect()
    if not bad:
        print("编码完好：未发现写坏的汉字。")
        return 0
    print(f"发现 {len(bad)} 个文件含写坏的汉字：")
    for p, hits in bad:
        print(f"- {p.relative_to(ROOT)}")
        for line_no, ln in hits:
            print(f"    :{line_no}  {ln.strip()[:90]}")
    print("→ 修法：Edit 工具直接替换（那是字面 U+FFFD 字符，文件本身仍是合法 UTF-8）")
    return 1


if __name__ == "__main__":
    sys.exit(main())