# 02: `read` 文本行为与 pi 一致

**What to build:** agent 用 `read` 读大文件时，行为与 pi coding agent 完全一致：可以用 1 起始的 `offset` / `limit` 分段读取；输出在 2000 行或 50KB（先到为准）处截断，并附带"用 offset=N 继续"的提示；`offset` 越界时报错并告知总行数；路径写法（`~`、前导 `@`、`file://`、Unicode 空格变体、macOS 文件名变体回退）都被正确规范化。参数名、工具描述、提示与错误文案照搬 pi。

**Blocked by:** 01

**Status:** done

- [x] `read` 的参数 schema 与描述文案与 pi 一致
- [x] `offset`（1 起始）与 `limit` 生效；先应用 limit 再做截断
- [x] 按行数或字节数先到者截断，不返回半行；三种截断提示文案（行数、字节上限、limit 提前停止）与 pi 一致
- [x] 首行即超过 50KB 时给出与 pi 一致的提示
- [x] `offset` 超出文件末尾时返回含总行数的错误
- [x] 路径规范化覆盖 `~`、`~/`、前导 `@`、`file://`、Unicode 空格；文件不存在时按 pi 尝试 macOS 变体（窄不换行空格、NFD、弯撇号）
- [x] 文件工具模块层单测覆盖截断边界、offset 越界、各路径规范化分支
- [x] 至少一个端到端测试验证分页读取与截断提示经 MCP 正确返回
