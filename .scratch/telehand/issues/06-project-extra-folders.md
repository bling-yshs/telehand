# 06: project 额外文件夹与管理命令

**What to build:** runner 主人能给 project 添加额外文件夹以扩大 agent 的可写范围（相对路径基准仍是主文件夹），能列出和删除 project；修改配置后 CLI 提示需重启 runner 才生效。agent 通过 `list_project` / `current_project` 能看到额外文件夹。

**Blocked by:** 04

**Status:** ready-for-agent

- [ ] runner `project folder add <name> <dir>` 为已有 project 添加额外文件夹；project 不存在时报错
- [ ] runner `project list` 列出所有 project 及其主文件夹、额外文件夹
- [ ] runner `project remove <name>` 删除 project
- [ ] 修改配置的命令执行后提示需重启 `run` 才生效
- [ ] 额外文件夹内的 write/edit 被允许；相对路径仍相对主文件夹解析
- [ ] `list_project` / `current_project` 返回中包含额外文件夹
- [ ] 端到端测试：添加额外文件夹并重启 runner 后，向额外文件夹 write 成功；未添加前被拒
