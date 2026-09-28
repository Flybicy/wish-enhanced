# Wish 前端 UI 升级计划（对标 openhanako）

## 现状评估
wish-web v2.0.0：Vue 3.5 + reka-ui + vue-router + echarts，架构成熟度高于 openhanako（Electron+React）：
- 分层清晰：core（api/state/theme/usage）/ features（connection/onboarding/sessions/settings/stats/usage）/ ui（components/motion）
- SSE 流式、虚拟滚动（@tanstack/vue-virtual）、i18n（en/tr/zh）、PWA、移动适配
- openhanako 的亮点（卡片区主页、视觉主题化、动画细节）可借鉴，但 wish-web 基础更好

## 结论：不做重写，做增量增强（保持 vue-tsc 全绿）

## Phase A：新能力面板（后端新功能的 UI 出口）
1. **CapabilitiesPane**（新 feature，挂在 SessionSettingsPane 同级侧栏）：
   - MCP 服务器列表 + 每服务器工具数（数据源：GET /config 的 mcp_servers + 会话 tools 里的 mcp_* 前缀统计）
   - Skills 列表（skill_search 的 schema summary 解析 + 技能开关状态）
   - Memory 状态（计数：observations/reflections；来源：memory_recall 工具可用性 + /config memory 段）
   - Subagents 活跃列表（subagent_result 枚举父会话的 agent 记录；运行状态徽标）
2. **Snapshot 徽标**：会话头显示当前 checkpoint rev（snapshot_checkpoint 工具可用时显示 undo/redo 快捷钮）
3. **Web 工具指示**：composer 工具图标行显示 web_search 可用性

## Phase B：视觉现代化（对标 openhanako 观感）
1. 主题 token 扩展：theme/index.ts 增加 accent 渐变组（aurora 风格三色），applyTokens 注入 CSS 变量
2. 会话卡片：SessionListRow 增加最新消息摘要行 + 模型徽标 + 运行状态光晕（phaseIndicator 已有状态源）
3. 主页 Dashboard 化：SessionsView 空态改为快速开始卡片区（新会话/最近会话/统计速览），风格对齐 openhanako 的欢迎页
4. 深浅色过渡动画：useMedia 断点切换时 200ms 颜色渐变

## Phase C：交互细节
1. 子代理运行时：ChatLog 内嵌 agent 卡片（id/状态/描述），点击跳转子会话视图
2. 快照恢复确认 Modal（reka-ui 已有组件）
3. 命令面板 CommandPanel 增加新命令：切换技能/开记忆/查看 MCP

## 验收标准
- pnpm typecheck && pnpm build 全绿
- 五个新面板数据全部来自真实 API/工具 schema（无硬编码假数据）
- 移动端不破版（useMedia 断点全覆盖）
