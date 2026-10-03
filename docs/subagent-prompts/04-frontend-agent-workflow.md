# P1：前端日常 Agent 工作流

使用 step/step-5-preview。先读后端事件/状态契约，再修改 D:/Downloads/hanapi/wish/wish-web/src；不要直接编辑 wish-core 镜像。

## 目标

用户在窄窗口和长任务中始终知道：当前连接、运行、模型、工具、审批、重试、上下文预算和下一步操作。

## 要求

- 连接状态、重连、轮询 fallback、旧 generation 和 run 完成状态可见；断线后提供继续/刷新而不是静默失败。
- Composer、审批卡、重试倒计时、取消、重新回复/修改、分支和队列操作键盘可达且有明确禁用态。
- session/project 列表支持运行中、失败、归档和未读变化的扫描；错误可复制诊断信息。
- 设置向导覆盖 provider/model/cwd/shell/permission，并解释凭据不会写入工作区。
- 保持现有 i18n 和移动布局，禁止假数据；补组件/状态测试。

## 验收

corepack pnpm typecheck、corepack pnpm build 通过；镜像前运行 robocopy；报告窄窗口和断线手测结果。更新 HANDOVER。