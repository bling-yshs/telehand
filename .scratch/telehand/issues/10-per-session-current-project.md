# 10: 按 MCP 会话隔离当前 project

**What to build:** 用户在同一个文件夹（同一个 MCP 地址）里同时开多个 Claude Code 进程，并行开发同一 runner 上的不同 project。当前 project 从挂在 key 上改为挂在 MCP 会话上：服务实例为每个会话新建 handler，各自在内存中持有选择，互不影响；新会话未选择时文件工具报错，绝不沿用别的会话或 key 级的选择。选择不再持久化，server 重启或会话过期后需重新选择。取代 07 中的持久化行为。

**Blocked by:** 07

**Status:** in progress

- [ ] 每个 MCP 会话一个 handler，`list_project` / `select_project` / `current_project` 与文件工具只看本会话的选择
- [ ] 会话空闲超时从 SDK 默认的 5 分钟调到 24 小时
- [ ] 未选择时的报错说明可能原因（新会话、server 重启、会话过期）并提示 list/select
- [ ] 状态文件去掉 current_project 字段；含该字段的旧文件仍能加载
- [ ] bash 后台任务仍按 runner 共享；同一进程内（subagent、清空对话）共享会话即共享选择
- [ ] 端到端测试覆盖：同一地址两个会话各选各的；server 重启后新会话未选择；runner 重连后会话选择保留
- [ ] 更新 spec（#38、#40、MCP 决策、非目标、风险说明）
