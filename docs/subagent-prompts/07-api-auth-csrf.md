# P0：控制面鉴权与 CSRF 边界

使用 step/step-5-preview。先读取 AGENT-ROADMAP.md、AGENT-ORCHESTRATOR.md 和当前 git diff。

审查证据：D:/Downloads/hanapi/wish/wish-core/src/server/http.rs:249-259 在 bearer_token_env 未配置时放行全部 /api；没有 Origin/Sec-Fetch-Site 防护，且带 token 时 ACAO *。写端点包括 run、interrupt、permission、approvals、shell、metadata、config、delete、input、rewind、fork。

目标：
- 保持带 token 的现有客户端兼容；token 比较改为 constant-time，禁止添加依赖。
- 未配置 token 时写 API fail-closed；health/version 可诊断；返回明确 401/403。
- 写请求拒绝明显 evil cross-origin；同源/CLI 无 Origin 仍可工作；GET/SSE 不被破坏；不要用 ACAO *。
- 添加纯单元测试覆盖无 token、错误/正确 token、evil Origin、同源/无 Origin。

边界：只修改 src/server/http.rs、必要的 src/server/config.rs 和对应测试；不要覆盖其他 subagent 对 app.rs、session/tools.rs、subagents.rs 的改动；不要 git 操作、不要修改 HANDOVER.md、不要引入新依赖。完成后运行 cargo test --bins 并报告剩余信任边界。