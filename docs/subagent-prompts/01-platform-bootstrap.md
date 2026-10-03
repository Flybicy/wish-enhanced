# P0-1：跨平台启动与浏览器入口

使用 step/step-5-preview。实现路线图 P0-1。

## 文件边界

主要修改 wish-core/src/server/mod.rs、相关 Rust 单元测试、CI workflow 和平台文档；不要修改前端功能。

## 要求

- auto_config() 在 Windows 使用用户目录，在 macOS 使用 ~/Library/Application Support/Wish，Linux 使用 XDG_CONFIG_HOME/HOME；数据目录必须与安装目录分离。兼容已有 Windows Documents/Wish 数据。
- 浏览器打开命令按 Windows start、macOS open、Linux xdg-open 选择；命令失败时仍打印可复制 URL。
- 非 Windows 无原生 WebView 时，明确把系统浏览器作为当前 fallback，不伪装成功。
- 提取纯函数并加入平台路径/命令选择测试；不要在测试中启动真实浏览器。
- 增加 macOS runner 的最小 cargo check/cargo test 保障；若 workflow 只能验证编译，明确记录。

## 验收

Windows 现有配置和启动行为不回归；macOS 不依赖 USERPROFILE/xdg-open；无权限目录和端口冲突有清晰错误；cargo test --bins 通过。更新 docs 和 HANDOVER。