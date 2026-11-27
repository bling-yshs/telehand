# 10: 每次调用指明 project，不再有"当前 project"

**What to build:** 用户在同一个文件夹（同一个 MCP 地址）里同时开多个 Claude Code 进程，并行开发同一 runner 上的不同 project，原先挂在 key 上的当前 project 会被互相抢。先尝试过按 MCP 会话隔离，但 Claude Code 已改用无状态的 MCP 2026-07-28 协议（不发 `initialize`、没有 `Mcp-Session-Id`，SDK 对每个请求新建 handler），会话里的选择每次调用后都会丢失，服务端也无法区分不同进程。因此去掉服务端状态：`read` / `write` / `edit` / `bash` / `grep` / `find` 增加必填的 `project` 参数，去掉 `select_project` / `current_project`。取代 07 中的持久化行为。

**Blocked by:** 07

**Status:** done

- [x] server 为需要 project 的工具在 schema 上追加必填 `project`；校验后从参数中取出，放进 WebSocket 请求的 project 字段转发，runner 与工具模块不变
- [x] 漏填或填错 `project` 时报错并列出可用 project
- [x] 去掉 `select_project` / `current_project`；`list_project` 不再标记当前 project
- [x] `bash_result` / `bash_kill` 不带 `project`，后台任务按 runner 共享
- [x] 状态文件去掉 current_project 字段；含该字段的旧文件仍能加载
- [x] e2e 测试客户端改用无状态的 2026-07-28 协议（与 Claude Code 一致）
- [x] 端到端测试覆盖：同一地址两个客户端交替操作不同 project；漏填 / 填错 project 报错；schema 中 project 必填
- [x] 更新 spec
