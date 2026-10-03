# P0-2：流式恢复与 durable run 状态

使用 step/step-5-preview。先审查 wish-web/src/core/api/sse.ts、chat/sync slices、后端 events/generation/session 路径，画出当前 cursor 和 snapshot 行为，再实现最小修复。

## 目标

断网、刷新、服务重启后，用户不会看到重复/错代消息；未完成的工具调用、审批和重试状态可恢复或明确标记为需要重试；取消只影响目标 run。已知 P0：chatSlice 只从一次 approval_required SSE frame 设置 pendingApproval，后端 SessionTools::request_approval 会持续等待；刷新或 SSE 重连后审批卡消失而 run 永久阻塞，必须把 pending approval 放入 snapshot/status 并在客户端恢复。

## 要求

- 以 session + generation + run/cursor 作为事件关联键；拒绝旧 generation 写入当前主线。
- SSE 重连必须有最后游标或可靠 snapshot fallback；重复事件幂等。
- approval_required、tool outcome、retry、run finished 等关键状态必须在恢复后可重建。
- 覆盖前端和后端测试：断线期间事件、刷新、服务重启、旧 generation、审批等待、取消竞态。
- 不改变现有 API 语义，除非同时兼容旧客户端并写 API 文档。

## 验收

typecheck/build、cargo test --bins 通过；至少一个可自动化的恢复回归测试；报告仍未解决的跨进程恢复限制。更新 HANDOVER。