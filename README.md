

# TokenMonitor
<p align="center">
  <img src="resources/tokenmonitor.png" alt="TokenMonitor" width="96" height="96">
</p>

![release](https://img.shields.io/github/v/release/iFence/TokenMonitor?style=flat-square) ![downloads](https://img.shields.io/github/downloads/iFence/TokenMonitor/total?style=flat-square) ![license](https://img.shields.io/github/license/iFence/TokenMonitor?style=flat-square) ![MSRV](https://img.shields.io/badge/MSRV-1.95-orange?style=flat-square) ![platform](https://img.shields.io/badge/platform-Windows%20x64%20%7C%20Linux%20x64-blue?style=flat-square) ![stars](https://img.shields.io/github/stars/iFence/TokenMonitor?style=flat-square)

一个基于 **Rust** 的 AI 编程工具 **Token 用量追踪**应用，提供 GPUI 桌面版与 ratatui 终端版（TUI）两个前端。它读取本地各 AI 编程工具（Claude Code、Codex、Gemini CLI、Antigravity、CodeBuddy、WorkBuddy、OpenCode、OpenClaw、Qoder、DeepSeek Harness、Pi）的用量记录，存入 SQLite，并展示聚合后的用量、成本、配额与趋势图表。

> 灵感与聚合/展示形式参考 [tokei](https://github.com/cclank/tokei)（按类别分组、排序、带合计的汇总表）。

## 功能概览

- **多工具支持**：通过 `providers/` 下的适配器读取各工具的本地数据目录
  - `claude`：`~/.claude/projects/**/*.jsonl`（已实现）
  - `codex`：`~/.codex/sessions/**/*.jsonl` 的 `token_count` 事件（已实现）
  - `codebuddy`：`~/.codebuddy/projects/**/*.jsonl` 的 `message.usage`（已实现）
  - `workbuddy`：`~/.workbuddy/projects/**/*.jsonl` 的 `message.usage` / `providerData.usage`（已实现）
  - `opencode`：`~/.local/share/opencode/opencode.db`（SQLite）的 `message.data.tokens`（已实现）
  - `openclaw`：`~/.openclaw/agents/**/sessions/*.jsonl` 的 `message.usage`（已实现，兼容旧版 `~/.clawdbot`）
  - `deepseek`：`~/.dsh/sessions/**/*.jsonl.zstd` 的 `assistant/message` 用量（已实现，zstd 解压，支持 WSL）
  - `pi`：`~/.pi/agent/sessions/**/*.jsonl` 的 assistant / toolResult / compaction 用量（已实现）
  - `antigravity`：`~/.gemini/antigravity-cli/` 与 `~/.gemini/antigravity-ide/` 的 `conversations/*.db`（SQLite + protobuf）中模型生成步骤的 Token 用量，同时覆盖 CLI 与 IDE（已实现）
  - `gemini` / `qoder`：待实现（桩）
- **SQLite 持久化**：用量记录（含 fingerprint 去重）、项目、配额、设置
- **采集管线**：`scanner`（扫描解析）、`watcher`（文件监听）、`scheduler`（定时重扫）
- **核心领域层**：用量归一化、成本计价、配额追踪、多维度聚合
- **GPUI 桌面界面**：仪表盘（汇总卡片 + 表格 + 365 天热力图/日报）、图表页（时间范围下拉 + 自定义日期区间）、Project 详情、设置页（扫描间隔 / 主题强调色 / 更新检查）
- **TUI 终端版**（`tokenmonitor-tui`）：无显示器环境 / 服务器下复用同一采集与存储管线，Tab 切换「汇总（热力图 + 应用/模型明细）/ 今日小时视图 / 更新检查」三个面板；`u` 检查更新、`d` 下载、`s` 跳过
- **命令行参数**：两个前端均支持 `-h/--help`、`-V/--version`；TUI 另有 `-u/--check-update` 一次性检查更新并退出（退出码：0 无更新 · 2 有更新 · 1 出错）
- **自动更新**：启动时检查 GitHub Releases，桌面版一键下载并安装新版（Windows）；TUI 同样支持检查与下载

## 目录结构

```
src/
├── main.rs        薄入口（桌面版，含 --help/--version）
├── lib.rs         模块声明 + 重导出
├── cli.rs         命令行参数解析（两个前端共用）
├── app/           GPUI 应用壳（bootstrap、根 Entity、状态、更新检查）
├── core/          核心领域层（无渲染依赖，纯 Rust + serde）
│   ├── model/      Usage / Quota / Pricing / Provider / Project / TimeWindow / ThemeColor
│   ├── usage/      UsageRecord + 归一化
│   ├── quota/      配额追踪
│   ├── pricing/    模型计价与成本计算
│   ├── aggregation/ 多维度聚合
│   └── update.rs    GitHub 更新检查共享逻辑（版本比对、更新说明解析）
├── providers/     ProviderSource trait + 各工具数据源适配器
├── storage/       SQLite 连接 + repository 模式
├── collector/     scanner / watcher / scheduler 采集管线
├── report/        无渲染依赖的统计 / 热力图几何与分级（两个前端共用）
├── format/        紧凑格式化（千分位、M/亿 单位）
├── platform/      平台相关（Windows 主，macOS/Linux 预留）
├── ui/            GPUI 视图（dashboard / charts / project / settings）
└── tui/           ratatui 终端前端（app / ui / update）
```

## 构建与运行

需要 Rust 1.95（`rust-toolchain.toml` 已固定）。

```powershell
rustup show                    # 应激活 1.95.0-x86_64-pc-windows-msvc
cargo fmt --check
cargo check                    # lib + bin
cargo test                     # 核心单测 + SQLite schema 测试
cargo run                      # 启动桌面版窗口
```

> 桌面版首次编译会拉取并编译完整的 GPUI 依赖树（git 依赖来自 zed-industries/zed 与 longbridge/gpui-component），可能需要数分钟到二十分钟。

**终端版（TUI）**——无显示器 / 服务器环境使用，依赖树不包含 GPUI：

```powershell
cargo build --release --no-default-features --features tui
.\target\release\tokenmonitor-tui.exe          # 启动 TUI
.\target\release\tokenmonitor-tui.exe --help   # 查看用法
.\target\release\tokenmonitor-tui.exe --check-update   # 检查更新并退出
```

**Linux 终端版（TUI）**

- Linux 只提供 `tokenmonitor-tui` 终端版，没有桌面版（GPUI）构建。
- 官方 GitHub Release 的 `linux-x64` tar.gz 包为**静态 musl 构建**，不依赖 glibc 版本，可在所有主流发行版上直接运行（含 glibc 2.28 的 RHEL 8 / AlmaLinux 8 / CentOS 8，以及 glibc 2.17 的 CentOS 7 系），无需安装额外依赖。
- 从源码编译（Rust stable ≥ 1.95 即可）：

```bash
cargo build --release --no-default-features --features tui
./target/release/tokenmonitor-tui          # 启动 TUI
./target/release/tokenmonitor-tui --check-update   # 检查更新并退出
```

- Linux 数据目录为 `~/.local/share/tokenmonitor`，与官方包一致。

## 许可

MIT
