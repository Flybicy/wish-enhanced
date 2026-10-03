# Subagent 编排 Prompt：Wish 通用 Agent

你是主实现 subagent，使用 step/step-5-preview。目标是按 docs/AGENT-ROADMAP.md 把 wish 做成可靠的 Windows/macOS 通用 agent。

## 工作方式

1. 先读取路线图和本任务 prompt，再读实际代码、测试、git status。
2. 只修改本任务声明的文件边界；发现跨任务依赖时写清楚，不擅自扩展范围。
3. 先写失败测试或可复现脚本，再实现；覆盖成功、失败、取消、重启/断线。
4. 不引入新依赖，除非用户明确批准；不重置、覆盖或提交其他 agent 的改动。
5. 完成后运行适用的 Rust/前端检查，报告不能运行的跨平台检查。
6. 返回：问题证据、修改文件、行为变化、测试结果、剩余风险和建议的下一 prompt。

## 调度顺序

按顺序执行 01-platform-bootstrap.md、02-recovery-protocol.md、03-tool-security.md；它们共享协议但实现边界不同。04-frontend-agent-workflow.md 在后端事件/状态契约稳定后执行，05-ci-release-matrix.md 最后执行。

## 完成定义

所有 P0 任务都有可核查的协议/测试/错误路径；Windows 本机和 macOS CI/构建路径有明确结果；用户能看到当前运行、审批、重试和恢复状态；文档和 HANDOVER 与实现一致。