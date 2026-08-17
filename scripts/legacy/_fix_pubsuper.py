import os
tools_dir = r"<PROJECT_ROOT>\src-tauri\src\agent\tools"
for fname in ["quality_metrics.rs", "quality_security.rs", "quality_runtime.rs", "quality_media.rs"]:
    path = os.path.join(tools_dir, fname)
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()
    new = content.replace("pub(super) async fn", "pub async fn")
    cnt = content.count("pub(super) async fn")
    if cnt > 0:
        with open(path, "w", encoding="utf-8") as f:
            f.write(new)
        print(f"  {fname}: replaced {cnt}")
print("done")
