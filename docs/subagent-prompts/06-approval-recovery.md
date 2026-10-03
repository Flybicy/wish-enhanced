# P0：审批恢复与断线可见性

使用 step/step-5-preview。这个任务优先级高于一般 UI 优化。先读取 AGENT-ROADMAP.md、02-recovery-protocol.md 和当前 git status。

## 已知故障证据

- wish-web/src/core/state/chatSlice.ts:175-182 只从 live approval_required frame 设置 pendingApproval。
- wish-web/src/core/state/chatStream.ts:72-81 的 snapshot replay 不恢复 pendingApproval。
- wish-core/src/server/session/tools.rs:285-312 只发送一次 approval_required 并阻塞等待；wish-core/src/server/session.rs:533-537 的 snapshot 不含 pending approval。
- wish-core/src/server/session/live.rs 的 observe 流程可能丢弃 approval_required 和 ToolStarted。

## 实现要求

1. 后端 status/snapshot 暴露当前 session 的 pending approvals，至少包含 call_id、tool name、脱敏 summary、generation/run 关联和可操作状态；不能暴露 token、密码、完整环境变量或完整 header。
2. approval_required 在 live preview 和 snapshot/reconnect 路径都可恢复；重复 frame 幂等。
3. approve/deny 必须绑定 session、call_id 和当前有效 approval；重复、过期、错误 generation、关闭服务和取消都返回明确结果并清理等待者。
4. 前端 snapshot 和 chat reconnect 恢复 approval card；auth denied/gone 与普通 offline 分开显示。
5. 为刷新、断网 2 秒、SSE 重连、服务重启、重复 approve、取消和旧 generation 添加回归测试。

## 文件边界

允许修改后端 session snapshot/events/approval 路径和前端 core/state/chat/connection/ChatPane；不要修改 provider、shell 执行或无关视觉主题；不要引入新依赖、不要 git 操作、不要修改 HANDOVER.md。

## 验收

ask 模式下工具等待审批时刷新页面，审批卡重新出现且 call_id/summary 一致；Allow/Deny 恢复原等待者；普通断网仍显示 offline，401/403 显示重新连接或凭据失效；cargo test --bins、typecheck、build 通过。