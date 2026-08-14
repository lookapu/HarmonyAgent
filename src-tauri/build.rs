fn main() {
    tauri_build::build();

    // Windows 下 `cargo test --lib` 的 0xC0000139（STATUS_ENTRYPOINT_NOT_FOUND）修复：
    // tauri 库代码导入了 comctl32 v6 专属符号（TaskDialogIndirect 等），但 lib 测试二进制
    // 没有 manifest 声明 comctl32 v6 依赖（tauri-build 仅通过 rustc-link-arg-bins 给 bin
    // 注入 RT_MANIFEST），于是加载 System32 的 comctl32 v5.82 → 入口点缺失 → 0xC0000139。
    // 方案：无条件（build script 无法区分 test/normal 构建）把含 comctl32 v6 声明的
    // RT_MANIFEST 资源加入所有链接目标；语言 ID 用 0（中性）与 bin 资源（0x0409）不冲突，
    // bin 得到冗余的相同声明，行为不变；lib 测试因此获得 manifest。
    #[cfg(target_os = "windows")]
    {
        // 引用仓库内 rc.exe 生成的 .res（RT_MANIFEST=24，中性语言 0x0000），
        // 手写 .res 缺目录条目会导致 LNK1136，故直接提交二进制资源。
        println!(
            "cargo:rustc-link-arg={}",
            concat!(env!("CARGO_MANIFEST_DIR"), "/res_comctl32_v6.res")
        );
    }
}
