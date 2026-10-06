//! ONNX Runtime 会话管理与推理内核。
//!
//! ## 分发方式
//!
//! 使用 `ort` 的 `download-binaries`：ONNX Runtime 原生库在**构建时**取回并
//! 静态链接进可执行文件。因此最终用户**不需要另行下载推理内核** ——
//! 安装包里就带着。需要用户下载的只有模型文件（体积大，见 `model_manager`）。
//!
//! ## 惰性加载
//!
//! 会话创建有一定开销（读盘 + 图优化），因此按需创建并缓存：
//! 启动时不加载任何模型（策划书 2.3），首次用到某个模型时才建立会话。

use crate::error::{AppError, Result};
use ort::session::Session;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// 推理内核：按模型文件路径缓存会话。
pub struct Inference {
    /// 已创建的会话（键 = 模型绝对路径）。
    sessions: Mutex<HashMap<String, Session>>,
}

impl Default for Inference {
    fn default() -> Self {
        Self::new()
    }
}

impl Inference {
    /// 创建一个空的内核（不加载任何模型）。
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 取得（必要时创建）某个模型的会话。
    ///
    /// 用闭包而不是返回 `&Session` 是为了避免把 `MutexGuard` 暴露出去。
    pub fn with_session<T>(
        &self,
        model_path: &Path,
        f: impl FnOnce(&mut Session) -> Result<T>,
    ) -> Result<T> {
        if !model_path.is_file() {
            return Err(AppError::NotFound(format!(
                "模型文件不存在: {}",
                model_path.display()
            )));
        }
        let key = model_path.to_string_lossy().to_string();
        let mut map = self
            .sessions
            .lock()
            .map_err(|_| AppError::Audio("推理内核状态锁已损坏".into()))?;

        if !map.contains_key(&key) {
            let session = Session::builder()
                .map_err(|e| AppError::Audio(format!("创建 ONNX 会话失败: {e}")))?
                // 桌面场景下并行度不需要很高，限制线程数避免抢 UI 的 CPU
                .with_intra_threads(2)
                .map_err(|e| AppError::Audio(format!("配置 ONNX 线程失败: {e}")))?
                .commit_from_file(model_path)
                .map_err(|e| AppError::Audio(format!("加载模型失败: {e}")))?;
            log::info!("已加载 ONNX 模型: {}", model_path.display());
            map.insert(key.clone(), session);
        }

        let session = map
            .get_mut(&key)
            .ok_or_else(|| AppError::Audio("会话缓存异常".into()))?;
        f(session)
    }

    /// 卸载所有会话，释放内存。
    pub fn unload_all(&self) {
        if let Ok(mut map) = self.sessions.lock() {
            map.clear();
        }
    }

    /// 卸载指定模型。
    pub fn unload(&self, model_path: &Path) {
        if let Ok(mut map) = self.sessions.lock() {
            map.remove(&model_path.to_string_lossy().to_string());
        }
    }

    /// 已缓存的会话数量。
    pub fn loaded_count(&self) -> usize {
        self.sessions.lock().map(|m| m.len()).unwrap_or(0)
    }
}

/// 进程级推理内核。
///
/// 后台索引线程与命令层都要用推理，但线程各自持有连接、不方便互相传引用，
/// 因此这里提供一个进程级实例：**惰性初始化**，只有第一次真正推理时才创建，
/// 会话在其中按模型路径缓存，多个线程安全共享。
pub fn global() -> &'static Inference {
    static INSTANCE: std::sync::OnceLock<Inference> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(Inference::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn new_inference_has_no_sessions() {
        let inf = Inference::new();
        assert_eq!(inf.loaded_count(), 0, "新建实例不应加载任何模型");
    }

    /// `global()` 必须是同一个实例（会话缓存在其中）。
    ///
    /// 注意**不能**断言「会话数为 0」：`global()` 是进程级共享的，
    /// 同一测试进程里其它测试可能已经加载过模型。
    /// 这条测试只锁定「同一实例」这个契约。
    #[test]
    fn global_is_stable_across_calls() {
        let a = global();
        let b = global();
        assert!(std::ptr::eq(a, b), "global() 应始终返回同一实例");
        // 惰性：新建的实例不带会话
        assert_eq!(Inference::new().loaded_count(), 0);
    }

    #[test]
    fn inference_is_shareable_across_threads() {
        // 编译期断言：AppState / 后台线程要求 Send + Sync
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Inference>();

        // 运行期：多线程同时访问不 panic
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    let _ = global().loaded_count();
                });
            }
        });
    }

    #[test]
    fn missing_model_reports_readable_error() {
        let inf = Inference::new();
        let p = PathBuf::from("C:/definitely/not/here/model.onnx");
        let r = inf.with_session(&p, |_| Ok(()));
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("模型文件不存在"), "{msg}");
        assert_eq!(inf.loaded_count(), 0, "失败不应留下缓存条目");
    }

    #[test]
    fn invalid_onnx_file_reports_error_without_panic() {
        let mut p = std::env::temp_dir();
        p.push(format!("aam_bad_{}.onnx", std::process::id()));
        std::fs::write(&p, b"this is not an onnx model").unwrap();

        let inf = Inference::new();
        let r = inf.with_session(&p, |_| Ok(()));
        assert!(r.is_err(), "损坏的模型文件应报错");
        assert!(
            r.unwrap_err().to_string().contains("加载模型失败"),
            "错误信息应说明是加载失败"
        );
        assert_eq!(inf.loaded_count(), 0);

        std::fs::remove_file(&p).ok();
    }
}
