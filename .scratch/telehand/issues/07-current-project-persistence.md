# 07: 当前 project 持久化

**What to build:** agent 选择的当前 project 挂在 key 上并持久化到 server 的状态文件，server 重启、runner 重连后依然保留。状态文件只由运行中的 server 进程拥有：启动时读取一次，运行时变更延迟 100ms 写盘（等待期间再有变更则重置计时），以临时文件 + 原子重命名写入，收到退出信号时立即写盘。未选择 project，或所选 project 不在 runner 上报的列表中时，文件工具报错并提示先 `list_project` / `select_project`。

**Blocked by:** 01

**Status:** done

- [x] 状态文件结构：以 key 为键，值含备注名、创建时间、当前 project（可空）
- [x] `select_project` 更新内存并触发延迟写盘；连续变更只在最后一次变更 100ms 后写盘一次
- [x] 写盘采用临时文件 + 原子重命名
- [x] 收到 Ctrl-C / SIGTERM 时立即写盘再退出
- [x] server 重启后 `current_project` 返回之前的选择
- [x] 未选择时 `read` / `write` / `edit` 返回提示先 list/select 的错误
- [x] 所选 project 已不在 runner 上报列表中时同样报错（不自动回退）
- [x] 端到端测试覆盖：选择后重启 server 仍保留；未选择时报错；所选 project 被删除后报错
