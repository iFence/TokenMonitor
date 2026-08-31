# 数据源路径对照（本应用 vs 参考项目 / tokscale）

> 目的：核对每个 AI 编程工具的本地用量数据到底从哪些文件/目录读取。
> 参考实现：Javis603/token-monitor 所使用的 [tokscale](https://github.com/junhoyeo/tokscale) 路径表。
> 本应用：`src/providers/<name>/` 通过 `ProviderSource`（`data_dirs` / `scan` / `scan_fingerprint`）读取。

## 总览表

| 工具 | 本应用扫描路径 | 参考（tokscale）路径 | 缺口 / 本次改动 |
|---|---|---|---|
| OpenCode | `~/.local/share/opencode/opencode.db` | `opencode.db` 或 `opencode-stable.db`（全渠道）**或/和**旧版 `storage/message/` | 新增 `opencode-stable.db` 与旧版 `storage/message/*.json`，按 message-id 跨源去重 |
| WorkBuddy | `~/.workbuddy/projects/**/*.jsonl` | 同上 + SQLite 兜底 | 改为 WSL 多 root；新增 `workbuddy.db` 的 `session_usage.used` 兜底（仅补未被 jsonl 覆盖的会话） |
| CodeBuddy | `~/.codebuddy/projects/**/*.jsonl` | 同上 + 扩展插件日志 | 改为 WSL 多 root；本机日志未发现 token 用量，未加第二源 |
| Claude Code | `~/.claude/projects/**/*.jsonl` | `~/.claude/projects/` **和** `~/.claude/transcripts/` | `transcripts/` 未实现：无本机样例、格式未验证，强行解析会双算/丢项目归属，见下“已知缺口” |
| OpenClaw | `.openclaw/agents/` + 旧版 `.clawdbot` | + 旧版 `.clawdbot`、`.moltbot`、`.moldbot` | 新增 `.moltbot`、`.moldbot` legacy 根 |
| DeepSeek Harness | `~/.dsh/sessions/**/*.jsonl.zstd` | `session.jsonl.zstd` 或未压缩 `session.jsonl` | 新增未压缩 `session.jsonl` 兼容 |
| Codex CLI | `~/.codex/sessions/**/*.jsonl` | `~/.codex/sessions/` | 对齐，无改动 |
| Pi | `~/.pi/agent/sessions/**/*.jsonl` | `~/.pi/agent/sessions/` | 对齐，无改动 |
| Antigravity | `~/.gemini/antigravity-cli/conversations/*.db` + `antigravity-ide` | CLI / IDE 双 DB | 已同时读 CLI 与 IDE，无改动 |
| Gemini | 原为 stub（`~/.gemini`，不解析） | `$GEMINI_CLI_HOME/tmp/*/chats/*.json`（回退 `~/.gemini/tmp/*/chats/*.json`） | 重写为真实 reader：解析 `usageMetadata`（输入/输出/缓存），未知 schema 静默跳过 |
| Qoder | 原为 stub（`~/.qoder`，不解析） | Qoder CN 本地 SQLite `%APPDATA%\QoderCN\SharedClientCache\cache\db\local.db`（及 macOS/Linux，可用 `TOKEN_MONITOR_QODER_CN_DB_PATH` 覆盖） | 改为正确路径 + OpenCode 式 `message.data` 解析（best-effort，schema 不符则静默为空） |

## 去重约定

跨源去重依赖存储层的 `fingerprint UNIQUE + INSERT OR IGNORE`：

- **OpenCode**：旧版 JSON 记录复用同 root 下 SQLite 记录的 dedup 键（`<root-label>/opencode.db:<message-id>`），使“已迁入 SQLite 但仍留在 JSON”的同一条消息只入库一次；root-label 命名空间继续隔离本地 home 与 WSL。
- **WorkBuddy**：jsonl 已覆盖的会话 id 会写入 `covered` 集合，SQLite 兜底只为未被 jsonl 覆盖的会话产出记录，避免双算。

## 已知缺口 / 说明

- **Claude Code `transcripts/`**：本轮未实现。本机无 `~/.claude`，无法取得真实样本；包装型 transcript 缺稳定的项目归属，且可能与 `projects/` 重复。强行解析会引入错误统计，故保留为待办：拿到真实 `~/.claude/transcripts/*.jsonl` 后再按其 wrapper 格式取总用量、按（会话+时间+模型）去重并保留真实计数。
- **WorkBuddy SQLite 兜底语义**：`session_usage.used` 按“会话级 input token 总数”处理（输出/缓存归 0）；未能确认 `used` 是否含缓存、是否为逐次增量。本机实测一例 `used=56852` 与同会话 jsonl 累计 input 一致，故按 input 计数。
- **CodeBuddy 扩展日志**：本机 `~/.codebuddy/logs/**/*.log` 未搜到 `token/usage/input_tokens` 等字样，未发现可作为 token 来源的字段，故未实现第二源；仅补 WSL 多 root。
- **Gemini / Qoder**：本机未安装，无法用真实样本校准。二者采用保守解析：命中已知 schema（Gemini `usageMetadata` / Qoder `message.data`）才产出记录，否则静默返回空、不影响其它工具。字段映射以后续真实样本为准。
