# Agent Session Manager

在一个 macOS 窗口里管理 **Qoder CLI、Claude Code、Codex、OpenCode 和 Pi** 的本地会话、配置与用量。

[下载最新版本](https://github.com/yk-liang/agent-session-manager/releases/latest) · [反馈问题](https://github.com/yk-liang/agent-session-manager/issues)

## 核心功能

- **集中管理会话**：按 Agent、项目浏览，搜索标题和对话正文；复制恢复命令或在终端继续会话，支持对应 Agent 的分叉与批量删除。
- **更清晰地阅读历史**：支持 Markdown、消息导览和长对话浏览；思考、工具调用和自动注入的上下文按需展开。
- **查看用量与请求明细**：按 Agent、模型和时间筛选 Token 用量。今日趋势按小时，近 7/30 天按日；请求表展示客户端来源、项目目录、模型、输入/输出、缓存、TPS 和耗时。
- **保留已采集的历史用量**：删除会话后，已保存的用量仍可查询。使用时自动同步；带开发者签名的构建还可开启后台保留。
- **管理配置方案**：保存和切换不同 Agent 的配置，修改前查看差异，自动备份并支持恢复。

Codex 支持 CLI、IDE 和桌面应用写入的本地会话；OpenCode 兼容 v1 和 v2 数据格式。

## 安装与更新

要求 **macOS 13 或更新版本**，支持 Apple Silicon（M 系列）和 Intel Mac。

1. 从 [Releases](https://github.com/yk-liang/agent-session-manager/releases/latest) 下载文件名以 `universal.dmg` 结尾的安装包。
2. 打开 DMG，将 **Agent Session Manager** 拖入 **Applications**。也可下载 `.app.zip`，解压后移动应用。
3. 更新时先退出旧版，再用新版替换应用。会话、配置和已采集用量保存在应用包之外。

当前安装包尚未经过 Apple Developer ID 公证。首次打开若被 macOS 阻止，在确认下载来源后，按 [Apple 官方步骤](https://support.apple.com/zh-cn/102445) 进入“系统设置 → 隐私与安全性 → 仍要打开”。Release 同时提供 SHA-256 校验文件。

**图形应用不需要 Node.js 或 ccusage。** 在终端恢复、分叉会话，以及删除 OpenCode 会话，需要安装对应的 Agent CLI。

## 开始使用

- 打开应用后，侧栏会列出本机已有数据的 Agent。选择 Agent 和项目即可浏览会话，使用搜索框查找历史内容。
- 在 **用量** 页查看汇总、趋势和请求明细。项目名称取自日志记录的工作目录，悬停或展开请求可查看完整路径；来源优先显示日志中的客户端名称。
- 首次同步会显示当前来源、文件进度和耗时。历史较多时可能需要几分钟，期间可以切换页面；每个来源完成后会自动显示已保存的结果。
- 在 **配置** 页保存方案、预览差异并切换配置。应用保存文件后，相关 Agent 是否需要重启取决于它自身的配置加载方式。

默认读取各 Agent 的本地数据目录：

| Agent | 常见数据位置 |
| --- | --- |
| Qoder CLI | `~/.qoder/projects/` |
| Claude Code | `~/.claude/projects/` |
| Codex | `~/.codex/sessions/`、`~/.codex/archived_sessions/` |
| OpenCode | `~/.local/share/opencode/` |
| Pi | `~/.pi/agent/sessions/` |

没有看到会话时，先确认对应 Agent 已在本机生成日志。配置了自定义数据目录时，应用需要能访问相同目录。

## 用量与数据说明

- 统计来自本机读取到的日志，不等同于服务商账单。缓存和思考用量作为细分显示，不重复加入总 Token。
- `—` 表示日志没有记录该信息；`≈` 表示估算值。TPS 按输出 Token 除以整个请求耗时计算，可能包含等待时间；费用只显示日志已有金额。
- 已采集用量会在删除原会话后保留；从未采集就已被删除的日志无法还原。当前本地签名安装包使用按需同步，退出应用后不继续采集；后台保留需要带开发者签名的构建。
- 普通会话删除会移入系统回收站；**OpenCode 通过官方 CLI 删除，无法通过本应用恢复**。
- 用量账本位于 `~/Library/Application Support/Agent Session Manager/`；配置方案与备份位于 `~/.agent-session-manager/`。关闭后台采集并退出应用后，备份这些完整目录可保留相应应用数据，原始会话仍由各 Agent 管理。

## 反馈与贡献

遇到问题可在 [Issues](https://github.com/yk-liang/agent-session-manager/issues) 中附上应用版本、Agent 版本和复现步骤。请先移除截图或日志中的对话内容、密钥等私人信息。

构建和测试说明见 [开发文档](docs/DEVELOPMENT.md)。

## 许可与致谢

项目采用 [MIT 许可证](LICENSE)。感谢 [agent-session-cleaner](https://github.com/haowang02/agent-session-cleaner)、[ccusage](https://github.com/ccusage/ccusage) 和 [agent-events](https://github.com/haowang02/agent-events) 提供参考。第三方许可见 [public/licenses](public/licenses)，Agent 标识保留各自的权利声明。
