# Wish 通用 Agent 路线图

更新时间：2026-03-08
基线：v0.3.6，commit 8dcb274，wish-enhanced

## 目标

把 wish 做成 Windows/macOS 上可长期运行、可恢复、可审计、易上手的通用 agent：用户给出工作目录和目标后，模型能够安全地使用 shell、文件、网页、skills、memory、MCP 和子 agent 完成多步任务；中断、断网、升级和重启不会丢失工作状态。

## 当前基线

- Rust/axum 单进程服务，SQLite 持久化会话和 entry；Vue 3 前端通过 HTTP/SSE 工作。
- 已有多 provider、流式输出、工具调用、shell 进程树、上下文压缩、memory、MCP、skills、subagents、workspace history、权限模式、审批卡、重试、分支/回退、模型目录和目录选择器。
- v0.3.6 已在 Windows CI 和本机部署验证；cargo test --bins 34 个测试、前端 typecheck/build 通过。
- CI 目前只在 .github/workflows/ci.yml:10-40 使用 windows-latest；原生窗口目前只在 src/server/shell_window.rs 的 Windows 编译分支存在。

## 已确认的 P0

### P0-0 控制面鉴权与跨源写保护

证据：src/server/http.rs:249-259 在 bearer_token_env 未配置时放行全部 /api 写端点；全库没有 Origin/Sec-Fetch-Site 防护，且带 token 时返回 ACAO *。这使本机恶意网页可以跨源提交 run、permission、approval、shell 和 config 请求。

交付：写 API 默认 fail-closed、health/version 保留诊断入口、同源与无 Origin CLI 行为明确、跨源写请求拒绝、token 比较常量时间、覆盖无 token/错误 token/正确 token/evil Origin 测试。UI 和本地启动流程必须明确如何配置或输入 token，不得用通配 CORS 掩盖信任边界。

### P0-1 macOS 首次启动与浏览器入口

证据：src/server/mod.rs:187-217 的 auto_config() 只读取 USERPROFILE；src/server/mod.rs:229-238 的非 Windows 浏览器入口固定执行 xdg-open。macOS 通常使用 HOME 和 open，因此默认启动可能无法生成配置或打开界面。

交付：跨平台用户目录解析（Windows、macOS、Linux）、macOS open/Linux xdg-open/Windows start 分支、无 GUI 时清晰错误、平台单元测试和 macOS CI 编译检查。不得把用户数据写入安装目录。

### P0-2 可恢复的事件与运行状态

SSE 客户端已有重连和轮询基础，但必须证明断线期间不会重复发送、丢失 approval/tool/result 或把旧 generation 混入当前主线。后端和前端需要明确的 event cursor、snapshot fallback、run id/generation 校验和可测试的恢复协议。

交付：断网/恢复、服务重启、浏览器刷新、审批等待、流式工具执行的端到端测试；恢复失败时给用户明确的可继续操作。核心闭环审查新增 P0/P1：Goal retry 必须有次数/时间预算和可见进度；interrupt 不能在空闲或 run 边界意外启动新 run，stop latch 不能被下一次 begin_run 清掉；OutputLimited 续段必须有上限；首字节后的中游断流要有明确重试或 Suspended 恢复语义。

### P0-3 工具权限边界与审计

src/server/session/tools.rs:157-167,317-330 通过工具名分类副作用并在 ask/read_only 拦截。需要验证所有未来工具（特别是 MCP、subagent 和文件/网络写操作）都不会漏过分类；审批必须绑定 session、generation、call id，支持取消、超时、重连后恢复和审计记录；敏感环境变量、请求 header、命令和路径不能泄露到 UI 或日志。

交付：权限矩阵、路径/命令/环境泄露测试、审批竞态测试、子 agent 权限衰减测试和可导出的审计事件。

## P1：变成日常可用的通用 Agent

1. 可靠长任务：统一 run/task 状态机；队列、暂停、继续、取消、重试、后台任务通知；崩溃后从最后一个 durable checkpoint 恢复。
2. 上下文与记忆可控：显示上下文来源和预算；压缩前预览/可撤销；记忆查看、编辑、删除、按项目隔离；保持稳定 prompt 前缀并把动态状态置尾。
3. 工作区工作流：文件树/搜索、差异审查、checkpoint/undo/redo、危险操作预览、多个 workspace 和 git worktree；所有路径显示使用规范化后的实际路径。
4. Provider 与凭据：首次配置向导、连接测试、模型能力探测、错误诊断；API key 默认环境变量，后续增加 Windows Credential Manager/macOS Keychain 的安全存储适配器。
5. 前端恢复体验：明确的连接状态、运行状态、当前工具、等待审批、重试倒计时、上下文占用、后台任务；键盘可达、窄窗口可用、中英文文案完整。
6. Skills/MCP/子 agent：导入/启用/禁用/版本与来源可见；MCP 启停和健康检查；子 agent 结果可追踪、可跳转、可取消、权限不超过父会话。
7. 诊断与升级：日志级别、诊断包（脱敏）、配置导入导出、数据目录迁移、升级前备份、回滚提示、服务端口冲突自愈。

## P2：产品差异化

- 插件/skill 市场及签名校验。
- 跨设备只读查看与安全远程连接。
- 会话分享、任务模板、团队协作。
- Office/PDF/diff 深度预览、语音/视觉输入和更丰富的自动化连接器。

## 执行顺序

1. 先完成 P0-1、P0-2、P0-3，并补 macOS/Linux/Windows CI 矩阵。
2. 再做可靠长任务和工作区审查闭环，这两项直接决定 agent 是否能用于真实项目。
3. 然后做 provider/凭据、前端恢复、skills/MCP/subagent 管理。
4. 最后做市场、分享和高级集成，不在基础可靠性完成前堆 UI。

## 统一工程约束

- 先读现有代码和测试；保持现有 Rust/Vue 架构，不引入新依赖，除非 prompt 明确批准。
- wish-web 是前端源代码；修改后先在 wish-web 验证，再按既有流程镜像到 wish-core/wish-web。
- 不重置或覆盖用户已有改动，不做无关格式化，不执行 git reset/checkout。
- 每个任务必须包含失败路径、取消路径、重启/断线路径和测试；不能只实现 happy path。
- Rust 验证使用 cargo test --bins；前端验证使用 corepack pnpm typecheck 和 corepack pnpm build；跨平台任务还要报告无法在当前 OS 执行的检查。
- 每个完成任务更新 D:/Downloads/hanapi/wish/HANDOVER.md，记录文件、行为、测试和剩余风险。

## 交付分组

- 01-platform-bootstrap.md：P0-1，先做。
- 02-recovery-protocol.md：P0-2，依赖现有 SSE/API 结构审查。
- 03-tool-security.md：P0-3，先建立权限矩阵再修复。
- 04-frontend-agent-workflow.md：P1 前端恢复与日常工作流。
- 05-ci-release-matrix.md：跨平台 CI、打包、诊断和升级验收。

## Round 46 状态

- P0-1 平台基础已实现并通过 cargo test --bins 及平台纯单测；CI 增加 macOS 检查。
- P0-0 控制面鉴权/跨源写保护已实现并通过 12 个 HTTP 鉴权测试；默认 bearer_token_env 为空的桌面 UI onboarding 仍是发布前阻断项。
- P0-3 权限默认拒绝、审批脱敏/超时/取消/审计和 generation 绑定已实现。P0-2 审批恢复已实现：status/live snapshot 回填、approval_closed、刷新/重连恢复和旧代际拒绝；前端专项测试 7/7。
- P0-2 核心运行循环和 P0-3 子 agent 权限衰减仍未完成，见 HANDOVER Round 46 的残余风险；下一轮优先处理 retry/interrupt/continuation 上限与子 agent 权限边界。
- 本路线图状态对应工作区改动，尚未发布新版本。

## Round 47 状态

- 审批 live preview 增加 call_id/generation 绑定的幂等去重与清理单测；审批恢复后端/前端验收保持通过。
- `cargo test --bins` 通过 61/61；前端 typecheck/build 和审批专项测试 7/7 通过；全量 tools 测试唯一既有失败仍是 virtual-core 3.17.0 与测试期望 3.17.9 的版本差异。
- 核心运行闭环和子 agent 权限衰减专项 agent 在完成前停止，未留下未验证改动；这些 P0 风险继续保持下一轮优先级。
- 未发布、未提交、未部署；默认 bearer_token_env onboarding 仍为发布前阻断项。