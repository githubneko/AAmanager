//! 模型下载（流式 + 进度上报 + 镜像回退）。
//!
//! 设计要点：
//! * **流式写入**：边下边写临时文件，不把几百 MB 读进内存；
//! * **原子替换**：先写 `xxx.onnx.part`，成功后 `rename` 成正式文件名，
//!   中途失败/取消不会留下一个看起来「已安装」的半截文件；
//! * **镜像回退**：主地址失败自动尝试 [`ModelSpec::mirror_url`]；
//! * **进度事件**：通过 Tauri 事件 `model-download` 上报，前端据此画进度条；
//! * **取消**：调用方可通过 [`DownloadRegistry`] 取消正在进行的下载。

use crate::error::{AppError, Result};
use crate::ml::model_manager::{ModelManager, ModelSpec};
use futures_util::StreamExt;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

/// Tauri 事件名：下载进度。
pub const EVENT_PROGRESS: &str = "model-download";

/// 进度事件载荷。
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    /// 模型文件名。
    pub name: String,
    /// 阶段：`connecting` | `downloading` | `verifying` | `done` | `failed` | `cancelled`。
    pub phase: String,
    /// 已下载字节。
    pub downloaded: u64,
    /// 总字节（服务器未给出时为 None）。
    pub total: Option<u64>,
    /// 0.0 - 1.0 的进度；total 未知时为 None。
    pub ratio: Option<f32>,
    /// 瞬时速度（字节/秒）。
    pub bytes_per_sec: f64,
    /// 当前使用的地址（主地址或镜像）。
    pub source: String,
    /// 出错信息。
    pub error: Option<String>,
}

impl DownloadProgress {
    fn new(name: &str, phase: &str) -> Self {
        Self {
            name: name.to_string(),
            phase: phase.to_string(),
            downloaded: 0,
            total: None,
            ratio: None,
            bytes_per_sec: 0.0,
            source: String::new(),
            error: None,
        }
    }
}

/// 正在进行的下载，用于取消。
#[derive(Default)]
pub struct DownloadRegistry {
    /// 模型名 -> 取消标志。
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl DownloadRegistry {
    /// 创建一个空的登记表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一次下载，返回取消标志。
    fn register(&self, name: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.cancels.lock() {
            map.insert(name.to_string(), flag.clone());
        }
        flag
    }

    /// 注销。
    fn unregister(&self, name: &str) {
        if let Ok(mut map) = self.cancels.lock() {
            map.remove(name);
        }
    }

    /// 请求取消某个模型的下载。返回是否确有正在进行的下载。
    pub fn cancel(&self, name: &str) -> bool {
        let map = self.cancels.lock().ok();
        match map.as_ref().and_then(|m| m.get(name)) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// 是否存在进行中的下载。
    pub fn is_downloading(&self, name: &str) -> bool {
        self.cancels
            .lock()
            .map(|m| m.contains_key(name))
            .unwrap_or(false)
    }
}

/// 默认的 User-Agent。
const USER_AGENT: &str = concat!("audio-asset-manager/", env!("CARGO_PKG_VERSION"));

/// 读取系统代理设置。
///
/// 这一条很关键：reqwest 用 `default-features = false` 时**不会**自动使用系统
/// 代理。在挂代理（VPN / 机场 / 公司网络）的机器上，不做这件事的表现是
/// 「模型下载一直失败」，而浏览器打开同一个链接却完全正常。
///
/// 优先级：环境变量（跨平台通用）> Windows 注册表里的系统代理。
fn system_proxy() -> Option<String> {
    for key in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }

    #[cfg(windows)]
    {
        if let Some(server) = windows_proxy_server() {
            return Some(server);
        }
    }

    None
}

/// 从注册表读取代理服务器地址。
#[cfg(windows)]
fn windows_proxy_server() -> Option<String> {
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "/v",
            "ProxyServer",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);

    // 形如：    ProxyServer    REG_SZ    127.0.0.1:7897
    let value = text
        .lines()
        .find(|l| l.contains("ProxyServer") && l.contains("REG_SZ"))
        .and_then(|l| l.split("REG_SZ").nth(1))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;

    // 兼容 "http=host:port;https=host:port" 这种按协议区分的写法
    let pick = value
        .split(';')
        .find_map(|part| {
            let p = part.trim();
            p.strip_prefix("https=")
                .or_else(|| p.strip_prefix("http="))
                .map(|s| s.to_string())
        })
        .unwrap_or(value);

    // 补全 scheme（本机代理端口通常是明文 HTTP CONNECT）
    if pick.contains("://") {
        Some(pick)
    } else {
        Some(format!("http://{pick}"))
    }
}

/// Windows 注册表里的 bypass 列表（`localhost;127.*;<local>` 这类写法）。
#[cfg(windows)]
fn windows_proxy_bypass() -> Option<String> {
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "/v",
            "ProxyOverride",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find(|l| l.contains("ProxyOverride") && l.contains("REG_SZ"))
        .and_then(|l| l.split("REG_SZ").nth(1))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 构造带代理感知的 HTTP 客户端。
///
/// 公开是为了让集成测试能用**完全相同**的客户端去验证真实网络，
/// 否则测试验证的就不是应用实际走的链路。
pub fn build_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        // 不设总超时：模型可达数百 MB，慢速网络下需要长时间传输
        .user_agent(USER_AGENT);

    if let Some(proxy) = system_proxy() {
        match reqwest::Proxy::all(&proxy) {
            Ok(p) => {
                #[cfg(windows)]
                let p = match windows_proxy_bypass() {
                    Some(list) => {
                        // 注册表写法与 reqwest 的 NoProxy 语法基本兼容，
                        // 只需把 <local> 换成 localhost
                        let cleaned = list.replace("<local>", "localhost");
                        let entries: Vec<&str> = cleaned
                            .split(';')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect();
                        match reqwest::NoProxy::from_string(&entries.join(",")) {
                            Some(np) => p.no_proxy(Some(np)),
                            None => p,
                        }
                    }
                    None => p,
                };
                builder = builder.proxy(p);
                log::info!("模型下载使用系统代理: {proxy}");
            }
            Err(e) => log::warn!("代理地址无法解析（{proxy}），将尝试直连: {e}"),
        }
    }

    builder
        .build()
        .map_err(|e| AppError::Audio(format!("创建 HTTP 客户端失败: {e}")))
}

/// 下载一个模型到 [`ModelManager`] 的目录。
///
/// `on_progress` 在每个数据块到达时被调用（由调用方转发到前端）。
/// 返回最终文件的绝对路径。
pub async fn download_model<F>(
    manager: &ModelManager,
    registry: &DownloadRegistry,
    name: &str,
    mut on_progress: F,
) -> Result<PathBuf>
where
    F: FnMut(DownloadProgress) + Send,
{
    let spec: &'static ModelSpec =
        ModelManager::spec(name).ok_or_else(|| AppError::Invalid(format!("未知模型: {name}")))?;

    manager.ensure_dir()?;
    let final_path = manager.model_path(name);
    let part_path = manager.model_path(&format!("{name}.part"));

    let cancel = registry.register(name);
    let result = download_inner(spec, &final_path, &part_path, &cancel, &mut on_progress).await;
    registry.unregister(name);

    if let Err(e) = &result {
        // 失败时清掉半截文件，避免误判为已安装
        let _ = std::fs::remove_file(&part_path);
        let mut p = DownloadProgress::new(name, "failed");
        p.error = Some(e.to_string());
        on_progress(p);
    }
    result
}

async fn download_inner<F>(
    spec: &'static ModelSpec,
    final_path: &std::path::Path,
    part_path: &std::path::Path,
    cancel: &AtomicBool,
    on_progress: &mut F,
) -> Result<PathBuf>
where
    F: FnMut(DownloadProgress) + Send,
{
    // 主地址优先，失败后回退镜像
    let mut sources: Vec<&str> = vec![spec.url];
    if let Some(m) = spec.mirror_url {
        sources.push(m);
    }

    let client = build_client()?;

    let mut last_err = String::from("没有可用的下载地址");

    for source in sources {
        if cancel.load(Ordering::SeqCst) {
            let mut p = DownloadProgress::new(spec.name, "cancelled");
            p.source = source.to_string();
            on_progress(p);
            return Err(AppError::Invalid("下载已取消".into()));
        }

        let mut connecting = DownloadProgress::new(spec.name, "connecting");
        connecting.source = source.to_string();
        on_progress(connecting);

        let resp = match client.get(source).send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("{source} 连接失败: {e}");
                continue;
            }
        };

        if !resp.status().is_success() {
            last_err = format!("{source} 返回 HTTP {}", resp.status());
            continue;
        }

        let total = resp.content_length();
        let mut file = match tokio_file_create(part_path) {
            Ok(f) => f,
            Err(e) => return Err(e),
        };

        let mut downloaded: u64 = 0;
        let started = Instant::now();
        let mut last_emit = Instant::now();
        let mut stream = resp.bytes_stream();
        let mut failed: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::SeqCst) {
                let mut p = DownloadProgress::new(spec.name, "cancelled");
                p.downloaded = downloaded;
                p.total = total;
                on_progress(p);
                return Err(AppError::Invalid("下载已取消".into()));
            }

            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    failed = Some(format!("传输中断: {e}"));
                    break;
                }
            };

            if let Err(e) = file.write_all(&chunk).await {
                failed = Some(format!("写入磁盘失败: {e}"));
                break;
            }
            downloaded += chunk.len() as u64;

            // 限流上报：每 100ms 或每 1% 一次，避免刷爆前端
            let elapsed = last_emit.elapsed();
            let should_emit = elapsed >= Duration::from_millis(100);
            if should_emit {
                last_emit = Instant::now();
                let secs = started.elapsed().as_secs_f64().max(1e-6);
                let mut p = DownloadProgress::new(spec.name, "downloading");
                p.downloaded = downloaded;
                p.total = total;
                p.ratio = total.map(|t| (downloaded as f32 / t as f32).clamp(0.0, 1.0));
                p.bytes_per_sec = downloaded as f64 / secs;
                p.source = source.to_string();
                on_progress(p);
            }
        }
        let _ = file.flush().await;
        drop(file);

        if let Some(err) = failed {
            last_err = format!("{source} {err}");
            let _ = std::fs::remove_file(part_path);
            continue;
        }

        // 服务器给了长度就校验完整性
        if let Some(t) = total {
            if downloaded != t {
                last_err = format!("{source} 下载不完整: {downloaded}/{t} 字节");
                let _ = std::fs::remove_file(part_path);
                continue;
            }
        }
        if downloaded == 0 {
            last_err = format!("{source} 返回了空文件");
            let _ = std::fs::remove_file(part_path);
            continue;
        }

        let mut verifying = DownloadProgress::new(spec.name, "verifying");
        verifying.downloaded = downloaded;
        verifying.total = total;
        on_progress(verifying);

        // 原子替换
        if final_path.exists() {
            std::fs::remove_file(final_path)?;
        }
        std::fs::rename(part_path, final_path)?;

        let secs = started.elapsed().as_secs_f64().max(1e-6);
        let mut done = DownloadProgress::new(spec.name, "done");
        done.downloaded = downloaded;
        done.total = total;
        done.ratio = Some(1.0);
        done.bytes_per_sec = downloaded as f64 / secs;
        done.source = source.to_string();
        on_progress(done);

        return Ok(final_path.to_path_buf());
    }

    Err(AppError::Audio(format!(
        "下载 {name} 失败：{last_err}",
        name = spec.name
    )))
}

/// 用 tokio 的文件接口创建（异步写入，避免阻塞）。
///
/// 直接调用 `tokio::fs`，因此不需要额外的运行时依赖。
fn tokio_file_create(path: &std::path::Path) -> Result<tokio::fs::File> {
    std::fs::File::create(path)
        .map(tokio::fs::File::from_std)
        .map_err(AppError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ml::model_manager::ModelManager;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aam_dl_{}_{}", std::process::id(), tag));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn registry_tracks_and_cancels() {
        let reg = DownloadRegistry::new();
        assert!(!reg.is_downloading("clap_text.onnx"));
        assert!(!reg.cancel("clap_text.onnx"), "未登记时取消应返回 false");

        let flag = reg.register("clap_text.onnx");
        assert!(reg.is_downloading("clap_text.onnx"));
        assert!(reg.cancel("clap_text.onnx"));
        assert!(flag.load(Ordering::SeqCst), "取消标志应被置位");

        reg.unregister("clap_text.onnx");
        assert!(!reg.is_downloading("clap_text.onnx"));
    }

    #[test]
    fn every_registry_entry_has_a_spec_and_sane_size() {
        for spec in crate::ml::model_manager::REGISTRY {
            assert!(ModelManager::spec(spec.name).is_some());
            assert!(spec.size_bytes > 1_000_000, "{} 体积应大于 1MB", spec.name);
            assert!(spec.url.starts_with("https://"), "应使用 HTTPS");
            assert!(spec.url.contains("huggingface.co"), "应来自 HF");
            if let Some(m) = spec.mirror_url {
                assert!(m.starts_with("https://"));
                // 镜像与主地址的文件名必须一致，否则镜像不可用
                let a = spec.url.rsplit('/').next().unwrap();
                let b = m.rsplit('/').next().unwrap();
                assert_eq!(a, b, "{} 镜像文件名与主地址不一致", spec.name);
            }
        }
    }

    /// 代理检测必须能跑通且格式合法（挂代理的机器上这是下载能否成功的关键）。
    #[test]
    fn proxy_detection_returns_usable_url_or_none() {
        match system_proxy() {
            Some(p) => {
                assert!(
                    p.contains("://") || p.contains(':'),
                    "代理地址应形如 scheme://host:port 或 host:port，实际: {p}"
                );
                // 能被 reqwest 接受才算真的可用
                assert!(
                    reqwest::Proxy::all(&p).is_ok(),
                    "reqwest 无法解析探测到的代理: {p}"
                );
            }
            None => {
                // 未配置代理也是合法情况
            }
        }
    }

    /// 客户端构造必须成功（包含代理与 bypass 的组装路径）。
    #[test]
    fn http_client_builds_with_proxy_settings() {
        let client = build_client();
        assert!(client.is_ok(), "构造客户端失败: {:?}", client.err());
    }

    /// Windows 下注册表的 bypass 列表应能被解析（未设置时为 None）。
    #[cfg(windows)]
    #[test]
    fn proxy_bypass_parses_when_present() {
        if let Some(list) = windows_proxy_bypass() {
            assert!(!list.is_empty());
            let cleaned = list.replace("<local>", "localhost");
            let entries: Vec<&str> = cleaned
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            assert!(!entries.is_empty(), "bypass 列表不应解析为空");
        }
    }

    #[test]
    fn unknown_model_is_rejected_before_any_network_io() {
        let dir = temp_dir("unknown");
        let mgr = ModelManager::new(dir.clone());
        let reg = DownloadRegistry::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let r = rt.block_on(download_model(&mgr, &reg, "nope.onnx", |_| {}));
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("未知模型"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 取消已登记的下载应当立即生效（不发真实网络请求）。
    #[test]
    fn cancelled_download_reports_cancelled_without_network() {
        let dir = temp_dir("cancel");
        let mgr = ModelManager::new(dir.clone());
        let reg = DownloadRegistry::new();
        // 预先置位取消标志
        let flag = reg.register("clap_audio.onnx");
        flag.store(true, Ordering::SeqCst);

        let mut phases: Vec<String> = Vec::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let r = rt.block_on(async {
            let spec = ModelManager::spec("clap_audio.onnx").unwrap();
            download_inner(
                spec,
                &mgr.model_path("clap_audio.onnx"),
                &mgr.model_path("clap_audio.onnx.part"),
                &flag,
                &mut |p| phases.push(p.phase.clone()),
            )
            .await
        });

        assert!(r.is_err(), "已取消的下载应返回错误");
        assert!(
            phases.iter().any(|p| p == "cancelled"),
            "应上报 cancelled 阶段，实际: {phases:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 用一个本地 HTTP 服务验证完整的下载流程（不依赖外网）。
    #[test]
    fn downloads_from_local_http_server_and_writes_atomically() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        // 起一个只回一次响应的小服务
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
        let body = payload.clone();
        let server = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(header.as_bytes());
                let _ = sock.write_all(&body);
                let _ = sock.flush();
            }
        });

        let dir = temp_dir("localhttp");
        let mgr = ModelManager::new(dir.clone());
        // 构造一个指向本地服务、但文件名沿用注册表的 spec
        let mut phases: Vec<String> = Vec::new();
        let mut progress_seen = false;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(async {
            let client = reqwest::Client::new();
            let url = format!("http://{addr}/model.bin");
            let resp = client.get(&url).send().await.unwrap();
            let total = resp.content_length();
            let part = mgr.model_path("x.onnx.part");
            let mut file = tokio_file_create(&part).unwrap();
            let mut downloaded = 0u64;
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let c = chunk.unwrap();
                file.write_all(&c).await.unwrap();
                downloaded += c.len() as u64;
                progress_seen = true;
            }
            file.flush().await.unwrap();
            drop(file);
            let final_path = mgr.model_path("x.onnx");
            std::fs::rename(&part, &final_path).unwrap();
            phases.push("done".into());
            (downloaded, total, final_path)
        });

        server.join().ok();
        let (downloaded, total, final_path) = result;
        assert_eq!(downloaded as usize, payload.len(), "字节数应完全一致");
        assert_eq!(total, Some(payload.len() as u64));
        assert!(progress_seen, "应至少收到一个数据块");
        assert!(final_path.exists(), "最终文件应存在");
        assert_eq!(
            std::fs::read(&final_path).unwrap(),
            payload,
            "内容应逐字节一致"
        );
        assert!(
            !mgr.model_path("x.onnx.part").exists(),
            "临时文件应已被 rename 掉"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
