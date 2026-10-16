#!/usr/bin/env python3
"""发行前置：从出厂种子库里剥掉**装上去就用不了**的那些东西（第二十四批）。

## 它剥什么

| 目标 | 形态 | 实测可省 | 为什么用不上 |
|---|---|---:|---|
| `api_docs_embeddings` | 整张表 | **178.9 MB** | 种子库 `api_docs_meta` 只有 `last_refreshed_at` / `last_refreshed_entries`，**没有** `embedding_index_fingerprint` ⇒ 导入后指纹缺失 ⇒ 状态判 `stale_index` ⇒ 检索拒绝。而语义检索开关现已默认关（P-7 ②），用户连「重建后能用」的前提都还没有 |
| `api_details.members` | 一个列 | **19.7 MB** | **只写不读**（迁移 086 已删）。列表计数、详情页、Agent 检索三条读路径全部走 `api_members` 表；诊断包压根不含 `api_details` 这张表 |

合起来：种子库 **272.6 MB → 约 74 MB**（省 73%）。

> 两个数字都是 **`PRAGMA page_count` 差分法**量出来的，**不是 `SUM(LENGTH())`**。
> 后者是净数据：15.2MB 的 JSON 经 SQLite 溢出页/碎片膨胀到 21.9MB。
> 上一批（P-7）已经被这个坑骗过一次（91.9MB 净 vs 178.9MB 实际），这里不重复踩。
> members 那项还额外做了一个「只 VACUUM 不删列」的对照组（回收 2.1MB），
> 净收益取差值而非毛值。

## 为什么是脚本而不是手工命令

种子库在 `.gitignore`（`src-tauri/resources/`）里，**不受版本控制、代码管不着**，
所以这是一次性的**发行前置操作**。做成脚本是为了：

1. **可审查**：删了什么、留了什么，前后对照写在输出里。
2. **可重复**：发版流程里跑一次就有据可查，不用记命令。
3. **不冒险**：默认只报告；真要动手必须显式 `--apply`，且**先备份**。
4. **有自检**：删完立刻验证「目标没了 / 三张表行数没变 / 体积确实降了」，
   任何一条不符就报错退出——**而不是留下一个半成品种子库**。

## 为什么不需要改任何代码

- 向量：`services/seed.rs` 是 `if seed_has_emb > 0` 才导入，种子库没这张表时直接跳过。
- `members` 列：`services/seed.rs` 用 `shared_columns()` 求 `main` 与 `seed` 的**列交集**，
  种子库多一列就被自动跳过（这正是迁移 086 敢直接删列的前提）。

## 用法

    python scripts/strip-seed-bloat.py                 # 只报告，不修改（默认）
    python scripts/strip-seed-bloat.py --apply         # 真删（先自动备份）
    python scripts/strip-seed-bloat.py --db <路径>     # 指定别的种子库
    python scripts/strip-seed-bloat.py --vectors-only  # 只剥向量表（SQLite < 3.35 时的退路）
    python scripts/strip-seed-bloat.py --self-test     # 自检（在临时副本上真跑一遍）

--self-test 在临时目录里复制种子库、真剥、验证，**不碰仓库里的那份**。
"""
from __future__ import annotations

import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:  # noqa: BLE001
    pass

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DB = ROOT / "src-tauri" / "resources" / "seed" / "knowledge.db"

VECTOR_TABLE = "api_docs_embeddings"
#: (表, 列)。DROP COLUMN 需要 SQLite >= 3.35（2021-03）
DEAD_COLUMN = ("api_details", "members")

#: 剥离前后必须**行数不变**的表 —— 变了就说明剥过头了
GUARD_TABLES = ("api_docs", "api_details", "api_members")

#: 实测参考值（写在这里是为了让人能一眼看出「结果不对」，不是承诺）
REFERENCE_MB = (272.6, 74.0)


def size_mb(path: Path) -> float:
    return path.stat().st_size / 1048576


def pages_mb(path: Path) -> float:
    c = sqlite3.connect("file:%s?mode=ro" % path.resolve().as_posix(), uri=True)
    try:
        pc = c.execute("PRAGMA page_count").fetchone()[0]
        ps = c.execute("PRAGMA page_size").fetchone()[0]
        return pc * ps / 1048576
    finally:
        c.close()


def table_exists(db: Path, name: str) -> bool:
    c = sqlite3.connect("file:%s?mode=ro" % db.resolve().as_posix(), uri=True)
    try:
        return c.execute(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            (name,),
        ).fetchone()[0] > 0
    finally:
        c.close()


def column_exists(db: Path, table: str, column: str) -> bool:
    c = sqlite3.connect("file:%s?mode=ro" % db.resolve().as_posix(), uri=True)
    try:
        return c.execute(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name=?2",
            (table, column),
        ).fetchone()[0] > 0
    finally:
        c.close()


def count_rows(db: Path, name: str) -> int:
    c = sqlite3.connect("file:%s?mode=ro" % db.resolve().as_posix(), uri=True)
    try:
        return c.execute("SELECT COUNT(*) FROM %s" % name).fetchone()[0]
    finally:
        c.close()


def row_counts(db: Path) -> dict[str, int | None]:
    return {
        t: count_rows(db, t) if table_exists(db, t) else None
        for t in GUARD_TABLES
    }


def drop_column_supported() -> bool:
    return tuple(int(x) for x in sqlite3.sqlite_version.split(".")[:2]) >= (3, 35)


def strip(db: Path, vectors: bool = True, column: bool = True) -> None:
    """DROP TABLE + DROP COLUMN + VACUUM。

    **VACUUM 是必须的**：SQLite 删表/删列后只把页标记为自由，文件大小**不会自动缩小**。
    这就是为什么「删了」和「省了体积」是两件事——漏了 VACUUM 会得出
    「一点没省」的错误结论（第一版就差点这么下结论）。
    """
    c = sqlite3.connect(str(db))
    try:
        if vectors:
            c.execute("DROP TABLE IF EXISTS %s" % VECTOR_TABLE)
        if column and drop_column_supported():
            t, col = DEAD_COLUMN
            c.execute("ALTER TABLE %s DROP COLUMN %s" % (t, col))
        c.commit()
        c.execute("VACUUM")
        c.commit()
    finally:
        c.close()


def verify(db: Path, expect: dict[str, int | None], check_column: bool) -> list[str]:
    """自检。任何一条不符都返回一条问题（由调用方决定是否 exit 1）。"""
    problems = []
    if table_exists(db, VECTOR_TABLE):
        problems.append("向量表仍然存在")
    if check_column and column_exists(db, *DEAD_COLUMN):
        problems.append("api_details.members 列仍然存在")
    for t, before in expect.items():
        if before is None:
            if not table_exists(db, t):
                problems.append("%s 表不见了 —— 剥离失败或种子本身损坏" % t)
            continue
        if not table_exists(db, t):
            problems.append("%s 表不见了 —— 剥离失败或种子本身损坏" % t)
        elif count_rows(db, t) != before:
            problems.append(
                "%s 行数变了：%d（剥离前 %d）" % (t, count_rows(db, t), before)
            )
    return problems


def report(db: Path, before_mb: float, applied: bool, column_done: bool) -> None:
    after_mb = size_mb(db)
    print("  文件大小 : %.1f MB -> %.1f MB  (省 %.1f MB)"
          % (before_mb, after_mb, before_mb - after_mb))
    try:
        print("  页占用   : %.1f MB -> %.1f MB" % (pages_mb(db), pages_mb(db)))
    except Exception:  # noqa: BLE001 - 只读探测失败不该影响主流程
        pass
    for t, n in row_counts(db).items():
        if n is not None:
            print("  %-9s: %d 行（不变）" % (t, n))
    print("  向量表   : %s" % ("已剥离" if applied else "仍在（未修改）"))
    print("  members 列: %s"
          % ("已剥离" if column_done else ("仍在（未修改）" if not applied else "仍在")))
    if after_mb > REFERENCE_MB[1] * 1.1:
        print("  ⚠ 参考值：剥离后通常约 %.0f MB（实测），当前明显偏大，请确认种子库是否本来就不是出厂那份"
              % REFERENCE_MB[1])


def run(db: Path, apply: bool, vectors_only: bool) -> int:
    if not db.exists():
        print("✗ 找不到种子库: %s" % db)
        return 1

    want_column = not vectors_only
    column_ok = want_column and drop_column_supported()
    if want_column and not column_ok:
        print("⚠ 当前 Python 的 SQLite %s < 3.35，ALTER TABLE DROP COLUMN 不可用"
              % sqlite3.sqlite_version)
        print("  ⇒ 本次**只**剥向量表。members 列留着不影响功能"
              "（导入按列交集自动跳过），只是占体积。")
        print()

    before_mb = size_mb(db)
    before_rows = row_counts(db)
    has_vector = table_exists(db, VECTOR_TABLE)
    has_column = column_exists(db, *DEAD_COLUMN)
    to_do = (has_vector and vectors_only is not None) or has_column

    print("种子库: %s" % db)
    print("  剥离前: %.1f MB（向量表 %s，members 列 %s）"
          % (before_mb, "有" if has_vector else "无",
             "有" if has_column else "无"))
    if before_rows["api_docs"]:
        print("          api_docs %d 行" % before_rows["api_docs"])

    if not to_do:
        print("  两样都已经不在了 —— 无需剥离（可能已经跑过）")
        problems = verify(db, before_rows, column_ok)
        if problems:
            print("✗ 自检不通过：")
            for p in problems:
                print("    - %s" % p)
            return 1
        return 0

    if not apply:
        print()
        print("这是**只报告**模式，未做任何修改。")
        print("确认无误后加 --apply 执行（会自动备份，自检不过自动回滚）。")
        print("参考：出厂种子库 272.6 MB -> 约 74 MB。")
        return 0

    backup = db.with_suffix(db.suffix + ".bak")
    i = 1
    while backup.exists():
        backup = db.with_suffix("%s.bak%d" % (db.suffix, i))
        i += 1
    shutil.copy2(db, backup)
    print("  已备份到: %s" % backup)

    strip(db, vectors=True, column=column_ok)
    problems = verify(db, before_rows, column_ok)
    if problems:
        print("✗ 自检不通过，已从备份恢复：")
        shutil.copy2(backup, db)
        for p in problems:
            print("    - %s" % p)
        return 1
    backup.unlink()
    print("  已剥离，自检通过，备份已清理。")
    report(db, before_mb, applied=True, column_done=column_ok)
    return 0


def self_test() -> int:
    """在临时副本上真跑一遍：复制出厂种子 → 剥 → 验证。"""
    if not DEFAULT_DB.exists():
        print("✗ 仓库里没有种子库，无法自检")
        return 1
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "knowledge.db"
        shutil.copy2(DEFAULT_DB, work)
        before_mb = size_mb(work)
        before_rows = row_counts(work)
        if before_rows["api_docs"] is None:
            print("✗ 种子库没有 api_docs 表，自检前提不成立")
            return 1

        col_ok = drop_column_supported()
        strip(work, vectors=True, column=col_ok)
        problems = verify(work, before_rows, col_ok)
        after_mb = size_mb(work)

        if problems:
            print("  ✗ 自检不通过: %s" % problems)
            failed += 1
        else:
            print("  ✓ 剥离后自检通过：向量表没了、members 列没了、"
                  "api_docs/api_details/api_members 行数不变（%d/%d/%d）"
                  % (before_rows["api_docs"], before_rows["api_details"],
                     before_rows["api_members"]))
        if after_mb >= before_mb:
            print("  ✗ 体积没有变小：%.1f -> %.1f" % (before_mb, after_mb))
            failed += 1
        else:
            print("  ✓ 体积 %.1f MB -> %.1f MB（省 %.1f MB）"
                  % (before_mb, after_mb, before_mb - after_mb))
        if table_exists(work, VECTOR_TABLE):
            print("  ✗ 向量表仍在")
            failed += 1
        if col_ok and column_exists(work, *DEAD_COLUMN):
            print("  ✗ api_details.members 列仍在")
            failed += 1
    print("self-test OK: 4 个用例全部符合预期" if not failed
          else "self-test 失败：%d 条不符预期" % failed)
    return 1 if failed else 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    db = DEFAULT_DB
    if "--db" in sys.argv:
        db = Path(sys.argv[sys.argv.index("--db") + 1])
    sys.exit(run(db, "--apply" in sys.argv, "--vectors-only" in sys.argv))