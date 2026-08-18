# AGENTS.md

## 项目意图

TokenMonitor 是一个轻量的 AI 编程工具 Token 用量追踪桌面应用。核心保持干净：`src/core` 是纯 Rust 领域层（无 GPUI 依赖、可单元测试），`src/ui` 只做展示，`src/providers` 负责把各工具非结构化的本地用量数据归一化为 `UsageRecord`。聚合逻辑（`src/core/aggregation`）与 tokei 一致：分类、排序、带合计。

## 架构规则

- **分层**：`core`（领域）← `providers` / `storage`（适配）← `collector`（编排）← `app` + `ui`（GPUI 展示）。依赖方向只能从外向 core 指向 core，禁止反向。
- **单 crate**：非 workspace。`lib.rs` 只做 `mod` 声明 + `pub use`；`main.rs` 只做薄入口（调用 `rtoken::app::run()`）。actions 定义在 `src/app/actions.rs`（因 lib + bin 拆分，动作类型必须在 lib 内）。
- **GPUI 约定**：GPUI trait 导入需在每个模块显式列出（不会跨模块带入）；`gpui`/`gpui_platform` 保持与 `gpui-component` 一致的**未固定 rev 的 git URL**，版本只通过提交的 `Cargo.lock` 锁定（**切勿加 `rev =`**，否则产生两个不兼容的 gpui）；`gpui_component::init(cx)` 必须在 UI 创建前调用；根视图包裹 `gpui_component::Root`；`on_click` 回调用提供的 `&mut Window` + 存储的 `WeakEntity`，避免 `update_in`。
- **模块组织**：一个文件一个职责；生产模块 ≤ 500 行；领域类型统一 `serde` 派生。
- **数据源适配**：每个 provider 实现 `ProviderSource` trait（`data_dir` / `scan` / `scan_fingerprint`）；新工具只需新增 `src/providers/<name>/` 目录并加入 `all_providers()` 与 `build_sources()`。
- **存储**：`storage/sqlite.rs` 只做连接与 schema 初始化；查询逻辑在 `storage/repository/`。去重靠 `fingerprint UNIQUE` + `INSERT OR IGNORE`。

## 验证命令

在交付前运行：

```powershell
rustup show              # 必须激活 1.95.0-x86_64-pc-windows-msvc
cargo fmt --check
cargo check --all-targets
cargo test
cargo run                # 涉及 UI 变更时
cargo tree -d            # 确认只有一个 gpui / gpui-component git 来源（无 rev）
```

## 提交规范（Angular）

提交信息遵循 Angular 约定，一条提交只做一件事，格式为：

```text
<type>(<scope>): <subject>
```

- **type**（必选）：`feat`（新功能）、`fix`（修复）、`docs`（文档）、`style`（格式，不影响逻辑）、`refactor`（重构）、`perf`（性能）、`test`（测试）、`build`（构建/依赖）、`ci`（CI 配置）、`chore`（杂项）、`revert`（回滚）。
- **scope**（可选）：改动范围，如 `ui`、`providers`、`storage`、`collector`。
- **subject**：祈使句、小写开头、结尾不加句号、不超过 50 字符。
- **body**（可选）：与 subject 之间空一行，说明「为什么」和「做了什么」，不写「怎么做的」。
- **footer**（可选）：破坏性变更写 `BREAKING CHANGE: ...`，或关联 issue `Closes #123`。

示例：

```text
feat(providers): add CodeBuddy adapter
fix(ui): format chart values with compact M/亿 units
refactor(storage): push aggregation into SQL
```

```text
feat(providers): rename data_dir to data_dirs

BREAKING CHANGE: ProviderSource::data_dir now returns a Vec.
```
