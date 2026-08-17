with open("src-tauri/src/agent/tools/quality_tools.rs", "r", encoding="utf-8") as f:
    s = f.read()
s = s.replace("crate::utils::process::hdc_shell", "hdc_shell")
with open("src-tauri/src/agent/tools/quality_tools.rs", "w", encoding="utf-8") as f:
    f.write(s)
print("done")
