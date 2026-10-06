# 09: 连接稳健性

**What to build:** runner 在网络断开后以指数退避自动重连；同一 key 有新 runner 接入时踢掉旧的，旧 runner 收到"已被替代"的专用关闭码后打印提示并退出、不再重连（避免互踢）。runner 离线时 agent 调用任何工具（包括三个 project 工具）立即得到 "runner offline" 错误，而 MCP 端点本身仍可连接和列出工具；server 在 runner 断线时清空该 key 缓存的 project 列表。工具调用在 runner 长时间无响应时约 60 秒超时。`register` 时连一次 server 校验 key 是否有效。

**Blocked by:** 01

**Status:** done

- [x] runner 普通断线后以指数退避重连，重连成功后重新上报 project 列表
- [x] 心跳检测失效连接
- [x] 同 key 新连接接入时，旧连接以"已被替代"关闭码关闭；server 记录日志
- [x] runner 收到"已被替代"关闭码后打印提示并退出，不重连
- [x] runner 离线时 6 个工具全部立即返回 "runner offline"；MCP initialize / 列出工具仍成功
- [x] runner 断线时 server 清空该 key 缓存的 project 列表（当前 project 选择不受影响）
- [x] 工具调用超过约 60 秒未响应返回超时错误，且不泄漏挂起的请求
- [x] `register` 时连接 server 校验 key，无效则报错且不保存配置
- [x] 端到端测试覆盖：断开后所有工具立即报 offline；第二个 runner 接入后旧 runner 收到替代关闭码并退出；runner 重启后恢复可用
