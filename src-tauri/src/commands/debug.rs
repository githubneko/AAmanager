//! 开发期诊断命令（仅在 debug 构建中存在）。
//!
//! 生产构建里整个模块都不会被编译，因此不会增加体积，也不会暴露额外接口。
//!
//! 用途：自动化验证时，前端把「渲染结果自述」回传给后端落盘，
//! 这样无需依赖 WebView2 的远程调试端口就能确认界面真的渲染出来了。

use serde::Deserialize;
use tauri::Manager;

/// 前端上报的渲染自述。
#[derive(Debug, Clone, Deserialize)]
pub struct UiReport {
    /// 是否已挂载根节点。
    pub mounted: bool,
    /// 页面标题。
    pub title: String,
    /// 列表渲染出的行数。
    pub rows: usize,
    /// 空列表引导是否出现。
    pub empty_hint: bool,
    /// 搜索框是否存在。
    pub has_search: bool,
    /// 筛选面板是否存在。
    pub has_filters: bool,
    /// 工具条按钮文案是否齐全。
    pub has_toolbar: bool,
    /// 初始化是否成功（false 时 `error` 有值）。
    pub init_ok: bool,
    /// 初始化错误信息。
    pub error: Option<String>,
    /// 库内音频总数（来自后端）。
    pub total: u32,
    /// 搜索模式。
    pub search_mode: String,
    /// 运行模式。
    pub app_mode: String,
}

/// 把前端自述写入 `%APPDATA%/audio-manager/ui-report.json`，便于外部脚本断言。
#[tauri::command]
pub fn debug_report_ui(app: tauri::AppHandle, report: UiReport) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建数据目录: {e}"))?;
    let path = dir.join("ui-report.json");

    let json = serde_json::json!({
        "mounted": report.mounted,
        "title": report.title,
        "rows": report.rows,
        "empty_hint": report.empty_hint,
        "has_search": report.has_search,
        "has_filters": report.has_filters,
        "has_toolbar": report.has_toolbar,
        "init_ok": report.init_ok,
        "error": report.error,
        "total": report.total,
        "search_mode": report.search_mode,
        "app_mode": report.app_mode,
    });
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(&path, &text).map_err(|e| format!("写入失败: {e}"))?;

    eprintln!("[INFO] UI 自述已写入 {}", path.display());
    Ok(path.to_string_lossy().to_string())
}

/// 读取 `AAM_AUTO_IMPORT` 环境变量指定的目录。
///
/// 端到端验证脚本用它驱动「真实导入」，从而在无人值守的情况下跑通
/// 「导入 → 后台分析 → 列表渲染」整条链路。未设置时返回 `None`。
#[tauri::command]
pub fn debug_auto_import_dir() -> Option<String> {
    match std::env::var("AAM_AUTO_IMPORT") {
        Ok(v) if !v.trim().is_empty() && std::path::Path::new(&v).is_dir() => {
            eprintln!("[INFO] 自动导入目录: {v}");
            Some(v)
        }
        _ => None,
    }
}

/// 是否要求前端启动后自动聚焦搜索框。
///
/// 设置 `AAM_PROBE_FOCUS=1` 时启用；用于在自动化验证中把
/// 「键盘输入是否可用」与「鼠标点击是否命中」两个问题分开定位。
#[tauri::command]
pub fn debug_should_focus_search() -> bool {
    std::env::var("AAM_PROBE_FOCUS")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 前端注册的「页面状态快照」回调名（仅 debug 构建使用）。
pub const PAGE_STATE_FN: &str = "__aam_page_state";

/// 前端把页面状态快照写入 `page-state.json`。
///
/// 用真实键盘输入驱动界面时，这是确认「按键是否真的进入了输入框」的唯一可信证据：
/// 前端把 `input.value`、`document.activeElement` 与 store 里的查询串一并回报。
/// 开发期由前端定时调用，外部脚本只需读取该文件。
#[tauri::command]
pub fn debug_write_page_state(
    app: tauri::AppHandle,
    state: serde_json::Value,
) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建数据目录: {e}"))?;
    let path = dir.join("page-state.json");
    let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    std::fs::write(&path, &text).map_err(|e| format!("写入失败: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// 读取并消费外部脚本投递的命令。
///
/// 这台机器上 WebView2 不允许开启远程调试端口，因此无法从外部直接求值 JS。
/// 改为「文件队列」：脚本把命令写进 `cmd-queue.json`，页面轮询取走并执行，
/// 执行结果再由页面写回 `cmd-result.json`。仅在 debug 构建中存在。
///
/// 返回待执行命令的列表；读取后立即清空队列（原子地 rename 掉文件）。
#[tauri::command]
pub fn debug_take_commands(app: tauri::AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?;
    let queue = dir.join("cmd-queue.json");
    if !queue.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&queue).map_err(|e| format!("读取命令失败: {e}"))?;
    // 先删除再解析：避免同一条命令被执行两次
    let _ = std::fs::remove_file(&queue);

    match serde_json::from_str::<Vec<serde_json::Value>>(&text) {
        Ok(v) => Ok(v),
        Err(_) => {
            // 也接受单条命令对象
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => Ok(vec![v]),
                Err(e) => Err(format!("命令格式错误: {e}")),
            }
        }
    }
}

/// 页面把命令执行结果写回 `cmd-result.json`。
#[tauri::command]
pub fn debug_write_command_result(
    app: tauri::AppHandle,
    result: serde_json::Value,
) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建数据目录: {e}"))?;
    let path = dir.join("cmd-result.json");
    let text = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
    std::fs::write(&path, &text).map_err(|e| format!("写入失败: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}
