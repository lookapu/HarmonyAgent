//! 多模态与密钥域工具：read_pdf / image_inspect / secret_store / secret_get / secret_delete
//! - read_pdf：pdf-extract 提取 PDF 文本（需求文档/规范文档常见格式）
//! - image_inspect：image crate 读取图片元数据（尺寸/格式/位深），截图质检辅助
//! - secret_*：keyring 系统钥匙串（Windows 凭据管理器），替代明文存储密钥

use super::*;

/// read_pdf：提取 PDF 文本内容（前 N 字符，默认 8000）。
pub(super) async fn read_pdf(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("需要参数 {\"path\":\"<PDF 文件路径>\"}")?;
    let path = crate::agent::tools::resolve_in_roots(roots, raw)?;
    if !path.exists() {
        return Err(format!("PDF 文件不存在：{}", path.display()));
    }
    let limit = (args["max_chars"].as_u64().unwrap_or(8000) as usize).clamp(200, 60000);
    // pdf-extract 是同步阻塞 API，丢到阻塞线程池避免卡住异步执行器
    let p = path.clone();
    let text = tokio::task::spawn_blocking(move || pdf_extract::extract_text(&p))
        .await
        .map_err(|e| format!("PDF 解析任务失败：{e}"))?
        .map_err(|e| format!("PDF 解析失败：{e}"))?;
    if text.trim().is_empty() {
        return Ok("（PDF 未提取到文本，可能是扫描件/图片型 PDF，需要 OCR）".into());
    }
    let cleaned: String = text
        .chars()
        .map(|c| if c == '\u{0}' { ' ' } else { c })
        .collect();
    let truncated = cleaned.chars().take(limit).collect::<String>();
    let total = cleaned.chars().count();
    Ok(format!(
        "PDF 文本提取完成（共约 {total} 字符，展示前 {} 字符）：\n{}",
        truncated.chars().count(),
        truncated
    ))
}

/// image_inspect：读取图片元数据（尺寸/格式/位深/文件大小）。
pub(super) async fn image_inspect(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("需要参数 {\"path\":\"<图片文件路径>\"}")?;
    let path = crate::agent::tools::resolve_in_roots(roots, raw)?;
    if !path.exists() {
        return Err(format!("图片文件不存在：{}", path.display()));
    }
    let p = path.clone();
    let meta = tokio::task::spawn_blocking(move || {
        let reader = image::ImageReader::open(&p).map_err(|e| format!("打开图片失败：{e}"))?;
        let format = reader.format().map(|f| format!("{:?}", f)).unwrap_or_else(|| "未知".into());
        let (w, h) = reader.into_dimensions().map_err(|e| format!("读取尺寸失败：{e}"))?;
        let file_size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        Ok::<(String, u32, u32, u64), String>((format, w, h, file_size))
    })
    .await
    .map_err(|e| format!("图片解析任务失败：{e}"))??;
    let (format, w, h, size) = meta;
    let size_s = if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
    } else if size >= 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    };
    Ok(format!(
        "图片元数据：\n- 尺寸：{w} x {h}\n- 格式：{format}\n- 文件大小：{size_s}\n- 路径：{}",
        path.display()
    ))
}

// ---------------- OCR（Windows.Media.Ocr） ----------------

/// Windows OCR 内嵌 C# 源（WinRT via .NET Framework 互操作），首次调用时用 csc.exe 编译为独立
/// exe 并缓存到 %TEMP%\deveco-agent\ocr_v1.exe。输入：图片绝对路径；输出：单行 JSON
/// （{text, line_count} 或 {error, detail}）。非 ASCII 字符全部转义为 \uXXXX，stdout 恒为
/// ASCII，规避控制台代码页乱码。
const OCR_CS: &str = r#"// Windows OCR (WinRT via .NET Framework interop).
using System;
using System.Linq;
using System.Text;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;
using Windows.Storage;

class OcrTool
{
    static int Main(string[] args)
    {
        Console.OutputEncoding = Encoding.UTF8;
        if (args.Length < 1)
        {
            Console.WriteLine("{\"error\":\"NO_ARGS\"}");
            return 0;
        }
        try
        {
            StorageFile file = StorageFile.GetFileFromPathAsync(args[0]).AsTask().GetAwaiter().GetResult();
            if (file == null)
            {
                Console.WriteLine("{\"error\":\"FILE_NOT_FOUND\"}");
                return 0;
            }
            using (var stream = file.OpenReadAsync().AsTask().GetAwaiter().GetResult())
            {
                BitmapDecoder decoder = BitmapDecoder.CreateAsync(stream).AsTask().GetAwaiter().GetResult();
                SoftwareBitmap bitmap = decoder.GetSoftwareBitmapAsync().AsTask().GetAwaiter().GetResult();
                OcrEngine engine = OcrEngine.TryCreateFromUserProfileLanguages();
                if (engine == null)
                {
                    Console.WriteLine("{\"error\":\"NO_OCR_LANG\"}");
                    return 0;
                }
                OcrResult result = engine.RecognizeAsync(bitmap).AsTask().GetAwaiter().GetResult();
                string[] lines = result.Lines.Select(l => l.Text).ToArray();
                string text = string.Join("\n", lines);
                Console.WriteLine("{\"text\":\"" + Escape(text) + "\",\"line_count\":" + lines.Length + "}");
            }
            return 0;
        }
        catch (Exception ex)
        {
            Console.WriteLine("{\"error\":\"OCR_FAILED\",\"detail\":\"" + Escape(ex.Message) + "\"}");
            return 0;
        }
    }

    static string Escape(string s)
    {
        var sb = new StringBuilder();
        foreach (char c in s)
        {
            switch (c)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': break;
                default:
                    if (c < 0x20 || c > 0x7E) sb.Append("\\u").Append(((int)c).ToString("x4"));
                    else sb.Append(c);
                    break;
            }
        }
        return sb.ToString();
    }
}
"#;

/// 定位/编译 OCR 引擎 exe（编译产物缓存到 %TEMP%\deveco-agent，存在即跳过）。
#[cfg(windows)]
async fn ocr_engine_exe(script_dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    const EXE_NAME: &str = "ocr_v1.exe";
    let exe = script_dir.join(EXE_NAME);
    if exe.exists() {
        return Ok(exe);
    }
    // csc.exe：.NET Framework 4.5+ 自带 C# 编译器（64 位框架目录）
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let csc = std::path::Path::new(&windir).join(r"Microsoft.NET\Framework64\v4.0.30319\csc.exe");
    if !csc.exists() {
        return Err("未找到 csc.exe（需安装 .NET Framework 4.5+，用于编译 OCR 引擎）".into());
    }
    // 合并版 Windows.winmd：Windows SDK UnionMetadata 目录下取版本最新者
    let kits_root = r"C:\Program Files (x86)\Windows Kits\10\UnionMetadata";
    let mut best: Option<(u64, std::path::PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(kits_root) {
        for e in entries.flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            let w = dir.join("Windows.winmd");
            if !w.exists() {
                continue;
            }
            let ver = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .split('.')
                .filter_map(|s| s.parse::<u64>().ok())
                .fold(0u64, |acc, v| acc * 10000 + v);
            if best.as_ref().map_or(true, |(v, _)| ver > *v) {
                best = Some((ver, w));
            }
        }
    }
    let winmd = best
        .map(|(_, p)| p)
        .ok_or("未找到 Windows.winmd（需安装 Windows SDK，用于编译 OCR 引擎）")?;
    // 写源文件并编译（产物缓存，下次直接运行）
    let cs_path = script_dir.join("ocr_v1.cs");
    std::fs::write(&cs_path, OCR_CS).map_err(|e| format!("写 OCR 源文件失败：{e}"))?;
    let csc_str = csc.to_string_lossy().into_owned();
    let winmd_str = winmd.to_string_lossy().into_owned();
    let compile_args = vec![
        "/nologo".to_string(),
        "/r:System.Runtime.WindowsRuntime.dll".to_string(),
        "/r:System.Runtime.dll".to_string(),
        format!("/r:{winmd_str}"),
        format!("/out:{}", exe.display()),
        cs_path.to_string_lossy().into_owned(),
    ];
    let mut cmd = crate::utils::process::command(&csc_str, &compile_args)
        .map_err(|e| format!("启动 csc 失败：{e}"))?;
    let out = cmd.output().await.map_err(|e| format!("csc 进程异常：{e}"))?;
    if !out.status.success() {
        let err = smart_decode(&out.stderr);
        let err_out = smart_decode(&out.stdout);
        return Err(format!("OCR 引擎编译失败：{err}{err_out}"));
    }
    Ok(exe)
}

/// ocr_image：Windows 系统 OCR（Windows.Media.Ocr）识别图片文字，无需外置引擎。
/// 参数：{"path":"<图片路径，支持 png/jpg/jpeg/bmp>"}。
pub(super) async fn ocr_image(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("需要参数 {\"path\":\"<图片文件路径>\"}")?;
    let path = crate::agent::tools::resolve_readable(roots, raw)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !["png", "jpg", "jpeg", "bmp"].contains(&ext.as_str()) {
        return Err(format!("不支持的图片格式：{ext}（支持 png/jpg/jpeg/bmp）"));
    }
    #[cfg(not(windows))]
    {
        let _ = (path, ext);
        return Err("ocr_image 仅支持 Windows（Windows.Media.Ocr）".into());
    }
    #[cfg(windows)]
    {
        // 绝对路径（WinRT StorageFile 要求）
        let abs = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir().map_err(|e| e.to_string())?.join(&path)
        };
        // 确保 OCR 引擎 exe 就绪（首次调用时用 csc 编译并缓存）
        let script_dir = std::env::temp_dir().join("deveco-agent");
        std::fs::create_dir_all(&script_dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
        let exe = ocr_engine_exe(&script_dir).await?;
        let mut cmd = crate::utils::process::command(
            &exe.to_string_lossy().into_owned(),
            &[abs.to_string_lossy().into_owned()],
        )?;
        cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(40),
            cmd.output(),
        )
        .await
        .map_err(|_| "OCR 超时（>40s）".to_string())?
        .map_err(|e| format!("OCR 进程启动失败：{e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("OCR 进程异常退出：{err}"));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let json_line = stdout
            .lines()
            .find(|l| l.trim_start().starts_with('{'))
            .unwrap_or(stdout.trim());
        match serde_json::from_str::<serde_json::Value>(json_line.trim()) {
            Ok(v) => {
                if let Some(err_code) = v["error"].as_str() {
                    let detail = v["detail"].as_str().unwrap_or("");
                    let hint = match err_code {
                        "NO_OCR_LANG" => "系统未安装可用的 OCR 语言包（设置 > 时间和语言 > 语言 > 添加 OCR）",
                        "FILE_NOT_FOUND" => "图片文件无法访问",
                        _ => "",
                    };
                    return Err(format!("OCR 失败（{err_code}）：{detail} {hint}"));
                }
                let text = v["text"].as_str().unwrap_or("").trim();
                let n = v["line_count"].as_u64().unwrap_or(0);
                if text.is_empty() {
                    return Ok("OCR 识别完成，但未识别到文字（图片可能无文本或对比度过低）。".into());
                }
                Ok(format!("OCR 识别完成（{n} 行）：\n{text}"))
            }
            Err(e) => Err(format!(
                "OCR 输出解析失败：{e}\n原始输出：{}",
                &stdout[..stdout.len().min(200)]
            )),
        }
    }
}

// ---------------- 系统钥匙串（keyring） ----------------

const KEYRING_SERVICE: &str = "deveco-switch";

/// 统一的 keyring 入口：service 固定，user=键名
fn entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, key).map_err(|e| format!("钥匙串不可用：{e}"))
}

/// secret_store：把密钥写入系统钥匙串（Windows 凭据管理器），返回成功即持久化。
pub(super) async fn secret_store(args: &Value, _roots: &[String]) -> Result<String, String> {
    let key = args["key"].as_str().ok_or("需要参数 {\"key\":\"<键名>\",\"value\":\"<密钥值>\"}")?;
    let key = key.trim();
    if key.is_empty() || key.len() > 64 {
        return Err("键名需为 1~64 字符的非空字符串".into());
    }
    let value = args["value"].as_str().ok_or("需要参数 value（要保存的密钥内容）")?;
    // 异步执行器里避免阻塞（keyring 走系统 API）
    let key = key.to_string();
    let value = value.to_string();
    let k = key.clone();
    tokio::task::spawn_blocking(move || {
        let e = entry(&k)?;
        e.set_password(&value).map_err(|err| {
            match err {
                keyring::Error::NoStorageAccess(..) => "无法访问系统钥匙串（Windows 凭据管理器不可用）".to_string(),
                other => format!("写入钥匙串失败：{other}"),
            }
        })?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("钥匙串任务失败：{e}"))??;
    Ok(format!("密钥 \"{key}\" 已存入系统钥匙串（Windows 凭据管理器），其他工具/进程不再明文落盘。"))
}

/// secret_get：从系统钥匙串读取密钥（返回明文给 Agent 使用）。
pub(super) async fn secret_get(args: &Value, _roots: &[String]) -> Result<String, String> {
    let key = args["key"].as_str().ok_or("需要参数 {\"key\":\"<键名>\"}")?;
    let key = key.trim().to_string();
    let k = key.clone();
    let v = tokio::task::spawn_blocking(move || {
        let e = entry(&k)?;
        e.get_password().map_err(|err| match err {
            keyring::Error::NoEntry => format!("钥匙串中不存在 \"{k}\"（可先 secret_store 保存）"),
            keyring::Error::NoStorageAccess(..) => "无法访问系统钥匙串".to_string(),
            other => format!("读取钥匙串失败：{other}"),
        })
    })
    .await
    .map_err(|e| format!("钥匙串任务失败：{e}"))??;
    // 明文返回给 Agent 使用；提示该值会出现在工具结果中（对话历史可见），用后建议 secret_delete
    Ok(format!(
        "已从系统钥匙串读取 \"{key}\"：\n{}\n\n注意：密钥明文会出现在工具结果中（对话历史可见），用完建议 secret_delete 清除。",
        v
    ))
}

/// secret_delete：删除系统钥匙串中的密钥。
pub(super) async fn secret_delete(args: &Value, _roots: &[String]) -> Result<String, String> {
    let key = args["key"].as_str().ok_or("需要参数 {\"key\":\"<键名>\"}")?;
    let key = key.trim().to_string();
    let k = key.clone();
    tokio::task::spawn_blocking(move || {
        let e = entry(&k)?;
        e.delete_credential().map_err(|err| match err {
            keyring::Error::NoEntry => format!("钥匙串中不存在 \"{k}\""),
            other => format!("删除钥匙串失败：{other}"),
        })?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("钥匙串任务失败：{e}"))??;
    Ok(format!("密钥 \"{key}\" 已从系统钥匙串删除。"))
}
