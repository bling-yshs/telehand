# AGENTS.md

## 开发环境

- **不要在本机安装 Rust**（rustup、cargo、rustfmt 等任何工具链组件都不要装），也不要在本机编译或运行 Rust 代码。本机资源很小，安装或编译会把机器拖垮。
- 一切验证都在远端完成：push 到 `dev/` 前缀的分支会触发 GitHub Actions（clippy、全部测试；不检查格式），根据 CI 结果修改代码。
