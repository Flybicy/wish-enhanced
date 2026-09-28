# Wish Enhanced

Wish 自托管 agent 的增强分支：在 wish-core 单二进制引擎（Rust + axum + SQLite）中原生融合了五个能力模块，
前端（wish-web，Vue 3）同步升级。Rust 侧无任何新运行时依赖进程——所有能力都是引擎内建工具。

## 融合的能力（原四插件 → 原生化）

| 原插件 | 现实现 | 工具 |
|---|---|---|
| pi-web-access | `src/tool/web.rs`：DuckDuckGo/Tavily/Brave 搜索 + 抓取（SSRF 防护/4MiB 流式上限/1h 缓存） | `web_search` `fetch_content` `get_search_content` |
| pi-workspace-history | `src/tool/snapshot.rs`：影子 git 裸仓快照，undo/redo 栈，脏工作区拒绝回滚 | `snapshot_checkpoint` `snapshot_undo` `snapshot_redo` |
| pi-observational-memory | `src/session/memory.rs`：独立 SQLite 账本（observations/reflections），recall 工具 | `memory_recall` |
| @tintinweb/pi-subagents | `src/server/subagents.rs`：进程内子会话池，完成通知回父会话 | `subagent` `subagent_result` |
| —（新增） | `src/tool/mcp.rs`：MCP stdio + streamable HTTP 双传输客户端 | `mcp_<server>_<tool>` |
| —（新增） | `src/server/skills.rs`：SKILL.md 惰性技能库 | `skill_search` |

## 配置面（config 默认全关，按需开启）

```toml
[web]        enabled/provider(DuckDuckGo|Tavily|Brave)/fetch_timeout_secs
[memory]     enabled
[skills]     enabled/dirs
[subagents]  enabled/max_concurrent/max_depth
[workspace_history] enabled = "auto"   # true/false/auto(项目特征检测)
[mcp_servers.<id>]  command/args/env/call_timeout_secs
```

## Windows 安装包

`.github/workflows/release.yml`：打 tag（`v*`）触发——GitHub Actions windows-latest 上构建
wish.exe（MSVC）+ wish-web（pnpm）+ 下载 niubash 官方 release 二进制，Inno Setup 打包成单安装包；
安装后 `niu` 作为默认 shell（`src/tool/shell/platform.rs` 优先发现 wish.exe 同目录的 niubash/niu.exe）。

本地跑 CI 同款构建：`cargo build --release`（wish-core）+ `pnpm install && pnpm build`（wish-web）。

## 分支说明

`wish-enhanced` 分支基于上游 WindustH/wish-core，全部增强以独立 commit 序列叠加，便于 review/cherry-pick。
UI 升级计划见 `docs/UI-PLAN.md`，交接文档见仓库外层 HANDOVER.md（开发过程全记录）。
