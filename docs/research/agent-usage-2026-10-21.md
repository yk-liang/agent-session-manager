# 本地用量调研

记录 2026 年 10 月 21 日至 22 日的日志检查、上游源码核对和首次实现。调研基线为 `8826970`（v0.1.22）。现行规则见[用量模块说明](../../src-tauri/src/usage/README.md)。

## 本机样本

首轮扫描只读取数据结构和计量字段。记录仍在追加，以下数字对应当时的样本。

| 来源 | 日志样本 | 主要发现 |
| --- | --- | --- |
| Qoder | 282 份 JSONL，约 111.5 MiB | 75 份日志共出现 1,425 条 `usage`，其中 1,250 条包含 Credits。缺少用量的分块可能在后续记录中补齐 |
| Claude Code | 191 份 JSONL，约 219.9 MiB | 20,292 条带 `usage` 的助手记录对应 7,544 个消息 ID。838 组 ID 跨文件重复，1,476 组存在多个计量版本 |
| Codex | 86 份 JSONL，约 1,451.8 MiB | 78 份日志含 11,217 条有效计量通知，另有 3 条通知缺少计量。411 组相邻累计快照相同 |
| OpenCode | 2,123 条助手消息 | 2,028 条同时有一个 `step-finish`，其 Token 与消息记录一致 |
| Pi | 默认目录无历史 JSONL | 字段和分叉规则根据本机安装包与 v0.87.0 源码核对 |

Claude Code 目录内还有 135 份子 Agent 日志。OpenCode 的另一份快照中，161 个会话里有 67 个记录了父会话。用量采集需要覆盖会话列表默认隐藏的子会话和归档日志。

## 来源与计量

| 来源 | 总量规则 | 去重与边界 |
| --- | --- | --- |
| Codex | `input_tokens + output_tokens`。缓存读写属于输入，推理属于输出 | 按线程历史顺序处理逐响应记录和累计快照，结合分页边界和父线程识别回放 |
| Claude Code | Anthropic 原生格式的输入由未缓存输入、缓存读取及缓存写入组成 | 同一响应的流式分块、回放和分叉需要归并。缓存写入的 5 分钟和 1 小时分项属于总写入 |
| Qoder | 保留已记录的输入、输出、缓存及 Credits。缓存与输入的包含关系在调研时仍未确认 | 使用已确认的请求或消息身份去重。缺字段与明确记录的零值分别处理 |
| OpenCode v1.19.3 | `input + cache.read + cache.write + output + reasoning` | 同一份计量可能同时写入助手消息和 `step-finish`。分叉复制的历史也需去重 |
| Pi v0.87.0 | `input + cacheRead + cacheWrite + output`。推理属于输出，1 小时缓存写入属于总缓存写入 | 读取全部分支中的已保存计量，按父会话与条目身份识别分叉副本 |

### Codex

`token_count` 通知含 `total_token_usage` 和 `last_token_usage`。样本中有同一线程的多个物理分页，也有共享父历史的分叉。应按逻辑线程建立累计基线，并保留缺页、缺基线或计数回退的状态。模型取当时记录的标识，日期取请求时间。缺少时间的记录单独保留。

当时安装的 Codex 可通过 `codex app-server generate-json-schema --experimental --out <目录>` 导出协议。导出结果中的 `ThreadTokenUsageUpdatedNotification` 包含线程、轮次及 `last` 和 `total` 计量。账号用量和额度窗口由独立的账号接口提供。额度百分比与本地 Token 都无法直接换算为订阅扣费。调研只导出了协议，未查询账号数据。[Codex App Server](https://learn.chatgpt.com/docs/app-server) · [Codex 登录说明](https://learn.chatgpt.com/docs/auth)

### Claude Code

Agent SDK 的同一步骤可产生多条共享 `message.id` 的助手消息。流式记录中的输出计量可能是占位值，应以本机日志确认完整版本。子 Agent 保持独立调用身份。启用 `--no-session-persistence` 的调用没有可供回读的本地历史。[SDK 用量说明](https://code.claude.com/docs/en/agent-sdk/cost-tracking) · [缓存字段](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) · [CLI 参考](https://code.claude.com/docs/en/cli-reference)

### Qoder

`/usage` 和 SDK `getUsageInfo()` 提供账号额度与会话快照。单次请求中，`credits` 为折扣后用量，`original_credits` 为折扣前用量，`billable` 为是否计入额度。SDK `result.total_credits` 为累计值，本机样本没有顶层 `result` 记录。[CLI 用量](https://docs.qoder.com/cli/usage) · [SDK 计量](https://docs.qoder.com/cli/sdk/cost-usage) · [Credits](https://docs.qoder.com/Credits)

### OpenCode

v1.19.3 的 `getUsage` 从总输入扣除缓存读写，也从输出扣除推理。旧版 v1.2.27 依据供应商元数据判断输入是否已扣缓存。适配时使用对应版本已保存的分项，保留上游总量。[v1.19.3 计量源码](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/session/session.ts#L333) · [v1.19.3 写入流程](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/session/processor.ts#L435) · [v1.2.27 计量源码](https://github.com/anomalyco/opencode/blob/v1.2.27/packages/opencode/src/session/index.ts#L747)

### Pi

`getSessionStats()` 读取助手消息、独立的 `usage` 条目，以及压缩摘要、分支摘要和工具结果中的可选计量。分叉会复制祖先条目及其 ID，`parentSession` 指向父会话。短条目 ID 的作用域为父会话链。[v0.87.0 计量类型](https://github.com/earendil-works/pi/blob/v0.87.0/packages/ai/src/types.ts#L396) · [会话统计](https://github.com/earendil-works/pi/blob/v0.87.0/packages/coding-agent/src/core/agent-session.ts) · [会话分叉](https://github.com/earendil-works/pi/blob/v0.87.0/packages/coding-agent/src/core/session-manager.ts#L1513)

## ccusage 评估

核对的源码为 `06a3343369c026e7e199aa4fa495bd730a0e17c4`，CLI 版本为 20.0.40。该版本使用 Rust workspace，npm 启动器选择平台二进制。Rust crate 标记为不可发布，公开函数的兼容性需按固定提交核对。[workspace](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/Cargo.toml) · [v20.0.40](https://github.com/ccusage/ccusage/releases/tag/v20.0.40)

直接链接 OpenCode 适配器会引入 `sqlite3-src`。它与项目当时使用的 `libsqlite3-sys` 均声明 `links = "sqlite3"`，Cargo 的依赖约束不允许二者共存。调研通过 manifest 确认冲突。[OpenCode 适配器依赖](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/opencode/Cargo.toml) · [Cargo links 约束](https://doc.rust-lang.org/cargo/reference/build-scripts.html#the-links-manifest-key)

复用时需要额外处理 Qoder、Pi 的非助手用量条目，以及 Codex 本机分页的 `history_base` 边界。采用内置 Rust 适配器，参考 Claude 的消息去重、Codex 的回放与差额逻辑、OpenCode 的字段映射和 Pi 的分叉识别。许可随应用分发。[固定版本适配器](https://github.com/ccusage/ccusage/tree/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters) · [MIT 许可](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/apps/ccusage/LICENSE)

## 首次实现与验证

v0.1.23 引入独立的用量模块和增量缓存。JSONL 只读取新追加的完整行，文件替换、截断或重写后重新解析。Codex 分页按字节和序号边界去重。OpenCode 从只读数据库提取计量。用量覆盖已写入本机的调用，包括子 Agent 和工具相关请求。

首次对照使用固定版本的 ccusage 20.0.40。运行 `ccusage <agent> daily --json --offline --no-cost --timezone Asia/Singapore`，仅比较 2026 年 10 月 22 日之前的统计。OpenCode 的 37 天逐日总量一致。Claude Code 有一天相差 652,455 Token，对应同一响应的多条流式与最终记录。Codex 少数日期仍有差异，相关分页、回放与损坏样本补入回归。Pi 当时只有合成样本。[v20.0.40 CLI 选项](https://github.com/ccusage/ccusage/blob/v20.0.40/docs/guide/cli-options.md)

同一台机器的 release 构建中，约 4.7 GB 的解压后 Codex 历史重建耗时约 4.6 秒，未变日志检查约 0.33–0.40 秒。其他有历史的来源热检查约 4–38 毫秒。后续性能对照见[用量模块说明](../../src-tauri/src/usage/README.md#存储性能对照)。

v0.1.24 将已采集用量移入累计账本，并迁移旧缓存中的计量。应用内删除会话前先保存可读用量。源文件移除后，账本仍保留已采集的计量和用于去重的身份关系。文件监听与页面切换解耦。
