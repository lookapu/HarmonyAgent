#!/usr/bin/env python3
"""第 19 道门禁：迁移注册表与 migrations/ 目录必须一一对应。

## 现象

`src-tauri/src/db/mod.rs` 里有一份**手写**的迁移清单：

```rust
pub static MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "001_initial", include_str!("../../migrations/001_initial.sql")),
    ...
    (86, "086_drop_api_details_members", include_str!("../../migrations/086_drop_api_details_members.sql")),
];
```

SQL 通过 `include_str!` **编译进二进制**，所以清单里指向一个不存在的文件时，
`cargo build` 当场失败。反过来却不成立：

**目录里多一个 `.sql`、或者少一个 `.sql`，只要没人同步改这份清单，就没有任何东西会响。**
应用照常启动（`run_migrations` 只遍历清单），那个迁移**一次都不会执行**。

## 为什么现有检查全都看不见

- **`cargo test` 确实从零重放整条链**——`db/mod.rs` 里几十个测试都调 `run_migrations`，
  在内存库上把 86 条全跑一遍。但它重放的是**清单**，不是**目录**：
  清单少一条，重放照样全绿。
- **`db/mod.rs` 的 `migrations_apply_ohpm_sort_columns`** 注释写着
  「迁移清单必须完整注册：验证关键后期迁移列，防止新增迁移文件却未登记」，
  但它只抽查 `ohpm_landscape` 与 `skills` 两张表的几个具体列，**不做对账**。
  抽查不是对账。
- **`check-docs.py` 的注册表判据比的是两个**计数**，不是集合**：

  ```python
  files = len(list((repo / "src-tauri/migrations").glob("*.sql")))
  registered = len(re.findall(r"include_str!\(\"\.\./\.\./migrations/", text))
  if files != registered:      # ← 只比个数
  ```

  ⇒ **同时删掉一个 `.sql` 和它的注册行，两个计数一起减一，仍然相等。**

  ⚠️ 实测时这里我第一次把话说满了，写成「check-docs 完全看不见」——**不对**。
  只删文件+注册、不动文档时，`check-docs.py` 确实会红，但它红在**别的地方**：
  文档里写的迁移数 86 与真源 85 对不上。而**改文档恰恰是删迁移时人会顺手做的事**。
  决定性实验：删掉 `050` 的文件与注册，**并把 7 处文档数字全部改成 85** ⇒

  | | 结果 |
  |---|---|
  | `check-docs.py` | **exit 0，全绿** |
  | 本门 | **exit 1**：`迁移编号不连续：共 85 个…缺失 [50]` |

  ⇒ 所以准确的说法是：**check-docs 的注册表判据是「计数相等」，
  它抓不到「同时删两边」；文档数字检查只是顺带兜住，而改文档是常规动作。**
  **兜底不是守卫。**

  一旦放过去，后果是**两种安装的 schema 从此分叉**：新机器没跑过被删的那条，
  老机器（`_migrations` 里已记 `applied`）还留着它——`cargo test` 每次从零重放，
  也永远发现不了，因为它重放的是**清单**，而清单已经和目录脱节了。

## 判据（七条，全部判红）

1. **集合相等**：`migrations/*.sql` 的文件名集合 == 注册表引用的文件名集合。
   两个方向都报——「有文件没注册」与「注册指向不存在的文件」。
2. **`id` == 文件名的数字前缀**：`run_migrations` 按 `id` 记账，
   对不上会让 `_migrations` 里的编号与磁盘上的文件错位。
3. **`name` == 文件名去掉 `.sql`**：同一理由，`migration_status` 会把它显示给用户。
4. **`id` 不得重复**：最隐蔽的一条。`run_migrations` 的循环体是 `if !applied { 执行 }`，
   第一条执行完就把该 `id` 记成 applied，**第二条同 `id` 的迁移会被静默跳过，永不执行**。
5. **注册必须按 `id` 升序**：数组顺序即应用顺序，升序才等于编号顺序。
6. **`include_str!` 出现次数 == 能解析成元组的条数**：守卫解析器自身。
   解析不到时，**静默漏判比误报危险得多**，所以宁可报红让人来看。
7. **迁移编号自 001 起连续递增**：**这一条才是唯一能抓到「同时删掉一个文件与它的
   注册行」的判据**——那种改法下 1~5 条全部照常成立，只有编号出现空洞。
   （前提：迁移从 001 起且不重排历史编号。本仓 86 条满足。）

## 已知边界

- 本门**只判对账**，不判 SQL 内容对不对、迁移之间的语义顺序是否合理。
  后者是 `run_migrations` 从零重放的职责（`cargo test` 每次都在做），本门不重复。
- 判据 5 只保证「数组顺序 == 编号顺序」，**不保证业务依赖顺序正确**（如「先建表后插数据」）。
  那要读 SQL 语义，属上界问题，不在本门判定范围内。
- 本门**按字面文本解析 Rust 源码**，不是编译。若清单改用宏生成或从文件读入，
  判据 6 会先报红——这是有意的：解析不到就要让人知道，而不是当作「通过」。
- 注册表里若出现指向 `migrations/` 之外的 `include_str!`，本门不解析也不误判。
- 判据 7 假设「迁移从 001 起且不重排历史编号」。若将来引入压缩/重排策略，
  本门必须同步改口径，**不能靠加豁免表绕过去**。

用法：python scripts/check-migration-registry.py            # 查，退出码 1 = 有问题
      python scripts/check-migration-registry.py --self-test # 合成用例
"""

from __future__ import annotations

import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MIGRATIONS_DIR = ROOT / "src-tauri" / "migrations"
REGISTRY = ROOT / "src-tauri" / "src" / "db" / "mod.rs"

#: 一条注册项：(id, "name", include_str!("../../migrations/xxx.sql"))
#: 引号两种写法都要认——Rust 的 `include_str!` 单双引号都合法，只认双引号会漏解析。
ENTRY_RE = re.compile(
    r"""\(\s*(\d+)\s*,\s*["']([^"']+)["']\s*,\s*"""
    r"""include_str!\(\s*["']\.\./\.\./migrations/([^"']+\.sql)["']\s*\)\s*\)"""
)
RAW_INCLUDE_RE = re.compile(r"""include_str!\(\s*["']\.\./\.\./migrations/""")

HEAD = "pub static MIGRATIONS: &[(i64, &str, &str)] = &[\n"
TAIL = "];\n"


def parse_registry(text: str) -> tuple[list[tuple[int, str, str]], int]:
    """返回 (注册项列表, include_str! 原始出现次数)。"""
    entries = [(int(a), b, c) for a, b, c in ENTRY_RE.findall(text)]
    return entries, len(RAW_INCLUDE_RE.findall(text))


def numbering(name: str) -> int | None:
    """文件名数字前缀；取不到返回 None（由调用方判红）。"""
    head = name.split("_", 1)[0]
    return int(head) if head.isdigit() else None


def audit(files: list[str], entries: list[tuple[int, str, str]], raw_count: int) -> list[str]:
    """返回问题清单；空列表表示对账通过。"""
    problems: list[str] = []

    # 判据 6 放在最前：解析不全时，后面的判据都建立在残缺数据上
    if raw_count != len(entries):
        problems.append(
            f"注册表里 include_str! 出现 {raw_count} 次，但只有 {len(entries)} 条能解析成"
            f" (id, name, include_str!(...)) 元组 ⇒ 有条目形状本门解析不到，"
            f"**下面的判据会静默漏判**，请先改本门的正则"
        )

    file_set = set(files)
    ref_set = {ref for _, _, ref in entries}
    for name in sorted(file_set - ref_set):
        problems.append(f"migrations/{name} 未在 db/mod.rs 注册 ⇒ 永远不会执行")
    for name in sorted(ref_set - file_set):
        problems.append(f"db/mod.rs 注册了 migrations/{name}，但该文件不存在")

    # 判据 7：编号自 001 起连续
    for name in sorted(f for f in files if numbering(f) is None):
        problems.append(f"migrations/{name} 文件名没有数字前缀，无法参与编号连续性判定")
    numbers = sorted(n for n in (numbering(f) for f in files) if n is not None)
    if numbers:
        expected = list(range(1, len(numbers) + 1))
        if numbers != expected:
            missing = sorted(set(expected) - set(numbers))
            problems.append(
                f"迁移编号不连续：共 {len(numbers)} 个，应为 001..{len(numbers):03d} 连续；"
                f"缺失 {missing[:10]}{'…' if len(missing) > 10 else ''}"
                f" ⇒ **同时删掉一个 .sql 与它的注册行时，计数判据（check-docs）看不出来，只有这一条能抓**"
            )

    seen: dict[int, str] = {}
    for index, (mid, name, ref) in enumerate(entries):
        where = f"第 {index + 1} 条 ({mid}, \"{name}\", {ref})"
        # 判据 4：id 唯一
        if mid in seen:
            problems.append(
                f"id {mid} 重复（{seen[mid]} 与 {ref}）"
                f" ⇒ run_migrations 的 `if !applied` 会让后一条**永远不执行**"
            )
        else:
            seen[mid] = ref
        # 判据 2 / 3
        if ref in file_set:
            prefix = numbering(ref)
            if prefix is not None and prefix != mid:
                problems.append(f"{where}：id {mid} 与文件名前缀 {prefix:03d} 不一致")
            if ref[:-4] != name:
                problems.append(f"{where}：name \"{name}\" 与文件名 stem \"{ref[:-4]}\" 不一致")

    # 判据 5：升序
    ids = [mid for mid, _, _ in entries]
    if ids != sorted(ids):
        bad = next(i for i in range(1, len(ids)) if ids[i] < ids[i - 1])
        problems.append(
            f"注册表未按 id 升序：第 {bad + 1} 条 id {ids[bad]} 小于前一条 {ids[bad - 1]}"
        )

    return problems


def scan(migrations_dir: Path, registry: Path) -> list[str]:
    files = sorted(p.name for p in migrations_dir.glob("*.sql"))
    if not files:
        return [f"{migrations_dir} 下没有任何 .sql 迁移文件"]
    entries, raw = parse_registry(registry.read_text(encoding="utf-8"))
    return audit(files, entries, raw)


# ---------------------------------------------------------------- 自测

#: 真源 db/mod.rs 第 166 行的**逐字**形状。自测基线里必须真的用到这一行，
#: 否则这条判据就只测了假想写法。
REAL_ENTRY_86 = (
    '    (86, "086_drop_api_details_members", '
    'include_str!("../../migrations/086_drop_api_details_members.sql")),\n'
)


def row(idx: int, stem: str | None = None) -> str:
    stem = stem or f"{idx:03d}_step"
    return f'    ({idx}, "{stem}", include_str!("../../migrations/{stem}.sql")),\n'


def baseline() -> tuple[list[str], str]:
    """干净基线：001..086 连续，最后一条**逐字**用真源写法。"""
    files = [f"{i:03d}_step.sql" for i in range(1, 86)] + ["086_drop_api_details_members.sql"]
    rows = "".join(row(i) for i in range(1, 86)) + REAL_ENTRY_86
    return files, HEAD + rows + TAIL


def self_test() -> int:
    files0, text0 = baseline()

    def drop(idx: int) -> tuple[list[str], str]:
        """把第 idx 条注册与其文件**同时**删掉——check-docs 的计数判据完全看不见的改法。"""
        kept = [
            line for line in text0.splitlines(keepends=True) if f"({idx}, " not in line
        ]
        names = [n for n in files0 if not n.startswith(f"{idx:03d}_")]
        return names, "".join(kept)

    files_num, text_num = drop(50)
    # 元组形状变了：多一个注释，解析器认不出来 ⇒ 判据 6 必须先报红
    files_shp = files0 + ["087_extra.sql"]
    text_shp = text0.replace(
        TAIL,
        '    (87, "087_extra", /* 改形状 */ include_str!("../../migrations/087_extra.sql")),\n' + TAIL,
    )

    cases: list[tuple[str, list[str], str, int, str | None]] = [
        ("干净基线必须零问题", files0, text0, 0, None),
        ("单引号 include_str! 也必须解析（Rust 两种引号都合法）", files0,
         HEAD + text0[len(HEAD):-len(TAIL)].replace('include_str!("', "include_str!('")
         .replace('.sql")', ".sql')") + TAIL, 0, None),
        ("有文件未注册", files0 + ["087_extra.sql"], text0, 1, "未在 db/mod.rs 注册"),
        ("注册指向不存在的文件", files0,
         text0.replace(TAIL, '    (99, "099_ghost", include_str!("../../migrations/099_ghost.sql")),\n' + TAIL),
         1, "该文件不存在"),
        # ↓ 本门存在的**主要理由**：计数判据对它完全隐形
        ("同时删掉一个文件与它的注册行 ⇒ 计数仍相等，只有编号连续性能抓",
         files_num, text_num, 1, "迁移编号不连续"),
        # 直接改真源那一行的 id：99 唯一、不破坏升序、name 仍与 stem 一致
        # ⇒ 只有「id 与文件名前缀不一致」这一条会响
        ("id 与文件名前缀不一致", files0,
         text0.replace(REAL_ENTRY_86, REAL_ENTRY_86.replace("(86,", "(99,")),
         1, "与文件名前缀"),
        ("name 与文件名 stem 不一致", files0,
         text0.replace('"050_step"', '"050_wrong"'), 1, "与文件名 stem"),
        # 51 改成 50 ⇒ 必然同时触发「id 重复」与「前缀不一致」两条**各自独立**的判据。
        # 这里期望 2 条而不是 1 条：两种改法做不到只触发一条，而标记断言保证
        # 「重复」那条确实响了（条数不是判据本身，标记才是）。
        ("id 重复 ⇒ 后一条永不执行", files0,
         text0.replace('(51, "051_step"', '(50, "051_step"'), 2, "重复"),
        ("注册未按 id 升序", files0,
         HEAD + "".join(reversed(text0[len(HEAD):-len(TAIL)].splitlines(keepends=True))) + TAIL,
         1, "升序"),
        ("元组形状变了 ⇒ 解析器漏判必须先报红", files_shp, text_shp, 2, "静默漏判"),
        # 加一个无数字前缀的文件**不会**造成编号空洞（编号列表仍是 1..86），
        # 所以这里只该报「没有数字前缀」一条——第一版误以为会有两条，被自测打回来了。
        ("文件名没有数字前缀", files0 + ["init.sql"],
         text0.replace(TAIL, '    (100, "init", include_str!("../../migrations/init.sql")),\n' + TAIL),
         1, "没有数字前缀"),
    ]

    passed = 0
    with tempfile.TemporaryDirectory() as tmp:
        empty_dir = Path(tmp) / "migrations"
        empty_dir.mkdir()
        reg = Path(tmp) / "mod.rs"
        reg.write_text(HEAD + TAIL, encoding="utf-8")
        empty_problems = scan(empty_dir, reg)

        for name, files, text, want_count, want_marker in cases:
            entries, raw = parse_registry(text)
            problems = audit(files, entries, raw)
            joined = " / ".join(problems)
            if len(problems) != want_count:
                print(f"  ✗ {name}：期望 {want_count} 条问题，实得 {len(problems)} 条 —— {joined[:160]}")
                continue
            if want_marker and want_marker not in joined:
                print(f"  ✗ {name}：问题里找不到判据标记「{want_marker}」—— {joined[:160]}")
                continue
            passed += 1

        # 空目录必须判红：判据落在 scan() 里而不在 audit() 里，
        # 所以这条只能靠真建一个空目录来验，不能用 audit([], [], 0) 糊弄过去。
        if empty_problems and "没有任何 .sql 迁移文件" in empty_problems[0]:
            passed += 1
        else:
            print(f"  ✗ 空目录必须判红（否则文件全删光会被当成集合相等）—— {empty_problems}")

    total = len(cases) + 1
    # 输出格式必须与其余门禁一致（`自测 N/N`）：check-self-test-counts.py
    # 解析的是**输出文本**，改格式等于破坏契约，该改门禁而不是放宽解析。
    print(f"自测 {passed}/{total}")
    return 0 if passed == total else 1


def main() -> int:
    # ⚠️ 必须在 `--self-test` 分支**之前**设好编码：中文 Windows 控制台是 GBK，
    # print 一个 `✗` 就抛 UnicodeEncodeError，退出码 1 与「自测发现真问题」完全一样。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in sys.argv:
        return self_test()

    problems = scan(MIGRATIONS_DIR, REGISTRY)
    if problems:
        print(f"迁移注册表与 migrations/ 目录对不上（{len(problems)} 条）：")
        for p in problems:
            print(f"- {p}")
        return 1

    entries, _ = parse_registry(REGISTRY.read_text(encoding="utf-8"))
    print(
        f"迁移注册表与目录一一对应：{len(entries)} 条，编号 001..{len(entries):03d} 连续，"
        f"id / name 与文件名一致且无重复。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
