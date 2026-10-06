# 01: 打通 agent 读文件的最短路径

**What to build:** 管理员能启动 server 并创建一个 key；runner 主人能用 key 注册 runner、添加一个带主文件夹的 project 并启动 runner，屏幕上看到 MCP 地址；agent 连上该地址后，能列出 project、选择 project、查看当前 project，并用 `read` 读取一个文本文件的全部内容（相对路径相对主文件夹，绝对路径照收）。同时建立项目骨架、CI，以及后续所有 ticket 复用的端到端测试框架（同一测试进程内起 server + runner，用 MCP 客户端调用）。本 ticket 中当前 project 选择只保存在内存中，`read` 暂不做截断/分页/图片。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] Rust workspace 含两个二进制（server、runner）及共享的协议模块与文件工具模块；runner 不依赖 server 的 HTTP/MCP 依赖
- [ ] CI 在 push 到 `dev/` 前缀分支时运行 fmt 检查、clippy、全部测试，且通过
- [ ] `serve` 可指定监听地址与数据目录；单端口同时提供 runner 的 WebSocket 接入与 `/mcp/<key>`；远程 Host 可访问（放开 MCP SDK 默认的 loopback 限制）
- [ ] `key create [--name]` 生成 UUID key 并写入状态文件（本 ticket 直接写文件即可）
- [ ] runner `register <server_url> <key>` 保存连接信息到本地配置并打印 `<server_url>/mcp/<key>`
- [ ] runner `project add <name> <dir>` 写入本地配置（名称唯一、主文件夹必填）
- [ ] runner `run` 主动建立 WebSocket，握手上报 key 与 project 列表，连接成功后打印 MCP 地址
- [ ] 每个 key 对应独立的 MCP 服务实例，按 key 分发；未知 key 返回 404
- [ ] 协议请求为通用形态（请求 id、project、工具名、任意 JSON 参数），新增工具无需改协议
- [ ] MCP 工具 `list_project`、`select_project`、`current_project` 可用；选择不存在的 project 返回明确错误
- [ ] MCP 工具 `read` 可读取文本文件全文，支持相对与绝对路径，不限制读取范围
- [ ] 端到端测试框架就绪，并覆盖：注册 → 选择 project → 相对/绝对路径 read 成功
