# P1：跨平台 CI、打包与诊断

使用 step/step-5-preview。审查现有 release/ci workflows、installer、docs/deployment。

## 要求

- CI 至少覆盖 Windows、macOS、Linux 的 cargo check/test；前端 typecheck/build 只需一个稳定 runner，但要明确。
- release 产物标明 target、架构、版本和校验和；Windows 安装器保留数据，macOS 提供可验证的 app/zip/dmg 方案或清晰记录阻塞。
- 增加启动、端口冲突、升级、卸载保留数据、配置迁移的 smoke checklist。
- 诊断包必须脱敏 token、密码、代理认证、MCP header 和环境变量。

## 验收

workflow YAML 可解析；每个 job 的工具链、缓存和失败输出明确；文档能让用户在 Windows/macOS/Linux 从零安装并验证。更新 HANDOVER。