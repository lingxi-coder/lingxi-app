# Code Optimizer — 剩余待改清单

日期：2026-08-25
范围：`lingxi-code/` + `clients/`（排除 `third_party`、`claude-code`、`claw-code`、`codex`、`target`）
来源：`/code-optimizer` 审计 + 已落地的 batch 1 / batch 2。

本文原列剩余待改项。2026-08-25 续做一轮后，文中 16 条「建议改」中 15 条已落地；Grep 自定义正则上限因与 Claude Code 2.1.245 的 ripgrep 默认值不一致而撤回（见下方说明）。
明确不改项仍成立。历史 batch 见文末附录。

---

## 本轮已改（2026-08-25 续）

1. CLI `Settings::load_scoped` 按 `(cwd, include_user, include_project)` 缓存
2. Write 1GiB 上限（与 Edit 同一常量）
3. file-history 先比 size，再 64KiB chunk 比较
4. HTTP 非 SSE 响应体 16MiB 流式 cap
5. headersHelper 流式读 stdout，超 1MiB 杀进程
6. ~~Grep `nest_limit(50)` + 10MiB size/dfa~~ — 已撤回；保持 Claude/ripgrep 默认的 nest 250、size 100MiB、DFA 1000MiB
7. AnalyticsBus 生产路径 spawn sink（测试仍 await）
8. OTEL counter/histogram instrument 缓存
9. markdown `render_with_width` LRU 64
10. stream-json 仅对 `stream_event` 做 8192 pending cap
11. 权限路径 walk 改 HashSet
12. local-apps 按 manifest 字段建 `json_extract` 表达式索引
13. iOS transcript message/item `UUID → index`
14. Electron `richPromptText` / 语音结果改 `join`
15. Electron `app.asar` 打包 + verify 解包校验
16. Android release R8 minify + UniFFI/JNI keep（**尚未设备冒烟**）

---

## 建议改（可落地、不破坏 Claude 字节锁）

按优先级：热路径 / 体积上限 → 客户端构建 → 较大重构。

### 1. CLI 启动重复加载 Settings

| | |
|---|---|
| **严重度** | HIGH |
| **文件** | `crates/apps/cli/src/init.rs`（约 411–589 行） |
| **现状** | `load_provider_profiles`、`load_settings_ax_screen_reader`、`load_always_thinking_enabled`、`load_settings_model`、`load_settings_plans_directory`、`load_settings_api_key_helper`、`load_settings_company_announcements`、`load_settings_emoji_completion_enabled`、`load_lingxi_md_excludes`、`load_routing` 各自 `Settings::load_scoped`，启动时反复读盘解析同一套 JSON。 |
| **建议** | 仿 `engine-desktop` 的 `load_merged_settings`：按 `(project_dir, include_user, include_project)` 缓存一次 `EffectiveSettings`，各 helper 只取字段。 |
| **参考** | `crates/apps/engine-desktop/src/lib.rs` `load_merged_settings`（约 4265 行）已做。 |
| **验证** | `cargo test -p cli` 里走 `--setting-sources` 的 boot 测试。 |

### 2. Write 工具无体积上限

| | |
|---|---|
| **严重度** | HIGH |
| **文件** | `crates/tools/file/src/write.rs` `FileWriteTool::call`（约 242 行起） |
| **现状** | 解析 `content` 后直接 `tokio::fs::write`，模型可塞入超大字符串导致内存和磁盘膨胀。Edit 已有 `MAX_EDIT_FILE_SIZE = 1GiB`。 |
| **建议** | 在 `emit_started` 之前用同一常量拒绝：`content.len() as u64 > MAX_EDIT_FILE_SIZE` → `ToolError::InvalidInput`，文案对齐 Edit 的 `format_file_size`。可加 `pub const MAX_WRITE_FILE_SIZE = MAX_EDIT_FILE_SIZE`。 |
| **不要** | 把 Edit 的 1GiB 改小（字节锁，见「明确不改」）。 |
| **验证** | `tools/file` 的 write 测试加超大 content 负例。 |

### 3. file-history 全量字节比较

| | |
|---|---|
| **严重度** | HIGH |
| **文件** | `lingxi-code/session/src/file_history.rs` `origin_file_changed`（约 499–510 行） |
| **现状** | `tokio::fs::read` 两边整文件进内存再 `a != b`。大文件 /rewind 快照会双倍峰值内存。 |
| **建议** | 先比 `metadata.len()`；长度不同直接 `true`。长度相同再 64KiB chunk 流式比较，避免两份完整 `Vec<u8>`。 |
| **验证** | `session` 里 file-history 的 snapshot / rewind 测试。 |

### 4. MCP HTTP 响应体无上限

| | |
|---|---|
| **严重度** | HIGH |
| **文件** | `lingxi-code/http-client/src/reqwest_http.rs` `send_request`（约 152–161 行） |
| **现状** | `resp.bytes().await` 把整段 body 读进内存，再 `to_vec()` 一份副本。MCP HTTP / 普通 request 都走这里。JSON-RPC 帧已有 16MiB 上限（`jsonrpc/src/codec.rs` `DEFAULT_MAX_FRAME_SIZE`），HTTP 路径没有。 |
| **建议** | 先看 `Content-Length`，超限直接 `HttpError::InvalidResponse`。无长度则 `bytes_stream` 累加，超过 `16 * 1024 * 1024` 中止。不要把这条 cap 套到 SSE / `stream_sse`（长流）。 |
| **验证** | `http-client` 现有 request 测试 + 一条超大 body 负例。 |

### 5. headersHelper 等整段 stdout 再截断

| | |
|---|---|
| **严重度** | MEDIUM |
| **文件** | `lingxi-code/mcp/src/headers_helper.rs` `run_helper`（约 111–143 行） |
| **现状** | `process.output()` 等子进程结束，之后才查 `stdout.len() > 1MiB`。恶意/失控 helper 仍会先把整段 stdout 灌进内存。注释写明每次 connect 都要跑 helper（短凭证），**不要做结果 TTL 缓存**。 |
| **建议** | `spawn` + 流式读 stdout，超过 `MAX_HELPER_STDOUT` 立刻 `start_kill` 并报错。stderr 同样封顶。 |
| **验证** | 现有 unix `helper_receives_session_and_plugin_context`；再加一条超大 stdout 的超时/截断测试。 |

### 6. Grep 正则无复杂度上限

| | |
|---|---|
| **严重度** | MEDIUM |
| **文件** | `crates/tools/file/src/grep.rs`（约 962–967 行） |
| **现状** | `RegexMatcherBuilder::new()...build(pattern)` 无 `nest_limit` / `size_limit` / `dfa_size_limit`。病态正则可打爆 CPU（ReDoS）。 |
| **结论** | **不实施。** Claude Code 2.1.245 使用 ripgrep/grep-regex 的默认编译限制；本项目保持相同默认值：nest 250、size 100MiB、DFA 1000MiB。 |
| **原因** | 自定义 `nest_limit(50)` 和 10MiB size/DFA 会拒绝 Claude 接受的合法表达式，破坏工具输入兼容性与字节级对齐。 |
| **验证** | `accepts_deeply_nested_regex_groups` 锁定深度超过 50 仍可编译；其余合法/非法 pattern 继续走 grep-regex 的上游限制。 |

### 7. AnalyticsBus 同步等待 sink

| | |
|---|---|
| **严重度** | MEDIUM |
| **文件** | `lingxi-code/telemetry/src/bus.rs` `log_event`（约 54–71 行） |
| **现状** | 已 attach 的 sink 上 `sink.log_event(...).await`，遥测 HTTP 会堵住 turn loop。 |
| **建议** | 生产路径 `Arc::clone(sink)` 后 `tokio::spawn`。`#[cfg(test)]` 仍 await，避免 `InMemorySink` 测试竞态。 |
| **验证** | `telemetry` 的 `privacy_gate_suppresses_events` 等 bus 测试。 |

### 8. OTEL 每次 record 重建 instrument

| | |
|---|---|
| **严重度** | MEDIUM |
| **文件** | `lingxi-code/telemetry/src/otel/runtime.rs` `record_counter` / `record_histogram`（约 270–308 行） |
| **现状** | 每次 `meter.f64_counter(name).build()` / `f64_histogram(name).build()`。 |
| **建议** | 在 `MetricsRuntime` 上加 `Mutex<HashMap<String, Counter<f64>>>` 和 histogram map，`or_insert_with` 后 `add`/`record`。`Counter` 未必 `Debug`，给 `MetricsRuntime` 手写 `Debug`。`Mutex` 目前只在 `#[cfg(test)]` import，需要改成始终 import。 |
| **验证** | `telemetry` 的 otel 单测 / debug mirror。 |

### 9. TUI markdown 无独立缓存

| | |
|---|---|
| **严重度** | MEDIUM |
| **文件** | `crates/tui-core/src/render/markdown.rs` `render_with_width`（约 163 行） |
| **现状** | transcript 已有 committed wrap cache（`tui/src/transcript.rs`）。wrap cache 在 `committed_len` 变化时整表失效，每个 cell 会再走一遍 pulldown-cmark。 |
| **建议** | 在 `render_with_width` 做 LRU（约 64 条），key = `(text hash, width, theme)`。`StyledLine` 已 `Clone`。 |
| **验证** | `tui-core` markdown 渲染测试；确认 theme 切换不会串色。 |

### 10. stream-json 无界队列

| | |
|---|---|
| **严重度** | MEDIUM |
| **文件** | `crates/apps/cli/src/stream_json.rs`（`OutboundTx` / `enqueue_line` / `spawn_drain_task`） |
| **现状** | `mpsc::unbounded_channel`。`--include-partial-messages` 的 `stream_event` 在 stdout 慢时会无限堆积。heartbeat 已有 `CoalescedHeartbeatLines`。 |
| **建议** | **不要**把 result/init/assistant 改成可丢。对 `stream_event` 单独做 pending cap（例如 8192）：`Arc<AtomicUsize>` 在 enqueue +1、drain -1，超限丢新的 stream_event。协议帧保持 FIFO。 |
| **验证** | `stream_json` 里 `stream_event_*` 测试；确认 result 帧从不丢。 |

### 11. 权限路径 walk 用 Vec::contains

| | |
|---|---|
| **严重度** | LOW–MEDIUM |
| **文件** | `lingxi-code/permission/src/filesystem.rs` `resolve_additional_permission_paths`（约 288–331 行） |
| **现状** | `lineage.contains` / `resolved.contains` 都是 O(n)。环检测和去重在深 symlink 链上是二次方。 |
| **建议** | `HashSet<PathBuf>` 做 seen/lineage；输出顺序仍用 `Vec` 保持现有比较顺序。不要缓存解析结果（FS 会变）。 |
| **验证** | `permission` 里 symlink / fail-closed 测试（约 1293 行附近）。 |

### 12. local-apps `json_extract` 无表达式索引

| | |
|---|---|
| **严重度** | MEDIUM |
| **文件** | `lingxi-code/local-apps/src/data.rs` `append_filter` / `query`（约 712–783、383 行） |
| **现状** | 过滤/排序走 `json_extract(document, ?)` + `LIKE`。表上只有 `(collection, updated_at_ms)` / `created_at_ms` / `revision`。`DATA_SCHEMA_VERSION = 1`。连接缓存和这三列索引已做。 |
| **建议** | **不必立刻 FTS5 / 升 schema**。在 `ensure_manifest` 成功后 `CREATE INDEX IF NOT EXISTS` 每个 collection 字段的表达式索引：`json_extract(document, '$.{field_id}')`。`field_id` 已是 `^[a-z][a-z0-9_]{0,63}$`。Contains 的 leading-wildcard `LIKE` 索引帮不上，保持原样。 |
| **若做 FTS5** | 需 `rusqlite` feature `fts5`、content 表 + trigger、Contains 语义从子串变成 token，属于行为变化，单独立项。 |
| **验证** | `local-apps` 的 query/filter 测试。 |

### 13. iOS transcript `firstIndex` 线性扫

| | |
|---|---|
| **严重度** | MEDIUM |
| **文件** | `clients/ios/Sources/Conversation/ConversationSource.swift` |
| **现状** | 流式替换走 `streamingIndex`（已优化）。`replaceMessage` 仍 `messages.firstIndex { $0.id == id }` + `items.firstIndex` 扫 message case（约 2043–2073 行）。长会话每次 token/替换都是 O(n)。 |
| **建议** | `ConversationModel` 维护 `[UUID: Int]`：`messageIndexByID`、`itemIndexByMessageID`。`messages`/`items` 的 `didSet` 重建；`indexOfMessage` 命中则校验下标仍指向同一 id，否则 rebuild。先替换这两处，tools/phases 等小数组可后做。 |
| **验证** | iOS conversation 单测；确认 streaming 稳定 id 不被 SwiftUI 整表刷新。 |

### 14. Electron 字符串拼接

| | |
|---|---|
| **严重度** | LOW |
| **文件** | `clients/electron/src/renderer/components/BetaDesktop.tsx` |
| **现状** | `richPromptText`（约 273–284）在 DOM 子节点上 `value +=`；语音 `onresult`（约 627–631）对 `event.results` 做 `transcript +=`。 |
| **建议** | 改成 `string[]` + `join('')`。markdown.ts 的 list continuation `+=` 可以不管。 |
| **验证** | `clients/electron` 的 markdown / conversation 测试。 |

### 15. Electron 未打 asar

| | |
|---|---|
| **严重度** | MEDIUM（包体积 / 启动 IO） |
| **文件** | `clients/electron/scripts/package-mac.mjs`（约 141–144 行） |
| **现状** | 把 `out/` 和 `node_modules` 拷进 `Contents/Resources/app/` 未打包。已删 `default_app.asar`。 |
| **建议** | 加 devDependency `@electron/asar`，`createPackage(appDir, resources/app.asar)` 后删未打包 `app/`。`bridge-server` 继续放 `Resources/bin/`（不要进 asar）。 |
| **验证** | `clients/electron/test/packaging.test.mjs` + `verify:package`。 |

### 16. Android release 未开 R8

| | |
|---|---|
| **严重度** | MEDIUM（APK 体积） |
| **文件** | `clients/android/app/build.gradle.kts`（约 61–71 行）、`proguard-rules.pro` |
| **现状** | `release { isMinifyEnabled = false }`。UniFFI/JNI/JNA keep rules 已准备好，但尚未通过 minified release 的真机/模拟器冒烟。 |
| **建议** | `isMinifyEnabled = true`（先不要 `isShrinkResources`）。keep：`native` 方法、`uniffi.**`、`com.lingxi.code.**`、JNA。真机/模拟器跑 UniFFI 冒烟后再考虑 shrinkResources。 |
| **风险** | UniFFI 反射/JNI 被 strip 会运行时崩。没过设备验证不要合。 |

---

## 明确不改（本轮不要动）

| 项 | 原因 |
|---|---|
| Anthropic `storageV5` 后端 | 主流程对齐时已排除；session crate 标明 `performCompactTranscriptV5` 超范围。 |
| `MAX_EDIT_FILE_SIZE = 1GiB` | 锁 Claude `DYa = 1073741824`。Write 可复用该常量，不可改小。 |
| 会话 history 全量 `Arc` 化 | `history_snapshot` 在 `turn_loop` 里会 `push`/`extend`，改 `Arc<Vec<ConversationMessage>>` 是编排层大重构，单独立项。 |
| JSON-RPC / MCP 16MiB 帧 | `lingxi-code/jsonrpc/src/codec.rs` `DEFAULT_MAX_FRAME_SIZE` **已经是 16MiB**。不要再改协议常量。 |
| memdir 跳过大文件 | `memory/src/memdir/scan.rs` 已按 `MAX_MEMORY_FILE_SIZE`（10MB）在 `read_to_string` 前 skip。 |
| headersHelper 结果缓存 | 每次 connect 必须重跑，短凭证会过期。只改流式读，不缓存 JSON。 |
| `reqwest` 请求级 `timeout` 套到 SSE | 会掐掉长流。只保留 `connect_timeout`（已做）。 |
| 同一 stdio MCP 上并行 catalog RPC | 单连接不是线程安全。`connect_all` 的 `join_all` 已做，catalog 不要再对同一 stdio 并发。 |
| GitStatus 按 cwd 冻结 | Claude 行为，不是 bug。 |
| bash-ast feature-off | 权限 AST 一次性解析已做；关掉 feature 会改授权行为。 |

---

## 建议实施顺序

1. **Settings 缓存 + Write 上限 + file-history 流式比较 + HTTP body cap** — 启动和 IO 峰值，风险低。
2. **headersHelper 流式 cap、Grep nest/size_limit、AnalyticsBus spawn、OTEL instrument map、markdown LRU、stream_event pending cap、permission HashSet、json_extract 表达式索引** — 中等、可单测。
3. **iOS message/item 索引、Electron join/asar、Android R8+keep** — 客户端；R8 必须设备验证。

每项改完跑对应 crate/客户端测试，不要一次 `cargo test --workspace`（`target/` 曾把磁盘打满）。

---

## 附录：已经落地（不必再改）

**Batch 1**

- `FileSystem::read_file_prefix`；posix / posix-minimal / windows 前缀读
- session JSONL `read_lite` 64KiB 前缀
- catalog 先隐藏 sidechain/daemon/sdk
- git_status snapshot、跳过未用 file_tree、additional_context 复用、HashSet 工具过滤
- TUI wrap cache、idle skip redraw、`needs_animated_redraw`
- Glob/Grep `spawn_blocking`
- MCP `connect_all` `join_all`、`servers_with_tools` 缓存
- HTTP `connect_timeout` 10s；SSE 不加 request timeout
- `LINGXI_SHELL` 泄漏缓存、SSE in-place parse、`os_version` OnceLock、bash AST 一次、`wants_partial_stream_events`
- Cargo `debug=1` + thin LTO

**Batch 2**

- 窗口读 `read_utf8_windowed`（BufReader）
- catalog 两遍：cheap hide/cwd → 只对 page 做全量 parse
- local-apps 连接缓存 + created_at/revision 索引
- todo list 不再二次 async get
- image 20MiB cap、prompt_history 256KiB tail、posix process drain 8MiB
- permission Gitignore 缓存、sandbox `glob_to_regex` OnceLock、hooks regex 缓存
- llm-client retry/extra_body OnceLock、eventstream `copy_within`
- telemetry NoOpSink 仅在 DEBUG 打日志
- Cargo `default-members` 去掉 test-harness / 移动端 crate；`split-debuginfo=unpacked`
- engine-desktop `load_merged_settings` 按 `project_dir` 缓存
- Android contacts `IN (...)`、calendar 90 天 + `QUERY_ARG_LIMIT`
- `gradle.properties` parallel + configuration-cache
- `build-jni.sh` release `llvm-strip`

2.1.245 主流程（非本优化清单）：真实 stdout TTY → `isTTY`；messageQueue `consume`；不移植 storageV5。
