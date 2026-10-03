# P0-3：工具权限、路径与审计

使用 step/step-5-preview。只读审查后再修改。重点文件：src/server/session/tools.rs、src/server/session.rs、src/tool/shell、src/tool/mcp.rs、src/server/subagents.rs 及相关 HTTP/SSE。

## 要求

- 建立完整工具权限矩阵：纯读、可写、网络、子 agent、MCP、会话控制；不要依赖容易漏更新的字符串散列表，或至少让未知/新增副作用默认拒绝。
- ask approval 绑定 session/generation/call id，处理重复、超时、取消、模式切换、断线和服务关闭；不得留下悬挂 oneshot。
- 检查 shell/file/MCP 参数、工作目录、环境变量、header、日志和 SSE 摘要的泄露与注入；修复必须保持用户明确授权边界。
- 子 agent 权限不得超过父会话；read_only/ask 的衰减应有测试。
- 记录最小审计事件（工具名、结果、时间、session/run/call id、脱敏参数摘要），禁止写入 token/密码/完整敏感环境。

## 验收

为每个发现补回归测试；cargo test --bins 通过；文档列出仍然有意保留的信任边界。不要把隐藏工具按钮当作权限控制。更新 HANDOVER。