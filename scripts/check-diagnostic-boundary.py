#!/usr/bin/env python3
"""诊断包隐私边界门禁（第十三批 P2-3）。

诊断包的设计目标是「用户敢直接发给客服」。这条承诺一旦被绕过，泄漏是**静默**
的——用户点了导出、拿到一个 zip、贴到公开 issue，内容已经在外面了。
Rust 测试能验「生成的包不泄漏」，但它只覆盖**已经想到的**的那些列；
本门禁覆盖另一件事：**以后有人往 SQL 里加了一列用户内容，门禁当场报红**。

## 为什么需要它，而不是只靠 Rust 测试

两道网的分工：

- Rust 测试（`services::diagnostic_bundle.rs`）：端到端证明**产物**安全。
  它能抓到「我写的清理函数有 bug」。
- 本门禁：**结构**证明**源码**没开新出口。它能抓到「有人加了一列
  `messages.content` 进 SELECT，而清理函数只处理了 error_message」。

结构门禁在**改动发生的时刻**报红，端到端测试要等下次跑才报——
而端到端测试的 fixture 里没有新列，它会安静地继续通过。

## 为什么列要分档，而不是一张禁列表

第一版是一张平铺的禁列表，结果它把两件**性质相反**的事判成了同一件：

- `collect` 里的 `SELECT content FROM messages` —— 泄漏出口，必须禁
- `collect_landmarks` 里的 `SELECT content FROM messages` —— 正是要读它，
  读进来只用于比对「错误文本有没有被用户内容污染」，比对完就丢

同一个 SELECT 形状，一边是漏洞一边是防线。判据分不出这两者，
于是一边误报一边漏报——而**漏报的那一半正好是「看起来像正常代码」的那一半**。

所以改成三档，并且**按函数作用域**判定：

| 档 | 含义 | 规则 |
|---|---|---|
| A 禁读 | 用户内容本身 | 任何作用域都不得 SELECT（`collect_landmarks` 除外，它以此为职） |
| B 限用 | 错误文本 / 标识类 | 可 SELECT，但只能用受限访问器取用（见下） |
| C 自由 | 枚举、计数、时间戳 | 不管 |

## B 档的受限访问器

Rust 侧 `RawRow` 只有四个取值口，门禁逐列核对用的是哪一个：

| 列 | 只允许 | 理由 |
|---|---|---|
| `error_message` / `error` / `message` | `row.clean(...)` | 唯一能写用户可读文本的出口 |
| `provider_id` | `row.present(...)` | 只要「有没有」，不要 ID 本身 |
| `model` / `error_kind` / `error_code` / `code` / `severity` | `row.text(...)` | 配置标识与系统枚举，非用户内容 |

用 `row.text("error_message")` 读一个 B 档错误文本列 = 绕过脱敏 = 报红。
这是本门禁最容易被绕过、也最值得守的一条。

## 契约（10 条）

1. **A 档不进 SELECT**（`collect_landmarks` 作用域除外）
2. **地标值不进条目**：地标只允许出现在 `clean` / `push_error_file` 的传参里，
   不得出现在 `text_entry` / `json_entry` 的构造中
3. **B 档限用**：错误文本列只能经 `row.clean(...)`；`provider_id` 只能经 `row.present(...)`
4. **地标层存在**：`collect_landmarks` 在，且 `clean()` 真的用了 `landmarks`
5. **地标来源齐全**：goal / title / name / content 四路
6. **脱敏只有一份实现**：必须调 `crate::utils::redact::`；不得自建 `redact_*` / `mask_*` / `scrub_*`
7. **两段式**：必须校验 `confirmed`
8. **摘要必须比对**：必须出现 `preview_digest != expected_preview_digest`
9. **条目构造收敛**：`text_entry` / `json_entry` 存在且过 `sanitize_text`
10. **对外承诺齐全**：`EXCLUDED_FROM_BUNDLE` 提到全部必需主题

## 判据的已知边界（重要，不要误读为「绝对安全」）

本门禁是**结构**检查，不是**语义**检查。它保证「没有新的用户内容列被 SELECT」，
但不保证「错误文本里的自由内容被识别」——后者由 Rust 侧的地标层 + 长度截断负责，
且包内 README 已写明残余风险。**两道网都只是降低风险，不是消除。**

还有一条必须写明：**它扫描的是字面量 SQL**。若将来 SQL 改由代码拼接生成，
契约 1/3 会静默失效（扫不到东西 → 报「没问题」）。因此额外有一条存活契约：
每个 `query_rows(` 调用点附近必须能解析到字面量 SELECT，解析不到就报红。

**最后一条边界，负向验证时实测出来的**：契约 4 查的是「`landmarks` 有没有
出现在 `clean` 里」，因此**挡不住「谓词被掏空」**——把
`.filter(|mark| …)` 改成 `.filter(|_| false)` 之后，`landmarks` 字样还在，
判定照常通过。这是结构门禁的固有上限，任何源码级启发式都补不上。
真正兜住它的是 Rust 端到端测试（`generated_bundle_contains_no_user_content_and_no_secrets`
把那条错误文本的原文与地标都埋进 fixture，谓词一失效就报出泄漏）。
**所以这两道网不是冗余，是各管一段：门禁管「出口结构」，测试管「谓词有效」。**

## 用法

    python scripts/check-diagnostic-boundary.py              # 全仓检查（CI 门禁）
    python scripts/check-diagnostic-boundary.py --report     # 打印找到的 SELECT 块
    python scripts/check-diagnostic-boundary.py --self-test
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SERVICE = REPO / "src-tauri" / "src" / "services" / "diagnostic_bundle.rs"

#: A 档——用户内容本身。任何作用域都不得 SELECT。
#:
#: 逐条写死而不是靠正则「看起来像内容」，因为下界要卡准：
#: `title` 要拦而 `severity` 不拦；`error_code` 不拦而 `error` 要拦。
TIER_A_COLUMNS = {
    # 会话与消息正文
    "content", "title", "references_json", "plan_json", "tool_calls_json",
    # 任务与运行
    "goal", "metadata_json", "acceptance_json", "recovery_plan_json",
    "goal_contract_json", "quality_json", "summary", "result_summary",
    # 工具与事件载荷
    "input_json", "result_json", "structured_result_json", "compensation_json",
    "metrics_json", "payload", "payload_json", "checkpoint_json",
    # 定位键（能反查回具体内容）
    "conversation_id", "session_id", "run_id", "trace_id", "parent_run_id",
    "dag_node_id", "call_id",
    # 用户起的名字与凭据
    "name", "base_url", "api_key", "notes", "settings_json", "budget_json",
}

#: B 档——可 SELECT，但只能经指定的访问器取用。
#: 列名 → 唯一允许的访问方法名。
TIER_B_ACCESSORS = {
    "error_message": "clean",
    "error": "clean",
    "message": "clean",
    "provider_id": "present",
}

#: B 档里可以自由取用的列（配置标识与系统枚举）。列在这里只是文档化：
#: 它们的访问口不在受限表里，误用不会报错，但也不在门禁的保护范围内。
TIER_B_FREE_COLUMNS = {"model", "error_kind", "error_code", "code", "severity"}

#: 读用户内容只为「比对后丢弃」的函数。它的 SELECT 不受 A 档约束。
LANDMARK_FUNC = "collect_landmarks"

#: 入口构造器。契约 2 要求它们体内不出现 landmarks。
#:
#: 两者的脱敏要求**不同**，不能一视同仁：
#: - `text_entry` 收任意 `&str`（设备端日志、README），所以它自己必须脱敏
#: - `json_entry` 收 `serde_json::Value`，自由文本只能经上游 `clean()` 进入；
#:   要求它也「过 sanitize_text」是错的方向——那些值在进到这里之前已经处理过了，
#:   再脱一次会把已遮蔽的字段改得面目全非
ENTRY_BUILDERS = ["text_entry", "json_entry"]

#: Rust 侧 EXCLUDED_FROM_BUNDLE 必须逐条覆盖的主题。
REQUIRED_EXCLUSION_TOPICS = [
    "会话标题", "消息正文", "任务目标", "工具输入与输出", "项目名称", "会话 ID",
]

#: 地标来源。少一路就等于那类用户内容混入错误时无人兜底。
REQUIRED_LANDMARK_SOURCES = [
    ("任务目标 goal", "goal FROM agent_runs"),
    ("会话标题 title", "title FROM conversations"),
    ("项目名 name", "name FROM projects"),
    ("用户消息 content", "content FROM messages"),
]

SQL_BLOCK_RE = re.compile(
    r"SELECT\s+([^;\"{}]*?)\s+FROM\s+(\w+)", re.IGNORECASE | re.DOTALL
)
LINE_COMMENT_RE = re.compile(r"//[^\n]*")
BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", re.DOTALL)
#: `query_rows(` 的**调用点**（排除函数定义本身）。
CALL_SITE_RE = re.compile(r"(?<!fn )\bquery_rows\s*\(")
#: `fn name(params) { body }`：同时捕获参数列表与函数体。
#: 本文件的函数都不嵌套花括号（复杂逻辑已拆成小函数），故到行首 `}` 为止。
FUNC_RE = re.compile(
    r"fn\s+(\w+)\s*(?:<[^>]*>)?\s*(\([^)]*\))[^{;]*\{(.*?)\n\}", re.DOTALL
)
#: `row.<accessor>("<column>"` —— B 档取用核对靠它。
ACCESSOR_RE = re.compile(r"row\.(\w+)\(\s*\"(\w+)\"")
#: 调用点之后多长的窗口内仍应能看到 SQL 字面量。
SQL_LITERAL_WINDOW = 700


def strip_comments(text: str) -> str:
    """把注释替换成等长空白，保留字符位置——否则基于位置的判据会跟着错。"""
    out = BLOCK_COMMENT_RE.sub(lambda m: " " * len(m.group(0)), text)
    return LINE_COMMENT_RE.sub(lambda m: " " * len(m.group(0)), out)


def production_source(text: str) -> str:
    """截掉 `#[cfg(test)] mod tests` 及其之后的内容。

    **测试代码必须排除在外**，原因很具体：好的测试会写
    `assert!(row.get("provider_id").is_none())`——用「取到了禁列」来断言
    「禁列没被取到」。判据若把它当成泄漏出口，下一个看到报红的人
    就会把这条正确的断言删掉，换成一条什么都不验的。
    门禁要防的是**生产路径上的出口**，不是测试里对出口的检视。
    """
    index = text.find("#[cfg(test)]")
    return text if index < 0 else text[:index]


def split_items(select_list: str) -> list[str]:
    """拆 SELECT 清单成原始项。只认顶层逗号：函数参数里的逗号不是分隔符。"""
    items, depth, current = [], 0, []
    for ch in select_list:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth = max(0, depth - 1)
        if ch == "," and depth == 0:
            items.append("".join(current).strip())
            current = []
        else:
            current.append(ch)
    tail = "".join(current).strip()
    if tail:
        items.append(tail)
    return [item for item in items if item]


def identifiers_in(item: str) -> set[str]:
    """取一个 SELECT 项里出现的**列名**。

    三处必须剥掉，否则判据会把不是列的东西当列：
    - 字符串字面量（`'x'` 里的 x 不是列）
    - 限定前缀（`t.error` 的 t 是表别名，不是列）
    - 函数名（`COUNT(` 的 COUNT 是函数，不是列；少这一步的话
      「聚合里藏了 conversation_id」这种绕过反而会被 COUNT 自己触发，报错理由还不对）

    `AS` 之后的别名**不剥**：别名就是输出列名，若它命中禁列说明出口确实叫这个名字。
    表达式内部则要一起看——只查别名的话 `COUNT(conversation_id)` 直接绕过。
    """
    text = re.sub(r"'(?:[^']|'')*'", " ", item)
    text = re.sub(r"\b[A-Za-z_]\w*\s*\.", " ", text)      # 去掉 t. / x. 限定前缀
    text = re.sub(r"\b[A-Za-z_]\w*(?=\s*\()", " ", text)  # 去掉函数名
    text = re.sub(r"\s+AS\s+.*$", "", text, flags=re.IGNORECASE)
    return set(re.findall(r"[A-Za-z_]\w*", text.lower()))


def output_name(item: str) -> str:
    match = re.search(r"\s+AS\s+([A-Za-z_]\w*)\s*$", item, flags=re.IGNORECASE)
    if match:
        return match.group(1).lower()
    tail = item.split(".")[-1].strip()
    return tail.lower() if re.fullmatch(r"[A-Za-z_]\w*", tail) else ""


def extract_sql_blocks(source: str) -> list[tuple[str, list[str]]]:
    """返回 [(表名, SELECT 项原文列表)]。"""
    blocks = []
    for match in SQL_BLOCK_RE.finditer(source):
        table = match.group(2)
        if table.startswith("sqlite_master"):
            continue  # 门禁/实现自己的探测语句，不是数据出口
        blocks.append((table, split_items(match.group(1))))
    return blocks


def func_body(stripped: str, name: str) -> str | None:
    for match in FUNC_RE.finditer(stripped):
        if match.group(1) == name:
            return match.group(3)
    return None


def func_params(stripped: str, name: str) -> str | None:
    for match in FUNC_RE.finditer(stripped):
        if match.group(1) == name:
            return match.group(2)
    return None


def landmark_span(stripped: str) -> tuple[int, int] | None:
    """地标函数在去注释源码里的位置区间，用于把 SQL 分到正确作用域。"""
    for match in FUNC_RE.finditer(stripped):
        if match.group(1) == LANDMARK_FUNC:
            return match.start(), match.end()
    return None


def call_sites_without_sql(stripped: str) -> list[int]:
    """返回「自身窗口内没有字面量 SELECT」的 query_rows 调用点序号。

    窗口右边界是**下一个调用点**，不是固定长度：固定 700 字符会让
    「第一个调用的 SQL 写成拼接」被第二个调用的 SQL 顶替掉，
    于是这一条判据只在最后一个调用点失效时才生效——恰好是最不容易被发现的情形。
    """
    sites = list(CALL_SITE_RE.finditer(stripped))
    missing = []
    for index, match in enumerate(sites, start=1):
        end = sites[index].start() if index < len(sites) else len(stripped)
        window = stripped[match.end() : min(end, match.end() + SQL_LITERAL_WINDOW)]
        if not SQL_BLOCK_RE.search(window):
            missing.append(index)
    return missing


def check_text(text: str) -> dict:
    """**唯一的判据实现**。

    自测与真实检查都走这里——拆成两份会各自漂移，而漂移的那份会让
    「自测全绿」变成一句没有意义的话。
    """
    problems: list[str] = []
    stripped = strip_comments(production_source(text))
    flat = " ".join(stripped.split())
    span = landmark_span(stripped)

    # --- 契约 1：A 档不进 SELECT；顺带记下哪些 B 档列真被取了 ---
    scanned = 0
    selected_b: set[str] = set()
    for match in SQL_BLOCK_RE.finditer(stripped):
        table = match.group(2)
        if table.startswith("sqlite_master"):
            continue
        in_landmark_scope = span is not None and span[0] <= match.start() < span[1]
        for item in split_items(match.group(1)):
            scanned += 1
            names = identifiers_in(item) | {output_name(item)} - {""}
            if in_landmark_scope:
                continue
            blocked = names & TIER_A_COLUMNS
            if blocked:
                problems.append(
                    f"SELECT 取了 A 档用户内容列 `{table}.{sorted(blocked)[0]}`"
                    f"（项：{item}）——诊断包不得读取用户内容。"
                    f"若只是要判断「有没有」，请用布尔/计数形态"
                )
            selected_b |= names & set(TIER_B_ACCESSORS)

    # --- 存活契约：每个 query_rows 调用点附近必须有字面量 SELECT ---
    call_count = len(CALL_SITE_RE.findall(stripped))
    if call_count == 0:
        problems.append("整个文件没有 query_rows( 调用——判据已失去被检查对象")
    for site in call_sites_without_sql(stripped):
        problems.append(
            f"第 {site} 处 query_rows( 调用附近找不到字面量 SELECT："
            f"解析规则已失效或 SQL 改成了拼接生成，门禁正在空转"
        )

    # --- 契约 2：地标值不进条目 + 入口的脱敏职责正确 ---
    for builder in ENTRY_BUILDERS:
        body = func_body(stripped, builder)
        if body is None:
            problems.append(f"缺少 fn {builder}——包内条目没有统一构造入口")
            continue
        flat_body = " ".join(body.split())
        if "landmarks" in flat_body:
            problems.append(
                f"fn {builder} 体内出现 landmarks——地标值只用于比对，绝不能写进包"
            )
        if builder == "text_entry":
            # 收任意自由文本 → 必须自己脱敏
            if "sanitize_text" not in flat_body:
                problems.append(
                    "fn text_entry 未过 redact::sanitize_text——它收的是任意文本"
                )
        else:
            # 收结构化 Value → 自由文本只能从上游 clean() 进来（契约 3 守着）
            params = func_params(stripped, builder) or ""
            if "serde_json::Value" not in params:
                problems.append(
                    f"fn {builder} 的入参不是 serde_json::Value"
                    f"（实际：{params.strip()}）——"
                    f"结构化入参是「自由文本只能经 clean() 进入」的唯一保证"
                )

    # --- 契约 3：B 档限用 ---
    for accessor, column in ACCESSOR_RE.findall(stripped):
        allowed = TIER_B_ACCESSORS.get(column.lower())
        if allowed is None:
            continue
        if accessor != allowed:
            problems.append(
                f"列 `{column}` 属 B 档（可读但受限），只能用 `row.{allowed}(...)`；"
                f"实际用了 `row.{accessor}(...)`"
            )
    # 只要求「被 SELECT 了的 B 档列」必须被合规取用。反过来要求所有 B 档列
    # 都被用到是错的：文件里未必引用了每一列，而那不构成缺陷。
    # 这一条真正抓的是**读了却不用**——SELECT 进来只为了丢掉的列，
    # 是「不读用不到的东西」这条原则的落点。
    for column in sorted(selected_b):
        accessor = TIER_B_ACCESSORS[column]
        # 注意不能写成 row.clean("col")：Rust 侧是 row.clean("col", landmarks)，
        # 第二个参数让右括号不在引号后面。带不带右括号在这里是两种不同的查法。
        if f'row.{accessor}("{column}"' not in stripped:
            problems.append(
                f"SELECT 取了 B 档列 `{column}`，但没有经 `row.{accessor}(...)` 取用——"
                f"要么它其实用不上（那就别 SELECT），要么取用方式写错了"
            )

    # --- 契约 4 / 5：地标层 ---
    landmarks_body = func_body(stripped, LANDMARK_FUNC)
    clean_body = func_body(stripped, "clean")
    if landmarks_body is None:
        problems.append(
            f"缺少 {LANDMARK_FUNC}——模式脱敏抓不到任意用户文本，地标层是唯一兜底"
        )
    if clean_body is None:
        problems.append("缺少 clean()——包内没有收敛的自由文本出口")
    elif "landmarks" not in " ".join(clean_body.split()):
        problems.append("clean() 未使用 landmarks——地标收集了却没参与判定")
    for topic, token in REQUIRED_LANDMARK_SOURCES:
        if token not in stripped:
            problems.append(f"地标来源缺少「{topic}」——该类用户内容混入错误时无人兜底")

    # --- 契约 6：脱敏只有一份实现 ---
    # 两种写法都算：全路径调用，或 `use crate::utils::redact;` 之后的短名调用。
    # 只认全路径的话，正常 idiomatic 风格的 Rust 会被误判成「自建脱敏」。
    uses_shared = "crate::utils::redact::" in stripped or re.search(
        r"use\s+crate::utils::redact\s*;", stripped
    )
    if not uses_shared:
        problems.append("未调用 crate::utils::redact——脱敏逻辑必须复用共享实现")
    for banned in ("fn redact_", "fn mask_", "fn scrub_"):
        for match in re.finditer(re.escape(banned) + r"\w*", stripped):
            problems.append(
                f"本文件出现自建脱敏函数 `{match.group(0)}`——规则集只能有一份实现"
            )

    # --- 契约 7 / 8：两段式 ---
    # 查 `!confirmed` 而不是 `confirmed`：后者只要函数签名里出现就会通过，
    # 于是「把守卫删掉、参数留着」这种最常见的退化完全逃得过去。
    if "!confirmed" not in stripped:
        problems.append("generate 未用 confirmed 做守卫——可绕过预览直接导出")
    if "expected_preview_digest" not in stripped:
        problems.append("generate 未校验预览摘要——预览与导出可以不一致")
    if "preview_digest != expected_preview_digest" not in flat:
        problems.append("未发现摘要比对——预览与导出可能不是同一份内容")

    # --- 契约 10：对外承诺齐全 ---
    for topic in REQUIRED_EXCLUSION_TOPICS:
        if topic not in flat:
            problems.append(f"对外承诺缺少主题「{topic}」——EXCLUDED_FROM_BUNDLE 未提到")

    return {
        "name": "diagnostic_bundle.rs",
        "problems": problems,
        "evidence": {
            "sql_blocks": len(extract_sql_blocks(stripped)),
            "query_calls": call_count,
            "items_scanned": scanned,
        },
    }


def run() -> int:
    if not SERVICE.exists():
        print(f"[FAIL] 找不到诊断包源文件：{SERVICE}")
        print("       P2-3 已被回退或改名；本门禁失去被检查对象。")
        return 1
    result = check_text(SERVICE.read_text(encoding="utf-8"))
    ev = result["evidence"]
    print(
        f"诊断包边界检查：{ev['sql_blocks']} 条 SQL / {ev['query_calls']} 处查询 / "
        f"{ev['items_scanned']} 个 SELECT 项已扫"
    )
    if result["problems"]:
        print(f"[FAIL] {len(result['problems'])} 项：")
        for p in result["problems"]:
            print(f"  - {p}")
        return 1
    print("[OK] 隐私边界契约全部满足")
    return 0


def report() -> int:
    stripped = strip_comments(SERVICE.read_text(encoding="utf-8"))
    span = landmark_span(stripped)
    print("诊断包 SELECT 清单（LM=地标作用域，其余为包内数据出口）：")
    for match in SQL_BLOCK_RE.finditer(stripped):
        table = match.group(2)
        if table.startswith("sqlite_master"):
            continue
        scope = "LM" if span and span[0] <= match.start() < span[1] else "  "
        parts = []
        for item in split_items(match.group(1)):
            names = identifiers_in(item)
            tier = "A!" if (names & TIER_A_COLUMNS) and scope == "  " else (
                "B" if names & set(TIER_B_ACCESSORS) else "C"
            )
            parts.append(f"{item}[{tier}]")
        print(f"  [{scope}] {table:16s} {', '.join(parts)}")
    return 0


# ---------------------------------------------------------------------------
# 自测
# ---------------------------------------------------------------------------

SYNTHETIC = """
use crate::utils::redact::sanitize_text;

fn collect_landmarks(conn: &Connection) -> Vec<String> {
    let _ = conn.query("SELECT goal FROM agent_runs WHERE goal IS NOT NULL");
    let _ = conn.query("SELECT title FROM conversations WHERE title IS NOT NULL");
    let _ = conn.query("SELECT name FROM projects WHERE name IS NOT NULL");
    let _ = conn.query("SELECT content FROM messages WHERE role='user'");
    Vec::new()
}

fn clean(value: Option<&str>, landmarks: &[String]) -> Option<String> {
    let _ = value;
    for mark in landmarks { let _ = mark; }
    Some(String::new())
}

fn text_entry(path: &str, kind: &str, text: &str) {
    let _ = sanitize_text(text);
}

fn json_entry(path: &str, kind: &str, value: &serde_json::Value) {
    let _ = to_vec_pretty(value);
}

pub fn generate(confirmed: bool, expected_preview_digest: &str, preview_digest: &str) {
    if !confirmed { return; }
    if preview_digest != expected_preview_digest { return; }
}

const EXCLUDED: &str = "会话标题与消息正文|任务目标|工具输入与输出|项目名称|会话 ID";

fn query_rows(_table: &str, _sql: &str, _limit: usize) -> Vec<String> {
    Vec::new()
}

fn collect(conn: &Connection) {
    let _a = query_rows("task_runs", "SELECT status, error_kind, error_message, provider_id, duration_ms FROM task_runs WHERE status <> 'success'", 30);
    let _b = query_rows("tool_runs", "SELECT tool_name, error_code FROM tool_runs WHERE status = 'error'", 50);
    let _c = row.clean("error_message", &collect_landmarks(conn));
    let _d = row.present("provider_id");
    let _e = row.text("error_kind");
    let _ = (_a, _b, _c, _d, _e);
}
"""


def _pairs(spec) -> tuple[tuple[str, str], ...]:
    """把 (a, b) 与 ((a, b), ...) 统一成「若干对」。"""
    items = tuple(spec)
    if items and all(isinstance(x, str) for x in items):
        return (items,)
    return tuple(tuple(pair) for pair in items)


def synthetic(
    *,
    drop: tuple[str, ...] = (),
    replace: tuple[tuple[str, str], ...] = (),
    swap: tuple[tuple[str, str], ...] = (),
    extra: str = "",
    append: str = "",
) -> str:
    """合成一份源文件。

    `drop` 删片段（缺失类违规）、`replace`/`swap` 替换（改写类违规）、`extra` 插入（多余类违规）。
    三者动手前都**断言锚点确实存在**——`.replace()` 锚点漂移会变成空操作，
    负向用例于是「通过」了，但通过的原因是没注入成功。
    这类假通过比没有负向用例更糟：它让人以为这里有网。

    `swap` 是「短词对短词」的整词替换，优先用它——`replace` 要把整行连同
    引号一起搬进参数，嵌套引号极易数错右括号，而数错之后的报错
    （「) 与 [ 不匹配」）离真正的原因十万八千里。

    `replace` / `swap` 同时接受 `(a, b)` 与 `((a, b),)` 两种写法：
    只放一对时写成前者最顺手，但「元组套元组」少一个括号就会变成
    「把 'a' 和 'b' 各解包成 old/new」而报一个与原因无关的错。
    """
    src = SYNTHETIC
    for old, new in _pairs(replace) + _pairs(swap):
        if old not in src:
            raise AssertionError(f"替换锚点不存在，自测会假通过：{old!r}")
        src = src.replace(old, new)
    for fragment in drop:
        if fragment not in src:
            raise AssertionError(f"删除锚点不存在，自测会假通过：{fragment!r}")
        src = src.replace(fragment, "")
    if extra:
        marker = "fn collect_landmarks("
        if marker not in src:
            raise AssertionError("插入锚点不存在，自测会假通过")
        src = src.replace(marker, extra + "\n" + marker, 1)
    if append:
        src = src + append
    return src


def self_test() -> int:
    cases: list[tuple[str, str, str, bool]] = [("干净源码无问题", synthetic(), "", False)]

    # A 档：在 collect 作用域里取用户内容
    a_columns = [
        ("content", "消息正文"),
        ("goal", "任务目标"),
        ("title", "会话标题"),
        ("input_json", "工具输入"),
        ("result_json", "工具输出"),
        ("conversation_id", "会话 ID"),
        ("session_id", "会话 ID"),
        ("name", "项目名"),
        ("api_key", "凭据"),
        ("payload", "事件载荷"),
    ]
    for col, why in a_columns:
        old = "SELECT status, error_kind, error_message, provider_id"
        new = f"SELECT status, {col}, error_kind, error_message, provider_id"
        src = synthetic(replace=((old, new),))
        assert f"SELECT status, {col}," in src, f"注入未生效：{col}"
        cases.append((f"A 档 {col} 被拦（{why}）", src, col, True))

    expr_old = "SELECT tool_name, error_code FROM tool_runs"
    expr_new_count = "SELECT tool_name, COUNT(conversation_id) FROM tool_runs"
    expr_new_alias = "SELECT tool_name AS t, content AS c FROM tool_runs"

    cases += [
        (
            "A 档藏在表达式里被拦",
            synthetic(replace=((expr_old, expr_new_count),)),
            "conversation_id",
            True,
        ),
        (
            "A 档用 AS 改名被拦",
            synthetic(replace=((expr_old, expr_new_alias),)),
            "content",
            True,
        ),
        (
            "地标作用域内 A 档合法（不误报）",
            synthetic(),  # 地标 SELECT content 本来就该放过；上面「干净源码」已覆盖
            "",
            False,
        ),
        (
            "B 档错误文本绕过 clean 被拦",
            synthetic(swap=(("row.clean", "row.text"),)),
            "B 档",
            True,
        ),
        (
            "B 档 provider_id 绕过 present 被拦",
            synthetic(swap=(("row.present", "row.text"),)),
            "B 档",
            True,
        ),
        (
            "B 档列用错访问器被拦（clean→int）",
            synthetic(swap=(("row.clean", "row.int"),)),
            "B 档",
            True,
        ),
        (
            "B 档列完全没被取用被拦",
            synthetic(drop=('let _d = row.present("provider_id");',)),
            "provider_id",
            True,
        ),
        (
            "C 档列用 clean 过度脱敏不算违规",
            # 过度脱敏只是把内容遮得更多，不是缺陷。门禁若把方向搞反，
            # 就会逼着后来的人去写更精确的解析器，而不是直接多脱一层。
            synthetic(swap=(("row.text", "row.clean"),)),
            "",
            False,
        ),
        (
            "测试模块里的禁列取用不算违规",
            # 测试正是靠 `row.get("content").is_none()` 来断言「没取到」。
            # 判据若把这条也算成泄漏出口，下一个看到报红的人会去删掉它。
            synthetic(
                append='\n#[cfg(test)]\nmod tests {\n    fn leak_check() {\n        let _ = row.get("content");\n        let _ = row.get("title");\n    }\n}\n'
            ),
            "",
            False,
        ),
        (
            "format! 拼的表名不会把后续代码读成列",
            synthetic(extra='fn count_of(conn: &Connection) -> i64 {\n    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))\n}'),
            "",
            False,
        ),
        (
            "地标值进条目被拦",
            synthetic(replace=("let _ = sanitize_text(text);\n}\n\nfn json_entry",
                               "let _ = sanitize_text(&landmarks[0]);\n}\n\nfn json_entry")),
            "landmarks",
            True,
        ),
        (
            "text_entry 不过脱敏被拦",
            synthetic(drop=("let _ = sanitize_text(text);",)),
            "sanitize_text",
            True,
        ),
        (
            "json_entry 改收自由文本被拦",
            synthetic(replace=("value: &serde_json::Value", "raw: &str")),
            "serde_json::Value",
            True,
        ),
        (
            "自建脱敏被拦",
            synthetic(extra="fn redact_local(x: &str) -> String { x.to_string() }"),
            "自建脱敏",
            True,
        ),
        (
            "不调共享脱敏被拦",
            synthetic(drop=("use crate::utils::redact::sanitize_text;",)),
            "utils::redact",
            True,
        ),
        (
            "地标层被删被拦",
            synthetic(drop=("fn collect_landmarks",)),
            "collect_landmarks",
            True,
        ),
        (
            "clean 不用地标被拦",
            synthetic(drop=("for mark in landmarks { let _ = mark; }",)),
            "未使用 landmarks",
            True,
        ),
        (
            "地标缺消息来源被拦",
            synthetic(drop=('let _ = conn.query("SELECT content FROM messages WHERE role=\'user\'");',)),
            "地标来源缺少",
            True,
        ),
        (
            "地标缺 goal 来源被拦",
            synthetic(drop=('let _ = conn.query("SELECT goal FROM agent_runs WHERE goal IS NOT NULL");',)),
            "地标来源缺少",
            True,
        ),
        (
            "confirmed 被删被拦",
            synthetic(drop=("if !confirmed { return; }",)),
            "confirmed",
            True,
        ),
        (
            "摘要比对被删被拦",
            synthetic(drop=("if preview_digest != expected_preview_digest { return; }",)),
            "摘要比对",
            True,
        ),
        (
            "text_entry 构造入口消失被拦",
            synthetic(drop=("fn text_entry(path: &str, kind: &str, text: &str) {",)),
            "缺少 fn text_entry",
            True,
        ),
        (
            "对外承诺缺主题被拦",
            synthetic(drop=("|会话 ID",)),
            "会话 ID",
            True,
        ),
        (
            "SQL 改拼接 → 解析失效被拦",
            synthetic(replace=(
                ('"SELECT status, error_kind, error_message, provider_id, duration_ms FROM task_runs WHERE status <> \'success\'"',
                 "build_sql()"),
            )),
            "解析规则已失效",
            True,
        ),
        (
            "query_rows 调用点全消失被拦",
            synthetic(drop=(
                'let _a = query_rows("task_runs", "SELECT status, error_kind, error_message, provider_id, duration_ms FROM task_runs WHERE status <> \'success\'", 30);',
                'let _b = query_rows("tool_runs", "SELECT tool_name, error_code FROM tool_runs WHERE status = \'error\'", 50);',
            )),
            "没有 query_rows(",
            True,
        ),
    ]

    passed = 0
    failures: list[str] = []
    for name, src, keyword, expect_problem in cases:
        problems = check_text(src)["problems"]
        if bool(problems) != expect_problem:
            failures.append(
                f"{name}: 期望{'有问题' if expect_problem else '无问题'}，"
                f"实际 {len(problems)} 项 -> {problems}"
            )
            continue
        if expect_problem and keyword and not any(keyword in p for p in problems):
            failures.append(f"{name}: 报红了但没提到「{keyword}」-> {problems}")
            continue
        passed += 1

    unit_cases = [
        ("status, error_message", {"status", "error_message"}),
        ("t.error AS e", {"error"}),           # 表别名不算列；AS 前的表达式要看
        ("COUNT(*)", set()),                    # 函数名与星号都不是列
        ("COALESCE(provider_id, 'x')", {"provider_id"}),  # 字符串字面量里的 x 不是列
        ("content AS c", {"content"}),          # 表达式里的原名要看（AS 挡不住绕过）
    ]
    for raw, expected in unit_cases:
        got: set[str] = set()
        for item in split_items(raw):
            got |= identifiers_in(item)
        if got != expected:
            failures.append(f"identifiers_in({raw!r}) = {got}，期望 {expected}")
            continue
        passed += 1

    output_cases = [("t.error AS e", "e"), ("status", "status"), ("COUNT(*)", "")]
    for raw, expected in output_cases:
        got = output_name(raw)
        if got != expected:
            failures.append(f"output_name({raw!r}) = {got!r}，期望 {expected!r}")
            continue
        passed += 1

    total = len(cases) + len(unit_cases) + len(output_cases)
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
