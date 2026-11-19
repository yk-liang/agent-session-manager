# 开发与测试

安装 Node.js 22 或更高版本、Rust 和 Xcode Command Line Tools。除非另有说明，命令均在仓库根目录运行。使用 `--offline` 前需下载 Rust 依赖。

## 数据目录

| Agent | 默认会话位置 |
| --- | --- |
| Qoder | `~/.qoder/projects/` |
| Claude Code | `~/.claude/projects/` |
| Codex | `~/.codex/sessions/`、`~/.codex/archived_sessions/` |
| OpenCode | `~/.local/share/opencode/` |
| Pi | `~/.pi/agent/sessions/` |
| Antigravity | `~/.gemini/antigravity*/conversations/`、`~/.config/antigravity/conversations/` |

`CODEX_HOME`、`CLAUDE_CONFIG_DIR` 和 `PI_CODING_AGENT_DIR` 可分别指定 Codex、Claude Code 和 Pi 的数据目录。OpenCode 优先读取 `OPENCODE_DATA_DIR`，其次使用 `XDG_DATA_HOME/opencode/`。Claude Code 的用量采集默认也会读取 `~/.config/claude/projects/`。`ANTIGRAVITY_DATA_DIR` 接受数据目录或会话目录，多个路径用逗号分隔。

配置方案和备份存放在 `~/.agent-session-manager/`。用量账本和缓存见[用量模块说明](../src-tauri/src/usage/README.md#存储)。

## 构建

```bash
npm ci
npm run tauri dev     # 开发模式
npm run tauri build   # 打包（产物在 src-tauri/target/release/bundle/）
```

### 应用图标

应用图标的源文件为 `src/assets/brand/app-icon.svg`。修改后运行 `npm run icons`，会生成深色 SVG 和 `src-tauri/icons/` 下的桌面图标。

浏览器标签页按系统配色选择 SVG，程序坞和访达使用默认的 `.icns`。

## 打包与发布

仅构建 Apple 芯片版 `.app` 时使用 `aarch64-apple-darwin`，产物位于 `src-tauri/target/aarch64-apple-darwin/release/bundle/macos/`。

```bash
npm run tauri -- build --target aarch64-apple-darwin --bundles app -- --locked
```

构建通用发行版前，安装 `aarch64-apple-darwin` 和 `x86_64-apple-darwin` 两个 Rust 目标。产物位于 `src-tauri/target/universal-apple-darwin/release/bundle/`。

```bash
CI=true npm run tauri -- build --ci --target universal-apple-darwin --bundles app,dmg -- --offline --locked
```

CI 构建跳过访达布局。发布前重新打包 DMG，校验双架构、签名和最低系统版本，并实际启动应用。应用、安装说明和校验值放在 `release/`。

### DMG 安装窗口

`npm run dmg:background` 从 `scripts/generate-dmg-background.swift` 生成背景图。应用图标和「应用程序」快捷方式的位置由 `tauri.conf.json` 的 `bundle.macOS.dmg` 指定。

登录 macOS 桌面后，使用已经验证的 `.app` 打包。先创建输出目录并选用未占用的文件名。

```bash
mkdir -p release/v0.1.39/repacked
npm run package:dmg -- \
  'src-tauri/target/universal-apple-darwin/release/bundle/macos/Agent Session Manager.app' \
  release/v0.1.39/repacked/Agent-Session-Manager_0.1.39_universal.dmg
```

脚本用应用副本打包，校验版本和签名。首次运行可能需要授权控制访达。隐藏资源使用窗口右侧的正坐标，避免重新挂载后访达重排图标。

只读挂载 DMG，检查窗口布局及 `/Applications` 快捷方式。核对包内签名和文件清单后更新 `SHA256SUMS.txt`。如果应用内容相同，可保留原 ZIP 和发布标签。替换发布附件前备份旧附件，上传后核对 SHA-256。

## 测试

`tests/` 包含前端单元测试、浏览器回归、采集进程集成测试和本机数据检查。合成数据放在 `fixtures/`，共用辅助代码放在 `helpers/`。截图写入 `test-results/<浏览器>/`。

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

浏览器测试需要安装 Playwright WebKit（`npx playwright install webkit`）；`ENGINE=chrome` 使用已安装的 Google Chrome。`BASE_URL` 可指定已运行的开发服务器，默认使用独立临时端口，结束后自动关闭。完整浏览器回归包含生产 CSP 检查，会先构建前端。OpenCode 和用量回归通过 Cargo 生成测试数据，需将 Cargo 加入 `PATH`。真实日志检查不包含在 `npm test` 或 `test:e2e` 中；手动导出和截图在私有临时目录中自动清理。`TEST_ARTIFACTS` 可覆盖合成数据截图目录。

## 会话读取诊断

排查时，在**发生故障的应用窗口**中重试原操作并复制诊断。优先核对阶段、原始错误、时间和进程。Qoder 和 Claude Code 的 JSONL 诊断区分打开、元数据检查、读取、UTF-8 解码与 JSON 解析。解析坏行时仍可读取其他合法行。诊断包含 `stage`、`error_kind`、系统错误码及进程信息。报告包含本机路径，分享前应检查并按需脱敏。

摘要和正文分别读取。卡片上的「正文读取失败」在成功重读后清除。目录扫描部分失败时，会话数量仅包含成功读取的记录。

如果应用在打开文件时返回 `PermissionDenied`、`os_code: 1`，可运行 `bash scripts/asm-access-diagnostics.command '<故障文件路径>'`。在应用中重试后按回车，脚本将签名、进程、权限相关日志和路径元数据写入个人临时目录，并在访达中定位。省略路径时仅抽查一份日志。`--collect-only` 只收集已有信息。脚本以只读方式采集诊断，会话正文无需提供。

也可在以下命令运行期间重试，再按 PID、时间和路径核对系统日志。

```bash
/usr/bin/log stream --timeout 45 --style compact --level debug \
  --predicate 'eventMessage CONTAINS[c] "agent-session" OR eventMessage CONTAINS[c] "Agent Session Manager" OR eventMessage CONTAINS[c] "/.qoder/" OR eventMessage CONTAINS[c] "asm-access-diagnostics"'
```

`anchor apple; status: -67050` 表示未满足指定的 Apple 代码要求。完整签名单独用 `codesign --verify --strict --all-architectures` 校验。访问故障以同一次重试的拒绝记录或已确认的设备策略为依据。保留原会话、缓存和 FIFO 非阻塞保护。

`cached_summary_does_not_hide_detail_open_error` 通过降低子进程文件描述符上限验证 errno 传递。其他回归覆盖读取中断、文件类型、同步重试及多条诊断并存。

## 数据源与性能验证

`agent-context` 使用 Rust 适配器生成的合成数据，验证上下文展示、消息计数和搜索定位。`agent-context-runtime` 用本地模型服务运行 Claude Code 和 Pi，再验证真实日志的只读解析。可通过 `CLAUDE_BIN` 和 `PI_BIN` 指定程序。Qoder 使用合成数据覆盖。

OpenCode 适配同时支持 v1（`session/message/part`）和 v2（`session_v2/session_message`），源库以只读模式打开。`opencode-runtime` 用实际 CLI 验证续聊、分叉、用量去重和界面展示，可通过 `OPENCODE_BIN` 指定程序。[OpenCode v2.0.15 数据库结构](https://github.com/anomalyco/opencode/blob/v2.0.15/packages/core/src/session/sql.ts)

### 用量与后台采集

在 `src-tauri/` 中运行 `cargo run --release --offline --locked --example usage_perf -- --storage <私有临时目录>`，测量首次同步、重启后的增量同步和请求查询。该命令以只读方式访问日志，在指定目录下使用 `cache.sqlite3` 和 `ledger.sqlite3`。加 `--cold` 时需提供空目录。

存储版本对照使用 `scripts/usage-storage-fixture.py` 和显式忽略的 `usage::storage_bench::storage_comparison`。先在仓库根目录生成固定合成日志，再到 `src-tauri/` 编译、运行 release 测试；以下目录必须与实际应用数据隔离：

```bash
python3 scripts/usage-storage-fixture.py "$PWD/test-results/storage-comparison/logs"
cd src-tauri
ASM_STORAGE_BENCH_DIR="$PWD/../test-results/storage-comparison/new" \
ASM_STORAGE_BENCH_LOGS="$PWD/../test-results/storage-comparison/logs" \
ASM_STORAGE_BENCH_REPORT="$PWD/../test-results/storage-comparison/new.json" \
cargo test --release --offline --locked --lib usage::storage_bench::storage_comparison -- --ignored --exact --nocapture
```

样本包含 63,500 个合成请求，覆盖 Codex、Claude Code、Qoder 和 Pi。报告记录库文件大小、首次同步耗时，以及 9 次热同步和查询的中位数，同时比较原始计量、请求事件和身份关系的 SHA-256。保留真实账本报告时请使用私有目录。

对照版本时，在独立检出中编译旧版本。两个版本使用同一路径下的合成日志，各自使用独立账本。验证升级时从已关闭的旧库复制副本，验证首次导入时使用空目录。先比较 SHA-256、统计和请求数，再比较空间和耗时。增量测试只运行一次 `python3 scripts/usage-storage-fixture.py <原固定日志目录> --append`，两个版本同步后的请求数应为 63,510。

`cargo test --offline --locked cold_start_tests -- --test-threads=1 --nocapture` 使用全新临时目录，导入 256 份约 270 MiB 的日志、32,768 个请求，并在写入期间并发查询；检查完整快照、取消、重启后的零重复读取。日志输出同步和查询耗时，不包含会话正文。常规 Rust 回归会包含这些检查。

用量页由应用内采集器更新。要验证采集器的独立进程入口，先构建 release，再运行 `npm run test:integration -- collector`。测试用临时日志验证数据目录迟建、单实例、重启、删除、暂停及资源开销。可用 `ASM_COLLECTOR_BIN` 指定可执行文件。

真实请求检查可用 `CLAUDE_BIN=/path/to/claude CODEX_REQUEST_ROOT=/path/to/rollouts npm run test:manual -- request-events-runtime`。Claude Code 部分使用临时目录和本地 SSE 服务，Codex 部分只读现有日志。账本分页性能测试使用 `ASM_REQUEST_LEDGER_COPY=/path/to/copy.sqlite3 cargo test --offline --locked request_events_ledger_copy_performance_if_requested -- --nocapture`，账本副本须通过 SQLite backup API 生成。

### Codex 会话

首屏及长会话复测命令为 `cd src-tauri && cargo run --release --offline --locked --example session_perf`（`-- --quick-only` 可验证重启后摘要命中）。

指定本机某条分页会话做只读完整性检查：在 `src-tauri/` 中运行 `ASM_TEST_CODEX_SESSION='<会话ID>' cargo test --release --offline --locked --lib agents::codex::history_tests::real_paginated_history_is_unique_and_complete_if_present -- --exact --nocapture --test-threads=1`（将占位内容替换为实际 ID）。会话未列出或存在历史告警时测试失败，避免指定目标未命中却被当作验证通过；默认只输出文件数、消息数、事件数和告警数，不导出正文。压缩首段与两段普通续页的独立回归为 `compressed_first_page_is_restored_before_two_plain_continuations`。

Codex 压缩历史回归：`cd src-tauri && cargo test --offline --locked --lib compressed -- --test-threads=1`。覆盖压缩与普通日志混合分页、解压后字节边界、按需工具读取、追加帧和缓存刷新。归档、取消归档和删除由本机 Codex App Server 执行。可用 `cargo run --release --offline --locked --example session_perf -- --catalog-only --include-archived` 核对本机目录，输出数量和耗时。

本机只读性能复测：`cd src-tauri && cargo run --release --offline --locked --example catalog_bench`。
输出各 Agent 三轮目录耗时，不输出会话正文。

### Qoder 元数据

会话列表按会话 ID 从 `com.qoder.app.stable/main.sqlite` 补充桌面端标题。无法读取数据库时沿用日志标题。模型使用日志中最后一次有效运行配置的标识。

真实会话只读核验：在 `src-tauri/` 运行 `cargo run --release --offline --locked --example qoder_metadata -- '<会话ID>' '<预期会话名>' '<预期模型标识>'`。只输出所选会话的标题、模型标识和告警数；同时检查列表与详情一致。后两个参数可省略，指定的会话不存在、重复或与预期不符时必须失败。

删除前用量保存的回归：在 `src-tauri/` 运行 `cargo test --release --offline --locked --lib usage::deletion -- --test-threads=1`。覆盖主日志与嵌套子会话、最后一轮增量、重启后持久化、无关来源隔离、缓存保留、读取/账本失败和同步期间日志变化。

真实数据性能复测使用 `usage::deletion::tests::real_qoder_deletion_preservation_benchmark`，必须显式加 `--ignored --exact --nocapture --test-threads=1`，并提供 `ASM_DELETE_PROBE_SESSION`（真实会话 ID）、`ASM_DELETE_PROBE_PROJECT`（源项目目录键）和 `ASM_DELETE_PROBE_STORE`。后者必须是独立目录，包含用 SQLite backup API 从应用数据库生成的 `cache.sqlite3` 与 `ledger.sqlite3`，不要直接复制运行中数据库的主文件。测试分别复制这份已关闭的备份，交替测量全量与局部保存，输出耗时和日志数量；实际日志只读，删除回调为空操作。目标缺失或检查失败必须报错，不能静默跳过。此测量不包含 GUI 调度、共享账本锁等待或 Finder 移入废纸篓耗时。

### 项目分组与文件监听

本机只读分组审计命令为 `cd src-tauri && cargo run --release --offline --locked --example grouping_audit`，可追加 `-- qoder` 限定数据源。输出会话总数、展示分组数、未归属数量及 pytest 临时目录漏归组数量，不输出正文、标题或完整路径。真实 Git 项目中的 pytest 目录应保留项目归属，因此漏归组计数需结合样本判断。

适配器通过 `agents/workspace_groups.rs` 识别自动生成的工作区，包括系统临时目录下的 `pytest-of-*/pytest-<数字或current>/...` 和以 `sessions/<源会话 ID>` 结尾的临时运行目录。规则支持 macOS `/private` 路径别名和已清理的历史目录；不将任意临时路径都视为无项目。实际 Git 仓库（包括 worktree、符号链接指向的仓库）优先，生成的 `AGENTS.md` 不作为这些自动目录的项目证据，权限异常时保留原分组。Codex 原生项目归属优先于公共推断；状态损坏时告警并保留目录分组。所有提示以 `(project_dir, session_id)` 定位，只影响展示，读取和删除仍使用原始来源身份。

监听资源复测：`cd src-tauri && cargo run --release --offline --locked --example watch_bench`。
仅写入自清理的临时目录，统计 10 秒空闲、100 次连续写入与后台暂停时的 CPU、峰值内存增量和通知数量。

### 配置

配置校验检查语法和根节点结构，并允许 CLI 自定义字段。保存后，正在运行的 CLI 可能需要重启。

用量解析、历史保留和兼容规则见 [用量模块说明](../src-tauri/src/usage/README.md)。

## README 截图

同步维护英文 `README.md` 和简体中文 `README.zh-CN.md` 的功能说明与安装步骤。

```bash
npm run docs:screenshots
```

脚本使用 Playwright WebKit 和 `scripts/readme-fixture.mjs` 的合成数据，生成带窗口标题栏和透明边距的截图，默认写入 `docs/images/`。可追加 `-- <输出目录>` 将截图写入其他目录。脚本拦截原生调用和外部请求，公开截图请使用这套合成数据。
