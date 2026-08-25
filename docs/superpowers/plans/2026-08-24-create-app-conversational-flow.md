# 对话式创建本地应用 —— §0-§H 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 mobile 的「创建本地应用」从两步原生表单改成对话式流程——点「+」立刻得到一个空壳记录，会话从第一轮起扎在它自己的工作区，代理逐步问清需求并经用户确认后调 `LocalAppScaffold` 落地脚手架。

**Architecture:** 新增持久判据 `AppRecord.scaffolded: bool`（`false` = 空壳）；服务层用 `CreateMode::{Shell, Scaffolded}` 把「空壳创建」与「今天的 create+scaffold」两条路彻底分开，空 brief 的放宽只发生在 `Shell`；新增 builtin 工具 `LocalAppScaffold` 在一次持锁事务里完成「清空可编辑面 → 写锁定文件与种子 → 写正式合约 → 四字段一次性提交」；`local_apps_mcp.rs` 的 `call()` 顶端加一道按 **provider operation** 判定的工具门，空壳期只放行 `scaffold`/`list`/`get`/`create`；协议 bless 到 **8.0.0**，删掉提议命令/事件，改用 `request_id` 关联创建结果。

**Tech Stack:** Rust（`local-apps`、`engine-mobile`、`client-protocol`、`permission`、`orchestrator`）、UniFFI、Swift/SwiftUI（iOS）、Kotlin/Compose（Android）、TypeScript（`clients/shared`）。

**Spec:** [`docs/superpowers/specs/2026-08-23-create-app-conversational-flow-design.md`](../specs/2026-08-23-create-app-conversational-flow-design.md) —— 本计划只实现该 spec 的 **§0–§H**。§I（Web runtime profile / Godot）是后续里程碑，**不在本计划范围内**。

**跨计划顺序:** [`docs/superpowers/specs/2026-08-24-local-app-implementation-order.md`](../specs/2026-08-24-local-app-implementation-order.md) 的**步骤 1**。本计划是 8.0.0 的唯一 bless owner；verification design 必须 rebase 上来，不得从 7.0.0 独立 bless。

---

## Global Constraints

以下每一条对**每个** Task 都成立。

- **不考虑向后兼容。** 这个 app 尚未发布。不实现旧 store 迁移，不给 `scaffolded` 加 serde default，不写「缺字段」的兼容分支。缺字段的记录**加载失败**并提示清除开发数据。
- **重装，绝不卸载。** 真机验收要清数据时用「重装覆盖」，卸载会毁掉用户设备上的本地应用。
- **`cargo test` 必须带 `--all-features`。** engine-mobile 的 local-apps 模块是 `#[cfg(feature = "uniffi")]`，不加整块被跳过。
- **全量输出落文件再 grep。** 只 grep `FAILED` 会丢掉 `failures:` 块里的测试名。**盯测试总数**是否下降，不只看失败数。
- **红要先见红。** 每条声称「钉住某个洞」的测试，必须先对着**未修复**的代码跑成红色，且红得点名缺失的具体内容。退出码不是证据。
- **命名禁区。** `local_apps_mcp.rs` 有一条断言对**全部工具 schema 的拼接串**做小写子串检查禁止 `template`；`local_app_template_removal_guard_test.rs` 另有四个禁用符号（`AppTemplateKind`/`AppTemplateDto` 等）。新工具名、参数名、枚举值、description 一律避开这些子串。
- **uniffi 结构体字段按位置编码** ⇒ 新字段一律**追加在最后**。
- **改 wire 之后必须先重新生成绑定再编客户端**（绑定是 gitignored 的构建产物）：
  - `bash clients/ios/scripts/build-xcframework.sh && (cd clients/ios && xcodegen generate)`
  - `bash clients/android/scripts/build-jni.sh --variant play`
- **i18n 只改 `clients/translations/*.json`。** `Localizable.xcstrings` 与 `values*/strings.xml` 是 `generate.py` 的产物，手改下次全还原。Android 另有一份**不由生成器管理**的 `values*/strings_local_apps_v3.xml`，需单独核对。
- **既有红测试（与本计划无关，不要试图修）:** `cargo test -p orchestrator` 有三个集成 target 在 debug 构建下 SIGABRT stack overflow：`mid_turn_input_test`、`streaming_cancel_test`、`streaming_partial_finalize_test`。已在干净树上复现确认。
- **⛔ 不要改** `local-apps/src/service.rs` 的 `create_app_enforces_brief_caps` 与 `create_app_rejects_an_empty_brief`。它们走默认包装 = `Scaffolded` 模式，必须继续绿；改了就等于拆掉该路径上唯一钉住空-brief 不变量的两条测试。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `lingxi-code/orchestrator/src/conversation.rs` | §0：`prompt_probe_cwd()` 单一推导；两个 prompt-side memory reader 共用它 |
| `lingxi-code/local-apps/src/types.rs` | `AppRecord.scaffolded` 持久判据 |
| `lingxi-code/local-apps/src/service.rs` | `CreateMode`；`scaffolded` 初值的唯一决定点；§C.1.5 的四字段提交写；`request_id` 参数 |
| `lingxi-code/local-apps/src/events.rs` | `AppEvent::AppCreated.request_id` |
| `lingxi-code/client-protocol/src/{commands,events,local_apps,version}.rs` | 8.0.0 的 wire 形状 |
| `lingxi-code/apps/engine-mobile/src/local_apps_build.rs` | `scaffold_workspace_initialized` 的清空开关；`detect_build_target` 分支 |
| `lingxi-code/apps/engine-mobile/src/local_apps_tools.rs` | `LOCAL_APP_TOOLS` 注册；`requires_bound_session_for_auto_allow` |
| `lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs` | `LocalAppScaffold` schema + dispatch；`call()` 顶端的工具门 |
| `lingxi-code/apps/engine-mobile/src/local_apps_host.rs` | `LocalAppScaffold` 实现；引导版 `LINGXI.md`；删提议实现 |
| `lingxi-code/apps/engine-mobile/src/local_apps_bridge.rs` | `lower_record` / `lower_app_event` 纯映射补齐 |
| `lingxi-code/apps/engine-mobile/src/host.rs` | `CreateApp` 按 mode 分叉；wire 空 `name` → 服务层 `None`；boot sweep 标题对账 |
| `lingxi-code/permission/src/defaults_per_tool.rs` | 新工具的默认权限 + 计数断言 |
| `clients/ios/Sources/LocalApps/**`、`clients/android/**/localapps/**` | 删表单、`request_id` 认领、草稿卡片、widget 入口搬家 |

---

## Task 1: §0 前置修复 —— 工作区 `LINGXI.md` 在移动端从未加载

> **状态：已完成，代码在工作区未提交。** 执行者请从 Step 5 开始（核对 + 提交）。

**Files:**
- Modify: `lingxi-code/orchestrator/src/conversation.rs`
- Test: `lingxi-code/orchestrator/src/conversation_test.rs`（`additional_context_tests` 模块内）

**Interfaces:**
- Produces: `ConversationOrchestrator::prompt_probe_cwd(&self, cwd: &Path) -> PathBuf` —— 后续任何 prompt-side 文件系统探针都必须走它，不要再写第四份 `match &self.prompt_probe_cwd_resolver`。

**为什么先做这个:** `additional_context_message` 是内存块的**唯一**渲染路径（`prompt/mod.rs:153` 起系统提示词不再拼它），而移动端 `session_cwd` 持的是 **guest** 路径（`host.rs:3291` 的 `model_cwd` 取自 `workspace_mount.guest_path`）。⇒ `apps/<id>/workspace/LINGXI.md` 在 iOS 上**从未进入模型上下文**。不修它，Task 5 写的引导版合约到不了模型，「先问用户」这一步比写入约束更早就断了。

🚨 **这三行修复的影响面远大于它的体积：它给整个移动平台同时打开 memory 加载**——嵌套 `@import` 展开、外部包含门、read-state 播种、每条首用户消息新增的 token。**必须独立提交、独立浸泡，不要和后面任何 Task 打进同一个提交。**

- [ ] **Step 1: 写会红的坐标测试**

在 `conversation_test.rs` 的 `additional_context_tests` 模块内加一个接受 `dyn MemoryHierarchyProvider` 的构造器和两条测试：

```rust
fn orch_with_provider(
    memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
) -> ConversationOrchestrator {
    ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        Arc::new(MockApiClient::new(vec![])),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        memory,
        std::env::temp_dir(),
    )
}

#[tokio::test]
async fn claude_md_is_probed_at_the_host_dir_behind_a_guest_session_cwd() {
    let host = tempfile::tempdir().expect("tempdir");
    let host_root = host.path().to_path_buf();
    std::fs::write(
        host_root.join("LINGXI.md"),
        "MARKER-host-probed-workspace-contract",
    )
    .expect("seed LINGXI.md");

    // The guest path the model sees. It does NOT exist on the host, which
    // is exactly why probing it directly yields nothing.
    let guest = std::path::PathBuf::from("/workspace/app-pathatlas-s3-probe");

    let resolver_root = host_root.clone();
    let orch = orch_with_provider(Arc::new(crate::prompt::RealMemoryHierarchyProvider))
        .with_session_cwd(tool_api::SessionCwd::new(guest.clone(), Vec::new()))
        .with_prompt_probe_cwd_resolver(Arc::new(move |_path: &std::path::Path| {
            resolver_root.clone()
        }));

    let body = text(&orch.additional_context_message().await.expect("present"));
    assert!(
        body.contains("MARKER-host-probed-workspace-contract"),
        "workspace LINGXI.md must reach the model; body was:\n{body}"
    );
    assert!(
        body.contains(&format!(
            "Contents of {}",
            host_root.join("LINGXI.md").display()
        )),
        "the memory block must name the HOST path; body was:\n{body}"
    );
}

#[tokio::test]
async fn claude_md_without_a_resolver_still_probes_the_session_cwd() {
    // Desktop INERT INVARIANT: no resolver installed ⇒ `probe_cwd == cwd`.
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        root.path().join("LINGXI.md"),
        "MARKER-desktop-session-cwd-probe",
    )
    .expect("seed LINGXI.md");

    let orch = orch_with_provider(Arc::new(crate::prompt::RealMemoryHierarchyProvider))
        .with_session_cwd(tool_api::SessionCwd::new(
            root.path().to_path_buf(),
            Vec::new(),
        ));

    let body = text(&orch.additional_context_message().await.expect("present"));
    assert!(
        body.contains("MARKER-desktop-session-cwd-probe"),
        "no-resolver path must keep probing the session cwd; body was:\n{body}"
    );
}
```

- [ ] **Step 2: 跑测试确认第一条红、第二条绿**

Run: `cd lingxi-code && cargo test -p orchestrator --lib claude_md_ -- --nocapture`
Expected: `claude_md_is_probed_at_the_host_dir_behind_a_guest_session_cwd` **FAILED**，panic 消息里 body 只有 `# currentDate`、**没有 `# claudeMd`**；`claude_md_without_a_resolver_still_probes_the_session_cwd` **ok**（证明红不是因为测试脚手架本身坏了）。

- [ ] **Step 3: 抽出单一推导并修复**

在 `conversation.rs` 里 `async fn build_prompt_context` 定义之前插入：

```rust
    /// PathAtlas S3: map the model-visible session cwd to the directory the
    /// prompt-side filesystem probes must actually read.
    ///
    /// On mobile the session cwd is a mobile-linux GUEST path
    /// (`engine-mobile`'s `model_cwd` comes from `workspace_mount.guest_path`),
    /// so probing it verbatim reads a directory that does not exist on the
    /// host. Desktop never installs a resolver, so `probe_cwd == cwd` there —
    /// byte-identical (INERT INVARIANT).
    ///
    /// Both prompt-side memory readers go through here: the system prompt
    /// ([`Self::build_prompt_context`]) and the per-turn `claudeMd` context
    /// message ([`Self::additional_context_message`]). Keeping ONE derivation
    /// is the point — the second reader having its own (raw, unresolved) copy
    /// is what kept every mobile workspace `LINGXI.md` out of the model.
    fn prompt_probe_cwd(&self, cwd: &std::path::Path) -> std::path::PathBuf {
        match &self.prompt_probe_cwd_resolver {
            Some(resolver) => resolver(cwd),
            None => cwd.to_path_buf(),
        }
    }
```

`build_prompt_context` 里原来的 inline match 换成 `let probe_cwd = self.prompt_probe_cwd(&cwd);`。

`additional_context_message` 里把

```rust
        let memory_files = self.memory.load(&self.session_cwd.cwd()).await;
```

换成

```rust
        //
        // PathAtlas S3: probe the HOST directory backing the (possibly guest)
        // session cwd — same hop `build_prompt_context` makes. This message is
        // the ONLY render path for the memory block (`prompt/mod.rs` no longer
        // splices it into the system prompt), so reading the raw guest path
        // here meant a mobile workspace `LINGXI.md` reached the model NOWHERE.
        let probe_cwd = self.prompt_probe_cwd(&self.session_cwd.cwd());
        let memory_files = self.memory.load(&probe_cwd).await;
```

- [ ] **Step 4: 跑测试确认两条都绿**

Run: `cd lingxi-code && cargo test -p orchestrator --lib claude_md_`
Expected: `test result: ok. 2 passed`

- [ ] **Step 5: 跑 orchestrator 与 engine-mobile 全量**

```bash
cd lingxi-code
OUT=/tmp/task1-orch.txt
cargo test -p orchestrator --all-features --no-fail-fast > "$OUT" 2>&1
grep -c '\.\.\. ok$' "$OUT"; grep -n '^failures:' -A 20 "$OUT"
cargo test -p engine-mobile --all-features --no-fail-fast > /tmp/task1-em.txt 2>&1
grep -c '\.\.\. ok$' /tmp/task1-em.txt; grep -n '^failures:' -A 20 /tmp/task1-em.txt
```

Expected: 无 `failures:` 块。orchestrator 会有那三个既有 SIGABRT target（见 Global Constraints），engine-mobile `EXIT=0`。

- [ ] **Step 6: 提交（只提交这一处）**

```bash
git add lingxi-code/orchestrator/src/conversation.rs lingxi-code/orchestrator/src/conversation_test.rs
git commit -m "Route the per-turn claudeMd block through the guest→host probe resolver"
```

- [ ] **Step 7: 真机验收 §G.0（在继续 Task 2 之前）**

重装覆盖安装 → 建一个测试用本地应用 → 进它的会话 → 让代理复述 `LINGXI.md` 里只在该文件出现过的一条约束。修复前它答不出来，修复后必须答得出。**这一轮不要和别的变更一起验。**

---

## Task 2: `AppRecord.scaffolded` + `CreateMode`

**Files:**
- Modify: `lingxi-code/local-apps/src/types.rs`（`AppRecord`，`:142` 起）
- Modify: `lingxi-code/local-apps/src/service.rs`（`create_app*` 五个函数，`:821`-`:960`）
- Modify: `lingxi-code/local-apps/tests/fixtures/v1/apps/*/app.json`
- Test: `lingxi-code/local-apps/src/service.rs` 内联测试 + `lingxi-code/local-apps/tests/serde_compat.rs`

**Interfaces:**
- Produces:
  - `AppRecord.scaffolded: bool`（**无 serde default**）
  - `pub enum CreateMode { Shell, Scaffolded }`（`local-apps/src/service.rs`，`pub use` 出 crate）
  - `create_app_with_git_and_workflow_model_and_initializer(..., mode: CreateMode, ...)` —— 四个包装构造函数一律传 `CreateMode::Scaffolded`

- [ ] **Step 1: 写会红的测试**

加到 `local-apps/src/service.rs` 的测试模块：

```rust
#[tokio::test]
async fn shell_mode_accepts_an_empty_brief_and_records_an_unscaffolded_shell() {
    let h = harness().await;
    let record = h
        .service
        .create_app_with_mode(None, "", None, CreateMode::Shell)
        .await
        .expect("shell creation must accept an empty brief");
    assert!(!record.scaffolded, "a shell is not scaffolded");
    assert_eq!(record.brief, "");
    assert_eq!(record.name, "untitled", "empty brief falls back to the placeholder");
}

#[tokio::test]
async fn scaffolded_mode_still_rejects_an_empty_brief() {
    // The old invariant must NOT be collateral damage of the Shell relaxation.
    let h = harness().await;
    let err = h
        .service
        .create_app_with_mode(None, "   ", None, CreateMode::Scaffolded)
        .await
        .expect_err("Scaffolded mode keeps rejecting an empty brief");
    assert!(
        matches!(&err, AppError::InvalidRequest(m) if m.contains("brief")),
        "got {err:?}"
    );
}

#[tokio::test]
async fn scaffolded_mode_records_a_scaffolded_app() {
    let h = harness().await;
    let record = h
        .service
        .create_app_with_mode(None, "a todo list", None, CreateMode::Scaffolded)
        .await
        .expect("create");
    assert!(record.scaffolded, "the create+scaffold path commits scaffolded=true");
}
```

> `harness()` / `create_app_with_mode` 的确切拼写按该文件既有测试模块的约定写；`create_app_with_mode` 是本 Task 新增的最短包装（`name, brief, conversation_id, mode` → 转调全参版本）。

`serde_compat.rs` 加一条：

```rust
#[test]
fn a_record_without_scaffolded_fails_to_load_instead_of_defaulting_to_a_shell() {
    // §A.1 clean-install: a missing field is an unsupported old store, NOT a
    // shell. Silently defaulting to `false` would let the next LocalAppScaffold
    // WIPE a real app's source (§C.0.1 clears the editable surface).
    let json = r#"{"id":"legacy","name":"Legacy","brief":"b","git_enabled":true,
        "created_at_ms":1,"updated_at_ms":1,"workflow_state":"draft",
        "workspace_rel":"apps/legacy/workspace"}"#;
    let err = serde_json::from_str::<AppRecord>(json)
        .expect_err("a record without `scaffolded` must not load");
    assert!(
        err.to_string().contains("scaffolded"),
        "the error must name the missing field; got {err}"
    );
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cd lingxi-code && cargo test -p local-apps --all-features 2>&1 | tail -30`
Expected: 四条新测试全 FAILED（`CreateMode` / `create_app_with_mode` / `scaffolded` 都不存在，编译错误也算红——但必须确认错误点名的是这些符号，不是别的）。

- [ ] **Step 3: 加字段与模式**

`types.rs` 的 `AppRecord` **末尾**追加：

```rust
    /// 工作区里是否已经落下脚手架。
    ///
    /// 每一条新记录都显式写入；缺字段是无效的旧 store（§A.1），不是空壳判据。
    ///
    /// 三个写入点，缺一不可：
    ///   1. `CreateMode::Shell` 在构造记录时写 `false`；
    ///   2. `CreateMode::Scaffolded`（`LocalAppCreate` 的 create+scaffold 路径）
    ///      在构造记录时写 `true`；
    ///   3. `LocalAppScaffold` 的提交点把 `false` 翻成 `true`。
    ///
    /// ⛔ 不要加 `#[serde(default)]`：缺字段必须加载失败并提示清除开发数据，
    /// 而不是静默变成一个可以被 `LocalAppScaffold` 清空的 shell。
    pub scaffolded: bool,
```

`service.rs` 加：

```rust
/// 创建模式——同时是 `AppRecord.scaffolded` 初值的唯一决定点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateMode {
    /// 「+」按钮建的空壳：brief 可以为空，记录写 `scaffolded: false`。
    Shell,
    /// create + scaffold 一步到位（`LocalAppCreate` 工具那条路）：
    /// brief 必须非空（保留今天的不变量），记录写 `scaffolded: true`。
    Scaffolded,
}
```

`create_app_with_git_and_workflow_model_and_initializer` 加 `mode: CreateMode` 参数，把 `:912` 的空 brief 拒绝改成：

```rust
        let trimmed_brief = brief.trim();
        if mode == CreateMode::Scaffolded && trimmed_brief.is_empty() {
            return Err(AppError::InvalidRequest(
                "app brief must not be empty".into(),
            ));
        }
```

名称派生（`:919`）在空 brief 时回落到占位常量：

```rust
            None => {
                let derived: String = trimmed_brief.chars().take(24).collect();
                if derived.is_empty() {
                    PLACEHOLDER_APP_NAME.to_string()
                } else {
                    derived
                }
            }
```

并加 `pub const PLACEHOLDER_APP_NAME: &str = "untitled";`（**非本地化**，客户端按 `scaffolded == false` 分支，从不渲染它）。

记录构造处写 `scaffolded: mode == CreateMode::Scaffolded`。

四个包装构造函数（`create_app`、`create_app_with_initializer`、`create_app_with_git`、`create_app_with_git_and_workflow_model`）一律传 `CreateMode::Scaffolded` ⇒ **行为逐字节不变**。

- [ ] **Step 4: 补齐所有结构体字面量与 fixtures**

```bash
cd lingxi-code && cargo build --workspace --all-features 2>&1 | grep -n "missing field \`scaffolded\`" | head -50
```

逐个补齐。fixtures 用：

```bash
cd lingxi-code && for f in local-apps/tests/fixtures/v1/apps/*/app.json; do
  python3 -c "
import json,sys
p=sys.argv[1]; d=json.load(open(p))
d['scaffolded']=True
json.dump(d, open(p,'w'), indent=2, ensure_ascii=False); open(p,'a').write('\n')
" "$f"; done
```

- [ ] **Step 5: 跑测试确认绿，且两条旧测试没被碰**

```bash
cd lingxi-code && cargo test -p local-apps --all-features --no-fail-fast > /tmp/task2.txt 2>&1
grep -n '^failures:' -A 20 /tmp/task2.txt
grep -n 'create_app_enforces_brief_caps\|create_app_rejects_an_empty_brief' /tmp/task2.txt
git diff --stat -- lingxi-code/local-apps/src/service.rs
```

Expected: 无 `failures:`；那两条旧测试出现在输出里且 `... ok`；**它们的代码一个字都没改**（`git diff` 里看不到它们）。

- [ ] **Step 6: 提交**

```bash
git add lingxi-code/local-apps/
git commit -m "Split app creation into Shell and Scaffolded modes with a persistent scaffolded flag"
```

---

## Task 3: `request_id` 穿到领域层

**Files:**
- Modify: `lingxi-code/local-apps/src/events.rs`（`AppEvent::AppCreated`，`:33`）
- Modify: `lingxi-code/local-apps/src/service.rs`（全参创建函数 + 四个包装）
- Test: `lingxi-code/local-apps/src/service.rs` 测试模块

**Interfaces:**
- Consumes: Task 2 的 `CreateMode`
- Produces: `AppEvent::AppCreated { record: AppRecord, request_id: Option<String> }`；全参创建函数新增 `request_id: Option<String>` 参数
- ⚠️ **Task 2 建的测试包装 `create_app_with_mode` 在本 Task 追加第五个参数 `request_id: Option<String>`**，Task 2 那三条测试的调用处各补一个 `None`。不要新造第二个包装——两个名字并存就是下一次漂移。

**为什么不能只改 `host.rs`:** `AppCreated` **不是** `host.rs` 发的——`AppService` 在自己的创建完成任务里发出（`service.rs:1053` 附近，早于创建函数返回），再经 `lower_app_event`（`local_apps_bridge.rs:211`，纯映射）到达 wire。`handle_create_app` 根本看不到那个事件，也没有装饰它的钩子。⛔ 只改 DTO + `host.rs`，成功路径上这个字段**永远是 `None`** ⇒ 客户端「只认领匹配的事件」永远不匹配 ⇒ 每次创建都超时，而 Task 13/14 又删掉了旧的 brief 认领 ⇒ **「+」按钮再也进不去新应用的会话**。

- [ ] **Step 1: 写会红的测试**

```rust
#[tokio::test]
async fn app_created_carries_the_request_id_the_caller_passed() {
    let h = harness().await;
    let _ = h
        .service
        .create_app_with_mode(None, "", None, CreateMode::Shell, Some("req-7".into()))
        .await
        .expect("create");
    let created = h
        .take_events()
        .await
        .into_iter()
        .find_map(|e| match e {
            AppEvent::AppCreated { request_id, .. } => Some(request_id),
            _ => None,
        })
        .expect("AppCreated must be emitted");
    assert_eq!(
        created.as_deref(),
        Some("req-7"),
        "the correlation key must survive the service-layer emission, \
         not just the host handler"
    );
}

#[tokio::test]
async fn the_default_wrappers_emit_app_created_without_a_request_id() {
    let h = harness().await;
    let _ = h.service.create_app(None, "a todo list", None).await.expect("create");
    let created = h
        .take_events()
        .await
        .into_iter()
        .find_map(|e| match e {
            AppEvent::AppCreated { request_id, .. } => Some(request_id),
            _ => None,
        })
        .expect("AppCreated must be emitted");
    assert_eq!(created, None, "LocalAppCreate's path has no request to correlate");
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cd lingxi-code && cargo test -p local-apps --all-features request_id 2>&1 | tail -20`
Expected: 编译失败并点名 `request_id` 字段/参数不存在。

- [ ] **Step 3: 实现**

`events.rs`：

```rust
    AppCreated {
        /// The freshly committed record.
        record: AppRecord,
        /// Correlation key from the originating `CreateApp`, echoed verbatim.
        /// `None` for creations that had no request to correlate (the
        /// `LocalAppCreate` tool path).
        request_id: Option<String>,
    },
```

全参创建函数追加 `request_id: Option<String>` 参数，move 进 completion task，在发 `AppCreated` 时原样带上。四个包装传 `None`。

- [ ] **Step 4: 跑测试确认绿**

Run: `cd lingxi-code && cargo test -p local-apps --all-features --no-fail-fast 2>&1 | grep -n '^failures:' -A 20`
Expected: 无输出。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/local-apps/
git commit -m "Carry a create request_id through the service-layer AppCreated emission"
```

---

## Task 4: 协议 8.0.0

**Files:**
- Modify: `lingxi-code/client-protocol/src/commands.rs`（`CreateApp`，`:364`；删 `ProposeAppIdentity`，`:542`）
- Modify: `lingxi-code/client-protocol/src/events.rs`（删 `AppIdentityProposed`，`:386`；`AppOperationFailed.request_id`，`:320`）
- Modify: `lingxi-code/client-protocol/src/local_apps.rs`（`AppRecordDto.scaffolded`，`:231`；`AppEventDto::AppCreated.request_id`，`:917`）
- Modify: `lingxi-code/client-protocol/src/version.rs`（`:52`）
- Modify: `lingxi-code/client-protocol/tests/{version_test,version_guard_test,snapshot_test}.rs`
- Modify: `lingxi-code/client-protocol/snapshots/{contract_index.json,blessed_major.txt}`
- Delete: `lingxi-code/client-protocol/snapshots/command/propose_app_identity.json`、`snapshots/event/app_identity_proposed.json`
- Modify: `clients/shared/src/protocol.ts`、`clients/shared/test/snapshots.test.ts`

**Interfaces:**
- Consumes: Task 2 的 `scaffolded`、Task 3 的 `request_id`
- Produces:
  - `AppCreateModeDto { Shell, Scaffolded }`
  - `CreateApp { …, mode: AppCreateModeDto, request_id: Option<String> }`（两个字段**追加在最后**）
  - `AppRecordDto.scaffolded: bool`（追加在最后）
  - `AppEventDto::AppCreated { record, request_id: Option<String> }`
  - `ClientEvent::AppOperationFailed { app_id, code, message, request_id: Option<String> }`
  - `CLIENT_PROTOCOL_VERSION = "8.0.0"`，`blessed_major.txt = 8`

🚨 **本 Task 会打破 `local_apps_bridge.rs` 的构建，而最省事的修法正是要防的那个 bug。**
给 `AppEventDto::AppCreated` 加 `request_id` 会让 `local_apps_bridge.rs:218` 的结构体字面量
变成非穷尽 ⇒ 编译错误 ⇒ 必须动它。**写 `request_id: None` 能编过、测试全绿、每台设备从此
在每个 `AppCreated` 上看到 `null`**，「+」永远匹配不到自己创建的应用。
⇒ 本 Task 必须把它接成真正的透传（`AppEvent::AppCreated.request_id` → DTO），并让
`every_app_event_arm_lowers_field_exact` 里那条 `AppCreated` 用例的期望值从
`None` 改成 `Some("req-1")`。⛔ 那条用例的输入端就是 `Some("req-1")`，把期望写成 `None`
不是「让它编过」，是**把 bug 钉成正确行为**。

⚠️ **`version_guard_test.rs` 的 `current_contract_index()` 是手写字符串字面量表。** 删枚举变体后代码**照样编过**，覆盖锚点只是抽样 ⇒ 不删表里那 7 行（`put("ClientEvent::AppIdentityProposed"…)` ≈ `:337-340`、`put("ClientCommand::ProposeAppIdentity"…)` ≈ `:599`），守卫**看不见删除**，8.0.0 不会被强制。同文件另有三处结构体字面量要补 `scaffolded`。行号会漂，实现前重新 grep。

- [ ] **Step 1: 先让守卫见红——删表行，不删代码**

只删 `version_guard_test.rs` 手写表里的那 7 行，其余不动。

Run: `cd lingxi-code && cargo test -p client-protocol --test version_guard_test 2>&1 | tail -30`
Expected: **FAILED**，且失败消息**点名** `ClientCommand::ProposeAppIdentity` / `ClientEvent::AppIdentityProposed`（守卫报告 index 里有它、表里没有）。这一步证明守卫真的能看见这两个符号——⛔ 如果它绿了，说明守卫对这两个符号是瞎的，停下来查明原因，不要继续。

- [ ] **Step 2: 改 wire**

```rust
// commands.rs —— 新枚举
/// 创建模式，与服务层 `local_apps::CreateMode` 是同一个概念、同一套名字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCreateModeDto {
    /// 只建空壳：记录写 `scaffolded: false`，不落脚手架。
    /// 此模式下 `surface` 必须为 `None`——形态是脚手架时才定的。
    Shell,
    /// 创建并脚手架（今天的行为）。
    Scaffolded,
}
```

`CreateApp` **末尾**追加：

```rust
        /// Shell = 只建空壳；Scaffolded = 创建并脚手架。
        mode: AppCreateModeDto,
        /// 客户端生成的关联键，成功与失败事件都原样回传。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
```

`AppRecordDto` 末尾追加 `pub scaffolded: bool,`（**无 default**）。
`AppEventDto::AppCreated` 追加 `request_id: Option<String>`。
`ClientEvent::AppOperationFailed` 追加 `request_id: Option<String>`。
删 `ClientCommand::ProposeAppIdentity` 与 `ClientEvent::AppIdentityProposed` 及其文档。
`version.rs` → `"8.0.0"`，并把 `:38` 那段解释 7.0.0 的注释改写成 8.0.0 的理由。

- [ ] **Step 3: 重新生成 snapshot 并 bless**

```bash
cd lingxi-code
rm client-protocol/snapshots/command/propose_app_identity.json \
   client-protocol/snapshots/event/app_identity_proposed.json
# 按仓库既有方式重新生成 contract_index.json（查 snapshot_test.rs 头部的说明；
# 通常是 UPDATE_SNAPSHOTS=1 cargo test -p client-protocol）
echo 8 > client-protocol/snapshots/blessed_major.txt
```

同步 `clients/shared/src/protocol.ts`：版本常量、`AppCreateModeDto`、`CreateApp` 的两个字段、`AppRecordDto.scaffolded`、两个 `request_id`，删提议那两个 union 成员（⚠️ `app_created` / `app_record_changed` **不动**）。⛔ 该文件是纯 interface，**没有任何 runtime guard / zod 校验**，不要去找。

- [ ] **Step 4: 跑协议全量**

```bash
cd lingxi-code && cargo test -p client-protocol --all-features --no-fail-fast > /tmp/task4.txt 2>&1
grep -n '^failures:' -A 20 /tmp/task4.txt; grep -n '8\.0\.0' /tmp/task4.txt | head
cd ../clients/shared && npm test 2>&1 | tail -20
```

Expected: 无 `failures:`；`version_test` 断言 `8.0.0` 通过。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/client-protocol clients/shared
git commit -m "Bless client protocol 8.0.0: conversational create mode, request_id, scaffolded"
```

---

## Task 5: 空壳创建落到宿主 —— `CreateApp` 分叉 + 引导版 `LINGXI.md`

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/host.rs`（`CreateApp` handler）
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_bridge.rs`（`lower_record`、`lower_app_event`）
- Test: `lingxi-code/apps/engine-mobile/src/host.rs` 测试模块

**Interfaces:**
- Consumes: Task 2 `CreateMode`、Task 3 `request_id`、Task 4 `AppCreateModeDto`
- Produces: `CreateApp{mode: Shell}` 落一个 `scaffolded == false` 的记录，工作区只有 `.lingxi/` 与引导版 `LINGXI.md`

⚠️ **wire 事实**：`commands.rs:366` 的 `name: String` 是**必填非可选**，客户端发的是空串**不是 nil**；`origin: AppCreateOriginDto` 同样必填（按钮路径发 `.library`）。宿主需要一条「wire 空串 → 服务层 `None`」的分支。

- [ ] **Step 1: 写会红的测试**

```rust
#[tokio::test]
async fn create_app_in_shell_mode_leaves_an_empty_workspace_with_the_guided_contract() {
    let h = mobile_host().await;
    let record = h.create_shell_app(Some("req-1")).await.expect("shell create");
    assert!(!record.scaffolded);

    let workspace = h.workspace_of(&record.id);
    assert!(workspace.join(".lingxi").is_dir());
    assert!(workspace.join("LINGXI.md").is_file());
    // ⚠️ 断言「没有应用源码」，不要断言目录列表逐项相等：`git_enabled` 默认为真，
    // `layout.initialize()` 可能在工作区里建 `.git`，逐项相等会因为一个与被测性质
    // 无关的原因而红。下面这组仍然能在脚手架泄漏时失败——那才是这条测试的用途。
    for leaked in ["app", "src", "package.json", "vite.config.mjs", "index.html"] {
        assert!(
            !workspace.join(leaked).exists(),
            "a shell workspace must hold no application source; found {leaked}"
        );
    }

    let contract = std::fs::read_to_string(workspace.join("LINGXI.md")).expect("guided contract");
    assert!(contract.contains("LocalAppScaffold"), "the contract must name the one useful tool");
    assert!(
        contract.contains("会在脚手架落地时被删除"),
        "the contract must warn that pre-confirmation source is wiped"
    );
}

#[tokio::test]
async fn create_app_in_shell_mode_rejects_a_surface() {
    // 形态只在脚手架时定（§B.1）。
    let h = mobile_host().await;
    let err = h.create_shell_app_with_surface(Some("dom")).await.expect_err("must reject");
    assert!(format!("{err}").contains("surface"), "got {err}");
}

#[tokio::test]
async fn a_shell_record_lowers_with_scaffolded_false() {
    let h = mobile_host().await;
    let record = h.create_shell_app(None).await.expect("shell create");
    let dto = crate::local_apps_bridge::lower_record(&record);
    assert!(!dto.scaffolded, "lower_record is a pure mapping — no extra IO");
}

// 🚨 必须有这一条。`lower_record` 不是 `lower_app_event`——上面那条测的是记录映射，
// 挡不住事件上的 request_id 被丢掉。
#[test]
fn lower_app_event_passes_request_id_through_on_app_created() {
    let event = AppEvent::AppCreated {
        record: sample_record(),
        request_id: Some("req-1".into()),
    };
    let lowered = crate::local_apps_bridge::lower_app_event(event).expect("lowered");
    match lowered {
        ClientEvent::AppEvent { event: AppEventDto::AppCreated { request_id, .. }, .. } => {
            assert_eq!(
                request_id.as_deref(),
                Some("req-1"),
                "a dropped correlation key here means the + button never opens the new app"
            );
        }
        other => panic!("expected AppCreated, got {other:?}"),
    }
}
```

⚠️ **本 Task 之前，`lower_app_event` 里是 `request_id: _`**（Task 3 为了保持工作区可编译留下的
刻意存根，DTO 那时还没有这个字段）。它编译干净、没有任何测试覆盖，是典型的
「命名了、算出来了、但从没接线」。Task 4 已要求把它接通；本 Task 用上面这条测试**证明**它接通了。
⛔ 不要相信 `every_app_event_arm_lowers_field_exact` 的注释说它覆盖了每个变体——
那句话在 Task 3 之前是假的（六个变体只列了五个，缺的正是 `AppCreated`）。

- [ ] **Step 2: 跑测试确认红**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features shell_mode 2>&1 | tail -30`

- [ ] **Step 3: 实现**

`host.rs` 的 `CreateApp` handler：

```rust
            let name = if name.trim().is_empty() { None } else { Some(name.as_str()) };
            match mode {
                AppCreateModeDto::Shell => {
                    if surface.is_some() {
                        return Err(/* InvalidRequest: a shell has no surface yet；
                                      形态在 LocalAppScaffold 时才定 */);
                    }
                    service
                        .create_app_with_git_and_workflow_model_and_initializer(
                            name, &brief, conversation_id, git_enabled,
                            workflow_model.as_deref(),
                            CreateMode::Shell, request_id,
                            move |record| async move { write_guided_contract(&record) },
                        )
                        .await
                }
                AppCreateModeDto::Scaffolded => { /* 今天的路径，逐字节不变 */ }
            }
```

引导版合约写在 initializer 里（此时 `layout.initialize()` 已跑过，工作区目录存在）。内容要点：

```text
# Local App（新建，尚未定形态）

这个应用刚刚创建，**还没有形态**，工作区是空的。

你现在的任务是引导用户，不是写代码。**你现在写下的任何源文件都会在脚手架落地时被删除**，
写了也是白写。

本地应用工具里，此刻只有 `LocalAppScaffold` 对你有意义；构建、安装依赖、运行时、
界面检查那一类都会拒绝你并告诉你原因。问需求用 `AskUserQuestion`。

步骤：
1. 先问用户想做什么。
2. 据回答推断意图，用 `AskUserQuestion` 把提议的**名称**与**形态**交给用户确认或修改：
   - `dom` —— 多屏界面（表单、列表、页面导航）
   - `canvas` —— 单一绘制面（游戏、3D、可视化）
3. 用户确认后调 `LocalAppScaffold`。
4. 重读本文件，按新合约继续。

形态一旦落地不可更改，所以必须让用户确认，不要自作主张。
```

`local_apps_bridge.rs`：`lower_record` 补 `scaffolded: record.scaffolded`（纯映射，**无额外 IO**）；`lower_app_event` 透传 `request_id`。

🚨 **Task 4 交接过来的两个「字段存在但恒为 None」的存根，本 Task 必须各自接线并各配一条测试。**
它们和 §D.1 里那个已经踩过的坑是同一个形状：编译干净、测试全绿、线上恒 `null`。

1. **`ClientEvent::AppOperationFailed.request_id` 目前没有任何生产者。** `emit_failure` 一律发
   `None`，Task 4 只加了字段。⇒ 创建失败时客户端**认领不到自己的失败**，只能等 30 秒超时，
   用户看到的是「创建结果未知，请在应用库确认」而不是真正的错误原因。
   本 Task 要让创建失败路径把 `request_id` 带上，并加测试：
   ```rust
   #[tokio::test]
   async fn a_failed_shell_create_reports_the_request_id_the_client_sent() {
       let h = mobile_host().await;
       h.fail_next_create();
       h.create_shell_app(Some("req-9")).await.expect_err("must fail");
       let failed = h.take_client_events().await.into_iter().find_map(|e| match e {
           ClientEvent::AppOperationFailed { request_id, .. } => Some(request_id),
           _ => None,
       }).expect("AppOperationFailed must be emitted");
       assert_eq!(failed.as_deref(), Some("req-9"), "否则客户端只能靠超时兜底");
   }
   ```
2. **`CreateApp.mode` 被接收但被忽略**（Task 4 写成 `mode: _mode` 加了指示性注释）。本 Task 的
   分叉就是消费它的地方。测试必须证明**两个分支各自生效**：`Shell` 落 `scaffolded == false`
   且工作区无源码，`Scaffolded` 落 `scaffolded == true` 且脚手架已落地。⛔ 只测 `Shell`
   等于没证明 `mode` 真的被读了——`_mode` 的行为和「永远走 Shell」在只测 Shell 时无法区分。

- [ ] **Step 4: 跑测试确认绿**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features --no-fail-fast > /tmp/task5.txt 2>&1; grep -n '^failures:' -A 20 /tmp/task5.txt`

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile
git commit -m "Create a shell app with the guided workspace contract on the Shell mode"
```

---

## Task 6: 首次脚手架清空可编辑面

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_build.rs`（`scaffold_workspace_initialized`，`:409`）
- Test: 同文件测试模块

**Interfaces:**
- Produces: `scaffold_workspace_initialized(layout, target, first_scaffold: bool)`

**这是本计划最重要的一条保证。** 判据只有一条：**确认之前写的代码，一个字节都不能进入正式应用。**

⚠️ **逐路径 `overwrite=true` 不够。** 覆盖只作用于种子自己那九个路径。Vite 8.2.1 的 `DEFAULT_EXTENSIONS` 把 `.js` 排在 `.jsx` **之前**，模板没有 `resolve.extensions` 覆盖，16 处 import 全是无扩展名的，`copy_workspace_tree` 会把多余文件一并拷进构建根 ⇒ 预先写下的 `app/app.js` 在解析时**赢过**种子的 `app/app.jsx`，种子沦为死代码。**清空可以。**

⚠️ **不要把它当「继承来的既有隐患」放过。** 今天的创建在**创建事务内部**就完成脚手架，根本不存在「工作区已建、尚未脚手架」的可写窗口。那个窗口是本方案开的，所以这个破口也是本方案自己的。

- [ ] **Step 1: 写会红的测试**

```rust
#[test]
fn a_first_scaffold_wipes_the_editable_surface_before_seeding() {
    let layout = shell_layout();
    let workspace = layout.root().join(layout.workspace_rel());
    // 三个预写文件：两个同路径异扩展的影子，一个种子里根本没有的野文件。
    write(&workspace, "app/app.js", b"// shadow that would WIN Vite resolution");
    write(&workspace, "lib/lingxi-provider.js", b"// shadow of a host-managed file");
    write(&workspace, "app/screens/rogue.jsx", b"// not in the seed at all");
    write(&workspace, "node_modules/.keep", b"");

    scaffold_workspace_initialized(&layout, LocalAppBuildTarget::Dom, true).expect("scaffold");

    assert!(!workspace.join("app/app.js").exists(), "extension shadow must be gone");
    assert!(!workspace.join("lib/lingxi-provider.js").exists(), "host-managed shadow must be gone");
    assert!(!workspace.join("app/screens/rogue.jsx").exists(), "rogue source must be gone");
    assert!(workspace.join(".lingxi").is_dir(), ".lingxi is preserved");
    assert!(workspace.join("node_modules/.keep").exists(), "node_modules is preserved");
    assert!(workspace.join("app/app.jsx").is_file(), "the seed landed");
}

#[test]
fn a_first_scaffold_overwrites_a_pre_written_seed_path() {
    let layout = shell_layout();
    let workspace = layout.root().join(layout.workspace_rel());
    write(&workspace, "app/screens/home-screen.jsx", b"// squatted by the agent");

    scaffold_workspace_initialized(&layout, LocalAppBuildTarget::Dom, true).expect("scaffold");

    let landed = std::fs::read(workspace.join("app/screens/home-screen.jsx")).expect("read");
    assert_ne!(landed, b"// squatted by the agent".to_vec(), "the seed must win");
}

#[test]
fn a_repin_never_wipes_a_formed_app() {
    // restore_host_managed_files 的路径：first_scaffold = false。
    let layout = formed_layout();
    let workspace = layout.root().join(layout.workspace_rel());
    write(&workspace, "app/screens/user-written.jsx", b"// the user's own code");

    scaffold_workspace_initialized(&layout, LocalAppBuildTarget::Dom, false).expect("repin");

    assert!(
        workspace.join("app/screens/user-written.jsx").exists(),
        "a formed app's source must survive a repin"
    );
}
```

- [ ] **Step 2: 跑测试确认前两条红**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features scaffold 2>&1 | tail -40`
Expected: `a_first_scaffold_wipes_the_editable_surface_before_seeding` 与 `a_first_scaffold_overwrites_a_pre_written_seed_path` 都 FAILED，且失败点名的是**残留的具体文件**（`app/app.js` 还在 / 内容仍是 squatted）——这证明测的正是 `overwrite=false` 那个洞。

- [ ] **Step 3: 实现**

```rust
pub(crate) fn scaffold_workspace_initialized(
    layout: &AppLayout,
    target: LocalAppBuildTarget,
    first_scaffold: bool,
) -> Result<(), AppError> {
    let workspace = layout.root().join(layout.workspace_rel());
    if first_scaffold {
        // §C.0.1: a shell has no legitimate application source by definition,
        // so wiping is safe — and it is what makes the retry in §C.1 safe too:
        // every attempt starts from clean ground. Per-path overwrite is NOT
        // enough: Vite's DEFAULT_EXTENSIONS resolves `.js` BEFORE `.jsx`, so a
        // pre-written `app/app.js` would beat the seeded `app/app.jsx` and the
        // seed would become dead code.
        wipe_editable_surface(&workspace)?; // keeps .lingxi/, LINGXI.md, node_modules/
    }
    for (relative, bytes) in VITE_LOCKED_FILES {
        write_file(&workspace, relative, bytes, true)?;
    }
    for (relative, bytes) in source_files(target) {
        write_file(&workspace, relative, bytes, first_scaffold)?;
    }
    Ok(())
}
```

`wipe_editable_surface` 遍历工作区顶层，保留 `.lingxi`、`LINGXI.md`、`node_modules`，其余 `remove_dir_all` / `remove_file`。

其余调用点（`restore_host_managed_files` 那条）传 `false`。

- [ ] **Step 4: 跑测试确认三条都绿**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features --no-fail-fast > /tmp/task6.txt 2>&1; grep -n '^failures:' -A 20 /tmp/task6.txt`

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_build.rs
git commit -m "Wipe the editable surface on a first scaffold instead of per-path overwrite"
```

---

## Task 7: 注册 `LocalAppScaffold`（表、计数、权限）

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_tools.rs`（`LOCAL_APP_TOOLS`，`:53`；`requires_bound_session_for_auto_allow`，`:187`）
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs`（schema；`:2087` 的精确名字列表）
- Modify: `lingxi-code/permission/src/defaults_per_tool.rs`（新条目；`:143` 的 `debug_assert_eq!(m.len(), 66)`）

**Interfaces:**
- Produces: builtin `LocalAppScaffold`，provider operation `scaffold`，入参 `{app_id, name, brief, surface, workflow_model?}`，`additionalProperties: false`

⚠️ **四处硬同步点，漏一处就撞红：**
1. `LOCAL_APP_TOOLS` 表（今天 **22** 条 → **23**）
2. `local_apps_mcp.rs:2087` `catalog_is_fixed_and_exposes_no_arbitrary_execution_surface` —— **精确名字列表的 `assert_eq!`**
3. `permission/src/defaults_per_tool.rs:143` `debug_assert_eq!(m.len(), 66, …)` → **67**，连同上一行注释 `// 44 oracle-parity tools + 22 mobile local-app builtins` 与模块文档里的同类计数
4. `requires_bound_session_for_auto_allow`（`:187`）—— builtin 对**每个**会话注册，全局会话里也看得见 `LocalAppScaffold`；不加进这张表，全局会话不弹框就能调

- [ ] **Step 1: 写会红的测试**

```rust
#[test]
fn scaffold_is_registered_and_bound_to_an_app_session() {
    assert!(
        LOCAL_APP_TOOLS.iter().any(|(name, op, _)| *name == "LocalAppScaffold" && *op == "scaffold"),
        "the builtin must map to the `scaffold` provider operation"
    );
    let tool = local_app_tool("LocalAppScaffold");
    assert!(
        tool.requires_bound_session_for_auto_allow(),
        "a global session must still be asked; only an app-bound session is auto-allowed"
    );
}

#[test]
fn scaffold_defaults_to_allow() {
    // ⚠️ 按 `permission/src/defaults_per_tool.rs` 里既有测试的写法取默认值——
    // 该模块的公开访问器名字实现前先 grep 确认，不要照抄一个猜的名字。
    assert_eq!(lookup_default("LocalAppScaffold"), ToolPermissionDefault::AllowByDefault);
}

#[test]
fn the_scaffold_schema_avoids_the_forbidden_substrings() {
    let schema = serde_json::to_string(&local_app_tool("LocalAppScaffold").input_schema()).unwrap();
    assert!(!schema.to_lowercase().contains("template"), "命名禁区");
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cd lingxi-code && cargo test -p engine-mobile -p permission --all-features scaffold 2>&1 | tail -30`

- [ ] **Step 3: 四处同步改到位**

- `LOCAL_APP_TOOLS` 加 `("LocalAppScaffold", "scaffold", true)`
- `requires_bound_session_for_auto_allow` 的 `matches!` 加 `| "LocalAppScaffold"`
- `local_apps_mcp.rs:2087` 的名字列表加 `"scaffold"`（⚠️ 是 **operation** 名，按该断言既有的元素形式写）
- `defaults_per_tool.rs` 加条目 + `66` → `67` + 两处计数注释

schema（避开 `template`）：

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["app_id", "name", "brief", "surface"],
  "properties": {
    "app_id": {"type": "string"},
    "name":   {"type": "string", "description": "The display name the user confirmed."},
    "brief":  {"type": "string", "description": "One line describing what the app does, as the user confirmed it."},
    "surface":{"type": "string", "enum": ["dom", "canvas"],
               "description": "dom = multi-screen interface; canvas = a single drawing surface (games, 3D, visualisation). Immutable once committed."},
    "workflow_model": {"type": "string"}
  }
}
```

- [ ] **Step 4: 跑测试确认绿（debug 构建才会触发那条 `debug_assert`）**

Run: `cd lingxi-code && cargo test -p engine-mobile -p permission --all-features --no-fail-fast > /tmp/task7.txt 2>&1; grep -n '^failures:' -A 20 /tmp/task7.txt`
Expected: 无 `failures:`；⚠️ 特别确认没有 `tool defaults table must list all 66 tools` 的 panic。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile lingxi-code/permission
git commit -m "Register the LocalAppScaffold builtin across all four sync points"
```

---

## Task 8: `LocalAppScaffold` 实现

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_host.rs`（实现）
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs`（dispatch）
- Modify: `lingxi-code/local-apps/src/service.rs`（§C.1.5 的四字段提交写）
- Test: `lingxi-code/apps/engine-mobile/src/local_apps_host.rs` 测试模块

**Interfaces:**
- Consumes: Task 6 的 `scaffold_workspace_initialized(.., first_scaffold)`；Task 2 的 `scaffolded`
- Produces: 服务方法 `commit_scaffold(app_id, name, brief, workflow_model) -> Result<AppRecord>` —— 在**一个** `with_app` 闭包里一次性写 `name`/`brief`/`workflow_model`/`scaffolded = true`（set-once CAS，照 `set_init_session` 的范式）

### 步骤顺序是规范的一部分

1. **预留：进程内，不落盘。** 宿主侧 `Mutex<HashSet<AppId>>`，RAII guard，panic 路径也要还。已在集合中 ⇒ 立刻拒绝。
   ⚠️ **不要照 `set_init_session` 做预留**——那个范式往**持久字段**做 set-once 写入，预留一旦落盘、进程被杀就再也清不掉，草稿永久变砖，直接和「失败可重试」互斥。`with_app` 也守不住：它的守卫只活到自身完成任务结束，第 2-4 步全在锁外跑。引擎在设备上是**单进程**，进程内预留就够。
2. **校验**：`brief.trim()` 非空、`name` 非空、长度在 `MAX_NAME_BYTES` / `MAX_BRIEF_BYTES` 内、`surface` 可解析。
3. **落地**（全部完成后才提交）。**记录在这一步里只是暂存，不落盘**：
   1. 取 `storage::lock_app_build(root, app_id)`，**持到第 4 步结束**
   2. 构造**未持久化**的 `proposed_record`（`record.clone()` 套上确认的 `name`/`brief`/`workflow_model`）
   3. manifest：`surface` + `name`
   4. `scaffold_workspace_initialized(.., first_scaffold = true)`
   5. 正式版 `LINGXI.md`（覆盖引导版），**用 `proposed_record` 渲染**
4. **提交点**：**一个** `with_app` 闭包内一次性持久化四个字段。任何一步失败 ⇒ 四个字段一个都没落盘、`scaffolded` 仍是 `false`，预留随 guard 析构释放，构建锁随之释放，安全重试。

⚠️ **必须全程持 `storage::lock_app_build`。** 它的文档原话是「Callers must hold this lock for the **complete** operation that mutates or removes an app's workspace/build tree」，**物理删除走同一把锁**（`storage.rs:899` 经 `lock_app_build_if_present`）。进程内预留只排斥另一个 `LocalAppScaffold`，**挡不住 `DeleteApp`**：并发删除会把应用目录 rename 进 trash，而脚手架还在往里写 ⇒ 写进一个已被摘除的目录，留下永不回收的孤儿。

⚠️ **`LINGXI.md` 渲染顺序。** 合约文本是 `format!("# Local App: {name} ({id})\n\nBrief: {brief}…", name = record.name, brief = record.brief)`。必须传 `proposed_record`，否则写出的是 `# Local App: untitled` + 空 Brief——而 `LINGXI.md` **只写这一次**（`restore_host_managed_files` 不含它，二次 `LocalAppScaffold` 被拒），且按 §0 它是**唯一**每轮到达模型的通道 ⇒ 整个对话问出来的需求会在唯一的长期载体里永久丢失。

⚠️ **两个标志的写入顺序相反，都是有意的，不要「统一」。** `scaffold_app_value` 现有注释**刻意**先盖 `manifest.surface` 再写文件（「Stamping first means a crash between the two steps leaves an app that can be scaffolded again, not one that cannot」）；`record.scaffolded` 是**外层**提交点，必须最后写。一个守「文件层可重入」，一个守「记录层已完成」。

⚠️ **不需要在成功后排队 dependency install。** `queue_dependency_install` 位于 `ensure_dependency_install`，由**构建路径**惰性触发，创建路径不碰。

- [ ] **Step 1: 写会红的测试**

```rust
#[tokio::test]
async fn scaffold_commits_all_four_fields_and_writes_the_formal_contract() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    let record = h.scaffold(&shell.id, "打飞机", "一个竖版射击小游戏", "canvas").await.expect("scaffold");

    assert!(record.scaffolded);
    assert_eq!(record.name, "打飞机");
    assert_eq!(record.brief, "一个竖版射击小游戏");

    let contract = std::fs::read_to_string(h.workspace_of(&shell.id).join("LINGXI.md")).unwrap();
    assert!(contract.contains("# Local App: 打飞机"), "must render the CONFIRMED name, not `untitled`");
    assert!(contract.contains("Brief: 一个竖版射击小游戏"), "must render the CONFIRMED brief");
}

#[tokio::test]
async fn a_failed_landing_persists_none_of_the_four_fields() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    h.fail_next_landing();                       // 注入落地阶段失败
    let _ = h.scaffold(&shell.id, "打飞机", "一个竖版射击小游戏", "canvas").await.expect_err("must fail");

    let after = h.record(&shell.id).await;
    assert!(!after.scaffolded, "the commit point never ran");
    assert_eq!(after.name, "untitled", "the名字 must NOT be half-committed");
    assert_eq!(after.brief, "", "库里不能出现「名字对了但还是空壳」的记录");
}

#[tokio::test]
async fn the_reservation_is_released_on_the_failure_path_so_a_retry_can_land() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    h.fail_next_landing();
    let _ = h.scaffold(&shell.id, "打飞机", "b", "canvas").await.expect_err("first fails");
    // 若预留落了盘，这条必红。
    let record = h.scaffold(&shell.id, "打飞机", "b", "canvas").await.expect("retry must land");
    assert!(record.scaffolded);
}

#[tokio::test]
async fn two_concurrent_scaffolds_reject_the_second_at_the_in_process_reservation() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    let (a, b) = tokio::join!(
        h.scaffold(&shell.id, "A", "b", "dom"),
        h.scaffold(&shell.id, "B", "b", "dom"),
    );
    assert_eq!(a.is_ok() ^ b.is_ok(), true, "exactly one wins");
}

#[tokio::test]
async fn a_concurrent_delete_cannot_orphan_a_scaffold_in_flight() {
    // 钉住 lock_app_build：删除在脚手架持锁期间发起。
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    let scaffold = h.scaffold_slowly(&shell.id, "A", "b", "dom");
    let delete = h.delete_app(&shell.id);
    let (s, d) = tokio::join!(scaffold, delete);
    // 两个都完成后：要么应用还在且成形，要么应用没了且**没有残留目录**。
    h.assert_no_orphan_workspace(&shell.id, s.is_ok(), d.is_ok());
}

#[tokio::test]
async fn scaffolding_a_formed_app_is_rejected() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    h.scaffold(&shell.id, "A", "b", "dom").await.expect("first");
    let err = h.scaffold(&shell.id, "B", "b", "dom").await.expect_err("second must be rejected");
    assert!(format!("{err}").contains("already"), "got {err}");
}

#[tokio::test]
async fn scaffold_rejects_an_empty_brief_and_an_unknown_surface() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    assert!(h.scaffold(&shell.id, "A", "   ", "dom").await.is_err());
    assert!(h.scaffold(&shell.id, "A", "b", "webgl").await.is_err());
}

#[tokio::test]
async fn the_manifest_name_may_only_be_written_before_any_database_exists() {
    // §C.1.4: AppManifest::hash() serialises the WHOLE struct INCLUDING `name`,
    // and AppDataStore::ensure_manifest compares it against the SQLite
    // `_lingxi_schema.manifest_hash`. Writing `name` after a store exists breaks
    // every data read and write with `database manifest mismatch`.
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    h.assert_no_app_database(&shell.id);
    h.scaffold(&shell.id, "A", "b", "dom").await.expect("first");
    h.open_app_database(&shell.id).await;
    let err = h.rewrite_manifest_name(&shell.id, "B").await.expect_err("must refuse");
    assert!(format!("{err}").contains("database"), "got {err}");
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features scaffold 2>&1 | tail -40`

- [ ] **Step 3: 实现**（按上面「步骤顺序是规范的一部分」逐条落）

- [ ] **Step 4: 跑测试确认绿**

Run: `cd lingxi-code && cargo test -p engine-mobile -p local-apps --all-features --no-fail-fast > /tmp/task8.txt 2>&1; grep -n '^failures:' -A 20 /tmp/task8.txt`

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile lingxi-code/local-apps
git commit -m "Implement LocalAppScaffold as a single locked transaction with a last-write commit point"
```

---

## Task 9: pin 会话标题的条件重命名 + boot 对账

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_host.rs`（`LocalAppScaffold` post-commit）
- Modify: `lingxi-code/apps/engine-mobile/src/host.rs`（boot backfill sweep，`:9354` 附近）
- Test: 同两文件测试模块

**Interfaces:**
- Consumes: Task 8 的提交点
- Produces: 判据函数 `latest_custom_title_is_mobile_placeholder(session_id) -> bool`，**即时重命名与 boot 对账共用它**

**问题:** `host.rs:9145` / `:9172` 用 `record.name` 作 pin 住的 init 会话标题，空壳期写进去的是 `"untitled"`，会落进持久化会话目录。

⚠️ **即时重命名和 boot 对账必须共用同一判据，不能只在 sweep 里保护用户标题。** 「标题不等于 `record.name` 就改」会抹掉用户自己改的标题。`/rename`（`orchestrator/src/handle_impl.rs:632` 的 `append_custom_title`）、hook 的 `sessionTitle` 和 mobile 的初始占位**写的是同一条 `custom-title` 通道**，光看标题分不出来。唯一的分辨依据是 `append_mobile_empty_session`（`session/src/jsonl/writer.rs:516` 附近）多带的一个字段：

```json
{"type":"custom-title","customTitle":"…","sessionId":"…","mobileEmptySession":1}
```

统一条件：**该会话最新生效的 `custom-title` 记录仍然带 `mobileEmptySession: 1`**（即用户从未改过名）**且** `record.scaffolded == true` **且**标题与 `record.name` 不符。一旦后面出现过普通 `custom-title`（没有该标记），**尊重用户，不动**。

⚠️ **「可重试」必须有真正的触发器，否则只是措辞。** 现有的 boot backfill sweep 遍历每条记录、自愈目录漂移、补缺失的 pin，但**不碰已存在会话的标题**——所以一次失败的重命名今天永远不会被修好。

- [ ] **Step 1: 写会红的测试（两个正例 + 两个反例）**

```rust
#[tokio::test]
async fn scaffold_renames_the_pinned_session_when_the_user_never_renamed_it() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    h.scaffold(&shell.id, "打飞机", "b", "canvas").await.expect("scaffold");
    assert_eq!(h.session_title(&shell.init_session_id()).await, "打飞机");
}

#[tokio::test]
async fn the_boot_sweep_reconciles_a_title_a_failed_rename_left_behind() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    h.fail_next_rename();
    h.scaffold(&shell.id, "打飞机", "b", "canvas").await.expect("scaffold still succeeds");
    assert_eq!(h.session_title(&shell.init_session_id()).await, "untitled");
    h.run_boot_backfill_sweep().await;
    assert_eq!(
        h.session_title(&shell.init_session_id()).await,
        "打飞机",
        "a failed rename must have a real trigger that fixes it later"
    );
}

#[tokio::test]
async fn an_immediate_rename_never_clobbers_a_user_rename() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    h.user_rename(&shell.init_session_id(), "我的宝贝项目").await;
    h.scaffold(&shell.id, "打飞机", "b", "canvas").await.expect("scaffold");
    assert_eq!(h.session_title(&shell.init_session_id()).await, "我的宝贝项目");
}

#[tokio::test]
async fn the_boot_sweep_never_clobbers_a_user_rename_either() {
    let h = mobile_host().await;
    let shell = h.create_shell_app(None).await.expect("shell");
    h.user_rename(&shell.init_session_id(), "我的宝贝项目").await;
    h.scaffold(&shell.id, "打飞机", "b", "canvas").await.expect("scaffold");
    h.run_boot_backfill_sweep().await;
    assert_eq!(h.session_title(&shell.init_session_id()).await, "我的宝贝项目");
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features rename 2>&1 | tail -40`

- [ ] **Step 3: 实现共用判据 + 两个调用点**

失败只记日志，不回滚（脚手架本身已提交成功）。

- [ ] **Step 4: 跑测试确认四条都绿**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features --no-fail-fast > /tmp/task9.txt 2>&1; grep -n '^failures:' -A 20 /tmp/task9.txt`

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile
git commit -m "Rename the pinned init session on scaffold, reconciled at boot, never over a user rename"
```

---

## Task 10: 工具门

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs`（`call()`，`:1146` 附近）
- Test: 同文件测试模块

**Interfaces:**
- Consumes: Task 2 的 `scaffolded`

**位置:** `call()` 的**最顶端**，紧随 `validate_input`、**在 `parse_dynamic_tool` 分支之前**。

⚠️ **仓库里不存在同时覆盖两条分支的先例。** `runtime_api_compatible()`（`:1173`）在动态分支**内部**，只覆盖应用自有 MCP 命名空间；静态 `match tool` 那条路**根本没有 runtime-api 检查**。门必须自己写在动态分支之上，app_id 来源分两种：`parse_dynamic_tool(tool)` 命中时取绑定值，否则取 `input["app_id"]`。

⚠️ **按 provider operation 判定，不是按 builtin 名。** `call()` 收到的是 operation：`LocalAppTool::call` 调 `transport.call_host_operation(self.operation, input)`，静态 match 的臂是 `"list"` / `"get"` / `"build"` / `"create"`。放行名单写 `LocalAppScaffold` 会**连 scaffold 自己一起拦掉**。

⚠️ 也**不能**按「入参里有没有 `app_id`」判定：`LocalAppTool::call`（`local_apps_tools.rs:367` 附近）在分发**之前**会从会话 cwd 注入 `app_id`。

**放行名单（operation）：`scaffold`、`list`、`get`、`create`。** `create` 放行是有意的：空壳会话里代理若真要另建一个应用，不是本门要防的错误；本门防的是「在空工作区上构建/安装/跑运行时」。

- [ ] **Step 1: 写会红的测试（集合从工具表派生）**

```rust
const SHELL_ALLOWED_OPERATIONS: &[&str] = &["scaffold", "list", "get", "create"];

#[tokio::test]
async fn every_non_allowlisted_operation_is_gated_on_a_shell() {
    let h = mcp_harness_with_shell_app().await;
    let gated: Vec<&str> = LOCAL_APP_TOOLS
        .iter()
        .map(|(_, operation, _)| *operation)
        .filter(|op| !SHELL_ALLOWED_OPERATIONS.contains(op))
        .collect();
    assert!(!gated.is_empty(), "derive the set from the table, never hardcode a count");
    for operation in gated {
        let err = h.call(operation, json!({"app_id": h.app_id()})).await
            .expect_err("a shell must reject {operation}");
        assert!(
            format!("{err}").contains("LocalAppScaffold"),
            "the refusal must point at the way out; operation={operation}, err={err}"
        );
    }
}

#[tokio::test]
async fn the_allowlisted_operations_pass_the_gate_on_a_shell() {
    let h = mcp_harness_with_shell_app().await;
    for operation in SHELL_ALLOWED_OPERATIONS {
        let outcome = h.call(operation, json!({"app_id": h.app_id()})).await;
        assert!(
            !h.was_gated(&outcome),
            "{operation} must reach its handler (it may still fail for its own reasons)"
        );
    }
}

#[tokio::test]
async fn scaffold_itself_is_not_gated_because_the_gate_keys_on_the_operation() {
    // 放行名单若误写 builtin 名 `LocalAppScaffold`，这条会红。
    let h = mcp_harness_with_shell_app().await;
    let outcome = h.call("scaffold", json!({"app_id": h.app_id(), "name": "A", "brief": "b", "surface": "dom"})).await;
    assert!(outcome.is_ok(), "the way out must not be gated: {outcome:?}");
}

#[tokio::test]
async fn the_gate_covers_the_dynamic_dispatch_path_too() {
    // 只测静态 match 等于没测到要害。
    let h = mcp_harness_with_shell_app().await;
    let err = h.call_dynamic(&format!("{}__build", h.app_id()), json!({})).await
        .expect_err("the dynamic path must be gated as well");
    assert!(format!("{err}").contains("LocalAppScaffold"), "got {err}");
}

#[tokio::test]
async fn a_formed_app_passes_the_gate_for_everything() {
    let h = mcp_harness_with_formed_app().await;
    for (_, operation, _) in LOCAL_APP_TOOLS {
        assert!(!h.was_gated(&h.call(operation, json!({"app_id": h.app_id()})).await));
    }
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features gate 2>&1 | tail -40`

- [ ] **Step 3: 实现**

拒绝文案：

> 应用 `<id>` 还没有形态。先与用户确认要做什么，再用 `LocalAppScaffold` 定下名称、简介与形态。

- [ ] **Step 4: 跑测试确认绿**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features --no-fail-fast > /tmp/task10.txt 2>&1; grep -n '^failures:' -A 20 /tmp/task10.txt`

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs
git commit -m "Gate build/install/runtime operations on an unscaffolded app, keyed on the provider operation"
```

---

## Task 11: `detect_build_target` 的组合判定

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_build.rs`（`:197`）
- Test: 同文件测试模块

**Interfaces:**
- Consumes: Task 2 的 `scaffolded`

| 条件 | 结论 |
|---|---|
| 有 `next.config.mjs` | 保持现有的 legacy 拒绝 |
| `scaffolded == false` 且 `surface == None` | 空壳 → 「这个应用还没有形态，先用 `LocalAppScaffold` 定下形态」 |
| `scaffolded == true` 且 `surface == Some(_)` | 对应目标 |
| `false + Some(_)` 或 `true + None` | 本版代码不可产生 → **存储损坏**报错，不要静默 |

⚠️ **判断顺序要以组合为准**，不能先看到 `surface == Some(_)` 就直接返回目标；否则一个撕裂/损坏记录会绕过空壳门。

⚠️ **`detect_build_target(layout: &AppLayout)` 拿不到 `AppRecord`，但不用改签名也不用穿 `AppService`**：`storage.rs` 的 `AppMetadataFile` 就是 `apps/<id>/workspace/.lingxi/app.json`——整个 `AppRecord` 的镜像，只凭 `layout` 就能读（`metadata_rel(app_id)`），且 `repair_torn_commit` 明确它在撕裂提交时**优先于索引**。读它就是读那个持久事实，**不违反**「不要嗅探文件系统」——那条禁的是 `package.json` / `vite.config.mjs` 这类**被脚手架自己重写**的文件（循环论证），不是记录镜像。

⚠️ 非测试调用点共 **3** 处：`local_apps_host.rs:1221`、`local_apps_host.rs:3542`、`local_apps_build.rs:636`。实现时逐个确认并各自加测试。

- [ ] **Step 1: 写会红的测试**

```rust
#[test]
fn an_unscaffolded_shell_says_to_define_the_surface_first() {
    let layout = layout_with(false, None);
    let err = detect_build_target(&layout).expect_err("a shell is not buildable");
    assert!(format!("{err}").contains("LocalAppScaffold"), "got {err}");
}

#[test]
fn scaffolded_without_a_surface_is_storage_corruption_not_a_shell() {
    let layout = layout_with(true, None);
    let err = detect_build_target(&layout).expect_err("this版 code cannot produce it");
    assert!(matches!(err, AppError::StorageCorrupt(_)), "got {err:?}");
}

#[test]
fn unscaffolded_with_a_surface_is_storage_corruption_and_does_not_bypass_the_shell_gate() {
    // 判断顺序的钉子：先看到 Some(surface) 就返回目标的实现会红。
    let layout = layout_with(false, Some(AppSurface::Dom));
    let err = detect_build_target(&layout).expect_err("a torn record must not build");
    assert!(matches!(err, AppError::StorageCorrupt(_)), "got {err:?}");
}

#[test]
fn a_formed_app_resolves_to_its_target() {
    assert_eq!(detect_build_target(&layout_with(true, Some(AppSurface::Dom))).unwrap(), LocalAppBuildTarget::Dom);
    assert_eq!(detect_build_target(&layout_with(true, Some(AppSurface::Canvas))).unwrap(), LocalAppBuildTarget::Canvas);
}

#[test]
fn the_legacy_next_marker_still_wins_over_everything() {
    let layout = layout_with(false, None);
    write(&layout.root().join(layout.workspace_rel()), "next.config.mjs", b"");
    let err = detect_build_target(&layout).expect_err("legacy rejection is preserved");
    assert!(format!("{err}").contains("Next"), "got {err}");
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `cd lingxi-code && cargo test -p engine-mobile --all-features detect_build 2>&1 | tail -40`

- [ ] **Step 3: 实现（读 `AppMetadataFile`，按组合判定）**

- [ ] **Step 4: 跑测试确认绿，并核对三个调用点各有覆盖**

```bash
cd lingxi-code
cargo test -p engine-mobile --all-features --no-fail-fast > /tmp/task11.txt 2>&1
grep -n '^failures:' -A 20 /tmp/task11.txt
grep -rn "detect_build_target" apps/engine-mobile/src/ | grep -v "^.*:.*//" | grep -v "fn detect_build_target"
```

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_build.rs
git commit -m "Resolve the build target from the scaffolded/surface combination"
```

---

## Task 12: 删除提议链路 + 技能与文档

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_host.rs`、`host.rs`
- Modify: `skills/create-local-app/SKILL.md`、`skills/create-local-app/agents/openai.yaml`
- Modify: `docs/local-apps/HANDOFF.md`（`:11`、`:45`、`:95`）

**Interfaces:**
- Consumes: Task 4 已删的协议变体

删除：`handle_propose_app_identity` / `propose_app_identity`、`APP_IDENTITY_SYSTEM_PROMPT`、`parse_app_identity`、`fallback_app_name`，及其全部测试。

⚠️ `skills/create-local-app/SKILL.md` **被 `include_str!` 编进引擎**（`skill-api/src/builtin/bundled.rs:31` 附近），其「Entry and confirmation」段落逐字写着旧契约。改它等于改引擎里的字节。
⚠️ `docs/local-apps/HANDOFF.md` 的 `:11`、`:45`、`:95` 三处陈述的正是本方案反转的不变量。

- [ ] **Step 1: 确认删干净**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
grep -rn "propose_app_identity\|ProposeAppIdentity\|AppIdentityProposed\|APP_IDENTITY_SYSTEM_PROMPT\|parse_app_identity\|fallback_app_name" \
  --include="*.rs" --include="*.ts" --include="*.swift" --include="*.kt" --include="*.md" . | grep -v "docs/superpowers/"
```
Expected: 删完后**零命中**（`docs/superpowers/` 里的 spec/plan 记录除外）。
⚠️ 一个 0-hit grep 只在 needle 能命中已知样本时才算证据——先在**未删**的树上跑一遍这条命令确认它有命中。

- [ ] **Step 2: 改 SKILL.md 与 HANDOFF.md**

`SKILL.md` 的「Entry and confirmation」改写成：创建入口只产生空壳；形态与名称由 `LocalAppScaffold` 在用户确认后落定；空壳期只有 `LocalAppScaffold` / list / get / create 可用。

- [ ] **Step 3: 跑全量**

```bash
cd lingxi-code && cargo test --workspace --all-features --no-fail-fast > /tmp/task12.txt 2>&1
grep -c '\.\.\. ok$' /tmp/task12.txt; grep -n '^failures:' -A 30 /tmp/task12.txt
```
Expected: 除 Global Constraints 列出的三个既有 SIGABRT target 外无失败；**测试总数不得下降**（删掉的提议测试除外——把删掉的条数记在提交信息里对账）。

- [ ] **Step 4: 提交**

⛔ **不要用 `git add -A`。** 这个 checkout 里另有一个会话在写
`lingxi-code/tasks/**` 与 `lingxi-code/tools/workflow/src/lib.rs`；`-A` 会把它们扫进本提交。
显式路径，提交前用 `git diff --cached --name-only` 核对：

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_host.rs \
        lingxi-code/apps/engine-mobile/src/host.rs \
        skills/create-local-app/ docs/local-apps/HANDOFF.md
git diff --cached --name-only     # 必须只有上面这些
git commit -m "Delete the app-identity proposal path and align the bundled skill and handoff docs"
```

---

## Task 13: iOS 客户端

**Files:**
- Modify: `clients/ios/Sources/LocalApps/LocalAppsLibraryView.swift`（`LocalAppCreateView` **声明在此文件内**，不是独立文件）
- Modify: `clients/ios/Sources/LocalApps/LocalAppsStore.swift`（`:891` widget 标签、`:1147` `makeWidgetSnapshot()`、`:414` `identityProposalTimeout`、`:857` brief 认领）
- Modify: `clients/ios/Sources/LocalApps/{LocalAppsModels,LocalAppsProtocolAdapter,LocalAppDetailView,LocalAppsDrawerSection}.swift`
- Modify: `clients/ios/Sources/App/RootView.swift`
- Test: `clients/ios/Tests/LocalAppsStoreTests.swift`（`:1571-1656` 的 brief 认领要整段替换）

**Interfaces:**
- Consumes: Task 4 的 wire

**「+」按钮三步：**
1. 生成 `request_id`（客户端 UUID），发
   `CreateApp{request_id, mode: .shell, brief: "", name: "", origin: .library, surface: nil, git_enabled: 默认, workflow_model: nil, conversation_id: 当前会话}`
2. 按 `request_id` 认领结果，再等 `AppRecordChanged` 带回 pin
   （⚠️ `AppCreated` **永远不带** `init_session_id`——它在创建事务内发出，pin 之后才铸。）
3. 打开 `.localApp(appID)` 会话并自动发 kickoff

⚠️ **一次性布尔不可用（已否决）**：引擎对**两条创建路径**都发 `AppCreated`，布尔在并发创建时会把代理在别处建的应用错认成本次「+」的结果，把用户劫持进错误的会话。

**pending 清理**：每个 pending `request_id` 设 **30 秒** `createResultTimeout`，超时或断线重连后清空 pending 并向用户报「创建结果未知，请在应用库确认」。**不得**在重连后认领任何未带匹配 `request_id` 的事件。
⚠️ 这是**新常量**。仓库里唯一相关的 `identityProposalTimeout = .seconds(20)`（`LocalAppsStore.swift:414`）是为**一次模型调用**设的、且随本计划删除——不要拿它当基准。30 秒的依据：创建本身是本地文件操作，但 pin 会铸一个会话，chat-origin 的还要 fork 源对话历史，冷设备上可能不快；超时只是止损，应用其实已经在库里。

**草稿态渲染**：`scaffolded == false` 的卡片标题渲染本地化的「新应用」、副标题「创建中」，**不显示** `name`/`brief`；点击进 pin 会话而非预览；删除照旧。
**同一判据要覆盖**：`LocalAppsDrawerSection.swift:33` 的 `Text(app.name)`、`LocalAppDetailView.swift:306` 的 `LabeledContent("local_apps_brief", value: app.brief)`、`LocalAppsStore.swift:891` 的 widget 标签。
⚠️ **widget 快照是「排除」不是「改文案」**：`makeWidgetSnapshot()`（`:1147`）里 `scaffolded == false` 的应用**整个不进快照**——否则空壳会以 `"untitled"` 出现在用户主屏上，而且是个点不开的图标。

**Widget 入口搬家**：挪到 `LocalAppDetailView.swift`。⚠️ 该文件今天 `grep -i widget` **零命中**，是净新增 UI（删表单等于回归这个功能）。

- [ ] **Step 1: 先重新生成绑定**

```bash
bash clients/ios/scripts/build-xcframework.sh && (cd clients/ios && xcodegen generate)
```
⚠️ 绑定是 gitignored 的构建产物；不重新生成，改 wire 后客户端不可能编过。

- [ ] **Step 2: 写会红的测试**

`LocalAppsStoreTests.swift` 新增：`request_id` 匹配才认领；**不匹配的 `AppCreated` 必须被忽略**；30 秒超时清 pending；断线重连不误认领；`AppOperationFailed` 带匹配 `request_id` 时报错给用户；草稿卡片不渲染 `name`/`brief`；`makeWidgetSnapshot()` 排除 `scaffolded == false`。

- [ ] **Step 3: 跑测试确认红**

Run: `cd clients/ios && xcodebuild test -scheme LingXi -testLanguage zh-Hans 2>&1 | tail -40`
⚠️ UI 测试要 `-testLanguage zh-Hans`。

- [ ] **Step 4: 实现 + 删除**

删：`LocalAppCreateView`、`AppIdentityProposal`、`proposeIdentity`、`identityProposalAnswers`、`creationBrief`、`identityProposalTimeout`、`:857` 的 brief 认领。

- [ ] **Step 5: 跑测试确认绿**

Run: `cd clients/ios && xcodebuild test -scheme LingXi -testLanguage zh-Hans 2>&1 | tail -40`

- [ ] **Step 6: 提交**

```bash
git add clients/ios
git commit -m "iOS: replace the create form with request_id-correlated shell creation"
```

---

## Task 14: Android 客户端

**Files:**
- Modify: `clients/android/.../localapps/LocalAppsScreen.kt`（`CreateAppDialog` 在 `:303`，`:270` 调用；`LocalAppCard` `:681`；溢出 `DropdownMenu` `:695`；`LocalAppDetailsScreen` 分发在 `:143`）
- Modify: `clients/android/.../localapps/LocalAppsViewModel.kt`（`:322` `pendingWidgetPin`、`:806` brief 认领、`:1032` `publishWidgetSnapshot()`、`:1109` 超时）
- Modify: `clients/android/.../localapps/LocalAppsContract.kt`（`LocalAppsDestination.Details` 在 `:214`）
- Modify: `clients/android/.../RootScreen.kt`
- Test: `clients/android/.../LocalAppsViewModelTest.kt`（`:302-528` 的 brief 认领要整段替换）

**Interfaces:**
- Consumes: Task 4 的 wire

与 Task 13 同一套行为契约（`request_id` 认领 / 30 秒超时 / 草稿卡片 / widget 快照排除）。

⚠️ **组件名是 `CreateAppDialog`，不是 `CreateAppSheet`。**
✅ **Android 有现成的详情页承载点**：`LocalAppsDestination.Details` 是一等目的地，在 `LocalAppsScreen.kt:143` 分发到 `LocalAppDetailsScreen`；`LocalAppCard` 自己还带一个溢出 `DropdownMenu`。⇒ widget 入口加一个 `LocalAppsAction` 并在其中一处置 `pendingWidgetPin` 即可，**不是新建屏幕**。

- [ ] **Step 1: 先重新生成绑定**

```bash
bash clients/android/scripts/build-jni.sh --variant play
```
⚠️ 该脚本历史上有过「退出非 0 但日志零字节」的坑（`set -e` + `read < <(无尾随换行)`）。若出现，先 `bash -x` 跑，别猜环境。

- [ ] **Step 2: 写会红的测试**（与 Task 13 同一组行为，Kotlin 拼写）

- [ ] **Step 3: 跑测试确认红**

Run: `cd clients/android && ./gradlew :app:testPlayDebugUnitTest --tests '*LocalAppsViewModelTest*' 2>&1 | tail -40`

- [ ] **Step 4: 实现 + 删除**

删：`CreateAppDialog`、`onProposeIdentity`、`LocalAppsViewModel` 的提议状态、`:806` 的 brief 认领、`:1109` 的提议超时。

- [ ] **Step 5: 跑测试确认绿**

Run: `cd clients/android && ./gradlew :app:testPlayDebugUnitTest 2>&1 | tail -30`

- [ ] **Step 6: 提交**

```bash
git add clients/android
git commit -m "Android: replace the create dialog with request_id-correlated shell creation"
```

---

## Task 15a / 15b: 文案与 i18n（拆成两半，见执行顺序）

> **⚠️ 执行顺序（preflight Ruling P-3）：本 Task 拆两半。**
> **15a —— 在 Task 13 之前做**：只**新增**草稿卡片的「新应用」「创建中」、创建超时提示、
> widget 入口标题这几个 key，跑 `generate.py`。Task 13/14 引用的 key 必须先存在，
> 否则它们的测试步骤会因为缺本地化而红。
> **15b —— 在 Task 14 之后做**：**删除**退役的 `local_apps_init_kickoff` 与创建表单全部
> key、改 kickoff 文案、再跑一次 `generate.py`，然后做 Step 4 的零悬空 grep。
> 那条 grep 在 13/14 删掉引用之前不可能通过。
>
> 下面的 Step 1-5 按这个切分执行：Step 1（新增部分）+ Step 2 属于 15a；
> Step 1（删除部分）+ Step 2 + Step 3 + Step 4 属于 15b；Step 5 各自提交一次。

## Task 15: 文案与 i18n

**Files:**
- Modify: `clients/translations/*.json`（5 语言）
- Regenerate: `clients/ios/Resources/Localizable.xcstrings`、`clients/android/**/values*/strings.xml`
- Check: `clients/android/**/values*/strings_local_apps_v3.xml`（**不由生成器管理**）

**Interfaces:**
- Consumes: Task 13/14 引用的新 key

**kickoff 文案**换成不带占位符的一句话（用户视角：「我想做一个新的本地应用。」）。新文案不带占位符，两端**共用一个 key**。
**同批删除**：`local_apps_init_kickoff %@` 与 Android 孪生 `local_apps_init_kickoff`，以及创建表单的全部文案 key。
**新增**：草稿卡片的「新应用」「创建中」、创建超时提示、widget 入口标题。

⚠️ **iOS 占位符在 KEY 里、Android 在值里** ⇒ 一个概念常需两个 key。本次新文案不带占位符，正好避开。

- [ ] **Step 1: 改 `clients/translations/*.json`**

- [ ] **Step 2: 重新生成**

```bash
cd clients/translations && python3 generate.py
git diff --stat -- clients/ios/Resources/Localizable.xcstrings clients/android
```
Expected: diff 只出现在生成产物里，且与 JSON 改动一一对应。

- [ ] **Step 3: 核对手工维护的那份**

```bash
grep -rn "local_apps_init_kickoff\|local_apps_create" clients/android/**/values*/strings_local_apps_v3.xml
```

- [ ] **Step 4: 确认没有悬空 key**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
grep -rn "local_apps_init_kickoff" --include="*.swift" --include="*.kt" --include="*.rs" clients/ lingxi-code/
```
Expected: 零命中。

- [ ] **Step 5: 提交**

```bash
git add clients/
git commit -m "Retire the create-form copy and add the draft-state strings"
```

---

## Task 16: 全量验证与真机验收

**Files:** 无（只跑）

- [ ] **Step 1: Rust 全量**

```bash
cd lingxi-code
OUT=/tmp/final-workspace.txt
cargo test --workspace --all-features --no-fail-fast > "$OUT" 2>&1
echo "EXIT=$?"
grep -c '\.\.\. ok$' "$OUT"
grep -n '^failures:' -A 40 "$OUT"
grep -n 'stack overflow' "$OUT"
```
Expected: 除 Global Constraints 列出的三个既有 SIGABRT target 外无失败。**记下通过总数**，与 Task 12 的基线对账。

- [ ] **Step 2: 两端客户端**

```bash
cd clients/ios && xcodebuild test -scheme LingXi -testLanguage zh-Hans 2>&1 | tail -20
cd ../android && ./gradlew :app:testPlayDebugUnitTest 2>&1 | tail -20
```

- [ ] **Step 3: 真机验收（§G）——不接受「编过了」**

**G.-1（clean install 前提）**：**重装覆盖安装**（⛔ 不要卸载——会毁掉设备上的本地应用），启动后确认本地应用库为空。不拿残留旧 store 验收。

1. 点「+」→ **不弹任何表单**，直接进对话，代理第一句在问你想做什么
2. 回答「打飞机」→ 代理提议名称，并用**原生选项**让你确认形态，且它选的是 `canvas`
3. 确认后应用成形、草稿标记消失、一路能构建出可玩的东西
4. 反向用例：第 1 步就退出 → 库里留一张「创建中」卡片，点回去续上**同一个**对话
5. **两端 widget 入口**：iOS 详情页、Android 新承载点各加一次成功
6. **空壳不在主屏 widget 快照里**：建一个空壳，检查主屏 widget 不出现 `untitled`

- [ ] **Step 4: 记录已知留口（不修，登记在案）**

- **成形之后**的同名异扩展影子文件（`app/app.js` 在 Vite 解析里赢过 `app/app.jsx`）仍然可行——那是既有面，今天任何已成形应用都可以，本计划既不引入也不扩大。**单独立项。**
- **门保证不了「代理真的问过用户」**。它可能问完第一句就自作主张调 `LocalAppScaffold`。只能靠提示词，G.2 就是在验它。若真机反复不过，下一步是让 `LocalAppScaffold` 要求一个「用户已确认」的证据字段——同样可被编造，本期不做。
- **空壳会堆积**，没有自动清理，靠用户删。
- **8.0.0 的破坏面只有桌面端**（Electron / `clients/shared`）在握手时硬失败。iOS/Android 走 UniFFI 进程内调用，既没有握手也没有版本交换，失败形式是重新生成绑定后**编译不过**。

---

## Self-Review

**Spec 覆盖对账（§0-§H）:**

| Spec | Task |
|---|---|
| §0 前置修复 | 1 |
| §A.1 clean install / §A.2 `scaffolded` / §A.3 `CreateMode` / §A.4 占位名泄漏 | 2（字段与模式）、13/14（渲染点）、9（会话标题） |
| §B.1 `mode` + `request_id` / §B.2 `AppRecordDto.scaffolded` / §B.3 删变体 + bless | 4 |
| §B.4 协议所有权与顺序 | 计划头部 + Task 4 |
| §C.0.1 清空保证 / §C.0.2 提示（两条否决记在案）/ §C.0.3 留口 | 6、10、16 Step 4 |
| §C.1 `LocalAppScaffold` 五步 / §C.1.4 hash 不变量 / §C.1.5 写入方法 / §C.1.6 权限 | 8、7 |
| §C.2 工具门 | 10 |
| §C.3 两份 `LINGXI.md` | 5（引导版）、8（正式版） |
| §C.4 `detect_build_target` | 11 |
| §C.5 删除 | 12 |
| §D.1 `request_id` 入口 / §D.2 kickoff / §D.3 草稿态 / §D.4 widget 搬家 / §D.5 删除 | 3、13、14、15 |
| §E 文件清单 | 各 Task 的 Files 段 |
| §F 测试 | 各 Task 的 Step 1 |
| §G 真机验收 | 1 Step 7（§G.0 单独）、16 Step 3 |
| §H 风险与留口 | 16 Step 4 |

**范围外（本计划不实现）：** §I 全部（Web runtime profile 五种、Godot `NativeGame`）。按 master order 是步骤 3a/4/8，前置是 verification Phase 1a/1b 与 IIFE spike。

**未决（需人工协调，不属于本计划的任何 Task）：** verification design 仍在 `2026-08-23-local-app-interactive-verification-design.md:5` 声明 baseline `7.0.0` / blessed major 7。Task 4 落地 8.0.0 之后，它必须 rebase；⛔ 不得人工拼接两个 contract snapshot。
