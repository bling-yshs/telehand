# 03: `read` 读取图片

**What to build:** agent 用 `read` 读取图片文件（jpg/png/gif/webp/bmp）时，得到 MCP image 内容和一条说明文字，过大的图片按 pi 的规则自动缩放后再返回，避免撑爆上下文。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] 通过内容嗅探（而非仅扩展名）识别图片 MIME 类型
- [ ] 返回 MCP image 内容块，外加与 pi 一致的说明文字（含缩放提示）
- [ ] 缩放规则（尺寸/大小上限）与 pi 一致
- [ ] 非图片的二进制/文本文件仍走文本路径，不受影响
- [ ] 文件工具模块层单测覆盖：小图原样返回、大图被缩放、MIME 识别
- [ ] 端到端测试：读取 png 得到 image 内容块
