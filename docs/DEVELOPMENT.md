# 开发与测试

开发需要 Node.js 22+、Rust 和 macOS Command Line Tools。以下命令在仓库根目录执行；首次构建需下载依赖，已缓存依赖后可使用 `--offline`。

```bash
npm ci
npm run tauri dev     # 开发模式
npm run tauri build   # 打包（产物在 src-tauri/target/release/bundle/）
```

应用图标的主源文件是 `src/assets/brand/app-icon.svg`。修改后运行 `npm run icons`，会生成深色 SVG 和 `src-tauri/icons/` 下的桌面图标。深色版只改变底座配色，与默认版共用同一套路径；不要手动修改生成文件。

应用目前使用深色界面，标题栏使用深色 SVG；浏览器标签图标随系统配色选择。macOS Dock/Finder 使用兼容 macOS 13+ 的默认 `.icns`。系统级 Default/Dark/Mono 图标切换需要单独编译并打包 [Icon Composer](https://developer.apple.com/icon-composer/) 资源，当前未启用；两份 SVG 不会让 `.icns` 自动切换。

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

`cargo test --offline --locked cold_start_tests -- --test-threads=1 --nocapture` 使用全新临时目录，导入 256 份约 270 MiB 的日志、32,768 个请求，并在写入期间并发查询；检查完整快照、取消、重启后的零重复读取。日志输出同步和查询耗时，不包含会话正文。常规 Rust 回归会包含这些检查。

后台进程验证：先完成 release 构建，再执行 `npm run test:integration -- collector`。它只在自清理的临时目录创建五类 Agent 日志，验证无窗口采集、迟建目录、单实例、重启/删除保留、暂停及真实进程 CPU/内存；不注册机器的登录服务。可用 `ASM_COLLECTOR_BIN` 指定待测可执行文件。

Codex 首屏及长会话复测：`cd src-tauri && cargo run --release --offline --locked --example session_perf`（`-- --quick-only` 可验证重启后摘要命中）。

本机只读性能复测：`cd src-tauri && cargo run --release --offline --locked --example catalog_bench`。
输出各 Agent 三轮目录耗时与 Qoder 后台名称补全耗时，不输出会话正文。

监听资源复测：`cd src-tauri && cargo run --release --offline --locked --example watch_bench`。
仅写入自清理的临时目录，统计 10 秒空闲、100 次连续写入与后台暂停时的 CPU、峰值内存增量和通知数量。

配置校验检查语法和根节点结构，不限制各 CLI 版本的自定义字段。已写入文件并不代表运行中的 CLI 已重新加载。

用量解析、历史保留和兼容规则见 [用量模块说明](../src-tauri/src/usage/README.md)。
