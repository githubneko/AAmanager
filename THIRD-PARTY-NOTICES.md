# 第三方声明 / Third-Party Notices

AAmanager 自身以 **MIT** 许可发布（见 [`LICENSE`](LICENSE)）。
本文件汇总**随安装包分发**或**运行时下载**的第三方组件及其许可，
以满足这些许可的署名要求。

> 生成方式：`Cargo.lock` 共锁定 624 个 crate 包（含传递依赖），
> 下方只列出**直接依赖**与**随包分发的二进制组件**；
> 其余全部为 MIT / Apache-2.0 / BSD 系宽松许可，无 copyleft 传染。

---

## 随安装包分发的组件

### Microsoft Visual C++ 运行库（`src-tauri/runtime/`）

应用依赖 4 个 VC++ 运行库 DLL（由静态链接的 ONNX Runtime 间接引入）：

| 文件 | 来源 |
| --- | --- |
| `msvcp140.dll` | Microsoft Visual C++ Redistributable |
| `msvcp140_1.dll` | 同上 |
| `vcruntime140.dll` | 同上 |
| `vcruntime140_1.dll` | 同上 |

分发依据：Microsoft 的
[Visual Studio 可再分发代码许可](https://visualstudio.microsoft.com/license-terms/mlt031424/)
允许随应用程序一同再分发这些运行库。

### ONNX Runtime

- **许可**：MIT — https://github.com/microsoft/onnxruntime/blob/main/LICENSE
- **用途**：ONNX 模型推理内核
- **分发形式**：由 `ort` crate 在**构建时**取回预编译库并**静态链接**进可执行文件
  （因此最终用户无需另行安装推理运行时）

即使用户不下载任何模型，推理内核本身也已包含在安装包内。

### SQLite

- **许可**：**Public Domain** — https://www.sqlite.org/copyright.html
- **用途**：本地资料库存储
- **分发形式**：由 `rusqlite` 的 `bundled` 特性静态编译进可执行文件

SQLite 官方声明：*"SQLite is in the Public Domain … does not require a license."*

### WebView2 Runtime

- **许可**：Microsoft 软件许可条款
- **用途**：界面渲染引擎（Tauri 在 Windows 上的 WebView）
- **分发形式**：**不随安装包分发**。安装程序使用 `embedBootstrapper` 模式 ——
  安装包内嵌微软的引导器，由它在目标机器上安装 WebView2。
  Windows 11 已预装，无需下载。

---

## 运行时下载的模型（不在仓库、不在安装包内）

模型体积较大（合计约 625 MB），由用户按需下载。**各自的许可归各自**，
再分发时请自行核对：

| 模型 | 上游 | 许可 |
| --- | --- | --- |
| CLAP 音频塔 / 文本塔 | [Xenova/clap-htsat-unfused](https://huggingface.co/Xenova/clap-htsat-unfused) | 上游为 [LAION-AI/CLAP](https://github.com/LAION-AI/CLAP) → **CC0 1.0**（公共领域贡献） |
| AST 音频事件分类 | [Xenova/ast-finetuned-audioset-10-10-0.4593](https://huggingface.co/Xenova/ast-finetuned-audioset-10-10-0.4593) | 上游为 [YuanGongND/ast](https://github.com/YuanGongND/ast) → **BSD 3-Clause** |

> **注意**：AST 为 **BSD 3-Clause**，该许可要求再分发时附带许可全文与版权声明。
> 本应用**不打包**模型，用户是从上游仓库直接下载的，因此该义务落在上游；
> 但如果你要**把模型打包进自己的发行版**，必须一并附上 BSD 3-Clause 全文
> 与 `Copyright (c) 2021, Yuan Gong`。

---

## Rust 直接依赖

| Crate | 用途 | 许可 |
| --- | --- | --- |
| `tauri` / `tauri-build` / `tauri-plugin-*` | 桌面应用框架与插件 | MIT OR Apache-2.0 |
| `ort` | ONNX Runtime 绑定 | MIT OR Apache-2.0 |
| `rusqlite` | SQLite 绑定 | MIT |
| `symphonia` | 音频解码（WAV/MP3/OGG/FLAC/M4A 等） | MPL-2.0 |
| `rubato` | 重采样 | MIT |
| `rustfft` | FFT | MIT OR Apache-2.0 |
| `hound` | WAV 读写（测试用） | Apache-2.0 |
| `rodio` | 音频播放 | MIT OR Apache-2.0 |
| `tokenizers` | RoBERTa BPE 分词 | Apache-2.0 |
| `reqwest` | HTTP 下载 | MIT OR Apache-2.0 |
| `tokio` | 异步运行时 | MIT |
| `futures-util` | 异步工具 | MIT OR Apache-2.0 |
| `serde` / `serde_json` | 序列化 | MIT OR Apache-2.0 |
| `ndarray` | 张量 | MIT OR Apache-2.0 |
| `walkdir` | 目录遍历 | MIT OR Unlicense |
| `blake3` | 内容哈希 | CC0-1.0 OR Apache-2.0 |
| `log` | 日志门面 | MIT OR Apache-2.0 |
| `thiserror` | 错误类型派生 | MIT OR Apache-2.0 |
| `tempfile`（dev） | 临时文件 | MIT OR Apache-2.0 |

> **`symphonia` 是 MPL-2.0**：这是唯一一个非 MIT/Apache 系的直接依赖。
> MPL-2.0 是**文件级** copyleft —— 修改它的源文件才需回馈，
> **链接使用不传染**，因此不影响本项目以 MIT 发布。
> 我们没有修改 symphonia 的任何源文件。

## 前端依赖

| 包 | 用途 | 许可 |
| --- | --- | --- |
| `svelte` / `@sveltejs/kit` | 前端框架 | MIT |
| `vite` | 构建工具 | MIT |
| `typescript` | 类型检查 | Apache-2.0 |
| `@tauri-apps/api` / `@tauri-apps/cli` | Tauri 前端接口 | MIT OR Apache-2.0 |

---

## 开发期工具（不随产品分发）

`src/bin/` 下的验证工具（`mel_dump`、`embed_dump`、`text_dump`、`align_check`、
`ast_dump`、`e2e_check`、`inherit_check`、`search_check`）通过 `dev-tools`
feature 隔离，**不会被打进安装包**。它们仅用于与官方实现逐点比对，
详见 README 的「开发期验证脚本」。

## 测试素材

`test-sfx/` 下的 6 个音频文件（覆盖 WAV / MP3 / OGG / FLAC / M4A）由本项目
用 ffmpeg 合成（正弦波、噪声、扫频），**不含任何第三方版权素材**。

## 图标

`src-tauri/icons/` 下的图标为本项目原创（`icon-source.png` 为源图），
以 MIT 许可随本项目一同发布。
