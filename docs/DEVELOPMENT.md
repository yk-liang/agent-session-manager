# 开发与测试

开发需要 Node.js 22+、Rust 和 macOS Command Line Tools。以下命令在仓库根目录执行；首次构建需下载依赖，已缓存依赖后可使用 `--offline`。

```bash
npm ci
npm run tauri dev     # 开发模式
npm run tauri build   # 打包（产物在 src-tauri/target/release/bundle/）
```

应用图标的主源文件是 `src/assets/brand/app-icon.svg`。修改后运行 `npm run icons`，会生成深色 SVG 和 `src-tauri/icons/` 下的桌面图标。深色版只改变底座配色，与默认版共用同一套路径；不要手动修改生成文件。

应用目前使用深色界面，浏览器标签图标随系统配色选择。macOS Dock/Finder 使用兼容 macOS 13+ 的默认 `.icns`。系统级 Default/Dark/Mono 图标切换需要单独编译并打包 [Icon Composer](https://developer.apple.com/icon-composer/) 资源，当前未启用；两份 SVG 不会让 `.icns` 自动切换。

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
npm run test:e2e -- cold-start     # 慢首次导入、进度、失败重试与空表/大数字布局
npm run test:e2e -- opencode opencode-v2  # 新旧 SQLite 结构，真实 Rust 解析器生成合成数据
npm run test:manual -- opencode-runtime # 已安装的官方 v2 CLI，隔离目录 + 本地模型接口，无需 API Key
npm run test:manual -- agent-context-runtime # 已安装的 Claude Code / Pi，真实上下文日志与 WebKit 验证
ENGINE=chrome npm run test:e2e -- opencode-v2
npm run test:e2e -- session-diagnostics # 缓存摘要与正文故障、诊断复制、重试、用量同步
npm run test:integration -- collector   # 先构建 release；后台采集进程使用自清理临时数据
npm run test:manual -- real             # 本机真实 Qoder 日志，只读
npm run test:manual -- codex-history    # 本机真实 Codex 分页历史，只读
```

浏览器测试需要安装 Playwright WebKit（`npx playwright install webkit`）；`ENGINE=chrome` 使用已安装的 Google Chrome。`BASE_URL` 可指定已运行的开发服务器，默认使用独立临时端口，结束后自动关闭。完整浏览器回归包含生产 CSP 检查，会先构建前端。OpenCode/用量 fixture 导出需要 Cargo 在 PATH 中。真实日志检查不包含在 `npm test` 或 `test:e2e` 中；手动导出和截图在私有临时目录中自动清理。`TEST_ARTIFACTS` 可覆盖合成数据截图目录。

## 会话读取诊断

Qoder/Claude 系 JSONL 读取失败会区分打开、元数据检查、读取、UTF-8 解码与 JSON 解析阶段；用量同步同时保留来源文件、同步锁和账本打开阶段的原始错误。界面的“复制诊断详情”包含 `operation`、`stage`、`error_kind`、`os_code`、`cause`、路径、时间，以及产生错误的进程角色、PID、应用/系统版本、架构、UID/EUID 和文件描述符上限。SQLite 错误另列 `sqlite_code`，不会把 SQLite 扩展码冒充系统 errno。没有系统错误码时记为 `none`，不能据此推断权限正常或异常。诊断不复制原始日志行，但会包含本机路径，外发前按需脱敏。

列表摘要缓存命中只代表文件版本相同，不证明当前 GUI 能打开正文。打开失败的卡片保留“正文读取失败”标记；摘要刷新与模型名称补全不会清除此状态，成功重读同一正文才清除。标记只保留在当前应用进程内。扫描告警条数不代表全部不可读会话；正文失败、扫描告警及用量同步失败都有持续可见的诊断和重试入口。部分 JSON 坏行允许继续显示合法内容，同时保留首条坏行的诊断；系统读取或 UTF-8 解码失败不再伪装成 JSON 坏行。

排查远程反馈时，让用户在**发生故障的 GUI** 中打开原会话或重试用量同步，复制该次诊断。终端读取成功不能替代 GUI 的失败证据；在看到阶段和原始错误前，不推定权限、`O_NONBLOCK` 或文件内容是根因。保留会话、缓存和 FIFO 非阻塞保护。

若所有文件访问均失败且没有可用摘要，界面显示“会话数量暂不可确认”，不能把本次空结果当作历史为空。部分扫描成功时只标明“已列出”的数量；无告警的正常空目录仍显示零个会话。

对于 GUI 明确返回 `open / PermissionDenied / os_code: 1`、授权后仍失败的情况，可让反馈用户保留当前安装包并运行 `bash scripts/asm-access-diagnostics.command`（也可单独转发此脚本）。脚本使用 macOS 自带命令，提示用户在当前 GUI 重试后回车，采集应用签名与校验、进程及父进程、系统扩展、设备管理状态、Qoder 路径元数据和关联的 TCC/sandbox 系统日志；报告保存在仅当前用户可读的临时目录并在 Finder 中定位。不读取会话正文，不修改权限、缓存、签名或系统日志设置，不上传报告。可附带一个加引号的故障文件路径；未提供时只检查第一份普通 JSONL 的元数据，不把该样本当作原故障文件。`--collect-only` 跳过交互与实时日志，适合检查脚本或采集已有事件。系统可能不记录/公开拦截原因，没有关联日志不能排除某种访问控制；发现安全扩展也不等于证明它造成了拦截。外发前检查报告中的路径、软件名和系统日志。

日志按应用身份及相关路径筛选，不限制发送进程，避免遗漏终端安全软件自己的记录。若下载的脚本本身无法读取，可直接在终端粘贴以下只读命令，并在 45 秒采集窗口内从当前 GUI 重试读取原失败会话；结束后连同该次 GUI 的 PID、时间和路径对照分析，无需重新安装或修改权限：

```bash
/usr/bin/log stream --timeout 45 --style compact --level debug \
  --predicate 'eventMessage CONTAINS[c] "agent-session" OR eventMessage CONTAINS[c] "Agent Session Manager" OR eventMessage CONTAINS[c] "/.qoder/" OR eventMessage CONTAINS[c] "asm-access-diagnostics"'
```

区分旁支日志与故障证据：`anchor apple; status: -67050` 表示不满足指定的 Apple 代码要求，不等于签名损坏；应与不加额外要求的 `codesign --verify --strict --all-architectures` 校验分开。`Microphone`、`Camera`、`ListenEvent`、`AppleEvents` 的权限检查也不证明 JSONL 打开被拦截，不能据此添加无关 entitlement。网络扩展列表不是文件访问拦截证据；需要同一次重试的拒绝记录或由管理员确认命中策略。只有启动时的检查、或没有匹配日志，均不能确定读取失败的根因。

`cached_summary_does_not_hide_detail_open_error` 在独立测试子进程中降低文件描述符上限，先命中摘要缓存，再制造真实打开失败，验证原始 errno 到详情错误的传递；这只验证错误传播，不证明用户故障是描述符耗尽。其余 Rust 回归覆盖中途 I/O 失败、UTF-8/JSON 区分、FIFO/普通符号链接、同步失败后的历史保留与重试去重。WebKit 回归检查多个失败卡片与单条扫描告警并存、跨 Agent 隔离、过期请求、复制及重试。

## 数据源与性能验证

`agent-context` 浏览器回归通过 Rust 适配器生成 Claude Code、Qoder、Pi 和 OpenCode v1/v2 的合成日志，检查上下文折叠、消息计数、引用标签和搜索定位。`agent-context-runtime` 使用实际 Claude Code/Pi 程序及本地模型服务，生成提示词快照、环境附件、扩展消息和压缩摘要，再验证只读解析与界面；可用 `CLAUDE_BIN`、`PI_BIN` 指定可执行文件。Qoder 的未登录运行会被官方 CLI 拒绝，因此不把格式样例测试描述为完整的真实会话测试。

OpenCode 同时支持 v1 的 `session/message/part` 和 v2 的 `session_v2/session_message`：按实际表结构识别；v2 按 `seq` 和内容数组顺序显示消息，读取历史模型、思考及工具结果，兼容新旧表共存。适配仅创建连接内临时视图，源库以只读模式打开。结构依据 [OpenCode v2.0.15](https://github.com/anomalyco/opencode/blob/v2.0.15/packages/core/src/session/sql.ts) 及固定提交 `5ed03ad1377b549df83aa52dc11bdb07593a5327` 的消息定义；回归使用合成数据库，不包含用户日志。`opencode-runtime` 额外运行官方 CLI，新建会话、调用真实文件读取工具、续聊和分叉，验证生成数据库的只读解析、用量去重及 WebKit 展示；`OPENCODE_BIN` 可选择特定版本的可执行文件。

OpenCode 本机只读复测：`cd src-tauri && cargo run --release --offline --locked --example opencode_perf`。

用量本机复测：`cd src-tauri && cargo run --release --offline --locked --example usage_perf -- --storage <私有临时目录>`；输出首次同步、重启后热同步及请求表查询的数字与耗时。加 `--cold` 时要求空目录，不清除既有数据。统计语义与 ccusage 来源见 `src-tauri/src/usage/README.md`。

用量复测要求传 `-- --storage <临时目录>`，使用该目录下的 `cache.sqlite3` 和 `ledger.sqlite3` 验证迁移，不修改应用的累计账本。源日志仍只读。

存储版本对照使用 `scripts/usage-storage-fixture.py` 和显式忽略的 `usage::storage_bench::storage_comparison`。先在仓库根目录生成固定合成日志，再到 `src-tauri/` 编译、运行 release 测试；以下目录必须与实际应用数据隔离：

```bash
python3 scripts/usage-storage-fixture.py "$PWD/test-results/storage-comparison/logs"
cd src-tauri
ASM_STORAGE_BENCH_DIR="$PWD/../test-results/storage-comparison/new" \
ASM_STORAGE_BENCH_LOGS="$PWD/../test-results/storage-comparison/logs" \
ASM_STORAGE_BENCH_REPORT="$PWD/../test-results/storage-comparison/new.json" \
cargo test --release --offline --locked --lib usage::storage_bench::storage_comparison -- --ignored --exact --nocapture
```

合成样本为 63,500 个请求：Codex 50,000（含逐响应及累计通知）、Claude 8,000、Qoder 3,200、Pi 2,300，OpenCode 目录为空；OpenCode 非空与继承去重由常规 Rust/采集进程测试覆盖。报告包括关闭连接后的主库/缓存字节数、首次打开与同步耗时、9 次热同步/查询中位数，以及全部原始观测、请求事件、完整身份关系的 SHA-256 和完整统计结果。热同步断言源日志读取为 0 字节，账本执行 `quick_check`；报告可能含模型名，真实账本报告应保留在私有临时目录。

比较旧版本时，在隔离的旧提交检出中只加入 `storage_bench.rs` 和对应 `mod` 声明（沿用其已有 zstd 依赖），保存编译出的 release 测试可执行文件。旧/新程序必须使用同一路径、同一份固定日志和独立存储目录；复制已关闭的旧测试数据库给新版验证升级，另建空目录验证首次导入。检查三种 SHA-256、完整统计和请求数都一致，再比较空间与耗时。增量对照只运行一次 `python3 scripts/usage-storage-fixture.py <原固定日志目录> --append`，向首份 Codex 日志追加 10 个新请求，然后让旧/新可执行文件各同步自己的数据库；结果应增至 63,510 个请求。不要把活跃应用主库直接复制给测试，也不要在原工作区来回覆盖生产源码。

`cargo test --offline --locked cold_start_tests -- --test-threads=1 --nocapture` 使用全新临时目录，导入 256 份约 270 MiB 的日志、32,768 个请求，并在写入期间并发查询；检查完整快照、取消、重启后的零重复读取。日志输出同步和查询耗时，不包含会话正文。常规 Rust 回归会包含这些检查。

后台进程验证：先完成 release 构建，再执行 `npm run test:integration -- collector`。它只在自清理的临时目录创建五类 Agent 日志，验证无窗口采集、迟建目录、单实例、重启/删除保留、暂停及真实进程 CPU/内存；不注册机器的登录服务。可用 `ASM_COLLECTOR_BIN` 指定待测可执行文件。

Codex 首屏及长会话复测：`cd src-tauri && cargo run --release --offline --locked --example session_perf`（`-- --quick-only` 可验证重启后摘要命中）。

本机只读性能复测：`cd src-tauri && cargo run --release --offline --locked --example catalog_bench`。
输出各 Agent 三轮目录耗时与 Qoder 后台名称补全耗时，不输出会话正文。

监听资源复测：`cd src-tauri && cargo run --release --offline --locked --example watch_bench`。
仅写入自清理的临时目录，统计 10 秒空闲、100 次连续写入与后台暂停时的 CPU、峰值内存增量和通知数量。

配置校验检查语法和根节点结构，不限制各 CLI 版本的自定义字段。已写入文件并不代表运行中的 CLI 已重新加载。

用量解析、历史保留和兼容规则见 [用量模块说明](../src-tauri/src/usage/README.md)。

## README 与公开截图

英文首页为 `README.md`，中文版为 `README.zh-CN.md`，两版通过顶部链接互相切换，功能和安装说明应同步更新。

```bash
npm run docs:screenshots
```

截图脚本使用 Playwright WebKit（首次运行需 `npx playwright install webkit`），自动启动并关闭独立的本地 Vite 服务，将截图写入 `docs/images/`。它渲染真实前端，但通过 `scripts/readme-fixture.mjs` 提供虚构会话、请求与配置，拦截全部原生调用，不启动 Tauri、不读取本机 Agent 日志或配置。浏览器上下文独立，不复用本机登录状态；截图固定时区与时间，拦截外部请求。

公开截图只使用这套合成数据，不将 `test-results/` 或真实用户截图直接加入 README。生成后检查可见文本、文件路径与图片内容；功能变更后按需重新生成，无需提交其他测试截图。
