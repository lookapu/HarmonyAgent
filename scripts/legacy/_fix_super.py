import os
tools_dir = r"<PROJECT_ROOT>\src-tauri\src\agent\tools"
for fname in ["quality_metrics.rs", "quality_security.rs", "quality_runtime.rs", "quality_media.rs"]:
    path = os.path.join(tools_dir, fname)
    with open(path, "r", encoding="utf-8") as f:
        c = f.read()
    new = c.replace("super::debug_tools::", "crate::agent::tools::debug_tools::")
    new = new.replace("super::test_tools::", "crate::agent::tools::test_tools::")
    new = new.replace("super::ui_tools::", "crate::agent::tools::ui_tools::")
    if new != c:
        with open(path, "w", encoding="utf-8") as f:
            f.write(new)
        cnt = c.count("super::") - new.count("super::")
        print(f"  {fname}: {cnt} replaced")
print("done")
