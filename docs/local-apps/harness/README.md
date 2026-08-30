# Local App plugin 实施 harness

本目录是 `docs/local-apps/LOCAL-APP-PLUGIN-DESIGN-V2.md` 的**执行契约**：任务清单、
测试基线，以及一批「今天就已经会骗人」的判据的实测记录。判据引擎在
`lingxi-code/scripts/lap_gate.py`（驱动 `lap-gate.sh`）。

所有数字都是在 `lap/baseline` 这个 commit 上**量出来的**，不是从设计文档抄的。
设计文档里的数字凡与本文件冲突，以本文件为准，并把冲突记下来。

## 基线

`baseline-stageB.json` —— 15 个爆炸半径 crate 的一次
`cargo test --all-features --no-fail-fast`：

    164 个测试二进制启动，164 个报告了 test result 行（无截断）
    5031 passed，3 red

三条红是**继承来的**，不是本项目造成的（landing commit 只动了 docs 和 scripts）。
每一条都必须带着「为什么它红」进 baseline，否则后面任何一次「全绿」都无法区分
「我没弄坏东西」和「我把别人的红一起洗掉了」。

### 1. `local_app_runtime_profiles::tests::published_r1_contract_digests_are_immutable`

phaser-2d 实际算出 `7fd39e60…`，钉住的是 `38b5fed9…`。

**这个 pin 从写下来的那天起就是错的。** 证据：实际值 `7fd39e60…` 在**整个仓库的
任何一个 commit 里都不存在**（`git log -S … --all` 零命中）；而 phaser-2d 的模板内容
和 profile 模块自 `391d89fff`（写下这个 pin 的那个 commit，2026-08-28）以来都没有再动过。
连跑两次结果一致，所以是确定性的错值，不是 flake。

⚠️ **不要「修」它。** 它守的是 Local App runtime contract 的不可变性，正是 Phase 1
packer 要做 deterministic digest 的同一个面。把它改成实际值 = 让门去追认现实，
和 `BLESS=1` 重新基线化 contract_index 是同一个失败形状。要动它，得先说清楚
`391d89fff` 当时打算钉住的是什么。

### 2. `every_tool_with_output_calls_truncate_or_opts_out`

`web_fetch.rs` / `web_search.rs` 既没有 truncation 调用，也不在豁免集里。
WebSearch(Tavily) 是**用户确认过的有意分歧**，所以这条红大概率是分歧的副作用而不是缺陷。
本项目不碰这两个文件。

### 3. `filesystem::apply_line_window_tests::prefix_read_trims_only_incomplete_trailing_utf8`

`traits/src/filesystem.rs:440` 的 `assert!(got.truncated)`。该文件最后一次改动是
`24e91f6ab`（2026-08-25），早于 plugin-LSP 那次提交，所以不是它带进来的。

## 已实测的「会骗人的判据」

写任何一条门之前先读这一节。每一条都是**跑出来的**，不是读代码推出来的。

### 品牌门今天看不见 Claude 侧的 plugin 标识符

在 `apps/engine-mobile/src/lib.rs` 里种字面量，`./scripts/check-brand-leaks.sh` 的表现：

| 种进去的东西 | 结果 |
| --- | --- |
| `.lingxi-plugin` | **exit 1**，点名文件，点名规则 G1/G3 —— 门是会咬的 |
| `.claude-plugin` | exit 0，一声不吭 |
| `CLAUDE_PLUGIN_ROOT` | exit 0，一声不吭 |
| `CLAUDE_PROJECT_DIR` | exit 0，一声不吭 |

`L1_PATTERNS` 守的是 LingXi 字面量**逃出** `branding`，没有任何一条规则管
Claude 侧标识符**流进来**——而后者正是本项目引入的方向。另外今天的输出是
`规则<TAB>路径<TAB>needle`，**没有行号**，而 §19.3 要求点名文件与行号。
这些都是 P0a.0 的内容，不是「把脚本抄过来」。

§19.3 品牌 planted-positive 的另一半（`plugins/lingxi-local-app/` 下的
`CLAUDE_PLUGIN_ROOT`）**在这里跑不了**，那个目录还不存在。它跟着 Phase 2 第一个任务走。
不要写一条 fixture 不存在的断言。

### `generate.py --check` 有一条什么都不比的绿色通道

    python3 generate.py --check                                   -> OK: 1894 keys, 5 locales
    python3 generate.py --check --ios-out X --android-out Y        -> OK: 1894 keys, 5 locales

后者**一个字节都没有比对**：`stale_problems()` 在 `if not args.ios_out and not
args.android_out:` 里面，而 `OK:` 那行 print 在外面。两条路径的输出逐字节相同。
§19.10 把这个字符串当通过判据，所以门必须**禁掉这个参数组合**，并且另外断言 key 数。

### i18n 的三个基线计数（1893 个 key，不含 `__info_plist__`）

| 量 | 值 |
| --- | --- |
| Android 非法 key 名（`[a-zA-Z_][a-zA-Z0-9_]*` 不匹配） | 113 |
| 其中有显式 `_fmt` 兄弟 key 的 | 7 |
| **其中在 Android 侧完全没有对应 key 的** | **106** |
| 空值：zh-Hant / en / ja / ko | 26 / 20 / 26 / 26 |
| 与 zh-Hans 逐字相同（未翻译）：zh-Hant / en / ja / ko | 227 / 20 / 41 / 15 |

对着真实产物核过，不是推的：`1893 - 113 = 1780`，而
`clients/android/app/src/main/res/values/strings.xml` 里恰好有 1780 个 `<string name=`。
iOS 把占位符放在 **key** 里、Android 放在**值**里，所以每个带参字符串需要两个 key，
而**没有任何东西在强制这个配对**。§17.5 会新增带参 key，门必须断言这几个计数不上涨。

### `ClientCommand` 的 UniFFI 元数据只剩 2,247 字节

    UNIFFI_META_CONST_CLIENT_PROTOCOL_ENUM_CLIENTCOMMAND   14,137 / 16,384 字节（86.3%）
    uniffi_core-0.28.3/src/metadata.rs:87                  const BUF_SIZE: usize = 16384

§17.1 要加 ~7 个 operation。**Rust doc comment 是逐字拷进这个 buffer 的**，所以成本主要
是散文而不是结构。溢出是 const-eval 的 `error[E0080]`：它同时打断两个移动端构建，
而 `cargo build --workspace` 和整个测试套件**全绿**。

`create_metadata_items` 是**每个类型一个** 16 KiB buffer，所以把新 operation 收进一个
`LocalApp(LocalAppCommand)` 变体是真的搬走了字节（≈50 B vs ≈1,980 B 平铺）。
这是 Phase 8 **设计期**的决定，不是编码期的。

### `bridge-server` 今天是稳的,但判据仍然是三次运行

    run 1: 12 binaries, 88 passed, 0 failed
    run 2: 12 binaries, 88 passed, 0 failed
    run 3: 12 binaries, 88 passed, 0 failed        -> 三次导出的计数逐字节相同

`apps/bridge-server/src/driver.rs:553` 的 `LOOP_KA_TEST_SERIAL` 是
`std::sync::Mutex<()>`,**不是环境变量**。它在一个二进制内部串行化线程,
跨二进制什么都不做。所以「今天绿」不能推出「明天绿」,任何拥有这个 crate 的任务
仍然必须 `repeatRuns: 3`——`lap-gate.sh tasks` 会强制这一点。

`cargo-nextest` **未安装**。面对一个 15,430 个测试的套件,装 nextest 是最自然的
反应,而它会把每个二进制拆进独立进程,于是上面那把进程内的锁保护的每一个 race
都会静默回来。⛔ 不要在本项目里引入 nextest。

### 其他量出来的事实

| 事实 | 值 |
| --- | --- |
| `engine-mobile` 测试数：无 feature / `--all-features` | 2 / 452 |
| workspace 成员 / default-members | 88 / 74（`engine-mobile` 被排除） |
| `plugin` crate 的 fixture 目录 / 品牌归一化 | 都不存在 |
| `git remote -v` | 空 —— **没有 CI**，每一条门必须是签入的脚本 |
| `drive_local_workflow("local-canvas-build")` 调用点 | 1（`tools/workflow/src/builtins.rs`） |
| Canvas 两个具名门 | `builtins.rs:1944`、`:2509`，都在 |
| `profile_file!` 调用点 | 112 |
| `runtime-profiles/` 已跟踪文件 / 字节 | 124 / 387,943 |
| `templates/` 全目录已跟踪 / 磁盘占用 | 156 文件 / **524 MiB** |
| `babylon-3d` / `phaser-2d` 磁盘文件数 | 20,093 / 13,014（`mcpb.rs:16` `MAX_FILES = 10_000`） |
| `skills/` | 10 目录 / 44 md；`bundled.rs` 里 44 个 `include_str!` |
| `CLIENT_PROTOCOL_VERSION` / `APPS_SCHEMA_VERSION` | `9.0.0` / `2` |
| `blessed_major.txt` / `contract_index.json` | `9` / 925 条 @ `391d89fff` |
| MCP 版本常量 | `mcp/src/initialize_params.rs:9` = `2025-11-25`；`apps/cli/src/commands/mcp.rs:358` = `2025-06-18`（分歧） |

## 用法

    cd lingxi-code
    ./scripts/lap-gate.sh list          # 每个子命令守哪一条判据
    ./scripts/lap-gate.sh selftest      # 引擎自己的 10 个种雷用例
    ./scripts/lap-gate.sh tasks         # 加载时校验任务清单
    ./scripts/lap-gate.sh precheck --baseline ../docs/local-apps/harness/baseline-stageB.json

重跑基线：

    cargo test -p engine-mobile -p plugin -p client-protocol -p branding -p tasks \
      -p tool-workflow -p local-apps -p skill-api -p engine-desktop -p permission \
      -p traits -p mcp -p bridge-server -p test-harness -p cli \
      --all-features --no-fail-fast 2>&1 | tee /tmp/stageB.txt
    ./scripts/lap-gate.sh parse --run /tmp/stageB.txt --out /tmp/stageB.json

⛔ `--all-features` 不能省：`engine-mobile` 的 Local App 模块全在
`#[cfg(feature = "uniffi")]` 后面，452 个测试里有 450 个在没有这个 flag 时**不存在**。
