# rToken

![release](https://img.shields.io/github/v/release/iFence/rToken?style=flat-square) ![downloads](https://img.shields.io/github/downloads/iFence/rToken/total?style=flat-square) ![license](https://img.shields.io/github/license/iFence/rToken?style=flat-square) ![MSRV](https://img.shields.io/badge/MSRV-1.95-orange?style=flat-square) ![platform](https://img.shields.io/badge/platform-Windows%20x64-blue?style=flat-square) ![stars](https://img.shields.io/github/stars/iFence/rToken?style=flat-square)

一个基于 **Rust + GPUI** 的 AI 编程工具 **Token 用量追踪**桌面应用。它读取本地各 AI 编程工具（Claude Code、Codex、Gemini CLI、CodeBuddy、OpenCode、OpenClaw、Qoder、DeepSeek Harness、Pi）的用量记录，存入 SQLite，并展示聚合后的用量、成本、配额与趋势图表。

> 灵感与聚合/展示形式参考 [tokei](https://github.com/cclank/tokei)（按类别分组、排序、带合计的汇总表）。

## 功能概览

- **多工具支持**：通过 `providers/` 下的适配器读取各工具的本地数据目录
  - `claude`：`~/.claude/projects/**/*.jsonl`（已实现）
  - `codex`：`~/.codex/sessions/**/*.jsonl` 的 `token_count` 事件（已实现）
  - `codebuddy`：`~/.codebuddy/projects/**/*.jsonl` 的 `message.usage`（已实现）
  - `opencode`：`~/.local/share/opencode/opencode.db`（SQLite）的 `message.data.tokens`（已实现）
  - `openclaw`：`~/.openclaw/agents/**/sessions/*.jsonl` 的 `message.usage`（已实现，兼容旧版 `~/.clawdbot`）
  - `deepseek`：`~/.dsh/sessions/**/*.jsonl.zstd` 的 `assistant/message` 用量（已实现，zstd 解压，支持 WSL）
  - `pi`：`~/.pi/agent/sessions/**/*.jsonl` 的 assistant / toolResult / compaction 用量（已实现）
  - `gemini` / `qoder`：待实现（桩）
- **SQLite 持久化**：用量记录（含 fingerprint 去重）、项目、配额、设置
- **采集管线**：`scanner`（扫描解析）、`watcher`（文件监听）、`scheduler`（定时重扫）
- **核心领域层**：用量归一化、成本计价、配额追踪、多维度聚合
- **GPUI 桌面界面**：仪表盘（汇总卡片 + 表格）、图表页（时间范围下拉 + 自定义日期区间）、Project 详情、设置页（应用选择 / 关于）
- **自动更新**：启动时检查 GitHub Releases，一键下载并安装新版（Windows）

## 目录结构

```
src/
├── main.rs        薄入口
├── lib.rs         模块声明 + 重导出
├── app/           GPUI 应用壳（bootstrap、根 Entity、状态、更新检查）
├── core/          核心领域层（无 GPUI 依赖，纯 Rust + serde）
│   ├── model/      Usage / Quota / Pricing / Provider / Project / TimeWindow
│   ├── usage/      UsageRecord + 归一化
│   ├── quota/      配额追踪
│   ├── pricing/    模型计价与成本计算
│   └── aggregation/ 多维度聚合
├── providers/     ProviderSource trait + 各工具数据源适配器
├── storage/       SQLite 连接 + repository 模式
├── collector/     scanner / watcher / scheduler 采集管线
├── platform/      平台相关（Windows 主，macOS/Linux 预留）
└── ui/            GPUI 视图（dashboard / charts / project / settings）
```

## 构建与运行

需要 Rust 1.95（`rust-toolchain.toml` 已固定）。

```powershell
rustup show                    # 应激活 1.95.0-x86_64-pc-windows-msvc
cargo fmt --check
cargo check                    # lib + bin
cargo test                     # 核心单测 + SQLite schema 测试
cargo run                      # 启动 rToken 窗口
```

> 首次编译会拉取并编译完整的 GPUI 依赖树（git 依赖来自 zed-industries/zed 与 longbridge/gpui-component），可能需要数分钟到二十分钟。

## 许可

MIT
