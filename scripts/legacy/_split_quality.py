#!/usr/bin/env python3
"""精确按行号+花括号配对切分 quality_tools.rs → 4 个子文件 + facade
每个 pub(super) async fn / helper fn 必须完整切完（花括号配对）。
helper 跟用到它的第一个 pub_async 走；未指定的自动就近分配。
"""
import os
import re

src = r"<PROJECT_ROOT>\src-tauri\src\agent\tools\quality_tools.rs"
tools_dir = r"<PROJECT_ROOT>\src-tauri\src\agent\tools"

# 先 git restore 还原（以防上轮半成品污染）
os.system("git checkout HEAD -- " + src.replace("\\", "/"))

with open(src, "r", encoding="utf-8") as f:
    lines = f.read().splitlines()

# 花括号配对找函数结束（处理字符串字面量 + 行注释）
def find_block_end(start):
    depth = 0
    found_open = False
    for i in range(start, len(lines)):
        line = lines[i]
        in_str = False
        quote = None
        idx = 0
        while idx < len(line):
            c = line[idx]
            if in_str:
                if c == '\\' and idx + 1 < len(line):
                    idx += 2
                    continue
                if c == quote:
                    in_str = False
                idx += 1
                continue
            if c == '/' and idx + 1 < len(line) and line[idx + 1] == '/':
                break  # 行注释
            if c == '"' or c == "'" or c == '`':
                in_str = True
                quote = c
                idx += 1
                continue
            if c == '{':
                depth += 1
                found_open = True
            elif c == '}':
                depth -= 1
                if found_open and depth == 0:
                    return i
            idx += 1
    return -1

# 收集所有顶级定义
blocks = []
i = 0
while i < len(lines):
    line = lines[i]
    m = re.match(r'^(pub\(super\)\s+async\s+fn|pub(?:\(crate\))?\s+fn|fn)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(', line)
    if m:
        s = i
        e = find_block_end(s)
        if e < 0:
            print(f"  FAILED at {s+1}: {m.group(1)} {m.group(2)}")
            break
        kind = "pub_async" if "async" in m.group(1) else "helper"
        blocks.append((kind, m.group(2), s, e))
        i = e + 1
    else:
        i += 1

assert len(blocks) == 38, f"expected 38 blocks, got {len(blocks)}"
print(f"  collected {len(blocks)} blocks (19 pub_async + 19 helper)")

# 分类
groups = {
    "quality_metrics.rs": {
        "funcs": ["code_metrics", "metric_export", "log_aggregate", "log_query",
                  "memory_snapshot", "snippet_insert", "replay_trace"],
    },
    "quality_security.rs": {
        "funcs": ["obfuscate", "sandbox_exec", "license_check", "vuln_scan"],
    },
    "quality_runtime.rs": {
        "funcs": ["api_test", "api_mock", "api_health",
                  "attach_debugger", "step_debug", "ota_pack"],
    },
    "quality_media.rs": {
        "funcs": ["docx_read", "audio_transcribe"],
    },
}

# 显式 helper 分配（按"谁用"原则）— target 是子文件短名
EXPLICIT_HELPERS = {
    "filter_by_level": "metrics",      # log_query 用
    "snippet_count": "metrics",       # snippet_insert 用
    "render_trace_chain": "metrics",  # replay_trace 用
    "analyze_source_file": "metrics", # code_metrics 用
    "is_ident_char": "metrics",       # code_metrics 用
    "is_function_line": "metrics",    # code_metrics 用
    "collect_source_files": "metrics",# code_metrics 用
    "parse_dep_line": "security",     # license_check 用
    "extract_quoted": "security",      # license_check 用
    "version_lt": "security",         # vuln_scan 用
    "extract_toml_string": "security",# vuln_scan 用
    "copy_tree": "security",          # sandbox_exec 用
    "sample_from_schema": "runtime",  # api_test 用
    "pick_response_sample": "runtime",# api_test 用
    "path_template_to_regex": "runtime",  # api_test 用
    "find_packaging_tool": "runtime", # ota_pack 用
    "find_whisper_binary": "media",   # audio_transcribe 用
    "find_whisper_model": "media",     # audio_transcribe 用
    "expand_home": "media",           # find_whisper_* 用
}

# 写 4 个子文件
def code_at(s, e):
    return "\n".join(lines[s:e + 1]) + "\n"

HEADER = """//! {} 子模块 — 按职责拆分（详见 quality_tools.rs facade）。
//!
//! 调用方式不变：quality_tools::xxx(...)，通过 pub use re-export 暴露。

use super::*;
"""

for fname, cfg in groups.items():
    parts = [HEADER.format(fname.replace("quality_", "").replace(".rs", "").replace("_", " "))]
    # pub(super) async fn
    for fn_name in cfg["funcs"]:
        item = next((b for b in blocks if b[1] == fn_name), None)
        if not item:
            raise RuntimeError(f"func {fn_name} not found")
        _, _, s, e = item
        parts.append(code_at(s, e))
        parts.append("\n")
    # 分配到本文件的 helper
    my_target = fname.replace("quality_", "").replace(".rs", "")
    my_helpers = [h for h, target in EXPLICIT_HELPERS.items() if target == my_target]
    for h_name in my_helpers:
        item = next((b for b in blocks if b[1] == h_name), None)
        if not item:
            raise RuntimeError(f"helper {h_name} not found")
        _, _, s, e = item
        parts.append("\n")
        parts.append(code_at(s, e))
        parts.append("\n")
    out = "".join(parts)
    out_path = os.path.join(tools_dir, fname)
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(out)
    print(f"  wrote {fname}  ({len(out)} bytes, {len(cfg['funcs'])} funcs + {len(my_helpers)} helpers)")

# 手动加 hdc_shell（带 generic 的 helper，原 quality_tools.rs 里这个）
hdc_shell_block = """
/// 包装 output_blocking：返回 stdout 字符串
/// 接受任意 AsRef<str> 切片，支持混合 &str / &String
fn hdc_shell<S: AsRef<str>>(args: &[S]) -> Result<String, String> {
    let owned: Vec<String> = args.iter().map(|s| s.as_ref().to_string()).collect();
    let out = crate::utils::process::output_blocking("hdc", &owned)
        .map_err(|e| format!("hdc 执行失败: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
"""
# quality_runtime.rs 头部插入
qr_path = os.path.join(tools_dir, "quality_runtime.rs")
with open(qr_path, "r", encoding="utf-8") as f:
    qr = f.read()
qr = qr.replace("use super::*;\n", "use super::*;\n" + hdc_shell_block)
with open(qr_path, "w", encoding="utf-8") as f:
    f.write(qr)
print("  added hdc_shell to quality_runtime.rs")

# facade
facade = """//! Agent 质量/安全/运行时/媒体工具（facade，按职责拆到 4 个子文件）。
//!
//! 调用方式不变：quality_tools::code_metrics(...) / quality_tools::api_test(...) 等，
//! 通过下面 pub use 把子文件函数全部 re-export 出来。

#[path = "quality_metrics.rs"]
mod quality_metrics;
#[path = "quality_security.rs"]
mod quality_security;
#[path = "quality_runtime.rs"]
mod quality_runtime;
#[path = "quality_media.rs"]
mod quality_media;

pub use quality_metrics::*;
pub use quality_security::*;
pub use quality_runtime::*;
pub use quality_media::*;
"""
with open(src, "w", encoding="utf-8") as f:
    f.write(facade)
print(f"  rewrote quality_tools.rs as facade  ({len(facade)} bytes)")

# pub(super) -> pub
for fname in ["quality_metrics.rs", "quality_security.rs",
              "quality_runtime.rs", "quality_media.rs"]:
    p = os.path.join(tools_dir, fname)
    with open(p, "r", encoding="utf-8") as f:
        c = f.read()
    c = c.replace("pub(super) async fn", "pub async fn")
    with open(p, "w", encoding="utf-8") as f:
        f.write(c)
print("  pub(super) -> pub in all 4 subfiles")
print("done")
