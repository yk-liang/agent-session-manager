# 各 Agent 本地用量统计调研

调研日期：2026-10-21，时区 Asia/Singapore；项目基线提交 `8826970`（v0.1.22）。依据本机只读日志检查、已安装版本的协议/源码及第一方文档。本文保留原始调研与候选方案，历史段落中的“尚未实现/未运行”指对应调研阶段；后续实施状态见文末。调研没有通过待统计 CLI 发起模型调用，也没有调用账号额度/账单 API。

2026-10-22 补充：用户已明确以“用了多少 token”为核心。费用/Credits 的章节保留为数据来源说明，当前产品建议以文末的 ccusage 复用和 token 汇总方案为准。

2026-10-22，0.1.24 需求修正：用户明确需要长期累计消耗。原始调研中“仅统计现存日志、删除后减少”的边界已被替换；现行实现使用独立持久化账本，删除会话保留已经采集的用量，见文末修正记录。

## 结论

可以增加“用量”tab，五个 Agent 都存在可接入的计量来源，但能力和历史覆盖程度不同。应承诺“当前机器保留记录中的用量”，不承诺“账号完整消耗或实际账单”。

| Agent | 推荐本地来源 | 可读取内容 | 当前约束 |
|---|---|---|---|
| Codex | rollout JSONL 的 `event_msg.payload.type=token_count` | 输入、缓存读取/写入、输出、思考及总 token | 累计快照、重复通知、分页、fork/子 Agent 继承都要处理；本地没有通用实际金额 |
| Claude Code | 项目及子 Agent JSONL 的 `message.usage` | 未缓存输入、缓存创建/读取、输出；部分版本还有缓存寿命/输出细分 | 分块、回放、分叉需去重；本地模型价格计算只能称估算 |
| Qoder CLI | JSONL 的 `message.usage` | 已记录的输入、输出、缓存及 request Credits | 部分日志无计量字段；input/cache 包含关系未证实，不能先给出统一总 token |
| OpenCode | SQLite `message.data` 的助手 `tokens/cost` | 输入、输出、思考、缓存读写、部分记录的上游总量和记录成本 | message 与 step-finish 不可相加；价格缺失也可能记 0，分叉需去重 |
| Pi | session JSONL 中所有保存了 usage 的 entry | 输入、输出、缓存读写、可选思考、分项估算成本 | 不能只取当前对话分支；本机无历史样本，需补真实运行回归 |

首期建议统计 token、按日活动、模型/项目/Agent 分布及参与会话数；Qoder Credits 单列。费用仅在来源和定价口径可解释时显示，并与 Credits、账号额度分开。统一总量暂时只合并语义已确认的来源，缺项用覆盖状态说明，不能把 Qoder 未确认的缓存项强行相加。

## 本机只读取证快照

扫描默认数据目录，仅读取结构和计量字段，未将对话正文或凭据写入本文。目录中包含 CLI、桌面端、SDK 或子 Agent 记录，这是一轮数据存在性检查，不是按最终产品范围结算的报表。后台会话仍在追加，因此各次检查的计数会增长，不能把它们看作冻结快照。

| 来源 | 首轮字段检查结果 |
|---|---|
| Qoder | 282 个 JSONL、约 111.5 MiB；75 个文件出现 `usage`，共 1,425 条，1,250 条包含 Credits 字段 |
| Claude Code | 191 个 JSONL、约 219.9 MiB；20,292 条助手记录带 usage，但只有 7,544 个不同的 `message.id` |
| Codex | 86 个物理 JSONL、约 1,451.8 MiB；78 个文件含有效 token info，共 11,217 条；info 为空的另有 3 条 |
| OpenCode | 2,123 条助手消息均有 tokens/cost 字段；其中 1,570 条 cost 大于 0。其余 0 不足以证明无费用 |
| Pi | 默认 `~/.pi/agent/sessions` 没有 JSONL，支持情况来自本机安装包和同版本第一方源码 |

进一步核验：

- Claude 的重复 ID 中，838 组跨文件出现，1,476 组存在不同 usage 版本。这证明不能逐行求和，也不能只对每个文件单独去重；最终值选择也需核验，不能无条件保留第一次记录。
- Qoder 缺 usage 的记录既有同一消息的分块，又有只保存 `content/role` 的简化 SDK 记录。部分无 usage 的 ID 后来有完整记录，因此“有 usage 的行占比”不是请求计量覆盖率。首轮 75/282 也只是物理文件出现率，不是完整会话比例。后续检查已增长至 76/283。
- Qoder 本机有实际 `usage.request_id`；检查中有 Credits=0 且 billable=false 的记录，也有 Credits>0 且 billable=true 的记录。缺字段、明确的零和不可计费要区分。未发现顶层 result 记录，不能依赖 SDK result 累计字段补历史。
- Codex 检查中存在 411 次相邻累计快照完全相同；直接把 `last_token_usage` 或累计 `total_token_usage` 的每次通知相加都会重复。86 个物理文件并非 86 条独立会话，有同 ID 分页及继承历史。
- OpenCode 本机有 2,028 条带 step-finish 的助手消息，每条恰有一个 step；message.tokens 与对应 step.tokens 一致，message.cost 与 step 的成本之和一致，未发现差异。另 95 条没有 step-finish，不能据此假设它们全是完整请求；未来多 step 仍需兼容测试。
- Claude 目录内有 135 个子 Agent 日志；Codex 也出现 subagent source，OpenCode 有 parent session。沿用会话页的隐藏过滤会漏掉这些消耗。

## Codex：本地 token 与账号用量分开

本机 rollout 中的计量形态已确认：

```text
event_msg.payload.type = "token_count"
payload.info.total_token_usage   # 累计快照
payload.info.last_token_usage    # 最近一次统计
  input_tokens
  cached_input_tokens
  cache_write_input_tokens
  output_tokens
  reasoning_output_tokens
  total_tokens
payload.rate_limits             # 额度窗口快照，不是 token 消耗
```

本机已安装 Codex 可通过 `codex app-server generate-json-schema --experimental --out <临时目录>` 导出协议。该导出未启动推理；临时目录已清理。导出的 `ThreadTokenUsageUpdatedNotification` 包含 `threadId/turnId/tokenUsage`，`ThreadTokenUsage` 包含 `last/total/modelContextWindow`，分项对应上面的 camelCase token 字段。官方也提供 `thread/tokenUsage/updated` 通知。来源：[Codex App Server](https://learn.chatgpt.com/docs/app-server)。这是协议能力与本机字段证据，不表示本次已接入该通知。

本机已读累计值满足 `total_tokens = input_tokens + output_tokens`，缓存读写是输入细分，思考是输出细分，不能再次加进 total。建议以逻辑会话的有效历史为顺序，识别计数基线与重复快照，再计算可归属到时间/模型的增量；不要在每个文件末尾取一次总量就求和。对 fork、子 Agent 的继承前缀要与原调用去重；遇到计数回退、缺页或缺基线，应标记不完整，不能简单取 max 或把负增量静默清零。

模型维度来自当时的历史 model/turn context；发生重路由且没有可确认的实际模型时保留“未知”，不能拿当前默认模型补齐。时间维度使用计量事件时间，统一转用户时区后归日，不用日志 mtime 替代真实发生时间。

Codex 还有两类账号接口：`account/usage/read` 返回 token activity 摘要及可选日桶，`account/rateLimits/read` 返回额度窗口百分比/重置时间。前者要求 Codex 服务支持的身份，API-key-only/Bedrock 不支持；后者百分比不能反算成本。二者都是账号服务数据，可包含不同于本机日志的范围，不能与本地数据合并累加；本轮未调用。来源：[App Server account APIs](https://learn.chatgpt.com/docs/app-server)。

ChatGPT 登录和 API key 登录的计费路径也不同；本地 tokens 无法直接换算 ChatGPT 订阅扣费。若以后增加美元估算，须明确标价来源、模型、供应商、缓存写入、服务档位和有效日期。来源：[OpenAI authentication](https://learn.chatgpt.com/docs/auth)。

## 可实现性与数据边界

五个 Agent 都有可用于统计的记录或接口，但“本机保留的历史消耗”“当前上下文占用”“账号剩余额度”“最终账单”是不同数据。历史文件被删除、禁用持久化、版本未写字段或调用发生在其他机器上时，本地统计无法补全。空缺应显示“未知/未记录”，不能补成 0。成本、Credits 与 token 也不能混为一种单位。

## Claude Code

可从本地会话助手记录读取 `message.usage`；第一方 Agent SDK 文档确认一次模型步骤可能分成多个 Assistant 消息，且它们共享同一个 `message.id`，因此必须按模型消息 ID 去重。`result.total_cost_usd` 与 `modelUsage` 属于累积估算；恢复会话会带回之前的累计值，不能把每个 result 相加，也不能与逐请求统计再加一次。该页面还指出 SDK 流中的逐步骤 `output_tokens` 可能是占位值；不能未经本机格式核验就把 SDK 流的规则套到所有历史 JSONL。来源：[Agent SDK cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking)。

Anthropic 原生 usage 的输入由三个互斥部分组成：`input_tokens` 是未缓存输入，`cache_creation_input_tokens` 是缓存写入，`cache_read_input_tokens` 是缓存读取；三项相加才是总输入。缓存写入如果还记录 5 分钟/1 小时细分，细分是总写入的组成部分，不能再叠加。该语义不能仅凭同名字段推广到 Qoder 或其他代理服务。来源：[Prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)。

统计范围应包含主会话及本地保留的 subagent 调用。官方 SDK 文档说明 result 的单轮 `usage` 只包含主循环，而累积成本/模型统计可涉及子代理，二者范围不同。应选择“全部原始调用去重后统计”或“对应范围的权威累积快照”之一，避免主子代理重复。来源：[Agent SDK cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking)。

费用应标为“估算成本”，不能当实际扣款；官方明确要求最终结算以 Usage and Cost API 或 Console 为准。订阅额度也不能由本地 token 总量反推。`--no-session-persistence` 会禁止会话落盘，因此缺历史不代表没消耗。来源：[SDK cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking)、[CLI reference](https://code.claude.com/docs/en/cli-reference)。

## Qoder CLI

官方可见用量分为三层：`/usage` 展示账号当前账期的套餐/Credits及本次会话时长和代码改动；SDK `getUsageInfo()` 提供账号额度与当前 CLI 会话快照；Assistant 的 `message.message.usage` 提供单次完成请求数据。账号额度需要有效认证，来源是账号服务，不能当作完全本地接口。来源：[CLI usage](https://docs.qoder.com/cli/usage)、[SDK cost and usage](https://docs.qoder.com/cli/sdk/cost-usage)。

官方明确定义：`credits` 是优惠/折扣后 Credits，`original_credits` 是折扣前值，`billable` 表示是否计入用户 Credits；旧版本可能缺失这些可选字段，缺失不能当 0。`total_credits` 是从当前 CLI 会话开始累计到该 result 的值，不能累加多个 result，也不能把 request Credits 与 result 累计值合并相加。账号剩余额度应读取账号 quota bucket，而不是用日志相减推算。来源：[SDK cost and usage](https://docs.qoder.com/cli/sdk/cost-usage)。本次本机扫描未发现顶层 `type:"result"` 历史记录，所以 result 累计字段只是 SDK/未来导入数据的可选来源，不能补回当前日志缺失历史。

Credits 是 Qoder 的资源计量单位，不是美元或人民币；消耗与模型/任务相关，失败模型请求按官方规则不扣 Credits。跨 Qoder IDE、CLI、云端等产品的账号总额可能大于本机 CLI 历史和。来源：[Credits](https://docs.qoder.com/Credits)。

待确认：当前官方 cost-usage 文档没有给出 `input_tokens` 与 `cache_read_input_tokens`、`cache_creation_input_tokens` 的互斥/包含契约，也没有保证所有历史版本均将完整 token/credits 落入本地 JSONL。本机虽然可见同名 token 字段，也不能直接套 Anthropic 的“输入+缓存”公式。首期可独立展示“日志记录 input/output/cache 明细”和“已记录 Credits”，对 Qoder 总输入/总 token 的归一化保留未知标记。请求去重优先验证本机 `usage.request_id`、`message.id` 的实际重复关系；缺 ID 的简化 SDK 日志不应冒充完整计量凭据。这一段是实施约束/待验证项，不是官方承诺。

## OpenCode（核对本机版本 1.19.3 的第一方源码）

本机数据库助手消息里的 `tokens` 可分为 `input`、`output`、`reasoning`、`cache.read`、`cache.write`，另有 `total` 与 `cost`。1.19.3 的 `Session.getUsage` 先从供应商总输入中扣缓存读写得到未缓存 `input`；存储的 `output` 已扣除 `reasoning`，故这一版本 reasoning 与 output 是并列项。这和 Pi 的“reasoning 包含在 output 中”不同。`tokens.total` 直接取上游值，缺失或不一致时不能未经验证补写。来源：[v1.19.3 session.ts](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/session/session.ts#L333)。

同一次 `step-finish` 的 usage 同时写入 Assistant 消息和 `step-finish` part。Assistant.cost 在此累计，Assistant.tokens 在此赋值；再把 message 与 part 相加会重复。建议按唯一 message ID 读取已保存的助手统计，part 只用于核对/兼容，不重复叠加；未来遇到一个 message 多 step，应单独验证赋值和累计差异。来源：[v1.19.3 processor.ts](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/session/processor.ts#L435)。

`cost` 通常由模型价格配置乘以分项 token 得到，缺价格默认 0；存在 Copilot 特殊计量路径，因此 0 不证明供应商免费，且此字段不能统一称为最终账单金额。应展示“OpenCode 记录成本”，保留来源与未知定价状态。来源：[v1.19.3 session.ts getUsage](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/session/session.ts#L333)。

OpenCode session 有 `parentID`，可表示子会话；统计“各 Agent 全部本机调用”不能只沿用仅列 root session 的会话目录。不同版本曾变化 SDK 的缓存归一化方式，1.2.27 仍按 Anthropic/Bedrock metadata 判断是否排除缓存，1.19.3 则按新版 SDK统一扣缓存；解析历史已存分项比用当前供应商公式重算更稳妥。来源：[v1.19.3 session metadata](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/session/session.ts)、[v1.2.27 getUsage](https://github.com/anomalyco/opencode/blob/v1.2.27/packages/opencode/src/session/index.ts#L747)。分叉是否复制了可计费历史、跨分叉全局去重键如何识别，仍需实现前用 fork 源码/受控样本核对，不能直接把每个 session 总数相加。

## Pi（核对本机 @earendil-works/pi-coding-agent 0.87.0）

`AssistantMessage` 保存 `provider`、`model`、可选 `responseModel`/`responseId` 与 `usage`。usage 有 `input`、`output`、`cacheRead`、`cacheWrite`、`totalTokens`、分项 `cost` 和 `cost.total`；可选 `reasoning` 明确是 output 的子集，不能再次计入总 token；`cacheWrite1h` 也是 cacheWrite 子集。来源：[v0.87.0 types.ts](https://github.com/earendil-works/pi/blob/v0.87.0/packages/ai/src/types.ts#L396)。

`getSessionStats()` 遍历 `sessionManager.getEntries()` 全部条目，包含已压缩历史及未处于当前分支的历史；不仅累加助手 usage，还处理独立 `type:"usage"`（例如 cache_warm）、compaction/branch_summary 的可选 usage、toolResult 的可选 usage。总 token 是 input+output+cacheRead+cacheWrite。只读当前对话分支或只加助手消息会漏掉真实保存的调用。来源：[v0.87.0 AgentSession](https://github.com/earendil-works/pi/blob/v0.87.0/packages/coding-agent/src/core/agent-session.ts)、[session-manager.ts UsageEntry](https://github.com/earendil-works/pi/blob/v0.87.0/packages/coding-agent/src/core/session-manager.ts#L76)。安装包内锚点：`dist/core/agent-session.js:3043`、`dist/core/usage-totals.js:10`；相对于 `@earendil-works/pi-coding-agent/` 包目录。

默认使用 `~/.pi/agent/sessions/<encoded-cwd>/*.jsonl`，自定义 session 目录及 inMemory 会改变可见范围。`createBranchedSession()` 会将既有根到叶路径复制到新文件并保留 entry ID，header.parentSession 指向原文件；forkFrom 也会复制源条目。因此全机求和时不能简单相加各文件统计，需结合 lineage、entry ID、provider responseId 去除复制前缀；短 entry ID 不宜在所有无关 session 间全局去重。来源：[v0.87.0 session-manager.ts](https://github.com/earendil-works/pi/blob/v0.87.0/packages/coding-agent/src/core/session-manager.ts#L1513)。

Pi 的 cost 由模型单价与 usage 计算（含长缓存写入/上下文阶梯），应标为记录的估算值；不证明订阅真实扣款或账单。默认不内置子代理；扩展可以另起调用/保存独立日志，只有进入上述 usage/会话条目的消耗才能被本地扫描覆盖。来源：[v0.87.0 models.ts calculateCost](https://github.com/earendil-works/pi/blob/v0.87.0/packages/ai/src/models.ts#L832)、[Pi 官方仓库](https://github.com/earendil-works/pi)。当前机器默认 Pi sessions 无日志的结论属于后续本机样本证据，不代表 Pi 不支持统计。

## 实现前仍需核对的事项

- Claude：本机 JSONL 流式/回放与 message ID 的最终 usage 合并规则；subagent、分叉与拷贝日志的全局去重。
- Qoder：不同模型/版本 input 与 cache 的包含关系；request_id/message.id 的重复与最终记录规则；缺 Credits 的请求应计为未知覆盖。
- OpenCode：fork 历史复制与全机去重；旧数据库版本的 output/reasoning 语义；上游缺统计时被置 0 的可识别性。
- Pi：旧版本缺 reasoning/summary usage 等字段的兼容；分叉共享前缀全局去重；扩展是否单独持久化用量。

这些边界不妨碍增加用量页，但决定它能承诺“本机可见历史统计”，不能承诺“所有账号的完整账单”。

## 建议的统计规则

1. **原始记录与归一化结果同时保留来源。** 按 Agent/版本记录输入是否已含缓存、输出是否已含思考，保存原始字段、数据来源、版本和完整性状态。不要全局使用一条 `input+output+cache+reasoning` 公式。
2. **只统计保存的计量，不用文本长度估 token。** 模型请求还包含系统提示、历史、工具定义和工具结果；助手气泡数也不等于请求数。没有稳定请求/响应 ID 的来源，不承诺精确请求数。
3. **同一调用只计一次。** Claude 按请求/模型消息身份合并最终计量版本；Qoder 按已验证的 request/message ID；OpenCode 按 message 或 step-finish 的一种权威来源；Pi 用分支 lineage/entry ID；Codex 结合逻辑会话、分页边界和继承前缀建立基线。不能仅按标题、路径或模型名去重。
4. **同一真实请求的工具调用仍是用量。** 不要求它有可见文本，不能沿用“真实对话量”过滤。子 Agent 要归到对应 Agent 的消耗，但继承的历史和父任务的权威累计统计不能再算一次。
5. **归档与删除分开。** 产品默认范围建议包括本机尚保存的归档记录和子 Agent，不只统计侧栏可见会话。本机 Codex 的 archived_sessions 目前没有 JSONL；若以后出现应扫描。OpenCode 后续快照为 161 条 session，其中 67 条有 parent_id，说明隐藏子会话确实存在。已删除日志无法凭本地恢复；首期定义为“当前本机保留记录”，不偷偷把缓存当永久计费账本。
6. **未知与零分开。** 缺字段、没有定价、请求未完成、计数基线不全及上游明确返回 0 是不同状态。各 Agent、日期、模型的覆盖状态应能单独查看。不要用缺字段的百分比直接推导花费。

归一化参考（仅适用于已确认语义的来源/版本）：

| 来源 | 总 token 参考 | 不可重复相加的字段 |
|---|---|---|
| Codex 本机记录 | `input_tokens + output_tokens` | cached/cache_write 属 input 细分，reasoning 属 output 细分 |
| Claude 的 Anthropic 原生 usage | `input_tokens + cache_creation_input_tokens + cache_read_input_tokens + output_tokens` | cache TTL、iterations 等细分不能再叠加；代理服务需另核验 |
| OpenCode 1.19.3 | `input + cache.read + cache.write + output + reasoning` | 同一计量不可同时取 message 和 part；上游 total 需单独验证 |
| Pi 0.87.0 | `input + cacheRead + cacheWrite + output` | reasoning 属 output，cacheWrite1h 属 cacheWrite |
| Qoder | 暂不推定统一总量；分别保留日志 input/output/cache 和 Credits | input/cache 包含关系待确认，不能照抄 Claude 公式 |

## 本项目的接入方式与性能建议

现有 `SessionMeta` 仅保存会话、对话数量和模型等摘要，没有用量 DTO（`src-tauri/src/agents/mod.rs:65`）；Pi 详情转换还会重建为纯文本事件，丢弃原有 usage（`src-tauri/src/agents/pi.rs:259`）。因此，不建议通过加载每个完整会话详情来统计。

建议建立独立 usage 读取/索引模块，复用目录定位、模型名称补全和变更检测；保留每个 Agent 的原始计量规则。数字汇总经 IPC 传到前端，前端不接收对话正文。已有源码可复用的部分：

- `src-tauri/src/agents/codex/history.rs:8`：history_base 与跨页去重；还需补用量继承基线，不直接拿会话摘要总计。
- `src-tauri/src/agents/codex.rs:682`：现有目录会过滤非交互来源，用量需要另定范围。
- `src-tauri/src/agents/claude_family.rs:304`：当前 CLI/QoderWork 来源过滤与用量范围有关，不能无条件复用。
- `src-tauri/src/agents/opencode.rs:123`：现有“有正文”的消息统计条件用于对话数量，不适合用量。
- `src-tauri/src/agents/opencode/events.rs:197`：现有详情忽略 step-finish，这是合理的展示处理，但不能作为计量数据源。
- `src/App.tsx:22`、`src/App.tsx:152`：tab 类型和监听生命周期目前只面向会话/配置；增加 tab 时需要给用量单独的刷新生命周期。

本机 Codex 文件已约 1.4 GiB，不能每次启动、切 Agent、切 tab 都全量扫描。建议：

- 首次进入用量时后台建索引，分 Agent 显示就绪状态；已有缓存立即展示，不阻塞会话/配置。
- JSONL 保存文件身份、读取偏移、未完成尾行和计量事件身份；正常追加只读新增数据。文件被替换、截断或分页结构改变时，按相关文件/逻辑历史失效，避免累加旧快照。
- OpenCode 只读查询 message 的计量字段，不连接大体积工具正文；其表具有 `time_updated`，要处理原消息补写，而不只追加新 ID。DB/WAL 变化后在一致读快照内更新缓存，删除也要反映。
- 缓存只存数字、日期、模型、来源及必要去重身份，不复制 prompt、工具正文或凭据；分离解析版本与源文件版本，升级计量逻辑时可重建。
- 沿用事件合并与切回应用检查的思路。用量页关注跨 Agent 统计时需要独立的 dirty/刷新范围，不能让每次日志写入都扫所有历史，也不依赖一直运行的高频轮询。
- 首期完全本地读取。Qoder quota、Codex account usage、供应商账单均为后续可选联网来源，单独展示范围，不自动作为本地统计的补差或校验真值。

以上是实现建议，尚未构建用量索引，也没有给出该功能的最终启动耗时或资源占用承诺。

## “用量”页的首期布局建议

顶部保留“会话 / 配置 / 用量”三个 tab。用量页只放必要统计：

- 时间：今日、近 7 天、近 30 天、自定义；Agent 筛选。
- 概览：已确认的 token、输入/输出、涉及会话数；Qoder 视图单列已记录 Credits。
- 按日趋势，下方按 Agent、模型、项目分组，可进入对应会话核对。
- 成本如显示，明确用“记录成本 / 估算成本”；不把美元、人民币和 Credits 混加，不将价格缺失显示为免费。
- 缓存、思考、统计范围、完整性和更新时间放进小型帮助提示/详情展开。保留简洁页面，不铺满解释文字。
- “无日志”“有日志但缺计量”“本地已统计”“统计部分缺失”分别显示。Qoder 语义未确认的项和 Pi 无真实样本的支持状态保留标识。

## 实现阶段的最低验证集

- 五来源各有精确字段 fixture，覆盖已知 0、未知、cache/reasoning 包含关系、不同历史版本及溢出边界。
- Claude 分块/回放、Codex 重复 token 通知/同 ID 分页/fork、OpenCode message 与 step-finish、Pi 分叉/compaction/独立 usage 都有防重与防漏案例。
- 续聊追加、半行写入、同一 SQLite 行更新、文件替换/截断、归档/删除、跨日及不同时区都更新正确。
- 模型切换按当时调用归属；未知模型不拿默认模型补齐；子 Agent 和压缩消耗可追溯。
- 有已保存用量的会话，可与对应版本 CLI 本地统计核对；本次 Pi 没有真实日志，实施后必须补同版本受控样本。
- 大历史冷启动、缓存启动、后台暂停、切 tab/Agent 及持续写入不拖慢现有会话页。数字正确性与界面响应分别验收。

最终建议：先实现五个来源的本地计量适配与覆盖状态，再接简洁用量页；Qoder 已记录 Credits 可直接接入，跨 Agent 总 token 合并与所有估算金额都必须经过各来源语义校验。


## ccusage复用评估（2026-10-22）

### 核对版本、包结构与 API

本节固定核对官方仓库 `06a3343369c026e7e199aa4fa495bd730a0e17c4`，避免“ccusage 只能通过 TypeScript/Node 接入”的过时判断。当前 npm `ccusage` 最新版及该提交 `apps/ccusage/package.json` 都是 **20.0.40**；核心已是 Rust workspace，npm 包仅通过 JavaScript 启动器选择对应平台的原生二进制，包含 macOS arm64/x64 等包。旧的 `@ccusage/codex`、`@ccusage/opencode`、`@ccusage/pi` npm 包 latest 仍为 19.0.0，不应混作当前统一 CLI 的版本。来源：[固定提交 package.json](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/apps/ccusage/package.json)、[启动器](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/apps/ccusage/src/cli.js)；旧包版本同时由 npm 官方 registry 只读核对。

官方最新发布 tag `v20.0.40` 指向 `600b3c9f40b995d23ac6b04b3d30c8a27a0a68b2`，发布时间是新加坡时间 2026-10-22 03:04。固定 main 提交只比它领先一条“更新 models.dev 价格快照”的提交，差异文件仅 `flake.lock` 和 `rust/crates/ccusage-pricing-snapshots/src/models-dev-pricing.json`，本次核对的包/API结构没有差异。之后升级仍需重新比对，不能用 main README 代替某个已交付版本的证据。来源：[发布页](https://github.com/ccusage/ccusage/releases/tag/v20.0.40)、[两版本比较](https://github.com/ccusage/ccusage/compare/v20.0.40...06a3343369c026e7e199aa4fa495bd730a0e17c4)。

Rust 已拆成 `ccusage-core`、`ccusage-cli`、配置/定价等 crate 及各 Agent adapter。core 导出计量类型、汇总、定价等；OpenCode adapter 公开 `load_entries`、`summarize_entries`、`report_json` 等。因此**可以通过固定 Git revision 或 vendoring 复用 Rust 源码**，不必先启动 Node。但这些 crate 全部继承 `version="0.0.0"`、`publish=false`、edition 2024；没有在已查官方文档中找到面向外部库使用者的稳定 API/兼容承诺。`pub` 表示可访问，不等于可无约束升级的公共 SDK。若直接嵌入，还要接管 binary main 原本负责的价格快照和可选网络 fetcher 初始化。来源：[workspace manifest](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/Cargo.toml)、[core exports](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/crates/ccusage-core/src/lib.rs)、[OpenCode adapter](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/opencode/src/lib.rs)、[初始化入口](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/crates/ccusage/src/main.rs)。

### 本项目直接嵌入的 SQLite 依赖冲突

当前 ccusage OpenCode adapter 依赖 `sqlite 0.37.4`，workspace 显式启用 `bundled, linkage`；锁定链为 `sqlite → sqlite3-sys 0.18.2 → sqlite3-src 0.7.1`，后者 manifest 声明 `links="sqlite3"`。本项目 `src-tauri/Cargo.toml:34` 使用 `rusqlite 0.37` 的 bundled 模式，锁定到 `libsqlite3-sys 0.35.0`，也声明相同 links。Cargo 明确禁止一个依赖图中多个包声明同一个 links，因此**按当前原样配置，将 ccusage OpenCode/all adapter 链入现有 Tauri 会产生原生链接依赖冲突**。这是 manifest 层面确认，尚未运行集成构建；不等于所有 ccusage Rust crate 都无法复用。来源：[OpenCode manifest](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/opencode/Cargo.toml)、[workspace SQLite features](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/Cargo.toml)、[sqlite3-src 发布 manifest 来源](https://crates.io/api/v1/crates/sqlite3-src/0.7.1/download)、[Cargo links 约束](https://doc.rust-lang.org/cargo/reference/build-scripts.html#the-links-manifest-key)。本机还只读核对了 `.cargo/registry/src/.../libsqlite3-sys-0.35.0/Cargo.toml:18`。

可选路径是统一 SQLite 绑定/改造 adapter，或将原生 ccusage 二进制作为独立进程读取 JSON；后者隔离此链接冲突。是否直接嵌入需要专门 PoC，不能写成“加一个 Cargo 依赖即可”。本次没有改项目依赖，也没有构建或安装 ccusage。

### 官方 CLI 接口与支持范围

官方支持 Claude Code、Codex、OpenCode、pi-agent，当前 adapter 列表和文档中**没有 Qoder**；有 Qwen 不代表 Qoder。默认统一报告也会扫描其他受支持 Agent，若只展示本项目四个来源，应按来源调用或过滤报告，不能把全部自动发现结果误归给某个 Agent。来源：[官方支持来源](https://ccusage.com/)、[固定 core Agent 列表](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/crates/ccusage-core/src/lib.rs#L55)。

可复用的 CLI 合同是 `--json`，日/周/月/会话报告、按 Agent 分组、模型细分、日期/时区筛选均有官方说明。`--offline` 使用离线可用价格数据，避免运行时价格网络请求；`--no-cost` **只删除输出成本字段，不等于禁止网络或关闭价格处理**，所以完全本地用途仍应显式传 `--offline`。不要在产品刷新时执行 `npx ...@latest`：包下载本身不受分析器 offline 选项控制。JSON 中定价未知有 `unpricedModels`/`missingPricing` 等标记，应保留，不能把 0 成本当免费。已另外打开发布 tag 本身核对，`v20.0.40` 已具有上述 `--json`、`--offline`、`--no-cost`，不是仅 main 才有。来源：[发布版 JSON 输出文档](https://github.com/ccusage/ccusage/blob/v20.0.40/docs/guide/json-output.md)、[发布版 CLI 选项](https://github.com/ccusage/ccusage/blob/v20.0.40/docs/guide/cli-options.md)、[在线 JSON 文档](https://ccusage.com/guide/json-output)。这些是 CLI 文档接口，不构成 Rust 函数级兼容保证。

### 许可

固定提交 `apps/ccusage/LICENSE` 与 npm manifest 均为 MIT，允许复用、修改和分发；分发包含其代码或二进制的产品时需要保留版权与许可声明。GitHub 仓库根目录 LICENSE 是指向该文件的符号链接，仓库 API 的自动 license 分类曾返回 `NOASSERTION`，不能据此说它不是 MIT。第三方依赖许可仍需随选择的构建产物核对。来源：[实际 MIT 文本](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/apps/ccusage/LICENSE)、[package license](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/apps/ccusage/package.json#L9)。

### 解析逻辑能复用到什么程度

| 本项目 Agent | ccusage 可复用内容 | 仍需处理 |
|---|---|---|
| Claude Code | JSONL usage、请求/消息身份去重、跨会话回放、按日/模型汇总 | 用本机存在 usage 变化和跨文件复制的样本核对最终值选择 |
| Codex | token 增量、父历史回放基线、compaction、缓存分项、压缩日志和多数据目录 | 本机同 ID 多物理页及 history_base 边界仍需对照测试；未知/回退模型标记要保留 |
| OpenCode | SQLite/旧格式读取、缓存和思考总量、分叉处理、标准汇总 | 对照本机 schema 和成本无关的 token 数；较新 schema 支持不代表所有历史都完整 |
| Pi | 助手消息 usage、分叉复制前缀去重、按日期/模型汇总 | 本机安装的 Pi 0.87.0 独立 usage、压缩、branch_summary、toolResult usage 需补充适配 |
| Qoder CLI | 没有现成 adapter | 自己读取已有 usage，继续核实 input/cache 语义和缺失状态；不能冒充 Qwen 或直接套 Claude 的缓存公式 |

源码审查中确认：

- Claude 已按消息 ID、请求 ID 和来源处理去重，且有替换保留记录的逻辑，不是简单永远取第一次。来源：[daily.rs](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/claude/src/daily.rs#L453)。
- Codex 已有父历史回放和 compaction 处理。因此可以复用大部分基础计量逻辑。固定版本源码未查到对 `history_base/end_byte_offset/end_ordinal_exclusive` 的直接读取；这只能说明还需用本机分页 fixture 验证，不能单凭未读取字段就认定总量有错。用量统计要包含真实发生的调用，也不应机械套用“当前会话可见历史”的筛选范围。来源：[Codex loader](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/codex/src/loader.rs)、[replay metadata](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/codex/src/replay.rs#L392)。
- Pi 的预过滤要求一行同时有 `usage` 和 `message`，随后只接受 type 为 message（或旧格式未声明 type）、role 为 assistant 的 usage。因此当前实现不能覆盖上文 Pi 0.87.0 的全部消耗入口。已有分叉去重可以复用，额外计量应在 adapter 中补齐，或让 Pi 单独走完整解析器；不可把完整 Pi 总量与 ccusage Pi 总量重复相加。来源：[Pi 读取过滤](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/pi/src/parser.rs#L310)、[Pi 条目判定](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/pi/src/parser.rs#L568)。
- OpenCode 会将 reasoning 和上游 total 交给总量补全工具；汇总中的 `totalTokens` 不应由界面随意重新计算。公共分项中的 input 与缓存是分列的，显示“完整输入”时需要一次性归并；来源未分配到明确输入/输出的额外总量也需保留状态。来源：[OpenCode token mapping](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/adapters/opencode/src/parser.rs#L180)、[total token helper](https://github.com/ccusage/ccusage/blob/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust/crates/ccusage-core/src/utils.rs#L14)。

以上为固定源码审查，没有安装或运行 ccusage，尚无本机计量一致性或冷启动性能通过的结论。

### 早期候选方案：随包交付 ccusage

此前提出的快速复用候选是把**固定版本的 ccusage 原生程序作为应用内随包交付的 sidecar**，Tauri 后端在后台调用其离线 JSON 接口，前端继续使用我们的界面。这样先复用现有解析、去重及日/模型汇总，也隔离 SQLite links 冲突和 Rust 函数接口变动。原生程序不要求用户额外安装 Node；发布时应锁定版本/校验值、处理架构和签名、附许可。用户随后提出希望直接将核心逻辑实现到软件中，下面补充内置方案；本节不再是当前推荐。

单个来源的调用形态示例（未在本次运行）：

```sh
ccusage codex daily --json --offline --no-cost --timezone Asia/Singapore
```

我们自己的层负责：

1. 将 ccusage JSON 转为稳定的 token DTO，归一化总量与输入/输出/缓存语义，保留回退模型、未知和覆盖状态。
2. Qoder adapter，以及 Pi 新版额外计量入口的补齐；补丁要有固定输入/预期 token 数，便于升级 ccusage 后回归。
3. 数据变化监听、缓存、取消旧查询和刷新节流。原生 CLI 不能据此被假设为跨进程增量索引；不能每次切 tab 都重新扫描约 1.4 GiB Codex 日志。
4. 时间/Agent/模型筛选、每日趋势和三个 token 概览。用户只关注 token 时默认只展示这些数字及必要的完整性提示。

此方案仍需验证每条请求的持续增量更新和本机样本计量一致性；没有因源码审查就完成集成或性能验收。

### 当前建议：参考并移植必要统计逻辑，内置到 Rust 后端

用户提出只统计 token，并希望不携带 ccusage 程序。这个约束下可以按功能抽取逻辑，在现有 Tauri 后端建立自己的用量模块。它与“把 ccusage 完整 Rust workspace 直接设为依赖”是两种接入方式：我们保留必要算法、字段映射与来源说明，读取和缓存使用本项目既有技术栈。这样没有单独的 ccusage 可执行文件或 Node 运行要求，也不引入它的第二套 SQLite 绑定。

建议边界：

- **移植/参考核心规则**：各来源 usage 字段映射、累计快照转增量、消息/请求去重、继承/分叉回放识别、缓存/思考归一化、按日期及模型分组。固定来源 revision 为上文 `06a3343369c026e7e199aa4fa495bd730a0e17c4`，每个移植模块记录来源和有意调整之处。
- **使用现有读取技术**：JSONL 继续流式读取，OpenCode 继续使用本项目 `rusqlite` 只读连接。统计模块向外只提供计量记录与汇总结果，避免连带引入 CLI 参数、终端表格、价格计算、价格下载和不需要的其他 Agent。
- **本项目负责补齐**：Qoder adapter、Pi 0.87.0 的额外 usage 入口、本机 Codex 分页/继承样本校验，以及缓存、变更监听、取消旧任务和后台刷新。适配规则保留来源完整性，不能借用 ccusage 的缺省值把未知变成零。
- **统一内部数据**：按记录保存 Agent、来源身份、实际模型、时间、输入/输出及缓存/思考细分、来源总量和完整性标记；明确各细分的包含关系。汇总与前端不直接依赖某个 CLI 原始 JSON，后续日志格式调整集中在 adapter 内。
- **维护与验证**：日志兼容和后续修复由本项目维护，不会自动获得 ccusage 升级。优先移植相关测试的输入/期望值，再加本机脱敏分页、分块及缺失样本，按固定版本做差异核验；只计 token 的算法应可在无价格表、无网络、无 CLI 进程时运行。
- **来源许可**：直接移植或改编的实质代码和测试保留 MIT 版权及许可，并记录固定提交；参见本节前文实际许可证。保留许可文件不要求携带原程序。

当前更推荐这个内置方案：既复用已查明的统计经验，也符合单一应用和 token 统计的范围。以上仍是设计建议，本次没有开始修改产品代码或引入依赖。

## 内置方案实施记录（2026-10-22，0.1.23）

已按用户确认的内置方案实现独立 Rust `usage` 模块、可重建 SQLite 数字索引和“用量”tab。产品不依赖 ccusage 可执行文件、Node 或价格服务；OpenCode 继续使用原有 rusqlite。来源和维护边界见 `src-tauri/src/usage/README.md`，MIT 声明及 Zstandard 许可随应用资源分发。

首期展示总/输入/输出 token、今日/近 7 天/近 30 天/全部、Agent/模型筛选与每日趋势。缓存/思考细分折叠，说明合并到一个 Lucide 帮助图标。没有落盘计量时不估算；细分未提供与明确的零分开。Qoder 只计已记录输入和输出，缓存包含关系不明时标记“部分统计”。Pi 的独立 usage、toolResult、compaction、branch_summary 已有 fixture 验证，本机仍无真实历史。

JSONL 只解析新增完整行；替换、截断及同长度重写会失效，Codex `.jsonl.zst` 变化后重读。Codex 同 ID 分页用 history_base 的字节/序号边界建立基线，逐响应 ID 与累计通知重叠时只记一次；旧 fork 的改写时间前缀按父请求序列及创建时的连续回放识别。部分旧日志的 subagent_history_start_ordinal 会推进到真实调用之后，不能将其当成固定继承截止线。只有完整扫描结束才提交索引；取消不发布半份结果。

用量页按需启动统计，先显示缓存，较大的 Codex 最后扫描；切走取消扫描并停止监听。前台通过文件事件合并更新，后台停止通知，切回补查；没有定时全量扫描。一次本机 release 复测读取约 4.7 GB 解压后 Codex 历史，重建约 4.6 秒，未变化后的检查约 0.33–0.40 秒；其他有历史来源的热检查约 4–38 毫秒。这是本机当时快照，不代表所有机器和目录规模。

开发验证临时下载了 npm 官方 `ccusage 20.0.40` arm64 原生包并验证 registry SHA-512，只用离线无费用 JSON 作对照；该程序没有进入依赖和应用包。对照冻结到新加坡日期 2026-10-22 之前，避开仍在增长的当日记录：

- OpenCode 的 37 天总量逐日相同。
- Claude 仅一天低 652,455 token，来自同一 chatcmpl 响应跨多个时间戳的流式/最终重复写入；本实现以稳定响应 ID 只保留最终完整计量，已有合成回归样本。
- Codex 的旧版回放、缺失父历史、分页和逐响应计量不能仅靠整体对齐确认正确。已据差异补充回归；少数历史日期仍有差异，且本机有损坏记录，界面保留“部分统计”。不将 ccusage 总数当作账单真值，也不将差异率当作统计误差上限。

回归覆盖归一化、流式/回放去重、分页与 fork、取消、缓存重启/损坏、半行/改写/替换/删除、SQLite WAL、时间单位及跨日筛选。浏览器验证使用 Rust 生成的无敏感 fixture，覆盖 WebKit/Chrome、统计/筛选/细分、错误保留与重试、切页取消及 960/1280/1600 窗口；原有自动更新流程一并回归。后续跟进 ccusage 时固定新 revision，先对照规则与测试，再决定采用哪些变更。

## 累计用量账本修正（2026-10-22，0.1.24）

用户确认需要长期累计用量。0.1.23 随现存日志重建数字缓存的行为不符合这一目的，现已改为：

- 独立账本保存在应用数据目录，而非系统缓存；缓存可清理，已采集消耗保留。旧缓存先迁移，源日志消失后仍保留计量。
- 按来源、会话身份合并计量观测，保留父历史去重上下文；恢复/导入、同一文件重读、SQLite 原消息更新、逐响应明细后补时不会把同一调用再算一次。
- 所有应用内删除入口先提交可读取的最后用量，持久化失败则保留会话。后台采集与当前 tab 独立，使用文件事件合并、空闲预算和切回窗口补查，不要求一直打开用量页。
- 数据库以事务和 FULL 同步提交，权限 0600；写入失败、损坏不静默创建空账本。多进程事务内合并，防止旧快照覆盖新记录。不保存对话正文。

回归包含五来源删除保留、重启/清理缓存、0.1.23 缓存迁移、恢复副本、Codex 父日志删除/后补逐响应计量、同路径新会话、删除前保存失败、多连接合并及无前端窗口的原生文件事件。未采集且已删除的历史仍无法凭空补回。
