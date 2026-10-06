//! Tauri 命令层：前端通过 `invoke` 调用这里的函数。
//!
//! 命名约定与前端 `src/lib/api.ts` 一一对应。

pub mod audio;
#[cfg(debug_assertions)]
pub mod debug;
pub mod notes;
pub mod search;
pub mod settings;
pub mod state;
pub mod tags;

pub use state::AppState;
