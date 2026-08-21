#!/usr/bin/env python3
"""EC-16 失败样本回流工具。

把脱敏后的问题复现包（EC-12 产物）校验、提炼并转换为固定评测场景草案，
用于把真实失败转化为可重复的回归场景。场景草案进入 fixtures 后，
必须在 src-tauri/src/agent/evals.rs 实现执行器并通过 reliability_gate 100% 门禁。

用法：
  python3 scripts/reflow_failure.py --validate <bundle.zip>    # 校验包完整性 + 脱敏状态
  python3 scripts/reflow_failure.py --draft <bundle.zip>       # 生成评测场景草案 JSON
  python3 scripts/reflow_failure.py --check-id <id>            # 检查场景是否已注册
  python3 scripts/reflow_failure.py --self-test                # 内置自测
"""

import argparse
import hashlib
import json
import re
import sys
import zipfile
from pathlib import Path

MANIFEST_SCHEMA = 1
FORMAT = "harmony-agent-reproduction-bundle"
FIXTURES = [
    Path(__file__).resolve().parent.parent
    / "src-tauri/tests/fixtures/agent_reliability_scenarios.json",
    Path(__file__).resolve().parent.parent
    / "src-tauri/tests/fixtures/harmony_task_scenarios.json",
]

# 与固定评测 fixture 的 domain 对齐的类别映射
DOMAIN_SIGNATURES = [
    ("compile_repair", re.compile(r"ArkTS:ERROR|ArkTSCheckError|Hvigor ERROR|Build failed", re.I)),
    ("device_diagnosis", re.compile(r"SIGSEGV|SIGABRT|CppCrash|faultlog|AppFreeze|ANR|deploy.*fail|install.*fail", re.I)),
    ("recovery", re.compile(r"restart|checkpoint|resume|recover|session.*lost", re.I)),
    ("idempotency", re.compile(r"duplicate|idempotency|replayed", re.I)),
    ("approval", re.compile(r"approval|permission denied|unauthorized|forbidden", re.I)),
    ("tool", re.compile(r"timeout|retry|worker crash|panic", re.I)),
    ("new_project", re.compile(r"create.*project|scaffold|new project", re.I)),
    ("cross_module_change", re.compile(r"cross.module|dependency|import.*not found", re.I)),
]

# 脱敏校验：不应出现在载荷中的模式（凭据、私钥、绝对用户路径）
LEAK_PATTERNS = [
    re.compile(r"BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY"),
    re.compile(r"(?i)(password|passwd|secret|api[_-]?key|access[_-]?key|token)\s*[=:]\s*\S{8,}"),
    re.compile(r"AKIA[0-9A-Z]{16}"),
    re.compile(r"-----BEGIN CERTIFICATE"),
    re.compile(r"/Users/[^/\s<]+/", re.I),
    re.compile(r"/home/[^/\s<]+/", re.I),
    re.compile(r"C:\\Users\\[^\\\s<]+\\", re.I),
]


def load_manifest(zip_path: Path) -> tuple[dict, zipfile.ZipFile]:
    archive = zipfile.ZipFile(zip_path)
    names = archive.namelist()
    if names.count("manifest.json") != 1:
        raise ValueError("复现包必须包含且仅包含一个 manifest.json")
    manifest = json.loads(archive.read("manifest.json"))
    if manifest.get("schema") != MANIFEST_SCHEMA or manifest.get("format") != FORMAT:
        raise ValueError(f"manifest 版本/格式不匹配：schema={manifest.get('schema')} format={manifest.get('format')}")
    entries = manifest.get("entries", [])
    payloads = [name for name in names if name != "manifest.json"]
    if len(entries) != len(payloads):
        raise ValueError(f"manifest 条目数 {len(entries)} 与载荷数 {len(payloads)} 不一致")
    if len({entry["path"] for entry in entries}) != len(entries):
        raise ValueError("manifest 存在重复条目路径")
    return manifest, archive


def validate_bundle(zip_path: Path) -> dict:
    """校验 manifest、逐项 SHA-256 与脱敏状态，返回校验报告。"""
    try:
        manifest, archive = load_manifest(zip_path)
    except (ValueError, zipfile.BadZipFile, json.JSONDecodeError) as error:
        return {
            "valid": False,
            "bundle_id": "",
            "title": "",
            "entry_count": 0,
            "redacted_entry_count": 0,
            "missing": [],
            "digest_mismatch": [],
            "leaks": [],
            "error": str(error),
        }
    entries = manifest["entries"]
    missing, digest_mismatch, leaks = [], [], []
    redacted_count = 0
    for entry in entries:
        path = entry["path"]
        if path not in archive.namelist():
            missing.append(path)
            continue
        content = archive.read(path)
        actual = hashlib.sha256(content).hexdigest()
        if actual != entry["sha256"]:
            digest_mismatch.append(path)
            continue
        if entry.get("redacted"):
            redacted_count += 1
        if entry["kind"] == "text" and content:
            text = content.decode("utf-8", errors="replace")
            for pattern in LEAK_PATTERNS:
                if pattern.search(text):
                    leaks.append(f"{path}: {pattern.pattern}")
                    break
    return {
        "valid": not missing and not digest_mismatch and not leaks,
        "bundle_id": manifest["bundle_id"],
        "title": manifest["title"],
        "entry_count": len(entries),
        "redacted_entry_count": redacted_count,
        "missing": missing,
        "digest_mismatch": digest_mismatch,
        "leaks": leaks,
    }


def draft_scenario(zip_path: Path) -> dict:
    """从复现包提炼评测场景草案：类别、错误签名与占位 expected。"""
    manifest, archive = load_manifest(zip_path)
    texts = {}
    for entry in manifest["entries"]:
        if entry["kind"] == "text":
            texts[entry["path"]] = archive.read(entry["path"]).decode("utf-8", errors="replace")
    issue = texts.get("issue.md", "")
    tool_runs = texts.get("diagnostics/tool-runs.json", "")
    haystack = issue + "\n" + tool_runs

    domain = "runtime"
    for candidate, pattern in DOMAIN_SIGNATURES:
        if pattern.search(haystack):
            domain = candidate
            break

    signatures = []
    for pattern in [r"ArkTS:ERROR[^\n]{0,120}", r"ArkTSCheckError[^\n]{0,120}",
                    r"Hvigor[^\n]{0,100}", r"SIG(SEGV|ABRT|BUS)[^\n]{0,60}",
                    r"error[:\s][A-Z0-9_]{4,32}", r"timeout[^\n]{0,80}"]:
        for match in re.finditer(pattern, haystack, re.I):
            signature = match.group(0).strip()
            if signature not in signatures and len(signature) <= 200:
                signatures.append(signature)
        if len(signatures) >= 5:
            break

    short_id = hashlib.sha1(manifest["bundle_id"].encode()).hexdigest()[:8]
    return {
        "id": f"reflow_{domain}_{short_id}",
        "domain": domain,
        "source": {"bundle_id": manifest["bundle_id"], "title": manifest["title"]},
        "error_signatures": signatures[:5],
        "expected": "unimplemented_reflow_scenario",
        "guidance": (
            "1) 把 id 加入 fixtures 的对应 JSON（expected 保持占位会失败关闭）；"
            "2) 在 src-tauri/src/agent/evals.rs 的 simulate_harmony_scenario 实现执行器，"
            "穿过与生产内核相同的解析/诊断/恢复代码；"
            "3) 把 expected 改为生产内核实际结论并确认 100% 通过；"
            "4) 评审 fixture 变更时必须连同生产策略、文档和测试一起评审。"
        ),
    }


def check_registered(scenario_id: str) -> dict:
    """检查场景 id 是否已注册到固定评测 fixtures。"""
    registered = False
    for fixture in FIXTURES:
        if not fixture.exists():
            continue
        scenarios = json.loads(fixture.read_text())
        if any(item.get("id") == scenario_id for item in scenarios):
            registered = True
            break
    return {"id": scenario_id, "registered": registered}


def self_test() -> None:
    """用合成复现包验证 validate 与 draft 主路径。"""
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        bundle = Path(tmp) / "sample.zip"
        issue = "Hvigor ERROR: ArkTS:ERROR File: entry/src/main/ets/Index.ets:12:5 " \
                "This API requires API version 14"
        entry = {"path": "issue.md", "kind": "text", "bytes": len(issue),
                 "sha256": hashlib.sha256(issue.encode()).hexdigest(), "redacted": True}
        manifest = {"schema": MANIFEST_SCHEMA, "format": FORMAT, "bundle_id": "00000000-0000-0000-0000-000000000001",
                    "title": "self-test", "preview_digest": "sha256:x", "generator_version": "test",
                    "generated_at": 0, "entries": [entry]}
        with zipfile.ZipFile(bundle, "w") as archive:
            archive.writestr("manifest.json", json.dumps(manifest))
            archive.writestr(entry["path"], issue)

        report = validate_bundle(bundle)
        assert report["valid"], report
        assert report["redacted_entry_count"] == 1
        draft = draft_scenario(bundle)
        assert draft["domain"] == "compile_repair", draft
        assert draft["id"].startswith("reflow_compile_repair_")
        assert any("API version 14" in s for s in draft["error_signatures"]), draft

        # 篡改载荷应导致校验失败
        with zipfile.ZipFile(bundle, "a") as archive:
            archive.writestr("issue.md", "tampered")
        assert not validate_bundle(bundle)["valid"]

        # 泄露模式应被检出
        leaky = issue + "\npassword=SuperSecret123"
        entry2 = dict(entry, bytes=len(leaky), sha256=hashlib.sha256(leaky.encode()).hexdigest())
        manifest2 = dict(manifest, bundle_id="00000000-0000-0000-0000-000000000002", entries=[entry2])
        bundle2 = Path(tmp) / "leaky.zip"
        with zipfile.ZipFile(bundle2, "w") as archive:
            archive.writestr("manifest.json", json.dumps(manifest2))
            archive.writestr(entry2["path"], leaky)
        assert not validate_bundle(bundle2)["valid"]
        assert validate_bundle(bundle2)["leaks"]

        # check-id 未注册场景应报告 registered=false
        assert not check_registered("reflow_nonexistent_00000000")["registered"]
    print("self-test: 全部通过")


def main() -> int:
    parser = argparse.ArgumentParser(description="失败样本回流工具（EC-16）")
    parser.add_argument("--validate", metavar="ZIP", help="校验复现包完整性与脱敏状态")
    parser.add_argument("--draft", metavar="ZIP", help="生成评测场景草案")
    parser.add_argument("--check-id", metavar="ID", help="检查场景 id 是否已注册")
    parser.add_argument("--self-test", action="store_true", help="运行内置自测")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0
    if args.validate:
        report = validate_bundle(Path(args.validate))
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 0 if report["valid"] else 1
    if args.draft:
        print(json.dumps(draft_scenario(Path(args.draft)), ensure_ascii=False, indent=2))
        return 0
    if args.check_id:
        result = check_registered(args.check_id)
        print(json.dumps(result, ensure_ascii=False))
        return 0 if result["registered"] else 1
    parser.print_help()
    return 2


if __name__ == "__main__":
    sys.exit(main())
