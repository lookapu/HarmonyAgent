//! 设备预览：直接驱动 SDK 内置的 Previewer 取真实渲染帧。
//!
//! 链路（2026-09-18 实测，详见 docs/MAINLINE_AUDIT_2026-09-14.md §52）：
//! 1. `hvigorw ... -p buildRoot=.preview PreviewBuild` 产出 `<module>/.preview/...`
//!    ——**必须带 `buildRoot=.preview`**，否则预览任务链退化、`.preview/` 不生成；
//! 2. 以引擎自己的 `common/bin` 为 cwd 启动 `Previewer`，用 `-lws <端口>` 开 WebSocket；
//!    cwd 不对时字体配置找不到，文字整片渲染不出来；
//! 3. 连 `ws://127.0.0.1:<端口>/<sid>` 收二进制帧，帧 = 40 字节头 + 一张标准 JPEG。
//!
//! 本模块只放可测的纯逻辑与取帧客户端，进程生命周期在 `commands/preview.rs`。

use std::path::{Path, PathBuf};

use crate::services::harmony_env::HarmonyEnv;

/// 端口下界（引擎硬约束：29000 < port < 50000，见插件源码）
const PORT_MIN: u16 = 29001;
const PORT_MAX: u16 = 49999;

/// 帧头长度：magic + 两组宽高 + 保留字段，其后即 JPEG
pub const FRAME_HEADER_LEN: usize = 40;
const FRAME_MAGIC: [u8; 4] = [0x12, 0x34, 0x56, 0x78];

/// Previewer 可执行文件与它所在目录。
/// **`dir` 必须作为子进程 cwd**，否则引擎找不到同目录的 fontconfig.json 与 fonts/。
#[derive(Debug, Clone)]
pub struct PreviewerBin {
    pub dir: PathBuf,
    pub exe: PathBuf,
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// 从环境快照里定位 Previewer：SDK 变体的 previewer 组件根是
/// `<sdk>/<variant>/previewer`，可执行文件在其 `common/bin` 下。
pub fn locate(env: &HarmonyEnv) -> Option<PreviewerBin> {
    for variant in &env.sdk_variants {
        for comp in &variant.components {
            if comp.name != "previewer" {
                continue;
            }
            let dir = Path::new(&comp.path).join("common").join("bin");
            let exe = dir.join(exe_name("Previewer"));
            if exe.is_file() {
                return Some(PreviewerBin { dir, exe });
            }
            // 部分设备类型只有 Simulator，预览取不到时交给调用方降级
            let sim = dir.join(exe_name("Simulator"));
            if sim.is_file() {
                return Some(PreviewerBin { dir, exe: sim });
            }
        }
    }
    None
}

/// 启动引擎所需的全部输入。
pub struct EngineSpec<'a> {
    pub module: &'a str,
    pub page: &'a str,
    pub device: &'a str,
    pub width: u32,
    pub height: u32,
    pub project_id: u32,
    pub sid: &'a str,
    pub port: u16,
    /// 含 modules.abc 的目录
    pub bundle_dir: &'a Path,
    pub loader_json: &'a Path,
    /// `<module>/.preview/<product>/intermediates/res/<product>`
    pub res_dir: &'a Path,
    /// 设备档 json（缺省时省略 `-f`）
    pub device_config: Option<&'a Path>,
    /// 管道名：缺失只会让引擎打印 "Trace pipe is not prepared"，不致命
    pub trace_pipe: &'a str,
}

/// 组装 Previewer 命令行。参数名与取值都被实测验证过（§52）。
pub fn engine_args(spec: &EngineSpec<'_>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-refresh".into(),
        "region".into(),
        "-projectID".into(),
        spec.project_id.to_string(),
        "-ts".into(),
        spec.trace_pipe.to_string(),
        "-j".into(),
        spec.bundle_dir.to_string_lossy().to_string(),
        "-s".into(),
        format!("{}_{}", spec.device, spec.sid),
        "-cpm".into(),
        "false".into(),
        "-device".into(),
        spec.device.to_string(),
        "-shape".into(),
        "rect".into(),
        "-sd".into(),
        "480".into(),
        "-ljPath".into(),
        spec.loader_json.to_string_lossy().to_string(),
        "-sid".into(),
        spec.sid.to_string(),
        "-or".into(),
        spec.width.to_string(),
        spec.height.to_string(),
        "-cr".into(),
        spec.width.to_string(),
        spec.height.to_string(),
        "-url".into(),
        spec.page.to_string(),
        "-av".into(),
        "ACE_2_0".into(),
        "-n".into(),
        spec.module.to_string(),
        "-arp".into(),
        spec.res_dir.to_string_lossy().to_string(),
        "-pm".into(),
        "Stage".into(),
        "-lws".into(),
        spec.port.to_string(),
    ];
    if let Some(cfg) = spec.device_config {
        args.push("-f".into());
        args.push(cfg.to_string_lossy().to_string());
    }
    args
}

/// 会话 id：引擎要求匹配 `^[a-zA-Z0-9]+$`（带连字符的 UUID 会被拒），
/// 同时它又是 WebSocket 的路径，故取 uuid 的无连字符形式。
pub fn gen_sid() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 在引擎允许的区间内挑一个空闲端口：先探测占用，再交给引擎监听。
pub fn pick_port() -> Option<u16> {
    use std::net::TcpListener;
    let span = (PORT_MAX - PORT_MIN) as u32;
    // 每次尝试都要换一个起点：只取时间的话，快速循环里算出的 seed 几乎相同，
    // 等于把同一个端口重试 64 次。
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
        .unwrap_or(0)
        ^ std::process::id();
    for attempt in 0..64u32 {
        let port = PORT_MIN + ((base ^ attempt.wrapping_mul(2654435761)) % span) as u16;
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            drop(listener);
            return Some(port);
        }
    }
    None
}

/// 从一帧里切出 JPEG：头之后应紧跟 SOI（实测固定在偏移 40）。
/// 不依赖固定偏移，按 magic 校验后取第一个 SOI，容错更好。
pub fn jpeg_in_frame(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() <= FRAME_HEADER_LEN || frame[..4] != FRAME_MAGIC {
        return None;
    }
    let rest = &frame[FRAME_HEADER_LEN..];
    if rest.len() > 3 && rest[..3] == [0xFF, 0xD8, 0xFF] {
        return Some(rest);
    }
    // 头长度若有变化，退化为在帧内搜索 SOI
    let idx = frame.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])?;
    Some(&frame[idx..])
}

/// 一帧的解析结果。
#[derive(Debug, PartialEq)]
pub enum WsFrame {
    /// 数据未收全，需要继续读
    Incomplete,
    /// opcode 与载荷
    Data { opcode: u8, payload: Vec<u8> },
    /// 对端关闭
    Close,
}

/// 解析一个服务端发来的 WebSocket 帧（服务端帧不掩码，但按规范兼容掩码位）。
/// 返回的载荷已去掉掩码。
pub fn parse_ws_frame(buf: &[u8]) -> WsFrame {
    if buf.len() < 2 {
        return WsFrame::Incomplete;
    }
    let opcode = buf[0] & 0x0f;
    let masked = buf[1] & 0x80 != 0;
    let len7 = (buf[1] & 0x7f) as usize;
    let (mut offset, payload_len) = match len7 {
        126 => {
            if buf.len() < 4 {
                return WsFrame::Incomplete;
            }
            (4usize, u16::from_be_bytes([buf[2], buf[3]]) as usize)
        }
        127 => {
            if buf.len() < 10 {
                return WsFrame::Incomplete;
            }
            let mut raw = [0u8; 8];
            raw.copy_from_slice(&buf[2..10]);
            (10usize, u64::from_be_bytes(raw) as usize)
        }
        n => (2usize, n),
    };
    if opcode == 0x8 {
        return WsFrame::Close;
    }
    let mut payload = match (masked, buf.len() >= offset + 4) {
        (true, true) => {
            let mask = [buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]];
            offset += 4;
            Some(mask)
        }
        (true, false) => return WsFrame::Incomplete,
        _ => None,
    };
    if buf.len() < offset + payload_len {
        return WsFrame::Incomplete;
    }
    let mut data = buf[offset..offset + payload_len].to_vec();
    if let Some(mask) = payload.take() {
        for (i, byte) in data.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }
    WsFrame::Data {
        opcode,
        payload: data,
    }
}

/// 默认预览页：取模块 `main_pages.json` 的首个页面，省得用户填路径。
pub fn default_page(project_root: &Path, module: &str) -> Option<String> {
    let profile = project_root
        .join(module)
        .join("src")
        .join("main")
        .join("resources")
        .join("base")
        .join("profile")
        .join("main_pages.json");
    let text = std::fs::read_to_string(profile).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value["src"]
        .as_array()?
        .first()?
        .as_str()
        .map(|s| s.trim_start_matches('/').to_string())
}

/// 已构建的预览中间件路径集合。
pub struct PreviewArtifacts {
    pub bundle_dir: PathBuf,
    pub loader_json: PathBuf,
    pub res_dir: PathBuf,
}

/// 在构建产物里定位引擎需要的三个路径。目录顺序按实测的两种布局：
/// 我们自己构建落 `loader_out/`，IDE 构建落 `assets/`。
pub fn locate_artifacts(project_root: &Path, module: &str, product: &str) -> Option<PreviewArtifacts> {
    let base = project_root
        .join(module)
        .join(".preview")
        .join(product)
        .join("intermediates");
    let bundle_dir = [base.join("loader_out").join("default").join("ets"),
                      base.join("assets").join("default").join("ets")]
        .into_iter()
        .find(|d| d.join("modules.abc").is_file())?;
    let loader_json = base.join("loader").join("default").join("loader.json");
    let res_dir = base.join("res").join("default");
    if !loader_json.is_file() || !res_dir.is_dir() {
        return None;
    }
    Some(PreviewArtifacts {
        bundle_dir,
        loader_json,
        res_dir,
    })
}

/// WebSocket 握手用的 Accept 值：base64(sha1(key + GUID))。
fn ws_accept(key: &str) -> String {
    use base64::Engine;
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    ctx.update(key.as_bytes());
    ctx.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(ctx.finish().as_ref())
}

/// 连上引擎的取帧 WebSocket。路径必须是 `/<sid>`（见模块头注释）。
pub async fn ws_connect(port: u16, sid: &str) -> Result<tokio::net::TcpStream, String> {
    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| format!("连接预览引擎失败（端口 {port}）：{e}"))?;
    let key = base64::engine::general_purpose::STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
    let request = format!(
        "GET /{sid} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("发送预览握手失败：{e}"))?;
    let mut head = Vec::new();
    let mut one = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = stream
            .read(&mut one)
            .await
            .map_err(|e| format!("读取预览握手响应失败：{e}"))?;
        if n == 0 {
            return Err("预览引擎在握手阶段关闭了连接（路径或会话 id 不匹配）".into());
        }
        head.push(one[0]);
        if head.len() > 8192 {
            return Err("预览握手响应异常（超长）".into());
        }
    }
    let text = String::from_utf8_lossy(&head);
    let status_ok = text.starts_with("HTTP/1.1 101");
    let accept_ok = text
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("sec-websocket-accept:") && l.contains(&ws_accept(&key)));
    if !status_ok || !accept_ok {
        return Err(format!(
            "预览握手被拒绝：{}",
            text.lines().next().unwrap_or("").trim()
        ));
    }
    Ok(stream)
}

/// 从已连接的流里读下一帧载荷（跳过 ping/pong 等控制帧）。
pub async fn next_payload(
    stream: &mut tokio::net::TcpStream,
    buf: &mut Vec<u8>,
) -> Result<Option<Vec<u8>>, String> {
    use tokio::io::AsyncReadExt;
    loop {
        match parse_ws_frame(buf) {
            WsFrame::Data { opcode, payload } => {
                let total = consumed_len(buf);
                buf.drain(..total);
                if opcode == 0x2 || opcode == 0x1 {
                    return Ok(Some(payload));
                }
                continue;
            }
            WsFrame::Close => return Ok(None),
            WsFrame::Incomplete => {}
        }
        let mut chunk = [0u8; 16 * 1024];
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("读取预览帧失败：{e}"))?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// 一个已解析帧在缓冲区里占用的字节数（供 `next_payload` 前进用）。
fn consumed_len(buf: &[u8]) -> usize {
    if buf.len() < 2 {
        return 0;
    }
    let masked = buf[1] & 0x80 != 0;
    let len7 = (buf[1] & 0x7f) as usize;
    let (mut offset, payload_len) = match len7 {
        126 => (4usize, u16::from_be_bytes([buf[2], buf[3]]) as usize),
        127 => {
            let mut raw = [0u8; 8];
            raw.copy_from_slice(&buf[2..10]);
            (10usize, u64::from_be_bytes(raw) as usize)
        }
        n => (2usize, n),
    };
    if masked {
        offset += 4;
    }
    offset + payload_len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> EngineSpec<'static> {
        EngineSpec {
            module: "entry",
            page: "pages/Index",
            device: "phone",
            width: 1080,
            height: 2340,
            project_id: 123,
            sid: "abc123",
            port: 29999,
            bundle_dir: Path::new(r"C:\p\entry\.preview\default\intermediates\loader_out\default\ets"),
            loader_json: Path::new(r"C:\p\entry\.preview\default\intermediates\loader\default\loader.json"),
            res_dir: Path::new(r"C:\p\entry\.preview\default\intermediates\res\default"),
            device_config: None,
            trace_pipe: "trace_x_commandPipe",
        }
    }

    #[test]
    fn engine_args_carry_the_verified_flags() {
        let args = engine_args(&spec());
        let joined = args.join(" ");
        for needle in [
            "-pm Stage",
            "-av ACE_2_0",
            "-lws 29999",
            "-sid abc123",
            "-or 1080 2340",
            "-cr 1080 2340",
            "-n entry",
            "-url pages/Index",
        ] {
            assert!(joined.contains(needle), "缺少 {needle}：{joined}");
        }
        // 未提供设备档时不带 -f，避免引擎读到不存在的路径
        assert!(!joined.contains("-f "), "不该带 -f：{joined}");
    }

    #[test]
    fn engine_args_include_device_config_when_given() {
        let mut s = spec();
        s.device_config = Some(Path::new(r"C:\d\phoneSettingConfig.json"));
        let joined = engine_args(&s).join(" ");
        assert!(joined.contains(r#"-f C:\d\phoneSettingConfig.json"#), "{joined}");
    }

    #[test]
    fn gen_sid_matches_engine_regex() {
        let sid = gen_sid();
        assert_eq!(sid.len(), 32);
        assert!(
            sid.chars().all(|c| c.is_ascii_alphanumeric()),
            "引擎要求 ^[a-zA-Z0-9]+$，实际 {sid}"
        );
        assert!(!sid.contains('-'), "连字符会被引擎拒绝：{sid}");
    }

    #[test]
    fn pick_port_stays_inside_the_engine_range() {
        let port = pick_port().expect("应能挑到端口");
        assert!(port > 29000 && port < 50000, "端口越界：{port}");
    }

    #[test]
    fn jpeg_in_frame_slices_from_header_end() {
        let mut frame = vec![0x12, 0x34, 0x56, 0x78];
        frame.extend_from_slice(&[0u8; 36]);
        frame.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, 0x01, 0x02]);
        let jpeg = jpeg_in_frame(&frame).expect("应切出 JPEG");
        assert_eq!(jpeg, &[0xFF, 0xD8, 0xFF, 0xE0, 0x01, 0x02]);
    }

    #[test]
    fn jpeg_in_frame_rejects_short_or_wrong_magic() {
        assert!(jpeg_in_frame(&[0u8; 10]).is_none());
        let mut bad = vec![0u8; FRAME_HEADER_LEN];
        bad[..4].copy_from_slice(&[0, 0, 0, 0]);
        bad.extend_from_slice(&[0xFF, 0xD8, 0xFF]);
        assert!(jpeg_in_frame(&bad).is_none());
    }

    #[test]
    fn parse_ws_frame_handles_all_length_forms() {
        // 短帧（<126）
        let short = [0x82, 0x03, 1, 2, 3];
        match parse_ws_frame(&short) {
            WsFrame::Data { opcode, payload } => {
                assert_eq!(opcode, 0x2);
                assert_eq!(payload, vec![1, 2, 3]);
            }
            other => panic!("短帧解析失败：{other:?}"),
        }
        // 126 扩展
        let mut mid = vec![0x82, 126, 0, 4];
        mid.extend_from_slice(&[9, 8, 7, 6]);
        match parse_ws_frame(&mid) {
            WsFrame::Data { payload, .. } => assert_eq!(payload, vec![9, 8, 7, 6]),
            other => panic!("126 帧解析失败：{other:?}"),
        }
        // 127 扩展
        let mut long = vec![0x82, 127];
        long.extend_from_slice(&5u64.to_be_bytes());
        long.extend_from_slice(&[1, 1, 1, 1, 1]);
        match parse_ws_frame(&long) {
            WsFrame::Data { payload, .. } => assert_eq!(payload.len(), 5),
            other => panic!("127 帧解析失败：{other:?}"),
        }
        // 收不全
        assert_eq!(parse_ws_frame(&[0x82]), WsFrame::Incomplete);
        assert_eq!(parse_ws_frame(&[0x82, 0x05, 1, 2]), WsFrame::Incomplete);
        // 关闭帧
        assert_eq!(parse_ws_frame(&[0x88, 0x00]), WsFrame::Close);
    }

    #[test]
    fn parse_ws_frame_unmasks_client_style_frames() {
        let mask = [0x11u8, 0x22, 0x33, 0x44];
        let mut frame = vec![0x82, 0x80 | 3];
        frame.extend_from_slice(&mask);
        for (i, b) in [10u8, 20, 30].iter().enumerate() {
            frame.push(b ^ mask[i % 4]);
        }
        match parse_ws_frame(&frame) {
            WsFrame::Data { payload, .. } => assert_eq!(payload, vec![10, 20, 30]),
            other => panic!("掩码帧解析失败：{other:?}"),
        }
    }

    #[test]
    fn locate_artifacts_prefers_loader_out_then_assets() {
        let root = std::env::temp_dir().join(format!("deveco-prev-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let base = root.join("entry").join(".preview").join("default").join("intermediates");
        for rel in ["loader/default", "res/default", "loader_out/default/ets"] {
            std::fs::create_dir_all(base.join(rel)).unwrap();
        }
        std::fs::write(base.join("loader_out/default/ets/modules.abc"), "x").unwrap();
        std::fs::write(base.join("loader/default/loader.json"), "{}").unwrap();
        let found = locate_artifacts(&root, "entry", "default").expect("应找到产物");
        assert!(found.bundle_dir.ends_with("loader_out/default/ets"));
        // 换成 IDE 布局（assets）也要能找到
        std::fs::remove_dir_all(base.join("loader_out")).unwrap();
        std::fs::create_dir_all(base.join("assets/default/ets")).unwrap();
        std::fs::write(base.join("assets/default/ets/modules.abc"), "x").unwrap();
        let found = locate_artifacts(&root, "entry", "default").expect("应找到 assets 布局");
        assert!(found.bundle_dir.ends_with("assets/default/ets"));
        // 缺 loader.json 时判失败
        std::fs::remove_file(base.join("loader/default/loader.json")).unwrap();
        assert!(locate_artifacts(&root, "entry", "default").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 端到端：对真实工程起一次引擎并把帧收回来。
    /// 需要 `DEVECO_PREVIEW_E2E_PROJECT=<已构建的鸿蒙工程>`；置
    /// `DEVECO_PREVIEW_E2E_SKIP_BUILD=1` 可跳过构建（复用工程里已有的 `.preview` 产物）。
    #[tokio::test]
    #[ignore = "需要本机 DevEco SDK 与已构建工程，按环境变量启用"]
    async fn e2e_preview_frames_from_real_engine() {
        let Ok(project) = std::env::var("DEVECO_PREVIEW_E2E_PROJECT") else {
            eprintln!("skip: 未设置 DEVECO_PREVIEW_E2E_PROJECT");
            return;
        };
        let root = Path::new(&project);
        if !root.is_dir() {
            eprintln!("skip: 工程不存在 {project}");
            return;
        }
        let env = crate::services::harmony_env::detect_auto();
        let Some(bin) = locate(&env) else {
            eprintln!("skip: 未找到 Previewer");
            return;
        };
        let module = "entry";
        let product = "default";
        if !std::env::var("DEVECO_PREVIEW_E2E_SKIP_BUILD").is_ok_and(|v| v == "1") {
            let hvigor = crate::services::harmony::hvigor_command(root).expect("hvigor 应可用");
            let mut args = hvigor.args.clone();
            args.extend([
                "--mode".to_string(),
                "module".to_string(),
                "-p".to_string(),
                format!("module={module}@default"),
                "-p".to_string(),
                format!("product={product}"),
                "-p".to_string(),
                "buildRoot=.preview".to_string(),
                "PreviewBuild".to_string(),
            ]);
            let mut cmd = crate::utils::process::command(&hvigor.program, &args).unwrap();
            cmd.current_dir(root);
            if !hvigor.env.is_empty() {
                cmd.envs(hvigor.env.iter().cloned());
            }
            let out = cmd.output().await.expect("构建应能启动");
            assert!(
                out.status.success(),
                "预览构建失败：{}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
        let artifacts = locate_artifacts(root, module, product).expect("应有预览产物");
        let page = default_page(root, module).unwrap_or_else(|| "pages/Index".to_string());
        let port = pick_port().expect("应能挑到端口");
        let sid = gen_sid();
        let spec = EngineSpec {
            module,
            page: &page,
            device: "phone",
            width: 1080,
            height: 2340,
            project_id: 1,
            sid: &sid,
            port,
            bundle_dir: &artifacts.bundle_dir,
            loader_json: &artifacts.loader_json,
            res_dir: &artifacts.res_dir,
            device_config: None,
            trace_pipe: "deveco_switch_e2e_commandPipe",
        };
        let mut cmd = tokio::process::Command::new(&bin.exe);
        cmd.args(engine_args(&spec))
            .current_dir(&bin.dir)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let _child = cmd.spawn().expect("引擎应能启动");

        let mut stream = None;
        for _ in 0..40 {
            if let Ok(s) = ws_connect(port, &sid).await {
                stream = Some(s);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let mut stream = stream.expect("应能连上引擎的取帧 WebSocket");
        let mut buf: Vec<u8> = Vec::new();
        let jpeg = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while let Some(frame) = next_payload(&mut stream, &mut buf).await.unwrap() {
                if let Some(jpeg) = jpeg_in_frame(&frame) {
                    return jpeg.to_vec();
                }
            }
            panic!("连接结束但没收到带 JPEG 的帧");
        })
        .await
        .expect("30 秒内应收到帧");
        assert!(jpeg.len() > 1024, "JPEG 过小：{}", jpeg.len());
        assert_eq!(&jpeg[..3], &[0xFF, 0xD8, 0xFF], "应是 JPEG SOI");
    }

    #[test]
    fn default_page_reads_first_entry_of_main_pages() {
        let root = std::env::temp_dir().join(format!("deveco-prev-page-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("entry/src/main/resources/base/profile");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("main_pages.json"),
            r#"{"src":["/pages/Second","pages/Index"]}"#,
        )
        .unwrap();
        assert_eq!(
            default_page(&root, "entry").as_deref(),
            Some("pages/Second")
        );
        assert!(default_page(&root, "missing").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
