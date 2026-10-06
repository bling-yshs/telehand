# Telehand Spec

## Problem Statement

我在一台机器上跑 AI agent（如 Claude Code），但要操作的代码和文件在另一台机器上（家里的电脑、内网服务器、NAT 后面的开发机）。目前没有一个轻量、可自托管的方式，让 agent 通过标准协议直接读写那台机器上的文件：要么得给 agent 开 SSH（权限过大、配置繁琐），要么得在目标机器上整套部署 agent。我想要的是：在目标机器上跑一个小程序，拿到一个 MCP 地址，填进 agent 就能用，并且写操作被限制在我指定的目录里。

## Solution

Telehand 由两个程序组成：

- **server**：部署在公网可达的机器上。管理员用它的 CLI 创建 key（UUID）。它对 agent 暴露 MCP 端点 `/mcp/<key>`，对 runner 暴露 WebSocket 接入点，本身只做转发，不碰文件。
- **runner**：部署在文件所在的机器上。用 key 向 server 注册后，屏幕上打印 agent 要连接的 MCP 地址。runner 主动连 server（因此可以在 NAT 后面），收到工具调用后在本地执行。

runner 上可以注册多个 **project**：每个 project 有一个必填的主文件夹，以及可选的额外文件夹。agent 通过 MCP 工具 `list_project` / `select_project` / `current_project` 选择要操作的 project，然后用 `read` / `write` / `edit` 操作文件。相对路径相对主文件夹解析；`read` 可读任意路径，`write` / `edit` 只能落在 project 的文件夹范围内。工具行为照搬社区验证过的 pi coding agent，让模型用起来顺手。

## User Stories

### 部署与 key 管理（管理员）

1. 作为管理员，我希望用一条命令启动 server 并指定监听地址和数据目录，以便在任何 Linux 机器上快速部署。
2. 作为管理员，我希望 server 只用一个端口同时提供 MCP 和 runner 接入，以便防火墙和反代配置最简单。
3. 作为管理员，我希望 server 不内置 TLS，以便我按自己的习惯用反代或其他方式处理 HTTPS。
4. 作为管理员，我希望用 `key create` 生成一个 UUID key 并可附带备注名，以便区分不同机器。
5. 作为管理员，我希望 key 是 UUID，以便不必担心被枚举猜中。
6. 作为管理员，我希望用 `key list` 看到所有 key 及其备注和创建时间，以便管理。
7. 作为管理员，我希望用 `key rm` 删除 key，以便在机器退役或 key 泄露时立即收回权限。
8. 作为管理员，我希望 server 运行中执行 key 命令也能立即生效，无需重启 server。
9. 作为管理员，我希望 server 未运行时也能执行 key 命令，以便提前准备好 key。
10. 作为管理员，我希望 server 运行中创建的 key 不会因为 server 自己的持久化而被覆盖丢失。
11. 作为管理员，我希望 server 正常退出（Ctrl-C / SIGTERM）时把所有未写盘的状态写入，以便重启后不丢失选择。
12. 作为管理员，我希望 server 的状态存在一个人类可读的 JSON 文件里，以便必要时手动检查。

### runner 注册与运行（runner 主人）

13. 作为 runner 主人，我希望用 `register <server_url> <key>` 一次性保存连接信息，以便之后只需 `run`。
14. 作为 runner 主人，我希望 `register` 时立即连一次 server 校验 key，以便尽早发现填错。
15. 作为 runner 主人，我希望注册成功后屏幕打印 agent 要用的 MCP 地址，以便直接复制给 agent。
16. 作为 runner 主人，我希望 `run` 常驻并在连接成功后再次打印 MCP 地址，以便随时找到它。
17. 作为 runner 主人，我希望 runner 主动连 server，以便机器在 NAT/防火墙后也能用。
18. 作为 runner 主人，我希望网络断开后 runner 自动以指数退避重连，以便笔记本睡眠唤醒后无需手动干预。
19. 作为 runner 主人，我希望在另一台机器上用同一个 key 启动 runner 时，新的接管、旧的收到"已被替代"提示并退出，以便迁移机器时不需要先去关旧的。
20. 作为 runner 主人，我希望被替代的 runner 不再自动重连，以便两台机器不会互相踢来踢去。
21. 作为 runner 主人，我希望 key 被管理员删除后，runner 收到"key 已移除"提示并退出，而不是无限重连。
22. 作为 runner 主人，我希望用已删除的 key 启动 runner 时得到明确的拒绝原因。
23. 作为 runner 主人，我希望 runner 的配置存在本机一个文件里，以便备份和查看。

### project 管理（runner 主人）

24. 作为 runner 主人，我希望用 `project add <name> <dir>` 注册一个带主文件夹的 project，以便 agent 有明确的工作目录。
25. 作为 runner 主人，我希望 project 名称唯一，以便 agent 用名称选择时不产生歧义。
26. 作为 runner 主人，我希望用 `project folder add <name> <dir>` 给 project 添加额外文件夹，以便 agent 能写入主目录之外的指定位置（例如共享配置目录）。
27. 作为 runner 主人，我希望额外文件夹只扩大可写范围、不改变相对路径基准，以便行为可预测。
28. 作为 runner 主人，我希望用 `project list` 查看所有 project 及其文件夹。
29. 作为 runner 主人，我希望用 `project remove <name>` 删除 project。
30. 作为 runner 主人，我希望修改 project 配置后 CLI 提示需重启 `run` 才生效，以便我不会误以为已经生效。
31. 作为 runner 主人，我希望 project 配置和访问权限判断都在 runner 本地完成，以便即使 server 被攻破，攻击者也无法写入我未授权的目录。

### project 选择（agent）

32. 作为 agent，我希望用 `list_project` 看到 runner 上所有 project 的名称、主文件夹、额外文件夹，以及哪个是当前 project。
33. 作为 agent，我希望用 `select_project(name)` 切换当前 project，以便之后的文件操作都作用于它。
34. 作为 agent，我希望选择一个不存在的 project 时得到明确错误。
35. 作为 agent，我希望用 `current_project` 查看当前 project 详情，以便确认工作目录和可写范围。
36. 作为 agent，我希望当前 project 的选择在 server 重启、runner 重连后依然保留，以便不必每次重新选择。
37. 作为 agent，我希望在还没有选择 project（或选择的 project 已被删除）时调用文件工具，得到提示我先 `list_project` / `select_project` 的错误。
38. 作为使用同一个 MCP 地址的多个 agent 会话的用户，我理解当前 project 是挂在 key 上全局共享的，一个会话切换会影响其他会话。

### 文件操作（agent）

39. 作为 agent，我希望 `read` 接受相对路径（相对主文件夹）和绝对路径。
40. 作为 agent，我希望 `read` 可以读取 runner 上任意路径的文件，以便查看系统配置、依赖源码等参考资料。
41. 作为 agent，我希望 `read` 支持 1 起始的 `offset` 和 `limit`，以便分段读取大文件。
42. 作为 agent，我希望 `read` 输出在 2000 行或 50KB（先到为准）处截断，并附带"用 offset=N 继续"的提示，以便不撑爆上下文又知道如何继续。
43. 作为 agent，我希望 `offset` 超出文件末尾时得到包含总行数的错误。
44. 作为 agent，我希望读取图片文件（jpg/png/gif/webp/bmp）时得到 MCP image 内容，且过大的图片被自动缩放，以便我能"看"图又不浪费上下文。
45. 作为 agent，我希望路径中的 `~`、前导 `@`、`file://` 前缀、Unicode 空格变体被自动规范化，以便我常见的路径写法都能用。
46. 作为 agent，我希望 `write` 在文件不存在时创建、存在时覆盖，并自动创建父目录。
47. 作为 agent，我希望 `write` 成功后得到简短的成功信息。
48. 作为 agent，我希望 `edit` 用一组 `{oldText, newText}` 在一次调用中完成多处替换，以便减少往返。
49. 作为 agent，我希望 `edit` 的每个 `oldText` 都对原文件匹配（而非逐步应用），并要求唯一、不重叠，以便结果确定。
50. 作为 agent，我希望 `oldText` 精确匹配失败时，`edit` 用模糊匹配（Unicode 规范化、忽略行尾空白、智能引号/破折号/特殊空格归一）再试一次，以便我复制文本时的细微差异不导致失败。
51. 作为 agent，我希望 `edit` 保留文件原有的换行风格（CRLF/LF）和 BOM，以便不产生无关 diff。
52. 作为 agent，我希望 `edit` 在找不到、多处匹配、重叠、空 `oldText`、替换后无变化时分别给出明确、可操作的错误信息。
53. 作为 agent，我希望 `edit` 宽容地接受旧式的顶层 `oldText`/`newText` 参数，或被序列化成字符串的 `edits`，以便不同模型的调用习惯都能成功。
54. 作为 agent，我希望 `write` / `edit` 的目标在 project 文件夹范围之外时被拒绝并说明原因。
55. 作为 runner 主人，我希望范围检查基于解析软链和 `..` 后的真实路径，以便 agent 无法借软链或相对路径逃逸。
56. 作为 runner 主人，我希望写入一个尚不存在的文件时，范围检查基于其最近已存在祖先目录的真实路径，以便新建文件同样受约束。
57. 作为 agent，我希望对同一文件的并发 `write` / `edit` 被依次执行，以便不会互相覆盖。

### 故障与可用性（agent）

58. 作为 agent，我希望 runner 离线时，任何工具（包括 project 相关工具）立即返回 "runner offline" 错误，以便我不会傻等。
59. 作为 agent，我希望 runner 离线时 MCP 端点本身仍能连接和列出工具，以便我的客户端配置不会因此报错。
60. 作为 agent，我希望使用无效或已删除的 key 访问 MCP 地址时得到 404。
61. 作为 agent，我希望工具调用在 runner 长时间无响应时超时并返回错误。

### 演进

62. 作为开发者，我希望以后加入 `bash` 工具时不需要修改 runner 与 server 之间的消息协议。

## Implementation Decisions

### 模块

- **协议模块**（runner ↔ server 共享）：定义 WebSocket 消息。握手消息携带 key 与 project 列表；请求消息为通用形态（请求 id、project 名、工具名、任意 JSON 参数），以便新增工具无需改协议；响应消息为 MCP 内容块列表或错误；另有心跳。专用 close code 至少包括"已被替代"与"key 已移除/无效"。
- **文件工具模块**（纯库，深模块）：对外接口仅为"给定 project 上下文（主文件夹、额外文件夹）与工具参数 → 返回内容块或错误"。内部封装路径规范化与解析、写范围检查、截断、read 的文本/图片处理与缩放、write、edit 的精确/模糊匹配、换行与 BOM 保留、同路径写串行化。所有 pi 行为（参数名、描述文案、截断提示、错误文案）集中在这里。
- **server**：key 存储、管理通道、runner 注册表、MCP 端点。
- **runner**：本地配置与 CLI、连接管理（重连、被替代/被移除时退出）、请求分发到文件工具模块。
- 两个独立二进制（server、runner）放在同一个 Rust workspace 中，runner 不携带 server 的 HTTP/MCP 依赖。

### server 状态与 key

- 状态文件结构：以 key 为键，值包含备注名、创建时间、当前 project（可空）。
- 状态文件只由运行中的 server 进程拥有：启动时读取一次；运行时变更先改内存，延迟 100ms 写盘（等待期间再有变更则重置计时），写入采用临时文件 + 原子重命名；收到退出信号时立即写盘。
- key 管理 CLI 通过数据目录下的 unix socket 管理通道请求运行中的 server 执行；连不上（server 未运行）时直接读写状态文件。
- 删除 key：立即从内存移除，向在线 runner 发送"key 已移除" close code；之后的 MCP 请求返回 404；用该 key 的握手同样被拒。

### 连接

- runner 主动建立 WebSocket；普通断线指数退避重连。
- 同一 key 新连接到来时踢掉旧连接（"已被替代" close code）；收到该 code 或"key 已移除" code 的 runner 打印提示并退出，不重连。
- server 在 runner 断线时清空该 key 缓存的 project 列表（当前 project 选择仍保留在状态文件中）。
- runner 离线时所有 6 个 MCP 工具立即返回 "runner offline"。
- 工具调用超时约 60 秒。

### MCP

- 使用官方 Rust MCP SDK 的 Streamable HTTP 服务端。SDK 不支持按路径区分租户，因此每个 key 懒创建一个独立的 MCP 服务实例，由一个按 key 分发的 HTTP 处理器转交；会话空间按 key 隔离。
- SDK 默认只允许 loopback Host，需要放开以支持远程访问。
- 暴露 6 个工具：`read`、`write`、`edit`、`list_project`、`select_project`、`current_project`。不提供 ls/find/grep。
- 当前 project 挂在 key 上（全局），不依赖 MCP 会话（新版 MCP 规范已移除会话）。

### project 与路径

- project 配置只存在于 runner 本地文件（同时保存 server 地址和 key）；变更需重启 runner 生效；runner 每次连接时上报 project 列表。
- project = 唯一名称 + 必填主文件夹 + 0..n 额外文件夹。
- 相对路径相对主文件夹；绝对路径照收。`read` 不限范围；`write` / `edit` 的真实路径（解析软链与 `..`；不存在时取最近已存在祖先）必须位于某个 project 文件夹的真实路径之内。
- 工具语义以 pi coding agent（earendil-works/pi 的 coding-agent 工具实现）为准逐项照搬，包括 read 图片缩放规则。

### CLI

- server：`serve`（监听地址、数据目录）、`key create [--name]`、`key list`、`key rm <key>`。
- runner：`register <server_url> <key>`、`run`、`project add <name> <dir>`、`project folder add <name> <dir>`、`project list`、`project remove <name>`。
- MCP 地址由注册时的 server_url 拼接 `/mcp/<key>` 得到。

## Testing Decisions

- **好测试的标准**：只通过模块对外接口断言外部可观察行为（工具返回的内容/错误文本、文件系统最终状态、连接被关闭及 close code、HTTP 状态码），不断言内部数据结构、私有函数或调用顺序。重构内部实现时测试不应需要改动。
- **两层测试接缝**：
  1. **MCP 接缝（主）**：同一测试进程内启动 server（临时数据目录、随机端口）和 runner，用 MCP 客户端经 `/mcp/<key>` 发起真实调用。覆盖：未选择 project 时文件工具报错；list/select/current_project；相对/绝对路径 read；范围外 read 成功、范围外 write/edit 被拒、软链逃逸被拒；edit 多处替换与模糊匹配的端到端效果；读取 png 得到 image 内容；runner 断开后所有工具立即 "runner offline"；同 key 第二个 runner 接入后旧 runner 收到"已被替代"；`key rm` 后 runner 收到"已移除"且 MCP 返回 404；server 运行中经管理通道创建的 key 立即可用且不被延迟写覆盖；当前 project 选择在 server 重启后保留。
  2. **文件工具模块接缝（补充）**：针对 pi 语义中组合数量大的细节，直接调用文件工具模块的公开接口：截断边界（行数/字节先到、单行超 50KB、limit 提前停止）、offset 越界、CRLF/BOM 保留、模糊匹配各类归一、唯一性/重叠/空文本/无变化错误文案、宽松参数解析、路径规范化、写范围判断（含软链、`..`、新文件祖先目录）。
- **Prior art**：仓库目前为空，无既有测试；以 pi 源码中的行为与错误文案作为期望值来源。
- **执行环境**：本地无 Rust 环境，所有测试在 GitHub Actions 上运行，push 到 `dev/` 前缀分支时触发（clippy、全部测试；不检查格式）。

## Out of Scope

- `bash` 工具（后续加入，协议已预留）。
- ls / find / grep 等文件发现工具。
- 多用户、账号体系、权限分级。
- 内置 TLS / ACME。
- 一个 key 同时服务多个 runner、负载均衡或 runner 高可用。
- 按 MCP 会话隔离当前 project。
- runner 配置热加载（修改后需重启）。
- runner 注册凭证与 agent 访问凭证分离（当前共用同一个 UUID key）。
- Windows 上的 server 管理通道（unix socket）。

## Further Notes

- **安全取舍**：MCP 地址中的 UUID 即全部凭证，泄露等同于给出该 runner 上任意文件的读权限与 project 范围内的写权限；同一 key 也可被用来冒充 runner。这是为简化使用而接受的风险，key 泄露时应立即 `key rm`。
- **读不受限**：`read` 可读取 runner 上该进程有权限读取的任何文件（包括密钥文件），应以低权限用户运行 runner。
- 参考实现：pi coding agent 的 read / write / edit / truncate / path 工具源码；官方 Rust MCP SDK 的 Streamable HTTP 服务端。
