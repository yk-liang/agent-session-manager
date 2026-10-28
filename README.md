# Agent Session Manager

面向 macOS 的多 CLI Agent 会话管理桌面应用，使用 Tauri 2、React 和 TypeScript 构建。

## 安装

要求 macOS 13 或更新版本。前往 [Releases](https://github.com/yk-liang/agent-session-manager/releases/latest) 下载 `universal.dmg`（Apple Silicon / Intel 通用），打开后将应用拖入 Applications；也可使用 `.app.zip`。安装包与 SHA-256 校验文件一同提供。

当前发布包使用本地签名，尚未经过 Apple Developer ID 公证。首次打开若被系统阻止，在确认下载来源与校验值后，按 [Apple 官方步骤](https://support.apple.com/zh-cn/102445) 从“系统设置 → 隐私与安全性 → 仍要打开”确认。

图形应用不需要 Node.js；终端恢复、分叉和 OpenCode 删除需要对应的 Agent CLI。应用读取本地日志并在本地保存用量，后台采集默认关闭。

## 支持的 Agent

| Agent | 存储位置 | 恢复命令 |
|---|---|---|
| Qoder CLI | `~/.qoder/projects/*.jsonl` | `qoder --resume <id>`（支持 `--fork-session`） |
| Claude Code | `~/.claude/projects/*.jsonl`（支持 `CLAUDE_CONFIG_DIR`） | `claude --resume <id>` |
| Codex CLI | `~/.codex/sessions/**/rollout-*.jsonl`（支持 `CODEX_HOME`） | `codex resume <id>` |
| OpenCode | `~/.local/share/opencode/opencode.db`（SQLite） | `opencode --session <id>`（支持 `--fork`） |
| Pi | `~/.pi/agent/sessions/**/*.jsonl`（支持 `PI_CODING_AGENT_DIR`） | `pi --session-id <id>` |

侧栏自动列出本机有数据的 agent。

## 功能

- 通用操作图标使用 Lucide，通过 `UiIcon` 统一尺寸、线宽与无障碍语义；Agent 品牌标识保留各自图标
- 用量页内置五类 Agent 的本地 token 统计：今日/近 7 天/近 30 天/全部、Agent/模型筛选、输入/输出与按请求发生时间绘制的趋势（今日按小时，近 7/30 天按本地日历天）；缓存/思考按日志提供的细分显示，不重复计入总量。Qoder 缺失或缓存口径未确认的历史标为部分统计；日志没有细分时显示“未提供”。统计规则参考 ccusage 固定源码，原生程序内运行，无 ccusage/Node/价格服务依赖
- 用量页的请求事件表显示模型、推理/速度档位、来源、结果、TPS/耗时、输入/缓存/输出、日志成本和会话 ID，支持筛选、分页和展开详情。解析后增量持久化、去重补齐；删除源会话后仍保留。Codex 耗时估算带 `≈`，整轮首 Token 单独标注；缺失信息不填造。请求事件字段参考 agent-events 固定源码，详见 `src-tauri/src/usage/README.md`
- 用量索引只保存数字、模型和去重元数据，JSONL 追加仅读新增完整行；Codex 同时支持压缩日志、继承历史和逐响应去重，Pi 包含压缩/分支摘要及独立 usage。启动应用、打开用量页、窗口焦点及文件事件触发增量同步；未变文件只核对小型状态索引；请求表只归并变化会话及相关父子/副本来源，不再重算整个 Agent 的历史。切换时间范围、Agent 或模型直接查询持久化请求表，不重复扫描源日志
- 用量采用独立持久化账本，已采集消耗在删除会话、重启和清理缓存后保留；恢复/重新导入日志会去重。账本位于 `~/Library/Application Support/Agent Session Manager/usage-ledger-v1.sqlite3`，不保存对话正文，首次升级迁移已有数字缓存。应用内删除先保存最后可读取的计量，保存失败则保留会话
- 默认按需同步；macOS 13+ 正式应用可额外开启后台保留，退出界面后继续、登录后自动启动，用量页可暂停/开启并显示授权状态。升级保留已有开关选择；关闭后台不影响使用时统计。通过系统文件事件合并约 150 毫秒后采集变化来源，不等待会话页的 5 秒刷新预算，也不每次汇总全部历史；每分钟仅补查遗漏变化。使用 SMAppService 注册，遵从系统的后台项目授权，开发模式不自动注册登录项。日志从未落盘、采集被暂停或外部工具在首次读取前立即删除的内容仍无法还原
- 缺少输入/输出分项时显示“未提供”，与已记录的 0 区分；分项只覆盖部分调用时显示 ≥ 下限。OpenCode fork 复制的历史计量按原始请求元数据与会话创建时间识别，分叉后的新调用保留；Claude/Qoder 的全局消息 UUID 与请求 ID 同样去重
- 按项目分组的会话列表、搜索、真实对话量统计（排除工具往返）
- 五种 Agent 日志中已记录的系统/开发者指令、运行环境、自动注入内容和压缩摘要默认隐藏；勾选“上下文”后显示紧凑折叠条，可查看原始记录。这些记录不参与对话计数、导览或正文搜索，原始日志与用量账本保持不变。Claude Code/Qoder 支持元消息、上下文附件与提示词快照，OpenCode 支持 v1/v2 自动片段和压缩记录，Pi 支持当前分支的扩展消息、技能与摘要。错误状态、用户实际命令和引用标签的正文仍保留；未写入日志的提示词不作推测或还原
- Codex 的“页面上下文”只表示应用注入的关联 Page 状态，`page_id: null` 表示当时没有打开 Page；它并非每个会话都有，也不是 Page 正文
- Agent 切换先展示内存中的目录快照并后台刷新；JSONL 摘要按文件版本复用，变化后重新解析该会话，Codex 分页以有效会话链为单位失效；摘要持久化到用户缓存目录（只含元数据，不复制正文），损坏时回退到日志解析。SQLite 同时校验主库与 WAL。删除会立即清除各筛选缓存，旧请求不会恢复已删除会话
- 会话页在前台使用系统文件事件监听当前 Agent，新建会话、续聊和日志替换后自动检查；切回应用补查一次。连续写入合并处理，自动扫描至少间隔 5 秒并随扫描耗时延长；后台、配置页和批量选择/删除期间暂停，恢复后处理待更新内容，不重置当前打开的对话
- Codex 桌面端明确登记为无项目的会话统一归入“未归属项目”，不再按自动目录散列；路径保留在卡片悬浮提示和详情中。项目归属不可用时沿用目录分组，分组不改变搜索、恢复和删除使用的真实目录标识
- Qoder 的已识别自动工作区归入“未归属项目”；项目内部运行目录按 `.git` 或项目 `AGENTS.md` 所在的根目录归组，跳过 `.local` 中临时生成的指令文件。侧栏和列表显示项目根目录，悬浮与详情保留真实 cwd，旧目录名的不可逆编码仅作缺少 cwd 时的回退
- 三栏宽度可拖拽调整并自动记忆；窗口缩小时保留最小可用宽度，放大后恢复。双击分隔线恢复对应栏默认宽度，也支持方向键调整；配置页沿用侧栏宽度
- Codex 首屏只读取文件头和有效摘要，精确计数在后台补齐；摘要尚未完成时省略计数。详情虚拟化显示可见消息，长文本分段展开，工具内容展开后再读取并校验日志行；目录跳转与搜索保留全局消息位置
- OpenCode 保留正文、思考、工具分段的原始顺序；支持工具输入/输出、失败与运行状态，折叠时不传输工具正文；用户多段正文仍合并为一条指令
- 历史卡片提供 Markdown 开关，默认关闭并记忆选择；正文、思考和文本工具结果按需渲染标题/列表/表格/引用/代码，纯文本、JSON 和工具参数保留原文；搜索可继续定位高亮，外部图片只显示可主动打开的说明链接
- 同一搜索框匹配标题、路径、模型及用户/助手正文；正文结果显示命中片段，点击后定位并高亮消息，支持当前 Agent 与项目范围。后台搜索可取消，内存缓存随源文件（含 SQLite WAL）变化更新，不额外保存正文索引文件
- 会话详情只读浏览：用户/助手气泡、思考过程与工具调用折叠卡片、用户指令导览（点击跳转高亮）
- Codex 同一 ID 的分页日志归并为一条会话，依据 `history_base` 的字节及序号边界还原有效历史，统一详情、统计和正文搜索；历史缺页时保留可读内容并提示，删除前复核整组分页再一起移入回收站
- 列表与详情统一展示模型名称，悬浮查看原始 ID；Qoder 结合历史请求日志与官方模型列表后台补全组织及自定义模型，不阻塞列表和详情，无法确认名称时保留原始标识
- Codex 模型取自有效历史的 `turn_context` / 逐响应计量元数据，分页时排除已截断的旧尾部；中途切换模型显示最近记录的值，未记录时不拿当前默认配置补填。模型随后台摘要缓存，首屏继续只读文件头
- 一键复制恢复命令 / 在新终端窗口恢复 / 分叉（按各 agent 能力显隐）
- 删除会话移入系统回收站（OpenCode 委托官方 `opencode session delete`）
- 会话来源治理：按来源标记过滤非 CLI / 云端会话、空会话、Codex 非交互会话（onboarding/子代理/exec/mcp 等）、OpenCode 子代理会话
- 多 Agent 配置方案快照切换（Qoder settings.json/mcp.json/模型默认、Codex config.toml、OpenCode opencode.jsonc、Claude settings.json、Pi settings.json，排除凭证文件）：按原始字节收发（保留注释与手写格式），真快照语义——快照中不存在的文件应用时移入回收站，应用前自动备份，中途失败自动回滚
- 配置工作区按“摘要 → 方案 → 文件 → 备份”展示；说明通过问号按需查看，无值摘要不展示，备份默认收起；依据文件内容匹配方案
- JSON / JSONC / TOML 语法校验、缺失文件创建、未保存草稿的切页/刷新/窗口关闭与菜单退出保护；保存和确认应用时复核磁盘及目标版本，冲突时保留草稿
- 应用方案和恢复备份前查看逐文件新增/覆盖/移除及行级差异；按一次操作恢复整组配置，也兼容旧版单文件 `.bak-*` 备份。备份记录保存在 `~/.agent-session-manager/config-history/`，界面展示最近 100 条可读取记录

## 开发

```bash
npm install
npm run tauri dev     # 开发模式
npm run tauri build   # 打包（产物在 src-tauri/target/release/bundle/）
```

分发给其他 Mac 用户时，构建 macOS 13+ 的通用包（M 系列与 Intel）：

```bash
CI=true npm run tauri -- build --ci --target universal-apple-darwin --bundles app,dmg -- --offline --locked
```

需要预先安装 `aarch64-apple-darwin` 和 `x86_64-apple-darwin` 两个 Rust target；产物在 `src-tauri/target/universal-apple-darwin/release/bundle/`。`CI=true` 避免构建时通过 AppleScript 操作 Finder。分发前验证 DMG、双架构签名、系统最低版本与实际启动；仅附应用、面向接收者的说明及校验值，不附本机 CLI 数据。当前本地签名包未做 Developer ID 公证，接收者首次打开的确认步骤见 [Apple 官方说明](https://support.apple.com/zh-cn/102445)。本地整理的分发包放在已忽略的 `release/`。

测试统一放在 `tests/`：`unit/` 为 Node 单元测试，`e2e/` 为使用模拟 Tauri 接口和合成数据的浏览器回归，`integration/` 为后台进程集成测试，`manual/` 为依赖本机日志或已安装 CLI 的显式检查，`fixtures/` 为共用数据，`helpers/` 为浏览器配置。测试源码提交 Git；截图等产物写入已忽略的 `test-results/<浏览器>/`。

```bash
npm test                           # 全部前端单元测试，Node 22+
npm run test:rust                  # Rust 解析、用量和集成测试
npm run test:e2e                   # 全部浏览器回归，默认 WebKit，自动启停测试服务器
npm run test:e2e -- config usage   # 按文件名选择测试，不必手动启动 Vite
npm run test:e2e -- opencode opencode-v2  # 新旧 SQLite 结构，真实 Rust 解析器生成合成数据
npm run test:manual -- opencode-runtime # 已安装的官方 v2 CLI，隔离目录 + 本地模型接口，无需 API Key
npm run test:manual -- agent-context-runtime # 已安装的 Claude Code / Pi，真实上下文日志与 WebKit 验证
ENGINE=chrome npm run test:e2e -- opencode-v2
npm run test:integration -- collector   # 先构建 release；后台采集进程使用自清理临时数据
npm run test:manual -- real             # 本机真实 Qoder 日志，只读
npm run test:manual -- codex-history    # 本机真实 Codex 分页历史，只读
```

浏览器测试需要安装 Playwright WebKit（`npx playwright install webkit`）；`ENGINE=chrome` 使用已安装的 Google Chrome。`BASE_URL` 可指定已运行的开发服务器，默认使用独立临时端口，结束后自动关闭。完整浏览器回归包含生产 CSP 检查，会先构建前端。OpenCode/用量 fixture 导出需要 Cargo 在 PATH 中。真实日志检查不包含在 `npm test` 或 `test:e2e` 中；手动导出和截图在私有临时目录中自动清理。`TEST_ARTIFACTS` 可覆盖合成数据截图目录。

`agent-context` 浏览器回归通过 Rust 适配器生成 Claude Code、Qoder、Pi 和 OpenCode v1/v2 的合成日志，检查上下文折叠、消息计数、引用标签和搜索定位。`agent-context-runtime` 使用实际 Claude Code/Pi 程序及本地模型服务，生成提示词快照、环境附件、扩展消息和压缩摘要，再验证只读解析与界面；可用 `CLAUDE_BIN`、`PI_BIN` 指定可执行文件。Qoder 的未登录运行会被官方 CLI 拒绝，因此不把格式样例测试描述为完整的真实会话测试。

OpenCode 同时支持 v1 的 `session/message/part` 和 v2 的 `session_v2/session_message`：按实际表结构识别；v2 按 `seq` 和内容数组顺序显示消息，读取历史模型、思考及工具结果，兼容新旧表共存。适配仅创建连接内临时视图，源库以只读模式打开。结构依据 [OpenCode v2.0.15](https://github.com/anomalyco/opencode/blob/v2.0.15/packages/core/src/session/sql.ts) 及固定提交 `5ed03ad1377b549df83aa52dc11bdb07593a5327` 的消息定义；回归使用合成数据库，不包含用户日志。`opencode-runtime` 额外运行官方 CLI，新建会话、调用真实文件读取工具、续聊和分叉，验证生成数据库的只读解析、用量去重及 WebKit 展示；`OPENCODE_BIN` 可选择特定版本的可执行文件。

OpenCode 本机只读复测：`cd src-tauri && cargo run --release --offline --locked --example opencode_perf`。

用量本机复测：`cd src-tauri && cargo run --release --offline --locked --example usage_perf -- --storage <私有临时目录>`；输出首次同步、重启后热同步及请求表查询的数字与耗时。加 `--cold` 时要求空目录，不清除既有数据。统计语义与 ccusage 来源见 `src-tauri/src/usage/README.md`。

用量复测要求传 `-- --storage <临时目录>`，使用该目录下的 `cache.sqlite3` 和 `ledger.sqlite3` 验证迁移，不修改应用的累计账本。源日志仍只读。

后台进程验证：先完成 release 构建，再执行 `npm run test:integration -- collector`。它只在自清理的临时目录创建五类 Agent 日志，验证无窗口采集、迟建目录、单实例、重启/删除保留、暂停及真实进程 CPU/内存；不注册机器的登录服务。可用 `ASM_COLLECTOR_BIN` 指定待测可执行文件。

Codex 首屏及长会话复测：`cd src-tauri && cargo run --release --offline --locked --example session_perf`（`-- --quick-only` 可验证重启后摘要命中）。

本机只读性能复测：`cd src-tauri && cargo run --release --offline --locked --example catalog_bench`。
输出各 Agent 三轮目录耗时与 Qoder 后台名称补全耗时，不输出会话正文。

监听资源复测：`cd src-tauri && cargo run --release --offline --locked --example watch_bench`。
仅写入自清理的临时目录，统计 10 秒空闲、100 次连续写入与后台暂停时的 CPU、峰值内存增量和通知数量。

配置校验检查语法和根节点结构，不限制各 CLI 版本的自定义字段。已写入文件并不代表运行中的 CLI 已重新加载。

## 致谢

会话发现逻辑参考了 [agent-session-cleaner](https://github.com/haowang02/agent-session-cleaner)（TUI 实现）。用量与请求事件的字段及计量规则参考了 [ccusage](https://github.com/ccusage/ccusage) 和 [agent-events](https://github.com/haowang02/agent-events)，具体来源和固定版本见 [用量模块说明](src-tauri/src/usage/README.md)。第三方许可保存在 [public/licenses](public/licenses)，Agent 品牌标识的来源和权利说明见 [图标来源](src/assets/agents/README.md)。

## 许可证

项目代码采用 [MIT 许可证](LICENSE)。第三方软件和 Agent 品牌标识保留各自的许可与权利声明。
