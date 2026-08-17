with open(r'src-tauri\src\agent\tools\quality_runtime.rs', 'r', encoding='utf-8') as f:
    content = f.read()
helper = """
/// 包装 output_blocking：返回 stdout 字符串
/// 接受任意 AsRef<str> 切片，支持混合 &str / &String
fn hdc_shell<S: AsRef<str>>(args: &[S]) -> Result<String, String> {
    let owned: Vec<String> = args.iter().map(|s| s.as_ref().to_string()).collect();
    let out = crate::utils::process::output_blocking("hdc", &owned)
        .map_err(|e| format!("hdc 执行失败: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
"""
content = content.replace('use super::*;\n', 'use super::*;\n' + helper)
with open(r'src-tauri\src\agent\tools\quality_runtime.rs', 'w', encoding='utf-8') as f:
    f.write(content)
print('added hdc_shell to quality_runtime.rs')
