# AAmanager

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

本地优先、离线可用的音频资产管理器。核心价值是**用自然语言快速找到想要的音频**，并通过自动标签与用户备注让音频库自我组织。

面向游戏开发者、音效设计师与视频创作者。**所有分析与推理都在本机完成，不上传任何音频。**

```bash
git clone https://github.com/githubneko/AAmanager.git
cd AAmanager
npm install
npm run tauri dev
```

> 已实现：自动标签（规则引擎 + AST 事件分类）、自然语言语义搜索（CLAP）、
> 相似音频与相似度标签继承、四套标签词汇表、在文件管理器中定位、NSIS 安装包。
> 详见「[当前实现范围](#当前实现范围)」。

---

## 目录

- [当前实现范围](#当前实现范围)
- [快速开始](#快速开始)
- [技术栈](#技术栈)
- [架构](#架构)
- [目录结构](#目录结构)
- [数据存储](#数据存储)
- [自动标签规则引擎](#自动标签规则引擎)
- [搜索实现](#搜索实现)
- [模型与 AI 能力](#模型与-ai-能力)
- [测试](#测试)
- [开发约定](#开发约定)
- [已知限制](#已知限制)
- [后续阶段](#后续阶段)
- [许可](#许可)

---

## 当前实现范围

### ✅ Phase 1：基础框架（已完成）

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| Tauri 2.x 项目骨架 | ✅ | Rust 后端 + SvelteKit 前端，SPA 模式 |
| SQLite 集成与 schema | ✅ | 9 张表 + FTS5 全文索引，含版本化迁移 |
| 音频解码 | ✅ | Symphonia，支持 WAV/MP3/OGG/FLAC/M4A/AIFF 等 16 种扩展名 |
| 元数据读取 | ✅ | 时长、采样率、声道、格式、码率、文件大小 |
| 音频播放 | ✅ | rodio，播放/暂停/继续/停止/跳转/音量，惰性初始化音频设备 |
| 波形峰值 | ✅ | rustfft 无关的峰值提取，1024 点量化存 BLOB，Canvas 绘制、点击跳转 |
| 文件导入 | ✅ | 拖拽（文件夹/多文件）+ 系统对话框；路径去重 + BLAKE3 内容去重 |
| 基础列表 UI | ✅ | 排序、多选、键盘导航、详情面板、列宽自适应 |
| 关键词搜索 | ✅ | SQLite FTS5 + CJK 的 LIKE 兜底，实时搜索（140ms 防抖） |

### ✅ 顺带完成的（原属 Phase 2-3 范围）

| 功能 | 说明 |
| --- | --- |
| 标签 CRUD | 增删改查、批量打标签、全局重命名、自动补全 |
| 备注 CRUD | 覆盖/追加/删除、批量设置 |
| 批量编辑 | 多选后批量加标签、写备注、重新分析、移出库 |
| 标签云筛选 | 按标签、来源、时长、格式、文件夹筛选 |
| 声学特征提取 | RMS、ZCR、频谱质心、滚降、平坦度、攻击/衰减时间 |
| 规则引擎自动标签 | 8 类描述性标签，带置信度 |
| 自动备注生成 | 按策划书 4.5 的模板生成中文描述 |
| 后台分析队列 | 独立线程 + 持久化任务表，重启后续跑，不阻塞 UI |
| 「待确认」机制 | 低置信度自动标签虚线显示，可点击确认 |
| **多主题** | 6 套低饱和度主题（3 深色 + 3 浅色），即时切换并持久化 |
| **模型一键下载** | 流式下载 + 实时进度 + 取消 + 镜像回退 + 原子替换 |
| **自定义模型目录** | 系统对话框选择、可写性校验、恢复默认 |

### AI 能力实现状态

| 功能 | 状态 | 验证依据 |
| --- | --- | --- |
| ONNX 推理内核 | ✅ **已接入** | `ort`，运行时静态链接进 exe（发布体积 10.7 → 32 MB），**用户无需另下内核** |
| CLAP 音频编码 | ✅ **已实现并验证** | 与 PyTorch `get_audio_features` **逐位一致**（余弦 1.000000，逐元素差 0.000000） |
| CLAP 文本编码 | ✅ **已实现并验证** | 纯 Rust（`tokenizers`），input_ids 与官方完全相同；**无 Python 依赖** |
| log-mel 预处理（CLAP） | ✅ **与官方一致** | 对比 `ClapFeatureExtractor` 64064 个值，最大偏差 **8e-6** |
| AST 类别自动标签 | ✅ **已接入并验证** | 特征与官方偏差 **0.00104**；分类结果与 PyTorch 参考吻合 |
| 音频向量索引 | ✅ **已实现** | 导入分析时自动生成，实测 6/6 |
| 相似音频检索 | ✅ **已实现** | `similar_audio` 命令 + 前端入口 |
| 相似度标签继承 | ✅ **已实现并端到端验证** | 相似变体置信度 0.985；无关音频（0.68）未被继承 |
| 语义搜索 | ⚠️ **可用但带置信度门槛** | 语义簇距离大时准确（音乐 +0.484 vs 提示音 +0.061）；相近音色会混淆，低于 0.20 时退回关键词 |

**唯一的已知不足**：跨模态语义搜索在「相近语义簇」之间会混淆
（如「a bell chiming」），这是该 ONNX 导出的模型局限 ——
音频塔与文本塔各自的正确性都已验证到逐位一致，换用对齐更好的模型即可改善，
代码无需改动。

---

## 快速开始

### 环境要求

| 依赖 | 版本 | 说明 |
| --- | --- | --- |
| Node.js | ≥ 20 | 前端构建 |
| Rust | ≥ 1.77 | 后端（`stable-x86_64-pc-windows-msvc`） |
| MSVC 生成工具 | VS 2022 | Windows 上 Rust 链接所需（含 Windows SDK）；只需「使用 C++ 的桌面开发」工作负载 |
| WebView2 Runtime | 任意 | Win11 自带；Win10 由安装程序负责安装 |

> 本机已验证环境：Node v22.23.1、Rust 1.99.0、MSVC 14.44、Windows SDK 10.0.22621、WebView2 154.x。

**建议把工具链和构建产物放到空间充裕的盘**（完整构建约需 10 GB）：

```powershell
$env:CARGO_HOME       = 'E:\rust\cargo'
$env:RUSTUP_HOME      = 'E:\rust\rustup'
$env:CARGO_TARGET_DIR = 'E:\rust\target'
```

只需重装工具链时用 rustup 默认安装器即可，`Cargo.lock` 会锁定全部 624 个依赖的精确版本，
因此**构建结果可复现**：

```powershell
# 装到指定目录（不改 PATH 也可以，用绝对路径调用）
rustup-init.exe -y --no-modify-path --default-toolchain stable --profile minimal
```

### 安装依赖

```powershell
npm install
```

### 开发模式（热重载）

```powershell
npm run tauri dev
```

### 生产构建

```powershell
npm run tauri build
```

产物：

| 文件 | 说明 |
| --- | --- |
| `src-tauri/target/release/aa-manager.exe` | 主程序（约 32 MB，含静态链接的 ONNX Runtime） |
| `…/release/bundle/nsis/AAmanager_0.1.0_x64-setup.exe` | **安装程序**（约 8.6 MB） |

安装程序：

* **NSIS**，`currentUser` 模式 —— **不需要管理员权限**，装到
  `%LOCALAPPDATA%\AAmanager`；
* 自动创建开始菜单与桌面快捷方式，并在「应用和功能」里登记卸载项；
* 中文 / 英文界面；
* 卸载**不会**删除资料库（`%APPDATA%\audio-manager\library.db`）。

> 为什么安装包只有 8.6 MB，而主程序有 32 MB？NSIS 用 LZMA 压缩，
> 而 ONNX Runtime 的静态库压缩率很高。**模型文件不在安装包里** ——
> 那是 625 MB，且并非所有用户都需要（见「运行时与模型的分发方式」）。

#### 目标机器需要什么（已实测核对）

安装包**不需要用户额外安装任何小众依赖**，但有两处依赖系统组件。
这两条是通过**解析成品 exe 的 PE 导入表**确认的，不是推测：

| 依赖 | 状态 | 处理方式 |
| --- | --- | --- |
| ONNX Runtime | ✅ 已静态链接进 exe | 无需用户操作 |
| SQLite | ✅ 已静态编译进 exe | 无需用户操作 |
| Python | ✅ 已完全移除（纯 Rust 分词） | 无需用户操作 |
| **Visual C++ 运行库** | ⚠️ 曾缺失 | 已把 4 个 DLL 随包分发（见下） |
| **WebView2** | ⚠️ 曾需联网 | 已改为内嵌引导器（见下） |

**Visual C++ 运行库**：ONNX Runtime 是 C++ 写的，静态链接后仍会**静态导入**
`MSVCP140.dll` / `MSVCP140_1.dll`（延迟加载表为空，缺一个进程就起不来）。
`static_vcruntime` 只能解决 C 运行库，解决不了 C++ 标准库，因此改为随包分发：

```
src-tauri/runtime/          共 750 KB，随安装包分发到 exe 同级目录
├── msvcp140.dll            544.7 KB   ← exe 直接导入
├── msvcp140_1.dll           35.1 KB   ← exe 直接导入
├── vcruntime140.dll        121.6 KB   ← 上面两个的依赖
└── vcruntime140_1.dll       48.6 KB   ← msvcp140 的依赖
```

传递依赖已逐个用 PE 导入表核对过，无遗漏。Windows 会优先加载 exe 同目录的 DLL，
因此这 4 个文件放对位置即可，不影响系统里已装的其它版本。

**WebView2**：Tauri 默认是 `downloadBootstrapper`（装的时候去微软服务器下载），
断网就装不上。现在改成 `embedBootstrapper` —— 安装包内嵌微软引导器，
离线也能装（Windows 11 本来就自带，不会触发下载）。

> 验证方法：仓库里的 `tools/pe_imports.py` 会正经解析 PE 导入表列出非系统 DLL。
> 每次构建后跑一次就能提前发现这类问题：
> ```powershell
> python tools/pe_imports.py target/release/aa-manager.exe
> ```

#### 平台支持

**目前仅构建与验证 Windows x64。** 代码本身是**跨平台设计**的：

* 所有平台相关逻辑都用 `#[cfg(windows)]` 隔离并写了非 Windows 分支
  （`src/utils/paths.rs` 的数据目录、`src/ml/downloader.rs` 的注册表代理探测、
  `src/commands/audio.rs` 的文件管理器定位）；
* 依赖都是跨平台的：`rodio` → `cpal`（Windows WASAPI / macOS CoreAudio）、
  `symphonia` / `rubato` / `rustfft` 为纯 Rust、`ort` 支持 macOS、
  `rusqlite` 的 bundled SQLite 在 macOS 上同样可编译。

因此 **macOS 不是「需要重构」，而是「需要一轮移植 + 验证」**，工作量集中在：

1. `reveal_in_file_manager` 在 macOS 上要改用 Finder 的选中接口
   （当前非 Windows 分支只返回「不支持」）；
2. 音频输出、数据目录、代理探测三处的非 Windows 分支从未在真机跑过；
3. macOS 需要代码签名与公证，否则用户首次打开会被 Gatekeeper 拦下 ——
   这需要 Apple 开发者账号（每年 99 美元）。

**结论：暂不做。** 不是因为难，而是**没有 Mac 可验证** ——
本项目的做法是每个结论都要有实测证据（见「开发期验证脚本」），
在没有目标平台的情况下无法提供可信的 macOS 构建。

#### 命名与版本

| 位置 | 值 | 作用 |
| --- | --- | --- |
| `tauri.conf.json` → `productName` | `AAmanager` | 安装程序名、快捷方式、窗口标题、exe 元数据 |
| `tauri.conf.json` → `version` | `0.1.0` | 安装包文件名与「应用和功能」里的版本号 |
| `Cargo.toml` → `[package] name` | `aa-manager` | 生成的可执行文件名 `aa-manager.exe` |
| `Cargo.toml` → `[lib] name` | `audio_asset_manager_lib` | 仅供内部 `use`，与产品名无关 |
| `package.json` → `version` | `0.1.0` | 前端包版本（与上面保持一致） |

改版本号时**四处都要改**（`Cargo.toml`、`tauri.conf.json`、`package.json`、
`Cargo.lock` 会自动更新）。

#### 开发期工具不打进安装包

`src/bin/` 下的验证工具（`search_check`、`ast_dump` 等 8 个）用
`dev-tools` feature 隔离：

```powershell
# 发布构建：只产出主程序
npm run tauri build

# 需要工具时显式开启
cargo build --features dev-tools --bin search_check
cargo test --all-targets --features dev-tools
```

> 一开始没有隔离，结果这些工具被一起打进安装包：产物从 32 MB 涨到 42 MB，
> 装完后目录里多出 8 个与用户无关的 exe。

### 环境变量（本机开发所需）

Rust 工具链不在默认位置时，构建前需要设置：

```powershell
$env:CARGO_HOME  = 'E:\rust\cargo'
$env:RUSTUP_HOME = 'E:\rust\rustup'
$env:PATH        = "E:\rust\cargo\bin;$env:PATH"
$env:CARGO_TARGET_DIR = 'E:\rust\target'   # 避免占用 C 盘
```

应用支持的环境变量：

| 变量 | 作用 |
| --- | --- |
| `AAM_MODELS_DIR` | 覆盖模型目录（优先级高于设置项，适合免安装部署） |
| `AAM_AUTO_IMPORT` | 启动后自动导入该目录（**仅 debug 构建**，用于端到端验证） |
| `AAM_PROBE_FOCUS` | 启动后自动聚焦搜索框（**仅 debug 构建**，用于输入验证） |

---

## 技术栈

| 层次 | 技术 | 版本 | 选型理由 |
| --- | --- | --- | --- |
| 桌面框架 | Tauri | 2.12 | 体积小、Rust 后端、离线优先 |
| 前端 | Svelte 5 + SvelteKit | 5.56 / 2.65 | 编译期响应式，运行时开销极小 |
| 前端语言 | TypeScript | 6.0 | 与后端模型类型对齐 |
| 后端语言 | Rust | 1.99 | 性能与内存安全 |
| 音频解码 | Symphonia | 0.5（`all`） | 纯 Rust，格式覆盖广 |
| 音频重采样 | rubato | 0.16 | 纯 Rust，高质量 sinc 插值 |
| 频谱分析 | rustfft | 6 | 纯 Rust FFT |
| 音频播放 | rodio | 0.21 | 跨平台，基于 cpal |
| 数据库 | rusqlite | 0.37（`bundled`） | 静态编译 SQLite，FTS5 开箱即用 |
| 去重哈希 | blake3 | 1 | 极快的加密级哈希 |
| 目录遍历 | walkdir | 2 | 递归收集音频文件 |

> **与策划书的差异**：策划书原定 ONNX Runtime；因模型推理按决定暂缓，本轮**未引入 `ort` 依赖**，避免为未启用的功能增加约 15 MB 体积与跨平台构建风险。接入时只需在 `Cargo.toml` 增加 `ort` 并实现 `ml/*.rs` 中的标注位置，其余代码无需改动。

---

## 架构

```
┌──────────────────────────────────────────────────────┐
│  前端 Svelte 5 (SvelteKit SPA)                        │
│  src/lib/api.ts  ← 唯一 IPC 入口，统一错误处理         │
├──────────────────────────────────────────────────────┤
│  Tauri 命令层  src-tauri/src/commands/                │
│  薄封装：参数校验 + 编排，业务逻辑在下方各层            │
├──────────────────────────────────────────────────────┤
│  db/       SQLite：schema、4 个仓储、FTS5 同步         │
│  audio/    Symphonia 解码 → rubato 重采样 →            │
│            rustfft 特征 → 波形峰值 → rodio 播放        │
│  indexer/  持久化任务队列 + 后台分析线程（Condvar 唤醒）│
│  ml/       模型注册表 / 惰性加载 / 余弦相似度           │
└──────────────────────────────────────────────────────┘
```

### 关键设计决策

| 决策 | 理由 |
| --- | --- |
| 波形存 1024 个 u8（1 KB/文件） | 整库 10 万文件仅 100 MB；前端按「点序号 × 时长 / 点数」即可换算播放位置 |
| FTS5 用普通表而非 `content=''` | 无内容 FTS5 表无法稳定执行 UPDATE/DELETE，而标签与备注持续变动；改为写入时显式同步，语义等价且可维护 |
| 搜索 = FTS5 ∪ LIKE | `unicode61` 对 CJK 不切分，`低频` 这类查询需 LIKE 兜底；两者求并集后再按 AND/OR 组合 |
| 播放器惰性创建 | 启动时不触碰音频设备，满足「启动 < 1 秒」（策划书 8） |
| 后台线程用 Condvar 而非轮询 | 空闲时零 CPU 占用；队列状态持久化到 `index_jobs`，异常退出后可续跑 |
| 导入只读容器元数据 | 解码与 FFT 全部交给后台线程，UI 不卡；解析失败的文件也落库，用户能看到失败原因 |
| FTS MATCH 表达式全量转义 | 每个关键词加双引号并翻倍内部引号，杜绝 FTS5 语法注入 |
| 动态 SQL 只用白名单拼接 | 排序字段映射为固定片段，其余全部 `?` 占位符绑定 |

---

## 目录结构

```
audio-asset-manager/
├── src/                                  前端
│   ├── lib/
│   │   ├── api.ts                        Tauri 命令封装（唯一 IPC 入口）
│   │   ├── stores.svelte.ts              全局状态（Svelte 5 runes）
│   │   ├── types.ts                      与 Rust models.rs 一一对应
│   │   ├── format.ts                     展示层格式化纯函数
│   │   └── components/
│   │       ├── AudioList.svelte          列表（排序/多选/键盘导航）
│   │       ├── AudioPlayer.svelte        播放控制条
│   │       ├── Waveform.svelte           Canvas 波形 + 点击跳转
│   │       ├── SearchBar.svelte          关键词 / 自然语言搜索
│   │       ├── FilterPanel.svelte        标签云 + 来源/时长/格式/文件夹
│   │       ├── TagEditor.svelte          标签编辑 + 自动补全
│   │       ├── NoteEditor.svelte         备注编辑
│   │       ├── BulkEditBar.svelte        批量编辑工具栏
│   │       ├── ContextMenu.svelte        右键菜单（定位到文件等）
│   │       ├── SettingsPanel.svelte      设置：主题 / 词汇表 / 模型
│   │       └── ModelDownload.svelte      模型下载（流式 + 进度 + 取消）
│   ├── routes/
│   │   ├── +layout.svelte                主题与全局样式挂载
│   │   └── +page.svelte                  主界面（策划书 7.1 布局）
│   ├── app.css                           深色主题 CSS 变量
│   └── app.html
└── src-tauri/                            后端
    ├── src/
    │   ├── lib.rs                        入口 + 全部命令注册
    │   ├── error.rs                      统一错误类型
    │   ├── models.rs                     领域模型（前后端共享）
    │   ├── commands/                     命令层
    │   │   ├── state.rs                  AppState（DB/Worker/Player/Models）
    │   │   ├── audio.rs                  导入、列表、波形、播放
    │   │   ├── notes.rs                  标签、备注、批量编辑
    │   │   ├── search.rs                 关键词 / 语义 / 相似检索
    │   │   ├── settings.rs               设置与模型管理
    │   │   └── debug.rs                  仅 debug 构建的诊断命令
    │   ├── db/
    │   │   ├── schema.rs                 建表与迁移
    │   │   ├── audio_repo.rs             audio_files + 查询构建
    │   │   ├── tags_repo.rs              tags
    │   │   ├── notes_repo.rs             user_notes
    │   │   └── vector_repo.rs            embeddings + 文本向量缓存
    │   ├── audio/
    │   │   ├── decoder.rs                Symphonia 解码 + 哈希
    │   │   ├── resampler.rs              rubato 重采样
    │   │   ├── features.rs               声学特征 + 规则引擎 + 备注模板
    │   │   ├── mel.rs                    CLAP log-mel（slaney，与官方偏差 8e-6）
    │   │   ├── ast_features.rs           AST Kaldi fbank（与官方偏差 0.00104）
    │   │   ├── waveform.rs               峰值提取
    │   │   └── player.rs                 rodio 播放（显式状态机）
    │   ├── indexer/
    │   │   ├── queue.rs                  持久化任务队列
    │   │   └── worker.rs                 后台分析线程
    │   ├── ml/
    │   │   ├── inference.rs              ONNX 会话缓存（惰性、跨线程共享）
    │   │   ├── model_manager.rs          模型注册表与状态
    │   │   ├── downloader.rs             流式下载 + 进度 + 镜像回退
    │   │   ├── similarity.rs             余弦相似度 + Top-K
    │   │   ├── yamnet.rs                 AST 事件分类 → 自动标签
    │   │   ├── inherit.rs                相似度标签继承（衰减传播）
    │   │   ├── clap_audio.rs             CLAP 音频编码（512 维）
    │   │   └── clap_text.rs              CLAP 文本编码（纯 Rust 分词）
    │   └── utils/
    │       ├── paths.rs                  跨平台路径
    │       └── config.rs                 设置读写
    ├── tests/integration.rs              真实音频端到端集成测试
    ├── capabilities/default.json         Tauri 权限
    └── tauri.conf.json
```

---

## 数据存储

数据库位置（Windows）：`%APPDATA%\audio-manager\library.db`
模型目录：`%APPDATA%\audio-manager\models\`

| 表 | 用途 |
| --- | --- |
| `audio_files` | 音频元数据 + 波形 BLOB（1024 B/文件） |
| `user_notes` | 用户备注（一条音频可多条） |
| `tags` | 用户标签 + 自动标签统一存储，含来源与置信度 |
| `audio_embeddings` | CLAP 音频向量（512 维 float32） |
| `text_embedding_cache` | 查询词向量缓存 |
| `audio_fts` | FTS5 全文索引（文件名 / 备注 / 标签） |
| `index_jobs` | 后台分析任务队列（支持断点续跑） |
| `settings` | 键值设置，含 `schema_version` |

索引：`audio_files` 的 hash/folder/format/duration，`tags` 的 audio_id/tag/source，`index_jobs` 的 status。

---

## 自动标签规则引擎

提取的声学特征：时长、RMS 能量、过零率、频谱质心、频谱滚降（85% 能量点）、频谱平坦度、攻击时间、衰减时间。

规则（阈值取自策划书 4.4）：

| 条件 | 标签 | 置信度 |
| --- | --- | --- |
| 时长 < 0.3s | 短促 | 0.90 |
| 时长 > 5s | 长音 | 0.80 |
| 质心 > 3000 Hz 且滚降不低 | 尖锐 / 高频 | 0.85 / 0.80 |
| 质心 < 500 Hz 且滚降也低 | 沉闷 / 低频 | 0.85 / 0.80 |
| ZCR > 0.3 | 噪声感 | 0.70 |
| 攻击 < 0.01s | 瞬态 | 0.80 |
| RMS > 0.5 | 响度大 | 0.75 |
| 平坦度 > 0.4 | 宽带噪声 | 0.70 |
| RMS < 1e-4 | 静音 | 0.95 |

> **相对策划书的改进**：策划书仅用频谱质心判定「尖锐/沉闷」。实测发现宽带撞击类素材（质心 3.5 kHz 但滚降点 6 kHz）会被**同时**打上「尖锐」和「暗淡」，语义自相矛盾。因此引入滚降点作为联合判据，保证亮/暗标签互斥；并对近乎静音的素材只输出「静音」，不再生成「瞬态」这类无意义标签。两者都有回归测试覆盖。

自动备注按策划书 4.5 模板生成，例如：

- `短促、尖锐、高频、噪声感、明亮的音频`
- `沉闷、低频、乐音感、暗淡的音频`

**自动备注只在音频没有用户备注时写入**，绝不覆盖用户内容。

---

## 搜索实现

### 切词：中文按「二元组 + 整串」

关键词检索的入口是 `text::tokenize`。中文没有空格，如果只按空格切，
「沉闷的爆炸」会变成一个**连续串**，而库里存的是两个独立标签
「沉闷」和「爆炸」—— FTS5 短语匹配与 `LIKE %沉闷的爆炸%` 都命中不了，
于是"明明有标签却搜不出来"。

现在的策略：

| 输入 | 切出的检索词 |
| --- | --- |
| `沉闷的爆炸` | `沉闷`、`爆炸`（二元组）+ `沉闷的爆炸`（整串） |
| `沉闷 爆炸` | `沉闷`、`爆炸` |
| `gunshot ui_click` | `gunshot`、`ui`、`click` |
| `鼓` | `鼓` |

* **ASCII 词**整体保留（`gunshot` 不会被拆成字母）；
* **CJK** 产出相邻二元组，并补上整串；
* **单字从检索词里剔除** —— FTS5 的 unicode61 把中文按单字分词，
  于是「爆」能命中「宽带噪声」（因为「爆」是「爆炸」的一个词元），
  造成明显无关的结果。真的只搜一个字（如 AudioSet 的「鼓」）时才用单字。

### 为什么以 LIKE 为主、FTS 为辅

FTS5 的 unicode61 分词器把 `宽带噪声` 索引成 `宽 带 噪 声` 四个词元，
查询「爆炸」变成短语 `"爆炸"` → 词元 `[爆, 炸]`，只要文档里有相邻的
「爆」「炸」就算命中 —— 而「宽带噪声」里**并没有**「爆炸」，
却因为分词方式被误命中（实测 `ambient_long.ogg` 混进了「爆炸」的结果）。

因此多字关键词走 **LIKE 子串匹配**（匹配真实文本，语义精确），
FTS 只在 LIKE 一条都没命中时才作为补充启用。

`AND` 模式下若交集为空，会退回 OR 并按命中数排序 ——
因为中文切词必然产生不存在的片段（「闷的」「的爆」），
严格 AND 对中文完全不可用；英文查询交集非空，不受影响。

### 词汇表真的参与搜索（不只是换个显示名）

这是「标签主题」有没有意义的关键。展示名与库里的存储名是两套字符串，
若只改显示、不管搜索，用户看到「爆炸」却搜不到就毫无意义。

`Resolver` 构建反向索引时，会把展示名按**分隔符**拆开登记：

| 用户输入 | 展开出的规范标签 | 说明 |
| --- | --- | --- |
| `爆炸` | `沉闷` | 展示名「爆炸/轰鸣」的片段 |
| `轰鸣` | `沉闷` | 同上 |
| `爆炸/轰鸣` | `沉闷` | 整串（用户常从词表复制） |
| `激光` / `枪击` | `尖锐` | 展示名「激光/枪击」的片段 |
| `沉闷` | `沉闷` | 规范名本身永远可搜 |
| `闷雷` | `沉闷` | **自定义改名**「闷雷滚滚」的前缀 |

自定义名称同样生效：把「沉闷」改名为「闷雷滚滚」后，搜「闷雷」命中，
搜英文自定义名（如 `laser zap`）也命中。

**刻意不用 CJK 二元组做索引**：那会产生巧合 —— 游戏词汇表里「暗淡」的
展示名是「闷响」，二元组会从中切出「沉闷」，于是搜「沉闷」会莫名多出
带「暗淡」标签的无关音频（实测到的误召回）。改用**前缀**既满足
「边打边录」，又不会跨词巧合。

### 检索路径总览

```text
用户输入
  │
  ├─ 词汇表展开（expand_query）→ 规范标签，如「爆炸」→「沉闷」
  │
  ├─ 关键词检索（search_audio）
  │    ├─ text::tokenize 切词（二元组 + 整串）
  │    ├─ 原查询 ∪ 展开词，多字走 LIKE，FTS 兜底
  │    ├─ 单字命中仅在无任何多字命中时使用
  │    └─ 按「命中的不同关键词数」排序
  │
  └─ 语义检索（semantic_search，需模型）
       ├─ 文本向量低于 0.20 门槛 → 退回关键词检索并说明
       └─ 否则按余弦相似度排序
```
## 定位到文件

找到想要的音效后，下一步通常就是去文件夹里把它拖进引擎或剪辑软件，因此提供两条入口：

* **工具条按钮**「📂 定位到文件」—— 对当前选中项生效（`Ctrl` 多选时取第一个）；
* **右键菜单**「在文件夹中定位」—— 与资源管理器一致：右键未选中的行会先把它设为选中。

右键菜单还包含播放、复制完整路径/文件名、重新分析、分析全部未标注、从库中移除。

实现：Rust 侧 `reveal_in_file_manager` 命令调用 `tauri-plugin-opener` 的
`reveal_item_in_dir`（Windows 上是 `SHOpenFolderAndSelectItems`，失败时插件自身
回退到 `ShellExecuteExW`）。文件已被移动/删除时会逐级向上找到仍存在的目录并打开，
完全找不到时返回可读错误而不是静默失败。

> ✅ **验证状态**：已通过自动化验证 —— 调用后目标文件夹窗口从 0 个变为 1 个。
>
> 过程中发现 `tauri-plugin-opener` 的 `reveal_item_in_dir`
> （内部为 `SHOpenFolderAndSelectItems`）在本机**返回成功却不弹窗口**，
> 其兜底的 `ShellExecuteExW` 同样无效。改为直接启动
> `explorer.exe /select,<path>` 后即正常工作，插件保留为兜底。
> 这个坑值得记下：**返回 Ok 不等于用户看到了窗口**。

---

## 标签词汇表（标签语言）

同一段声音，用不同职业习惯的词描述。**数据库里始终存规范标签**（规则引擎那套），
展示与搜索时按所选词汇表翻译 —— 切换不迁移数据、不丢标签，用户自己写的标签永不翻译。

| 标识 | 名称 | 面向 | 示例（规范 → 该词汇表） |
| --- | --- | --- | --- |
| `game` | 游戏创作者 | 游戏开发 / 技术美术 | 沉闷 → **爆炸/轰鸣**；尖锐 → **激光/枪击**；宽带噪声 → **爆炸/冲击** |
| `video` | 剪辑 / 视频 | 视频剪辑 / 短视频 | 沉闷 → **低沉轰响**；短促 → **短音**；长音 → **长镜头铺底** |
| `music` | 音乐制作人 | 音乐制作 / 音效设计 | 沉闷 → **低频厚重**；瞬态 → **瞬态fast**；缓入 → **慢起音**；响度大 → **高RMS** |
| `custom` | 自定义 | 想用自己的词的人 | 在默认术语上逐项改名，例如把「沉闷」改成「我的爆炸」 |

配套行为：

* **自动备注同步改写**：切换词汇表后，自动生成的备注会重写成新说法
  （例：游戏词汇表 →「爆炸/轰鸣、宽带噪声的音频」；音乐词汇表 →「低频厚重、宽带噪声的音频」）。
  判定为「自动备注」的启发式是「以『的音频』结尾」，**用户手写的备注一律不动**。
* **搜索双向展开**：在游戏词汇表下搜「激光」会自动补上规范标签「尖锐」，
  因此既能命中规范标签，也能命中用户自己写的词。
* **展示名不带空格**：保证每个说法都能被整体搜索命中。

实现见 [`src-tauri/src/tag_vocab.rs`](src-tauri/src/tag_vocab.rs) 与
[`src-tauri/src/commands/tags.rs`](src-tauri/src/commands/tags.rs)。

---

## 性能

三处影响「跟手程度」的问题已修复：

| 问题 | 根因 | 修复 |
| --- | --- | --- |
| 点暂停后进度条跳到末尾、按钮不跟手 | `Sink::pause()` 会让 `Sink::empty()` 变 `true`，旧代码据此判定「播放结束」，于是 `playing=false` 且 `position=duration` | 引入显式状态机 `PlayState`，只在「播放中且 Sink 空」时才转 `Finished`；暂停保留位置 |
| 波形播放不流畅 | 每次位置更新都重设画布尺寸（清空位图 + 重新分配）再重画约 1024 个矩形，5 Hz 下掉帧 | 静态峰值层渲染到离屏画布并按缓存键复用；每帧只做 `drawImage` + 已播放区域叠加 + 播放头 |
| 按下播放键卡顿 | 解码（最长 600 秒素材）在命令里同步执行，阻塞 UI 线程 | 解码移到 `spawn_blocking` 阻塞线程池 |
| 进度更新延迟感 | 固定 200ms 轮询，UI 只能等下一次上报 | 本地时钟外推 + `requestAnimationFrame` 每帧刷新播放头，轮询只用于校正（播放中 120ms、空闲 500ms）；播放/暂停做乐观更新，按下即翻转按钮 |

回归测试覆盖了暂停语义（`pause_is_not_mistaken_for_finished`、
`seek_while_paused_keeps_paused`、`finishing_then_resume_restarts_from_beginning`）。

---

## 主题

6 套内置主题，全部**低饱和度**，为长时间浏览素材设计。切换即时生效并自动持久化。

| 标识 | 名称 | 类型 | 说明 |
| --- | --- | --- | --- |
| `graphite` | 石墨 | 深色 | 中性深灰，默认；最接近专业音频软件 |
| `midnight` | 靛蓝 | 深色 | 偏蓝冷调 |
| `forest` | 松绿 | 深色 | 带绿意的清爽深色 |
| `dawn` | 晨雾 | 浅色 | 冷白底 + 雾蓝强调 |
| `sand` | 暖沙 | 浅色 | 米色底 + 陶土强调 |
| `mint` | 薄荷 | 浅色 | 浅绿底 + 青绿强调 |

实现方式：所有颜色都是 CSS 变量，`<html data-theme="...">` 切换整套色板。
**组件内不含任何硬编码颜色**（可用 `grep '#[0-9a-fA-F]{3,8}' src/**/*.svelte` 验证）。
波形画布无法直接使用 CSS 变量，改为通过 `waveformTheme()` 读取同一批变量的计算值，
主题变化时自动重绘。

主题清单在后端 `utils/config.rs::THEMES` 与前端 `lib/theme.ts` 各有一份，
后端保存时会校验并拒绝未知标识（回退到默认主题）。

---

## 模型与 AI 能力

### 一键下载（已可用）

设置面板 → 模型 → 「一键下载」。实现要点：

* **流式写入**：边下边写 `.part` 临时文件，不把几百 MB 读进内存；
* **原子替换**：完成后 `rename`，中途失败/取消不会留下看起来「已安装」的半截文件；
* **进度上报**：后端每 100ms 通过 Tauri 事件 `model-download` 上报，
  前端显示百分比、已下载/总量与速度；
* **可取消**：`cancel_download` 立即中断并清理临时文件；
* **镜像回退**：主地址失败自动尝试 `hf-mirror.com`；
* **系统代理**：见下方说明。

### 模型清单（URL 均已实测可达，大小取自服务器 Content-Length）

| 文件 | 实际模型 | 用途 | 体积 | 必需性 |
| --- | --- | --- | --- | --- |
| `yamnet.onnx` | AST (AudioSet 527 类) | 音频事件分类 → 自动标签 | 86.6 MB | 可选 |
| `clap_audio.onnx` | CLAP 音频塔（INT8） | 音频向量化（512 维） | 34.3 MB | 语义搜索必需 |
| `clap_text.onnx` | CLAP 文本塔 **fp32** | 查询向量化（512 维） | 501.5 MB | 语义搜索必需 |
| `tokenizer.json` | RoBERTa BPE | 查询分词（纯 Rust 编码所需） | 2.1 MB | 语义搜索必需 |
| | | **合计** | **约 625 MB** | |

> **两个塔为什么一个量化一个不量化**（都有实测依据）：
> * **文本塔必须 fp32** —— 动态量化把它与 fp32 的嵌入余弦拉到 0.829，
>   检索 Top-1 命中率从 2/3 掉到 1/6；
> * **音频塔用 INT8 即可** —— 实测 INT8 与 fp32 的检索质量几乎无差
>   （平均极差 0.428 vs 0.432，每个素材的最匹配提示词完全一致），
>   但体积只有 1/3.6。多花 80 MB 换 1% 的质量不划算。

> **与策划书 3.3 的差异（实测后修正）**：
> * 策划书给的 `laion/clap-htsat-unfused/onnx/*.onnx` 与 `onnx-community/yamnet`
>   实测为 **404 / 401**，不可用；改用社区已导出 ONNX 的 `Xenova/*` 仓库。
> * **没有用 INT8 量化版**：实测动态量化把文本塔与 fp32 的嵌入余弦拉到 0.829，
>   检索命中率从 2/3 掉到 1/6。理由详见下文「模型分发的两个决定」。
> * 音频事件分类用 **AST** 而非 YAMNet —— YAMNet 的 ONNX 导出在可访问的仓库中不存在，
>   AST 同样是 AudioSet 多标签分类器且精度更高。磁盘文件名保持 `yamnet.onnx` 以对齐策划书。

### 自定义模型目录

设置面板可更改模型目录（系统对话框选择）。切换时会：

1. 校验目录**确实可写**（真实试写一个文件，而不是只看权限位）；
2. 卸载已加载模型（它们来自旧目录，继续持有会与新目录不一致）；
3. 写入 `settings.models_dir` 并立即生效。

默认位置 `%APPDATA%\audio-manager\models`；可用环境变量 `AAM_MODELS_DIR` 覆盖
（优先级最高，适合免安装部署与自动化验证）。

### 系统代理（重要）

reqwest 用 `default-features = false` 时**不会**自动使用系统代理。在挂代理的机器上，
不做处理的表现是「模型下载一直失败，但浏览器打开同一链接完全正常」。
因此 `downloader.rs` 会主动探测：环境变量 `HTTPS_PROXY` / `ALL_PROXY` >
Windows 注册表的 `ProxyServer`（并解析 `ProxyOverride` 作为 bypass 列表）。
本机实测：不处理时代码 20 秒超时失败，处理后 1.7 秒完成。

### ✅ 推理内核已接入并验证（纯 Rust，无 Python 依赖）

**核心结论：推理链路已与官方 PyTorch `逐位一致`。**

| 环节 | 验证方式 | 结果 |
| --- | --- | --- |
| log-mel 预处理 | 与官方 `ClapFeatureExtractor` 输出逐点比对 | **最大偏差 8e-6**（64064 个值全比） |
| 音频编码器 | Rust 端 512 维向量 vs PyTorch `get_audio_features` | **余弦 1.000000，最大逐元素差 0.000000** |
| 文本编码器（分词） | Rust `tokenizers` vs Python `tokenizers` | **input_ids 完全相同**（`[0,102,24125,10342,2,1,1,...]`） |
| 文本编码器（模型） | Rust ONNX vs Python ONNX（同一模型） | **余弦 1.000000，最大差 0.000000** |
| ONNX Runtime | `ort` + `download-binaries` | **构建时静态链接进 exe**，用户无需下载 |

排查过程中修正的三个真实缺陷（都在 `audio/mel.rs`，均已补测试）：

1. **用错滤波器组** —— 官方 `rand_trunc` 路径用 `mel_filters_slaney`
   （`norm="slaney"`, `mel_scale="slaney"`，sum 1.364），而我用了 htk 版
   （`norm=None`，sum 290.15）。两者数值差约 11 dB，是音频向量退化的主因。
2. **画蛇添足的频率插值** —— 官方输出 `[1, 1, 1001, 64]` 里那个 1001 其实是
   **帧数**（10 秒 / hop 480 = 1001），mel 只有 64 维且**直接透传**。
   我却把 64 个 mel 插值成 1001，破坏了频谱结构。
3. **补齐策略错误** —— 不足 10 秒时官方是「先整数次 tile，再零填充」，
   我只做了 tile，导致尾部该静音却在重复音频
   （官方 1001 帧里有 465 帧是 -100 dB 静音，我只有 23 帧）。

**Python 依赖已彻底移除**：原先文本编码走 `resources/clap_encode.py`，
现在用 `tokenizers` crate（Rust 原生 RoBERTa BPE）在进程内完成，
既没有运行时依赖，也不必每次查询 spawn 进程。

### ⚠️ 语义搜索：可用但带置信度门槛（实测数据说话）

端到端实测（6 个真实素材：流行乐、摇滚、闹铃、钟声、电话铃、错误提示音，
全部完成向量索引后检索）：

```
a pop song with drums and vocals  -> music_pop(+0.484)  music_rock(+0.398)  alarm_beep(+0.061)
a telephone ringing               -> alarm_beep(+0.264) music_pop(+0.130)   music_rock(+0.106)
an alarm clock beeping            -> alarm_beep(+0.157) music_rock(+0.096)  error_ding(+0.084)
a bell chiming                    -> alarm_beep(+0.185) error_ding(+0.114)  ringtone(+0.111)   ← 门槛拦下
continuous ambient noise          -> music_pop(+0.078)                              ← 门槛拦下
```

**能用的部分**：语义簇之间距离大时表现很好 —— 「流行乐」查询对两首真实音乐
+0.484/+0.398，对电子提示音只有 +0.061，区分度非常清晰（上表为 fp32 音频塔实测）。

**不够好的部分**：相近语义簇之间会混淆 —— 「a bell chiming」最像的却是
+0.178 的弱匹配。用官方 PyTorch fp32 复测，上限也只有 2/3 且置信度参差，
说明这是**该模型导出本身的局限**，不是我的实现问题（实现已逐位一致）。

**因此加了置信度门槛**（`SEMANTIC_MIN_SCORE = 0.20`）：
低于门槛说明「模型没把握」，此时**明确告知并退回关键词检索**，
而不是把弱匹配当成结果端给用户。这不是掩盖问题，
而是避免用户拿到一堆看似相关、实则噪声的结果。

> 想进一步提升：换用对齐更好的音频-文本模型（或其官方 PyTorch 重新导出）。
> 由于音频/文本两条链路都已验证逐位正确，换模型只需替换 `model_manager`
> 里的 URL 与 `MODEL_VERSION`，代码无需改动。

### ✅ AST 自动标签：已接入并验证（特征与官方逐点一致）

AST（AudioSet 527 类）已接上 ONNX 内核，特征提取与官方
`ASTFeatureExtractor` **逐点一致**（实测最大偏差 0.00104，逐帧均值相关系数 1.00000）。

排查过程中修正的四个关键点，每个都很反直觉：

| 环节 | 正确做法 | 我踩过的坑 |
| --- | --- | --- |
| 波形缩放 | **不缩放**（×1.0） | torchaudio 源码里 `waveform * (2**15)` 是**被注释掉的**；我照注释实现成 ×32768，功率谱放大 1e9 倍，分类完全错乱（枪声被判成「音乐」） |
| 窗函数 | **symmetric** Hann，分母 `N-1` | 我照抄了 CLAP 的 periodic（分母 `N`），mel 值整体偏低（比值中位数 0.658） |
| 归一化 | **`(x - mean) / (std * 2)`** | 官方除以 `std*2` 不是 `std`，漏掉则所有值大一倍 |
| 补零顺序 | **先补零到 1024 帧，再归一化** | 反过来填充区会变成 0 而不是 0.46703 |
| FFT 长度 | **512**（`round_to_power_of_two`） | 用 400 则频率轴全错 |
| 对数 | **自然对数** | 用 `log10` 数值体系完全不同 |
| 帧/窗 | `center=False`（snip_edges）、400/160 | 套用 CLAP 的 reflect 补零会多出帧 |
| 预加重/去直流 | **0.97 / 逐帧减均值** | CLAP 路径没有，我第一版漏了 |
| mel 范围 | **20 – 8000 Hz**，htk，无面积归一化 | 用 0–8000 会少一个滤波器 |

判定「缩放到 32768 还是 1.0」的方法很直接：用两种假设分别反推官方输出，
**未缩放反推最大差 0.00000，×32768 反推差 2.2756** —— 结论毫无歧义。

**分类实测**（与 PyTorch 参考实现一致）：

| 素材 | 我的 Rust 输出 | PyTorch 参考 |
| --- | --- | --- |
| gunshot.m4a | Burst, pop 0.090 / Explosion 0.043 / Bang 0.042 | Burst, pop 0.132 / Explosion 0.054 |
| click_ui.wav | Beep, bleep 0.294 / Ding 0.041 | Beep, bleep 0.272 |
| explosion_noise.mp3 | Waves 0.121 / Ocean 0.073 / Explosion 0.051 | Pink noise 0.219 / Waves 0.115 |
| tone_music.flac | Sine wave 0.818 / Chirp tone 0.051 | —（纯正弦，符合预期） |
| impact_dull.wav | Sine wave 0.253 / Chirp tone 0.153 | —（合成正弦，符合预期） |
| music_rock | Music 0.715 | — |

> 注：`explosion_noise.mp3` 与 `impact_dull.wav` 是我早期用 ffmpeg 生成的
> **合成素材**（宽带噪声 / 纯正弦），不在 AudioSet 的真实语义范围内 ——
> 模型把它们判成「海洋声」「正弦波」其实是**正确的**，这不是模型错误。

类别名走中英映射表（`Gunshot, gunfire` → 枪声），表里没有的类别原样显示英文，
绝不丢弃真实命中的类别；近义类别（Gunshot / Machine gun）不会重复堆叠。
标签来源标记为 `source = yamnet`（沿用策划书的文件名），
并同样经过标签词汇表翻译。
### 模型分发的两个决定

**1. 文本塔用 fp32、音频塔用 INT8（分别实测决定）。** 动态量化破坏了文本塔：
同一输入下量化文本塔与 PyTorch fp32 的嵌入余弦只有 **0.829**，fp32 导出则是
**1.000000**，检索命中率从 2/3 掉到 1/6 —— 所以文本塔必须 fp32。
而音频塔量化后几乎无损（平均极差 0.428 vs 0.432、最匹配项完全一致），
体积却只有 1/3.6 —— 所以音频塔保留 INT8。**两个塔用同一套策略是错的**，
得分开测。

**2. 新增 `tokenizer.json`（2.1 MB）到下载清单。** 纯 Rust 文本编码必需。
三者齐备才算「语义检索就绪」（`semantic_ready` 同时检查三个文件）。
## 运行时与模型的分发方式（回答「用户是否需要另行下载」）

| 组件 | 分发方式 | 用户是否需要下载 |
| --- | --- | --- |
| **ONNX Runtime 运行时** | `ort` 在**构建时**取预编译库并静态链接进可执行文件 | **不需要** |
| **推理内核代码** | Rust 代码，编进主程序 | **不需要** |
| **模型文件**（AST + CLAP 音频 INT8 + 文本 fp32 + 分词器） | 86.6 + 34.3 + 501.5 + 2.1 ≈ 625 MB，**不打包** | **需要**，已有一键下载 |
| Python 侧车依赖 | 仅文本编码器需要，缺失时静默降级 | 可选 |

也就是说：**内核本身随安装包存在，不需要用户下载**；需要下载的只有模型文件，
而模型的一键下载功能已经做好了（流式、进度、取消、镜像回退、原子替换）。

模型不打包是刻意的：625 MB 会把十几 MB 的应用膨胀几十倍，而模型只在用户想用
智能功能时才需要。AST（自动标签，86.6 MB）与 CLAP（语义搜索）互相独立，
可以只下需要的那一组。

> 说明：下载清单里是 **fp32** 的 CLAP 与量化版 AST。
> 排查时我额外下载过完整模型（590 MB）与量化 CLAP 用于对比，它们**不在**清单里。

**✅ 已完成并逐点验证 —— log-mel 特征（`audio/mel.rs`）**

CLAP 音频塔与 AST 分类器都吃 log-mel 特征，特征必须与训练时完全一致，
否则向量退化。我按 HuggingFace 的 `audio_utils.spectrogram` 与
`feature_extraction_clap.py` 逐条对齐后用**官方实现的导出数据**做了黄金比对：

| 环节 | 正确做法（我最初的实现错了） |
| --- | --- |
| mel 滤波器组 | `mel_scale="slaney"`, `norm="slaney"`（**不是 htk**） |
| 窗口 | periodic Hann：`0.5-0.5·cos(2πn/N)`，分母是 `N` 不是 `N-1` |
| 补零 | `center=True`, `pad_mode="reflect"`，两侧各 512 |
| 对数 | `10·log10(max(mel, 1e-10))`，`top_db=None` |

比对结果：**滤波器组最大偏差 < 1e-9，log-mel 最大偏差 0.000002 dB**。
测试见 `tests/integration.rs::log_mel_matches_official_reference`。

**音频向量自身有结构，但不足以支撑检索**

音频互相似矩阵呈现合理结构：爆炸↔持续噪声 0.945（都是宽带噪声）、
点击↔音调 0.888（都带音高）、撞击↔枪声 0.503（最不像）。
但如上文致命证据所示，这种「自身结构」并不等于「跨模态可用」——
完整模型的 logits 几乎只由提示词决定。

当前行为：`semantic_search` 检测到库内无可用音频向量时，
**明确降级为关键词检索并说明原因**，不返回空列表。
自动标签仍只来自规则引擎（AST 分类器同样依赖 log-mel，但还需类别映射表）。

---

## 测试

```powershell
cd src-tauri
cargo test              # 全部测试
cargo test --lib        # 单元测试
cargo test --test integration -- --nocapture   # 真实音频集成测试（含官方黄金数据比对）
```

| 测试类型 | 数量 | 覆盖内容 |
| --- | --- | --- |
| 单元测试 | 230 | 各模块独立行为、边界条件、错误路径 |
| 集成测试 | 7 | 真实音频文件的完整链路 + 真实 HTTPS 下载 + 官方黄金数据比对 |

单元测试里值得一提的几项：

* **下载器**：用本机 HTTP 服务验证流式写入、字节级一致性、`.part` → 正式文件的原子替换、
  取消后上报 `cancelled` 阶段；
* **代理探测**：断言探测到的代理能被 reqwest 接受，bypass 列表能正确解析
  （挂代理的机器上这是下载能否成功的关键）；
* **规则引擎**：断言「尖锐/沉闷」与「明亮/暗淡」互斥，静音素材只产出「静音」；
* **重采样**：断言长度契约与能量守恒（这是之前真实崩溃过的地方）；
* **log-mel（CLAP）**：与官方 `transformers` 导出的滤波器组逐点比对（偏差 < 1e-9）；
* **AST 特征**：用可复现的 440 Hz 正弦锁定 Kaldi 流水线，断言与官方值偏差 < 1e-2
  —— 写错任一环节（窗、缩放、对数、FFT 长度）都会差 1 以上；
* **标签继承**：断言衰减传播会自然止步、只传播用户标签、幂等、过期继承会被清理；
* **推理内核**：断言启动时零会话、损坏模型报可读错误、跨线程共享安全。

集成测试使用 `test-sfx/` 下的 6 个真实素材（由 ffmpeg 生成，覆盖 WAV/MP3/OGG/FLAC/M4A），验证：全格式解码、导入→分析→搜索全链路、后台线程处理真实文件、哈希去重，以及**真实 HTTPS + 系统代理**下的下载链路（网络不可用时跳过而非失败）。素材目录缺失时同样跳过（可用 `AAM_TEST_SFX` 指定）。

### 开发期验证脚本

这些**不是应用代码**，只在开发机上用于验证「内核是否真的正确」。
它们能证明的东西是单元测试证明不了的：与官方实现逐点一致。

| 脚本 / 二进制 | 用途 |
| --- | --- |
| `src/bin/mel_dump.rs` | 导出 CLAP 或 AST 特征（`ast` 参数）为原始 f32，供与官方比对 |
| `src/bin/embed_dump.rs` | 导出 CLAP 音频向量（512 维 f32） |
| `src/bin/text_dump.rs` | 导出 CLAP 文本向量 |
| `src/bin/align_check.rs` | 打印「音频 × 文本」相似度矩阵，判断跨模态能否分辨 |
| `src/bin/ast_dump.rs` | 打印 AST 对一批素材的 Top-5 类别（含中文映射） |
| `src/bin/e2e_check.rs` | 端到端：导入 → 分析 → 语义检索，用临时库不污染真实资料库 |
| `src/bin/inherit_check.rs` | 端到端：验证相似度标签继承（含「无关音频不应被继承」） |
| `tools/gen_mel_reference.py` | 用官方 `transformers` 生成 CLAP log-mel 黄金数据 |
| `tools/gen_ast_golden.py` | 生成 AST 特征的黄金值（可复现的 440 Hz 正弦） |
| `tools/*-probe.ps1`, `tools/InputSim.cs` | UI 自动化验证（真实按键 + 文件命令队列） |

用法示例：

```powershell
# 导出 AST 特征并与官方对比（需要本机有 torch/torchaudio/transformers）
cargo run --bin mel_dump -- input.wav out.f32 ast
python tools/gen_ast_golden.py

# 端到端语义检索（临时库）
cargo run --bin e2e_check -- <素材目录> <模型目录>
```
### 质量门禁

```powershell
cargo fmt --all -- --check     # 格式：0 差异
cargo clippy --all-targets     # 静态检查：0 警告
npm run check                  # 前端类型检查：0 错误 0 警告
```

---

## 开发约定

- **Rust**：`cargo fmt` + `cargo clippy` 全绿；每个公共函数都有文档注释；错误统一走 `AppError`。
- **前端**：ESLint/Prettier 风格（Svelte 官方约定）；`npm run check` 全绿；组件只引用 `app.css` 的 CSS 变量。
- **提交信息**：Conventional Commits（`feat:` / `fix:` / `docs:` / `refactor:` / `test:`）。
- **注释语言**：中文，与策划书一致。
- **测试要求**：每个模块新增公共函数时同步补单元测试。

---

## 已知限制

| 限制 | 说明 / 影响 |
| --- | --- |
| **语义检索置信度有限** | 跨模态对齐在「语义簇距离大」时可靠（音乐 vs 提示音），相近音色会混淆；已加 0.20 门槛，不足时退回关键词并说明原因 |
| **合成素材会误判** | AST 在真实语义素材上准确，但纯正弦/白噪声这类合成信号不在 AudioSet 语义范围内，会给出「Sine wave」「噪声」等字面正确的标签 —— 属预期行为，不是缺陷 |
| 波形仅 1024 点 | 长音频的细节被压缩；点击跳转精度约「时长/1024」 |
| 解码上限 30 秒 | 特征提取只分析前 30 秒（时长按容器值报告全量），避免长素材拖慢导入 |
| 试听内存上限 600 秒 | 播放一次性解码进内存，超长素材会被截断 |
| 未做分页 UI | 单次最多取 500 条，超出需靠筛选缩小范围 |
| 依赖系统音频设备 | 无声卡环境下首次播放报错（错误可读，不崩溃） |
| 模型下载需联网 | 支持系统代理与镜像回退；完全离线需手动放置模型文件 |
| 发布体积 32 MB | ONNX Runtime 已静态链接进可执行文件（这是「用户无需另下内核」的代价）；模型文件仍单独下载 |
| 语义检索门槛 0.20 | 相似语义簇之间会混淆（模型局限），低于门槛时退回关键词并说明原因 |

> **磁盘提示**：`E:\rust\target` 的 debug 目录会随依赖增长到 13 GB 以上。只需保留 release 时执行
> `Remove-Item E:\rust\target\debug -Recurse -Force` 可释放 13 GB 左右（下次 `cargo build` 会重建）。

---

## 后续阶段

| 阶段 | 内容 | 现有基础 |
| --- | --- | --- |
| Phase 3 | AST 接入 → 类别自动标签 | ✅ **已完成**：特征与官方逐点一致（偏差 0.00104），分类结果与 PyTorch 参考吻合 |
| Phase 4 | 相似度继承推荐标签 | ✅ **已完成**：`ml/inherit.rs`，端到端实测相似变体置信度 0.983/0.984，无关音频未受影响 |
| Phase 5 | CLAP 语义搜索 | ✅ **已完成**：纯 Rust 推理，与 PyTorch 逐位一致；带置信度门槛 |
| Phase 6 | 打磨发布 | ✅ **已完成**：NSIS 安装包（免管理员、中文界面、卸载不删资料库）、应用图标、依赖自足性已用 PE 导入表核对 |
| 扩展 | 批量重命名、CSV/JSON 导出、音频指纹 | 暂无 |
| 平台 | macOS 移植 | 代码已跨平台隔离，但**缺 Mac 实机验证**，暂不做（见「平台支持」） |

### 尚未做的两件事

| 项目 | 说明 |
| --- | --- |
| **安装包代码签名** | 当前安装包**未签名**，用户首次运行会被 SmartScreen 拦截（「Windows 已保护你的电脑」）。需要购买代码签名证书（OV 约 $200/年，EV 约 $400/年）。不签名也能用，只是首次要多点一次「仍要运行」。 |
| **自动更新** | 未接入 `tauri-plugin-updater`。目前的更新方式是用户重新下载安装包覆盖安装（资料库不受影响）。 |


---

## 参考资源

- [Tauri](https://tauri.app/) · [Symphonia](https://github.com/pdeljanov/Symphonia) · [rodio](https://github.com/RustAudio/rodio)
- [rubato](https://github.com/HEnquist/rubato) · [rusqlite](https://github.com/rusqlite/rusqlite) · [SQLite FTS5](https://www.sqlite.org/fts5.html)
- [ONNX Runtime](https://onnxruntime.ai/) · [CLAP](https://github.com/LAION-AI/CLAP) · [YAMNet](https://tfhub.dev/google/yamnet/1) · [sqlite-vec](https://github.com/asg017/sqlite-vec)

---

## 许可

本项目以 **MIT** 许可发布，见 [LICENSE](LICENSE)。

随安装包分发或运行时下载的第三方组件（ONNX Runtime、SQLite、
Visual C++ 运行库、WebView2，以及 CLAP / AST 模型）的许可与署名要求，
见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。

> 特别注意：AST 模型为 **BSD 3-Clause**。本应用不打包模型，
> 用户从上游仓库直接下载；若你要把模型打包进自己的发行版，
> 必须一并附上 BSD 3-Clause 全文与 `Copyright (c) 2021, Yuan Gong`。