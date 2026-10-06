# 08: 管理通道与 `key rm`

**What to build:** 管理员在 server 运行中执行 `key create` / `key list` / `key rm` 时，命令通过数据目录下的 unix socket 交给运行中的 server 执行，立即生效且不会被 server 的延迟写盘覆盖；server 未运行时命令直接读写状态文件。`key rm` 后，在线 runner 收到"key 已移除"的专用关闭码，打印提示并退出、不再重连；之后该 key 的 MCP 请求返回 404，用该 key 握手的 runner 也以同一关闭码被拒绝。

**Blocked by:** 07

**Status:** ready-for-agent

- [ ] `serve` 在数据目录下监听管理 unix socket
- [ ] `key create [--name]`、`key list`、`key rm <key>` 优先通过管理通道执行；连不上时直接读写状态文件
- [ ] server 运行中经管理通道创建的 key 立即可用于 runner 注册与 MCP 访问
- [ ] server 运行中创建的 key 不会被后续延迟写盘覆盖丢失
- [ ] `key list` 显示 key、备注名、创建时间
- [ ] `key rm` 立即从内存移除并触发写盘；在线 runner 收到"key 已移除"关闭码
- [ ] runner 收到"key 已移除"关闭码后打印提示并退出，不重连
- [ ] 已删除 key 的 MCP 请求返回 404；用已删除/未知 key 握手被同一关闭码拒绝
- [ ] 端到端测试覆盖上述行为
