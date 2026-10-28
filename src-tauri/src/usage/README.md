# 内置 token 统计

`parse` 将五类原始计量映射为完整输入、完整输出和不重复计入的缓存/思考细分；`normalize` 负责响应去重、累计增量及继承历史；`cache` 保存可重建索引和 JSONL 读取偏移，`ledger` 独立持久化已经观察的计量，`events` 保存归并后的请求表，`summary` 只从该表计算用量和趋势，`opencode` 继续使用本项目的 rusqlite。

规则参考 ccusage 固定提交 `06a3343369c026e7e199aa4fa495bd730a0e17c4`：

- `rust/adapters/claude/src/daily.rs`：分块/回放及最终计量去重。
- `rust/adapters/codex/src/parser.rs`、`replay.rs`：累计计数与逐响应明细的重叠处理、父历史基线。
- `rust/adapters/opencode/src/parser.rs`、`rust/crates/ccusage-core/src/utils.rs`：缓存/思考与源总量归一化。
- `rust/adapters/pi/src/parser.rs`、`loader.rs`：分叉复制条目；本实现还覆盖 Pi 0.87 的独立 usage、compaction、branch_summary 和 toolResult。

源码入口：https://github.com/ccusage/ccusage/tree/06a3343369c026e7e199aa4fa495bd730a0e17c4/rust

本实现按本项目接口重新组织，不包含 ccusage CLI、定价、网络请求或第二套 SQLite 绑定。许可与来源声明保存在 `public/licenses/ccusage.txt`；后续参考上游修复时必须重新核对固定提交、增量基线和测试期望，不自动跟随 main。

Qoder 的缓存输入包含关系未确认时，总量只保守计入已记录 input+output，并标记部分统计。缺失 usage 不按文字长度推算。统计范围是本机已经采集的累计消耗，包括子 Agent/归档；删除源记录不会删除已经入账的用量。

账本位于应用数据目录的 `Agent Session Manager/usage-ledger-v1.sqlite3`，文件权限 0600，使用 SQLite 事务与 FULL 同步提交。缓存仍位于系统缓存目录，缓存淘汰、解析版本或数据根切换不会清空累计用量。首次读取先迁移 0.1.23 的相应数字缓存，再处理源文件消失；从未读取且已经删除的计量无法还原。

账本按来源及会话身份保存计量观测，不保存正文。旧观测与新扫描合并，SQLite 消息删除、JSONL 截断/替换不会抹去此前读到的调用；同路径被新会话复用时保留各自计数序列。父历史元数据也保留，使 Codex/Pi 的已删除父日志仍能参与去重；恢复或重新导入同一记录只记一次。写入事务内重读，防止多进程以旧快照覆盖新计量。账本损坏/不可写会返回错误并保留原文件，不静默回退为空账本。

默认按需同步：每次启动应用同步一次；打开用量页、切回该页所在窗口、手动刷新和前台文件通知触发后续增量同步。后台保留关闭时没有常驻采集进程。`usage_file_stamps` 只保存路径、解析版本和文件状态；未变文件不加载完整 `FileCache`，重启后也复用。源改变才读新行/数据库计量，再更新变化会话及相关来源的请求事件。

正式 macOS 应用通过 `service` 的 SMAppService 注册包内 LaunchAgent。后台保留是可选项，新设置默认为关闭，升级保留用户原开关选择。`--usage-collector` 在 Tauri/WebView 初始化前分流，单独启动 Rust 采集进程；退出界面后继续、登录后自动启动，应用内可暂停/开启，授权与实际运行状态分开报告。`flock` 确保同一数据目录只有一个采集进程；进程退出自动释放，不凭残留 PID 文件认定正在运行。开发/非应用包模式不自动注册后台项目。

`collector` 监听五类日志根与控制目录。尚未存在的根先监听最近的已有父目录，创建后立即重建监听；macOS 物理路径通知会映射回 Adapter 的原路径。变化路径集合及唤醒队列有界，约 150 毫秒合并，不因持续写入无限推迟；每分钟补查元数据/重建监听，失败按 1～60 秒退避重试。空闲每秒只检查退出信号，不周期读取日志正文。`ingest` 仅读变化来源并逐源事务入账，不生成全历史报表、不常驻所有历史；JSONL 只解析新增完整行，SQLite 只读请求元数据，不读 part 正文。外部账本写入标记请求索引待更新；使用时统一归并，统计查询不额外解析源日志。

服务设置、进程锁及状态位于应用数据目录的 `Agent Session Manager/collector/`，文件 0600。设置保存前台实际解析的五类数据根，登录服务不依赖终端环境变量。系统可禁用后台项目，界面不会把“等待授权”显示为正在采集。升级后前台会重启仍运行旧版本的采集服务。注册与授权接口参考 [Apple SMAppService](https://developer.apple.com/documentation/ServiceManagement/SMAppService?language=objc) 和 [后台项目包结构](https://developer.apple.com/documentation/servicemanagement/updating-helper-executables-from-earlier-versions-of-macos)。

本地替换正式 `.app` 时，先暂停/注销旧服务并退出 GUI，备份后再替换。清理已经备份的临时旧应用副本及其 LaunchServices 注册，再从最终安装位置开启服务。2026-10-23 的实机升级曾在旧副本注册尚存时出现“注册成功但 launchd EX_CONFIG”，清理旧副本注册并重新注册后恢复；不能只凭 register 返回成功验收。需同时检查 `launchctl` 的实际运行状态、PID 锁、五类采集状态及进程映射的可执行文件，避免误测仍运行的旧二进制。

删除会话的后端入口调用 `preserve_before_delete`，先同步采集并持久化最后可读取的计量，成功后才执行删除。外部工具在系统通知送达/首次读取前立即删除的日志、从未落盘的 usage、系统暂停或服务停用期间已消失的内容无法补回；事件通知不携带已删正文。这不是一分钟的采集间隔。

有意调整与本地兼容：

- Claude 的 `chatcmpl` 响应也使用稳定 ID 去重，避免同一流式请求在不同时间戳重复计入；cache 创建总数为 0 而 TTL 细分有值时取完整细分，不叠加两个口径。
- Claude/Qoder 的 UUID 消息 ID 同样是全局身份；旧账本中按 session/request 定位的 UUID 消息在汇总时统一身份，非全局短 ID 仍保留会话范围。
- Codex 优先使用明确的单次 last 计量，累计只用于推进/去重与缺失单次时的增量。逐响应 ledger 和通知可能使用不同累计范围，因此先匹配响应 ID/单次计量，再补未覆盖增量。history_base 分页采用精确边界，不套用 fork 的时间启发式。
- 旧 fork 可把父请求改写到创建时刻；参考上游连续 1 秒回放窗口，并要求父记录存在、首条紧邻创建、至少两条不同累计值。停止连续回放后保留子线程新调用。subagent_history_start_ordinal 在本机部分旧日志会推进到真实调用之后，不能单独作为继承截止线。
- Pi 扫描所有已保存分支及额外 usage 类型；短 entry ID 仅在同一父分支链内去重。OpenCode 的 message 计量不与 step-finish 相加，模型来自历史记录。
- OpenCode fork 会生成新消息 ID、复制原始时间与 tokens，且不保证根 fork 有 parent_id。使用原始请求创建时间/模型/Agent/path 的摘要与会话创建时间识别继承历史：原始调用即使时间和计数相同也分别保留，副本只补原始调用未保留的历史，分叉后新调用分别计量。来源：[OpenCode v1.19.3 fork](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/session/session.ts#L653-L689)。已有旧观测按消息 ID 补充元数据，无法重读的旧观测保留并标记去重不完整，不凭相同计数删除历史。
- 输入/输出分别记录字段可用性及缺失覆盖；同一调用的最终完整快照可补齐早期缺失分项，汇总遇到任何缺分项调用则保留 incomplete。旧账本没有这类标记时只保守识别完整记录/正值，旧的部分记录不能凭零推断“确实用量为零”。

修改映射/解析字段时更新对应来源的缓存版本；JSONL 来源使用 `FileCache.parser_version=3`，OpenCode 为 `4`。0.1.31 升级会重读现存源以补充请求元数据；0.1.32/0.1.33 复用该解析版本及偏移，保留原缓存 scope 以迁移已删日志。账本不能跟随缓存版本删除。界面取消不发布半份报表；后台每份来源完整解析、事务提交后独立保留，后续大文件/失败不回滚已入账来源。历史损坏、缺基线或语义不明应保留完整性标记。

验证入口：`cargo test --offline --locked usage:: -- --test-threads=1`、`cargo run --release --offline --locked --example usage_perf -- --storage <私有临时目录>`、`npm run test:e2e -- usage` 和 `npm run test:integration -- collector`。后者独立启动 release 采集进程，仅使用临时五类日志/账本，验证无窗口采集、单实例、目录创建、重启/删除保留、暂停与真实 CPU/内存。本机对账和已知差异见 `docs/research/agent-usage-2026-10-21.md` 的实施记录。后续维护由本项目负责；参考上游时先锁定 revision，再核对字段和回归期望，不能只为对齐总数而覆盖已验证的本地规则。

OpenCode v2 从 `session_message` 的 assistant 及带 tokens 的 compaction 读取请求计量，模型取消息中的 `model.providerID/id`。合成提示、模型切换和用户记录不计量；会话聚合 tokens 不与消息相加。v1 与 v2 会话 ID 重叠时读取 v2，历史账本保持原有消息身份和继承去重规则。解析层使用只读连接内的临时视图，不写入源库或持久化正文。结构来源：[v2.0.15 SQL](https://github.com/anomalyco/opencode/blob/v2.0.15/packages/core/src/session/sql.ts)、[消息定义](https://github.com/anomalyco/opencode/blob/5ed03ad1377b549df83aa52dc11bdb07593a5327/packages/schema/src/session-message.ts)。

请求事件沿用同一次增量读取，`request_parse` 提取白名单元数据（模型、请求/会话身份、时间、来源、推理档位、结果及日志自带成本），与原始计量一起事务入账；无 usage 的明确错误单独保存，不加入 token 总数。不保存 API 参数、请求/响应正文或错误正文。旧账本原地迁移为 SQLite schema 4（保留原始计量；首次查询按会话建立来源身份索引和事件归属），文件名保持不变；降级的旧程序会拒绝新 schema，不能用旧程序覆盖账本。

`events` 只归并标记为变化的来源和与其共享身份的会话。`usage_source_keys` 是普通 SQLite 查找索引，记录现有去重规则用到的会话/请求 ID、父历史引用、累计通知身份和 OpenCode 副本摘要；不保存第二份计量，不引入任务队列或通用依赖框架。相关来源复用原来的 `normalize::records`，稳定请求按 ID UPSERT，只更新新增/补齐行；新父历史或明确响应取代的累计差额只在该组内清除。无关会话的观测不读取，事件不改写。完整性信息随来源更新，汇总时合并小型摘要。

这不是所有格式都逐请求 O(1) 的承诺：发生变化的会话仍合并自身历史，Codex/Pi 父子分页链和共享请求副本需一并处理，OpenCode 目前按变化数据库读取元数据。取舍是保留经过验证的兼容规则，避免再引入一套增量归并状态机。全量重建只用于旧 schema 升级；源日志删除、截断或移动不会删除已采集计量。身份索引保留曾经出现的关系，确保旧差额仍能随迟到记录被修正。

后续翻页走有索引的时间/ID 游标，count/页/修订号使用同一读快照；期间有新数据时重置旧游标，避免跨快照漏行或重复。生成索引被取消会回滚并保留待更新标记；升级失败不清空账本。原有完整归并作为测试对照，持续验证局部更新与全历史结果一致。

`summary` 与请求表共用 `usage_request_events`：概览、模型/Agent 汇总和趋势均由已保存的模型、usage 和事件时间计算。同步时间仅用作“已同步”状态，不参与绘图。界面传入本地日历的绝对时间边界；今日按小时（含夏令时 23/25 小时日和非整点时区），近 7/30 天按日，左闭右开，空桶补零。无时间的历史只计入“全部”，缺失 usage 的失败事件不伪装成零计量调用。查询先用事件时间索引选范围，按模型/Agent/桶归组；筛选只查本地表。旧的日志汇总入口仅保留在测试编译中，用于解析与迁移回归，正式程序无第二套报表来源。

请求与计时语义参考 [agent-events 固定提交](https://github.com/haowang02/agent-events/tree/13a30e20d710fd504d21e9e1773752d5d677b550) 的 `internal/collect/{claude,codex}.go` 与 `internal/event/event.go`，MIT 来源在 `public/licenses/agent-events.txt`。本实现重新组织为 Rust 增量状态和现有账本，未引入其服务、网络定价或第二套日志监控。

- Claude 新版 `api-request.id` 与 `assistant.requestRef` 关联；耗时取开始日志到最后响应分块的时间差。旧日志没有开始记录时不推算。流式更新保持同一请求，补齐输出计量和结束状态。
- Codex `token_usage_record.response_id` 优先，累计 `token_count` 复用既有覆盖/继承归并。耗时按 `task_started`、用户消息完成及前次请求/计量事件边界估算，可能包括准备、工具等待或重试；界面带 `≈`。`task_complete.time_to_first_token_ms` 是整轮 TTFT，只附在首个请求上且注明“本轮首 Token”，不当作每次请求的首字延迟。无法拆分的累计差额标注“累计差额”且不提供 TPS。
- OpenCode 保留消息 `time.created/completed`、结束原因、错误标志及原始 cost，Pi 保留 usage 中的原始 cost；其他来源缺失计时字段时显示“—”。成本只显示源日志金额，不估算价格。HTTP 传输方式、接口路径等日志未记录的信息不虚构。
- TPS 是完整输出（含思考）除以整个请求耗时；不是扣除首字等待后的纯解码速度。失败和无法确认单请求范围的记录不显示 TPS。结果没有明确标志时显示“未记录”，不能仅凭存在 usage 判为成功。

验证：`cargo test --offline --locked usage:: -- --test-threads=1`，`npm run test:e2e -- request-events usage`；`CLAUDE_BIN=/path/to/claude CODEX_REQUEST_ROOT=/path/to/rollouts npm run test:manual -- request-events-runtime` 使用真实 Claude 程序、隔离目录和本机 SSE 服务，Codex 部分只读现有日志。账本副本性能检查使用 `ASM_REQUEST_LEDGER_COPY=/path/to/copy.sqlite3 cargo test --offline --locked request_events_ledger_copy_performance_if_requested -- --nocapture`，不要传正在使用的原账本。2026-10-27 验证 Claude 2.1.309 两次流式调用与续聊、Codex 52 次请求及 5 条整轮 TTFT；1,112 条旧账本事件首次索引约 104 ms，后续页不足 1 ms（调试构建、本机实测）。

0.1.32 验证：299 项 Rust、38 项 Node 单元测试、19 组 WebKit 回归通过，覆盖请求表/图表对账、夏令时与非整点时区、升级和删除保留。2026-10-27 本机 release 隔离空账本读取 19 份 Codex 日志（120,868,529 字节）约 116 ms，关闭并重开存储后的热同步约 0.33 ms、源读取 0 字节；今日/全部请求统计约 0.74/2.20 ms。可选采集进程空闲 10 秒测得 CPU 时间增量 0 ms、RSS 14.3 MiB，100 次追加约 178 ms 入账。以上为本机样本，不代表其他日志规模或磁盘的耗时。


0.1.33 验证：300 项 Rust、38 项 Node、19 组 WebKit 回归和采集进程集成测试通过。原有日志兼容测试在每次扫描后增加局部索引与全量归并的逐事件对照；另验证重启后追加一个会话时，无关历史即使不可解码也不会被读取，其事件不会被改写。采集进程空闲 10 秒 CPU 增量 0 ms、RSS 14.3 MiB，100 次追加约 188 ms 入账；默认仍关闭后台保留。

2026-10-27 23:40（America/Los_Angeles），以 19 份日志的稳定快照对比 `npx --yes ccusage@20.0.45` 的今日、7 天和 30 天（包含今日，离线、不计算费用）。新建账本与旧 schema 3 账本升级后，1,225 条 Codex 事件的身份、时间、模型和各项 token 完全一致；旧账本中其他 Agent 的历史也原样保留。

| 来源 / 范围 | 本应用总 tokens | ccusage 总 tokens |
| --- | ---: | ---: |
| Claude：今日 / 7 天 / 30 天 | 0 | 0 |
| Codex：今日 | 55,145,221 | 55,040,853 |
| Codex：7 天 | 149,332,755 | 149,228,387 |
| Codex：30 天 | 149,332,755 | 149,228,387 |

差额 104,368 对应采样时当前对话最新的一笔请求（输入 103,868，输出 500，其中缓存读 102,400、思考 95）。应用读取到 `token_usage_record` 时计量已落盘，ccusage 所用 `token_count` 在 3.709 秒后才写入。把这条实际迟到通知追加到隔离快照后，本应用仍为 1,225 条事件且所有用量不变，没有重复计量。排除这一时间差后，逐日、逐模型及各分项完全一致；比较时仅将 ccusage 回退为 `gpt-5.6-luna` 的 `codex-auto-review` 对齐名称，应用保留原始模型名。Claude 本机没有会话，因此这里只验证空历史一致性，非空计量由合成日志回归覆盖。
