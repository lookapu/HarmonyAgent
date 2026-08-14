//! 通用工程概览：识别非鸿蒙工程类型（Node/Go/Rust/Python/Java/C-C++/Flutter/.NET 等）
//! 并返回工程类型、元数据与构建/测试命令建议。
//!
//! 只读取工程配置文件，不执行任何命令；供 Agent 工具 analyze_generic_project
//! 与前端"工程能力分析"面板（analyze_generic_project 命令）共用。

use std::path::Path;

/// 生成非鸿蒙工程的概览文本；目标不是目录 / 无法识别类型时返回 Err。
pub fn generic_project_overview(root: &Path) -> Result<String, String> {
    if !root.is_dir() {
        return Err(format!("目录不存在：{}", root.display()));
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    // 鸿蒙工程由 harmony 分析负责
    if crate::services::workspace::classify(&root)
        .is_some_and(|k| k == crate::services::workspace::ModuleKind::Harmony)
    {
        return Err("该目录是 HarmonyOS 工程，请用鸿蒙工程分析（get_project_info / 工程能力面板）读取工程信息。".into());
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("工程根：{}", root.display()));
    let mut detected = false;
    // Node.js / npm
    if root.join("package.json").is_file() {
        detected = true;
        lines.push("- 工程类型：Node.js（npm/pnpm/yarn）".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("package.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(n) = v["name"].as_str() {
                    lines.push(format!("- 包名：{n}"));
                }
                if let Some(ver) = v["version"].as_str() {
                    lines.push(format!("- 版本：{ver}"));
                }
                if let Some(scr) = v["scripts"].as_object() {
                    if !scr.is_empty() {
                        let names: Vec<&str> = scr.keys().take(10).map(|k| k.as_str()).collect();
                        lines.push(format!("- 可用脚本：{}", names.join(", ")));
                    }
                }
            }
        }
        if root.join("tsconfig.json").is_file() {
            lines.push("- 使用 TypeScript（tsconfig.json）".to_string());
        }
        if root.join("vite.config.ts").is_file() || root.join("vite.config.js").is_file() {
            lines.push("- 构建工具：Vite".to_string());
        }
        lines.push("- 构建：npm run build（也可用 run_command 执行其它 script）".to_string());
        lines.push("- 测试：npm test".to_string());
    }
    // Go
    if root.join("go.mod").is_file() {
        detected = true;
        lines.push("- 工程类型：Go".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("go.mod")) {
            for l in s.lines().take(8) {
                let l = l.trim();
                if let Some(m) = l.strip_prefix("module ") {
                    lines.push(format!("- module：{m}"));
                } else if let Some(v) = l.strip_prefix("go ") {
                    lines.push(format!("- go 版本：{v}"));
                }
            }
        }
        lines.push("- 构建：go build ./...".to_string());
        lines.push("- 测试：go test ./...".to_string());
    }
    // Rust / Cargo
    if root.join("Cargo.toml").is_file() {
        detected = true;
        lines.push("- 工程类型：Rust（Cargo）".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("Cargo.toml")) {
            let (mut name, mut ver) = (None, None);
            for l in s.lines() {
                let l = l.trim();
                if name.is_none() && l.starts_with("name ") && l.contains('=') {
                    name = Some(l.split('=').nth(1).unwrap_or("").trim().trim_matches('"').to_string());
                }
                if ver.is_none() && l.starts_with("version ") && l.contains('=') {
                    ver = Some(l.split('=').nth(1).unwrap_or("").trim().trim_matches('"').to_string());
                }
            }
            if let Some(n) = name {
                lines.push(format!("- 包名：{n}"));
            }
            if let Some(v) = ver {
                lines.push(format!("- 版本：{v}"));
            }
        }
        lines.push("- 构建：cargo build".to_string());
        lines.push("- 测试：cargo test".to_string());
    }
    // Java / Maven
    if root.join("pom.xml").is_file() {
        detected = true;
        lines.push("- 工程类型：Java（Maven）".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("pom.xml")) {
            for l in s.lines() {
                let l = l.trim();
                if let Some(a) = l.strip_prefix("<artifactId>").and_then(|x| x.strip_suffix("</artifactId>")) {
                    lines.push(format!("- artifactId：{a}"));
                    break;
                }
            }
        }
        lines.push("- 构建：mvn package（或 ./mvnw）".to_string());
        lines.push("- 测试：mvn test".to_string());
    }
    // Python
    if root.join("pyproject.toml").is_file() || root.join("setup.py").is_file() || root.join("requirements.txt").is_file() {
        detected = true;
        lines.push("- 工程类型：Python".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("pyproject.toml")) {
            for l in s.lines() {
                let l = l.trim();
                if l.starts_with("name ") && l.contains('=') && !l.starts_with('[') {
                    lines.push(format!(
                        "- 包名：{}",
                        l.split('=').nth(1).unwrap_or("").trim().trim_matches('"').trim_matches('\'')
                    ));
                    break;
                }
            }
        }
        lines.push("- 测试：python -m pytest（或 pytest）".to_string());
    }
    // C/C++ CMake
    if root.join("CMakeLists.txt").is_file() {
        detected = true;
        lines.push("- 工程类型：C/C++（CMake）".to_string());
        lines.push("- 构建：cmake -B build && cmake --build build".to_string());
        lines.push("- 测试：ctest --test-dir build（如有测试目标）".to_string());
    }
    // Makefile
    if root.join("Makefile").is_file() || root.join("makefile").is_file() {
        detected = true;
        lines.push("- 工程类型：Makefile 工程".to_string());
        lines.push("- 构建：make（可先 run_command 执行 make help 查看目标）".to_string());
        lines.push("- 测试：make test（如已定义）".to_string());
    }
    // Flutter / Dart
    if root.join("pubspec.yaml").is_file() {
        detected = true;
        lines.push("- 工程类型：Flutter / Dart".to_string());
        lines.push("- 构建：flutter build".to_string());
        lines.push("- 测试：flutter test".to_string());
    }
    // .NET：目录下存在 *.csproj / *.sln
    if !detected {
        let mut dotnet = false;
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten().take(50) {
                let n = e.file_name().to_string_lossy().to_string();
                if n.ends_with(".csproj") || n.ends_with(".sln") {
                    dotnet = true;
                    break;
                }
            }
        }
        if dotnet {
            detected = true;
            lines.push("- 工程类型：.NET（C#）".to_string());
            lines.push("- 构建：dotnet build".to_string());
            lines.push("- 测试：dotnet test".to_string());
        }
    }
    if !detected {
        return Err(format!(
            "未识别该目录的工程类型（{}）。\n可用 list_dir 浏览结构确认构建方式，或直接用 run_command 执行构建命令。",
            root.display()
        ));
    }
    let mut out = String::from("【工程概览】（非鸿蒙工程）\n");
    out.push_str(&lines.join("\n"));
    out.push_str("\n\n提示：非鸿蒙工程统一用 run_command 执行构建/运行命令；run_tests 会自动按工程类型选择测试命令。\n");
    Ok(out)
}
