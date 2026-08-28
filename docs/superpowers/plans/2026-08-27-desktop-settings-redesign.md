# 桌面设置重设计 实施计划（Part A）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除桌面端未接线的设置 mock，重建为分层感知的全页面设置，并打通引擎设置的读写通道。

**Architecture:** 引擎全权——`bridge-server` 新增 `GetSettings` / `UpdateSettings` 及权限、MCP、Skills 命令族；合并、校验、provenance 各只有一个实现，都留在引擎侧。桌面是薄客户端，只渲染引擎回传的快照。写入分层目标由 UI 顶部的三层 tab（用户 / 项目 / 本地）选择。

**Tech Stack:** Rust（client-protocol / bridge-server / permission / migrations / engine）、TypeScript + React（Electron 渲染进程）、`node --test`（Electron 测试）、`cargo test`（Rust 测试）。

**Spec:** `docs/superpowers/specs/2026-08-27-desktop-settings-redesign-design.md`

## 契约修订 R1（2026-08-28，执行期实测后写入 —— 覆盖下文 §A2 的 DTO 设计）

下文 Task 3 的接口块是**错的**，执行时以本节为准。三项实测：

1. **`client-protocol` 不得引入 `serde_json`。** 其 `Cargo.toml` 记录着 governing
   decision §0.4：`serde_json::Value` 不是 UniFFI-representable，故契约里的结构化
   载荷一律是 JSON **字符串**（既有写法：`ToolUseStarted.input_json`、
   `ToolUseResult.result_json`）。`serde_json` 只在 `[dev-dependencies]`。
   `BTreeMap` 同样非 UniFFI-representable。**不要动这个 Cargo.toml。**
2. **`ClientEvent::SettingsSnapshot { effective_json: String, provenance_json: String }`
   已经存在**（`client-protocol/src/events.rs:262`）。新增同名变体编译不过。
3. **读通道已在协议里完整定义但从未接线。**
   `RefreshListings { which: [{type:"settings"}] }` → `ListingKindDto::Settings`
   （`commands.rs:681`）→ 上述事件，并有 `version_guard_test.rs:438-440` 的字段级
   契约钉子。而 `apps/bridge-server/src/router.rs:626` 的处理是
   `ListingKindDto::Memory | ListingKindDto::Settings => { tracing::debug!(...) }`
   —— 打一行日志，不发任何事件。

**据此修订：**

- **不新增 `GetSettings` 命令**；接线既有的 `ListingKindDto::Settings` 分支。
  桌面侧只需在 `AllowedClientCommand` 的 `refresh_listings.which` 联合里加
  `'settings'`（`refresh_listings` 本就在白名单内）。
- **`SettingsSnapshot` 扩三个新的可选字段**（F1-09 下 additive，不 bump）：
  `files_json: Option<String>`、`active_json: Option<String>`、
  `locked: Option<Vec<String>>`；同步补 `version_guard_test.rs` 的 `put(...)` 行。
- **写命令**：`UpdateSettings { destination: SettingsDestinationDto, patch_json: String }`。
  `patch_json` 是一个 JSON 对象，值为 `null` 表示删除该键 —— 语义等价于原文的
  `Vec<(String, Option<Value>)>`，但不用元组也不用 `Value`。
  Tasks 5/6/7 的命令同理：结构化入参一律 `_json: String`。
- **`serde_json::Value` 与 `BTreeMap` 在 `settings_bridge.rs` 内部照常使用** ——
  bridge-server 本就依赖 serde_json。约束只针对**契约 crate**。
- 桌面侧收到的是 JSON 字符串，需 `JSON.parse` 后再交给 `useEngineSettings`；
  Task 13 的 `SettingsSnapshot` TS 接口不变，解析在桥接层做。

---

## Global Constraints

- `CLIENT_PROTOCOL_VERSION` 保持 `"8.0.0"`。本计划的协议改动全部是新增变体 / 新增可选字段，属 F1-09 guard 的 additive，**不得 bump，不得 re-bless `snapshots/blessed_major.txt`**。
- **I1**：`UpdateSettings` 必须拒绝 `permissions` 顶层键。权限只经 `permission/src/persist.rs`。唯一例外是原始 JSON 逃生口（Task 19）。
- **I2**：任何设置文件写入前必须调用 `permission::mark_internal_write(path)`。`settings_watch.rs:343` 以 5 秒窗口消费该标记。
- **I3**：UI 不做乐观更新。写入成功后引擎重读并发新的 `SettingsSnapshot`，界面只渲染该快照。
- 设置层合并优先级（高→低）：`env → managed → cli → local → project → user → defaults`。
- 全部**写**命令的 `destination` 只接受 `User` / `Project` / `Local`。
- Rust 测试：`cargo test -p <crate>`。**捕获完整输出到文件再 grep**，绝不单独 grep `FAILED`（会丢掉点名测试的 `failures:` 块）。
- Electron 测试：`cd clients/electron && npm test`（`node --test`，文件放 `test/*.test.ts`）。
- Electron 类型检查：`cd clients/electron && npm run typecheck`。

---

## 文件结构

**Rust 新建**
- `lingxi-code/apps/bridge-server/src/settings_bridge.rs` —— 设置快照构建 + 带标记的原子通用写入器。落在 bridge-server 是因为 `engine` crate 依赖极简（无 `traits` / `permission`），而 bridge-server 已依赖 client-protocol / permission / migrations / traits / engine。

**Rust 修改**
- `lingxi-code/client-protocol/src/commands.rs` —— 新增命令变体
- `lingxi-code/client-protocol/src/events.rs` —— 新增事件变体
- `lingxi-code/client-protocol/src/lib.rs` —— 导出新 DTO
- `lingxi-code/apps/bridge-server/src/router.rs` —— 命令分发
- `lingxi-code/migrations/src/settings_update.rs` —— `SettingsSource::Project`

**Electron 新建**
- `clients/electron/src/shared/clientCommands.ts` —— `AllowedClientCommand` 单一来源
- `clients/electron/src/renderer/components/settings/{SettingsScreen.tsx,nav.ts,rows.tsx,useEngineSettings.ts}`
- `clients/electron/src/renderer/components/settings/pages/*.tsx`（15 页）

**Electron 修改**
- `clients/electron/src/main/{settings.ts,host.ts,bridge.ts,host-utils.ts}`
- `clients/electron/src/preload/index.ts`
- `clients/electron/src/renderer/bridge/lingxi.d.ts`
- `clients/electron/src/renderer/App.tsx`
- `clients/electron/src/renderer/components/BetaDesktop.tsx`（删除 `BetaSettings`，1843-2130）
- `clients/electron/src/renderer/theme/tokens.ts`

**Electron 删除**
- `clients/electron/src/renderer/components/settings/` 现有 8 个文件

---

# 阶段 1 —— 引擎侧读通道

## Task 1: `SettingsSource::Project`

**Files:**
- Modify: `lingxi-code/migrations/src/settings_update.rs:16-35`
- Test: `lingxi-code/migrations/src/settings_update.rs`（同文件 `#[cfg(test)]` 内）

**Interfaces:**
- Consumes: 无
- Produces: `SettingsSource::Project` 变体；`settings_path(SettingsSource::Project, lingxi_home, project_dir) -> PathBuf` 返回 `<project_dir>/<DOT_DIR>/settings.json`

- [ ] **Step 1: 写失败测试**

在 `settings_update.rs` 的测试模块中加入：

```rust
#[test]
fn project_source_resolves_to_project_settings_json() {
    let home = std::path::Path::new("/home/u/.lingxi");
    let project = std::path::Path::new("/work/repo");
    let path = settings_path(SettingsSource::Project, home, project);
    assert_eq!(
        path,
        std::path::Path::new("/work/repo").join(branding::DOT_DIR).join("settings.json"),
        "Project source must resolve to <project>/<DOT_DIR>/settings.json, got {}",
        path.display()
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p migrations project_source_resolves 2>&1 | tee /tmp/t1.log`
Expected: 编译失败，`no variant named \`Project\` found for enum \`SettingsSource\``

- [ ] **Step 3: 最小实现**

在 `SettingsSource` 中加入变体：

```rust
pub enum SettingsSource {
    /// `userSettings` → `<lingxi-config-home>/settings.json`.
    User,
    /// `projectSettings` → `<project>/.lingxi/settings.json`.
    Project,
    /// `localSettings` → `<project>/.lingxi/settings.local.json`.
    Local,
}
```

在 `settings_path` 的 match 中加入分支：

```rust
        SettingsSource::Project => project_dir
            .join(branding::DOT_DIR)
            .join("settings.json"),
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p migrations 2>&1 | tee /tmp/t1.log; grep -c "^test " /tmp/t1.log`
Expected: PASS，且测试计数不低于改动前（计数下降是红旗，见 Global Constraints）

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/migrations/src/settings_update.rs
git commit -m "Teach settings_update the project layer"
```

---

## Task 2: 设置快照构建

**Files:**
- Create: `lingxi-code/apps/bridge-server/src/settings_bridge.rs`
- Modify: `lingxi-code/apps/bridge-server/src/lib.rs`（加 `mod settings_bridge;`）
- Test: `lingxi-code/apps/bridge-server/src/settings_bridge.rs`（同文件 `#[cfg(test)]`）

**Interfaces:**
- Consumes: Task 1 的 `SettingsSource::Project`
- Produces:
  - `pub struct SettingsPaths { pub lingxi_home: PathBuf, pub project_dir: PathBuf }`
  - `pub fn build_snapshot(paths: &SettingsPaths, active: BTreeMap<String, Value>) -> SettingsSnapshotDto`
  - `pub fn writable_path(paths: &SettingsPaths, dest: SettingsDestinationDto) -> Result<PathBuf, String>`

- [ ] **Step 1: 写失败测试 —— provenance 指向合并结果的真实来源**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// 四层同时定义同一个键；断言 provenance 指向优先级顺序推出来的那一层，
    /// 而不是一个硬编码字符串。优先级：local > project > user。
    #[test]
    fn provenance_names_the_layer_the_merged_value_actually_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        std::fs::create_dir_all(home.join(".lingxi")).unwrap();
        std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();

        std::fs::write(home.join("settings.json"), r#"{"outputStyle":"from-user"}"#).unwrap();
        std::fs::write(
            project.join(branding::DOT_DIR).join("settings.json"),
            r#"{"outputStyle":"from-project"}"#,
        ).unwrap();
        std::fs::write(
            project.join(branding::DOT_DIR).join("settings.local.json"),
            r#"{"outputStyle":"from-local"}"#,
        ).unwrap();

        let paths = SettingsPaths { lingxi_home: home, project_dir: project };
        let snap = build_snapshot(&paths, BTreeMap::new());

        assert_eq!(
            snap.effective.get("outputStyle").and_then(|v| v.as_str()),
            Some("from-local"),
            "local must win over project and user"
        );
        assert_eq!(
            snap.provenance.get("outputStyle"),
            Some(&SettingsDestinationDto::Local),
            "provenance must name Local, the layer the winning value came from"
        );
    }

    #[test]
    fn writable_path_rejects_non_writable_destinations() {
        let paths = SettingsPaths {
            lingxi_home: "/home/u/.lingxi".into(),
            project_dir: "/work/repo".into(),
        };
        let err = writable_path(&paths, SettingsDestinationDto::Managed).unwrap_err();
        assert!(err.contains("managed"), "error must name the rejected value, got: {err}");
        assert!(
            err.contains("user") && err.contains("project") && err.contains("local"),
            "error must name the writable set, got: {err}"
        );
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p bridge-server settings_bridge 2>&1 | tee /tmp/t2.log`
Expected: 编译失败，`cannot find function \`build_snapshot\``

- [ ] **Step 3: 最小实现**

```rust
//! 设置快照构建与带标记的原子写入器。
//!
//! 落在 bridge-server 而非 `engine::settings`，因为 `engine` crate 的依赖极简
//! （无 `traits` / `permission`），而带标记的原子写需要两者。

use std::collections::BTreeMap;
use std::path::PathBuf;

use client_protocol::{SettingsDestinationDto, SettingsFileDto, SettingsSnapshotDto};
use migrations::settings_update::{read_settings_map, settings_path, SettingsSource};
use serde_json::Value;

/// 解析设置层文件路径所需的两个根。
pub struct SettingsPaths {
    pub lingxi_home: PathBuf,
    pub project_dir: PathBuf,
}

/// 按优先级从低到高排列的可读文件层。合并时后者覆盖前者。
const FILE_LAYERS: [(SettingsSource, SettingsDestinationDto); 3] = [
    (SettingsSource::User, SettingsDestinationDto::User),
    (SettingsSource::Project, SettingsDestinationDto::Project),
    (SettingsSource::Local, SettingsDestinationDto::Local),
];

/// 构建一个设置快照。`active` 是运行中会话启动时实际加载的值。
pub fn build_snapshot(
    paths: &SettingsPaths,
    active: BTreeMap<String, Value>,
) -> SettingsSnapshotDto {
    let mut effective: BTreeMap<String, Value> = BTreeMap::new();
    let mut provenance: BTreeMap<String, SettingsDestinationDto> = BTreeMap::new();
    let mut files: Vec<SettingsFileDto> = Vec::new();

    for (source, destination) in FILE_LAYERS {
        let path = settings_path(source, &paths.lingxi_home, &paths.project_dir);
        let exists = path.exists();
        let (map, parse_error) = match read_settings_map(&path) {
            Ok(m) => (m, None),
            Err(e) => (serde_json::Map::new(), Some(e)),
        };
        for (key, value) in &map {
            effective.insert(key.clone(), value.clone());
            provenance.insert(key.clone(), destination);
        }
        files.push(SettingsFileDto {
            destination,
            path: path.display().to_string(),
            exists,
            writable: parse_error.is_none(),
            parse_error,
        });
    }

    SettingsSnapshotDto {
        files,
        effective,
        active,
        provenance,
        locked: Vec::new(),
    }
}

/// 解析一个**可写**目标层的文件路径。只有 User / Project / Local 可写。
pub fn writable_path(
    paths: &SettingsPaths,
    dest: SettingsDestinationDto,
) -> Result<PathBuf, String> {
    let source = match dest {
        SettingsDestinationDto::User => SettingsSource::User,
        SettingsDestinationDto::Project => SettingsSource::Project,
        SettingsDestinationDto::Local => SettingsSource::Local,
        other => {
            return Err(format!(
                "settings destination {other:?} is not writable; writable destinations are user, project, local"
            ).to_lowercase())
        }
    };
    Ok(settings_path(source, &paths.lingxi_home, &paths.project_dir))
}
```

> `locked` 在本任务留空 `Vec`；managed 层的锁定在 Task 3 接入真实的 `engine::settings` 加载后填充。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p bridge-server settings_bridge 2>&1 | tee /tmp/t2.log; grep -E "^test result|failures:" /tmp/t2.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/bridge-server/src/settings_bridge.rs lingxi-code/apps/bridge-server/src/lib.rs
git commit -m "Build a settings snapshot that names each value's layer"
```

---

## Task 3: `GetSettings` 命令与 `SettingsSnapshot` 事件

**Files:**
- Modify: `lingxi-code/client-protocol/src/commands.rs`（`ClientCommand` 新增变体）
- Modify: `lingxi-code/client-protocol/src/events.rs`（`ClientEvent` 新增变体 + DTO）
- Modify: `lingxi-code/client-protocol/src/lib.rs`（导出）
- Modify: `lingxi-code/apps/bridge-server/src/router.rs`
- Test: `lingxi-code/client-protocol/tests/commands_test.rs`、`lingxi-code/apps/bridge-server/tests/router_test.rs`

**Interfaces:**
- Consumes: Task 2 的 `build_snapshot` / `SettingsPaths`
- Produces:
  - `ClientCommand::GetSettings`
  - `ClientEvent::SettingsSnapshot(SettingsSnapshotDto)`
  - `ClientEvent::SettingsOperationFailed { message: String }`
  - `SettingsDestinationDto { Env, Managed, Cli, Local, Project, User, Defaults }`
  - `SettingsFileDto { destination, path, exists, writable, parse_error }`
  - `SettingsSnapshotDto { files, effective, active, provenance, locked }`

- [ ] **Step 1: 写失败测试 —— 线格式与协议版本不变**

在 `client-protocol/tests/commands_test.rs` 加入：

```rust
#[test]
fn get_settings_serializes_as_snake_case_tag() {
    let json = serde_json::to_value(&ClientCommand::GetSettings).unwrap();
    assert_eq!(json, serde_json::json!({ "type": "get_settings" }));
}
```

在 `client-protocol/tests/version_test.rs` 加入：

```rust
#[test]
fn adding_settings_commands_is_additive_and_does_not_bump() {
    assert_eq!(
        client_protocol::CLIENT_PROTOCOL_VERSION, "8.0.0",
        "settings commands are new variants only — additive under the F1-09 guard, no bump"
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p client-protocol 2>&1 | tee /tmp/t3.log`
Expected: 编译失败，`no variant named \`GetSettings\``

- [ ] **Step 3: 最小实现**

`client-protocol/src/events.rs` 加入 DTO：

```rust
/// 一个设置值可能来自的层。读方向可为任意层；写命令只接受 User/Project/Local。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
pub enum SettingsDestinationDto {
    Env,
    Managed,
    Cli,
    Local,
    Project,
    User,
    Defaults,
}

/// 一个设置层文件的状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct SettingsFileDto {
    pub destination: SettingsDestinationDto,
    pub path: String,
    pub exists: bool,
    pub writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_error: Option<String>,
}

/// 设置快照。`effective` 是盘上当前的合并结果，`active` 是运行中会话启动时
/// 实际加载的值；两者的差集就是「待应用」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct SettingsSnapshotDto {
    pub files: Vec<SettingsFileDto>,
    pub effective: BTreeMap<String, serde_json::Value>,
    pub active: BTreeMap<String, serde_json::Value>,
    pub provenance: BTreeMap<String, SettingsDestinationDto>,
    pub locked: Vec<String>,
}
```

`ClientEvent` 加入两个变体：

```rust
    SettingsSnapshot(SettingsSnapshotDto),
    SettingsOperationFailed { message: String },
```

`ClientCommand` 加入：

```rust
    // ── Settings ──────────────────────────────────────────────────────────
    /// 请求一份设置快照（四层合并结果 + 每键来源 + 各层文件状态）。
    GetSettings,
```

`router.rs` 在命令 match 中加入：

```rust
            ClientCommand::GetSettings => {
                let snapshot = crate::settings_bridge::build_snapshot(
                    &self.settings_paths(),
                    self.handle.active_settings().await.unwrap_or_default(),
                );
                sink.emit(ClientEvent::SettingsSnapshot(snapshot)).await;
            }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p client-protocol -p bridge-server 2>&1 | tee /tmp/t3.log; grep -E "^test result|failures:" /tmp/t3.log`
Expected: 全部 PASS，且 `CLIENT_PROTOCOL_VERSION` 仍为 `8.0.0`

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/client-protocol lingxi-code/apps/bridge-server
git commit -m "Expose the settings snapshot over the client protocol"
```

---

# 阶段 2 —— 引擎侧写通道

## Task 4: 带标记的原子通用写入器 + `UpdateSettings`

**Files:**
- Modify: `lingxi-code/apps/bridge-server/src/settings_bridge.rs`
- Modify: `lingxi-code/client-protocol/src/commands.rs`
- Modify: `lingxi-code/apps/bridge-server/src/router.rs`
- Test: `lingxi-code/apps/bridge-server/src/settings_bridge.rs`

**Interfaces:**
- Consumes: Task 2 的 `writable_path`
- Produces: `pub fn apply_patch(paths: &SettingsPaths, dest: SettingsDestinationDto, patch: Vec<(String, Option<Value>)>) -> Result<(), String>`；`ClientCommand::UpdateSettings { destination, patch }`

- [ ] **Step 1: 写失败测试 —— 三条，含两条反向用例**

```rust
    /// I1：通用补丁不得写 permissions。错误必须点名 permissions 与替代命令。
    #[test]
    fn generic_patch_refuses_the_permissions_key() {
        let dir = tempfile::tempdir().unwrap();
        let paths = SettingsPaths {
            lingxi_home: dir.path().join("home"),
            project_dir: dir.path().join("repo"),
        };
        let err = apply_patch(
            &paths,
            SettingsDestinationDto::User,
            vec![("permissions".to_string(), Some(serde_json::json!({})))],
        ).unwrap_err();
        assert!(err.contains("permissions"), "error must name the key, got: {err}");
        assert!(
            err.contains("update_permission_rules"),
            "error must name the替代命令, got: {err}"
        );
    }

    /// I2：写入必须留下内部写标记，否则 watcher 会把它当成外部编辑。
    #[test]
    fn a_write_leaves_an_internal_write_mark() {
        let dir = tempfile::tempdir().unwrap();
        let paths = SettingsPaths {
            lingxi_home: dir.path().join("home"),
            project_dir: dir.path().join("repo"),
        };
        apply_patch(
            &paths,
            SettingsDestinationDto::User,
            vec![("outputStyle".to_string(), Some(serde_json::json!("terse")))],
        ).unwrap();
        let path = writable_path(&paths, SettingsDestinationDto::User).unwrap();
        assert!(
            permission::consume_internal_write(&path, std::time::Duration::from_secs(5)),
            "apply_patch must call mark_internal_write before writing {}",
            path.display()
        );
    }

    /// I2 的反向用例：证明上面那条断言真的能红。一个未经标记的写入
    /// 必须让 consume_internal_write 返回 false —— 否则该门恒绿。
    #[test]
    fn the_internal_write_assertion_can_fail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unmarked.json");
        std::fs::write(&path, "{}\n").unwrap();
        assert!(
            !permission::consume_internal_write(&path, std::time::Duration::from_secs(5)),
            "an unmarked write must NOT be consumable; if this passes, the门 in \
             a_write_leaves_an_internal_write_mark proves nothing"
        );
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p bridge-server settings_bridge 2>&1 | tee /tmp/t4.log`
Expected: `generic_patch_refuses_the_permissions_key` 与 `a_write_leaves_an_internal_write_mark` 编译失败（`apply_patch` 未定义）；`the_internal_write_assertion_can_fail` 编译后应 PASS

- [ ] **Step 3: 最小实现**

在 `settings_bridge.rs` 加入：

```rust
/// 有专用写入器、因此禁止走通用补丁的顶层键（I1）。
const RESERVED_KEYS: [(&str, &str); 1] = [("permissions", "update_permission_rules")];

/// 对一个可写层应用一批浅层补丁。`None` 即删除该键。
/// 未知键逐字保留（`read_settings_map` 的语义）。
///
/// # Errors
/// 目标层不可写、命中保留键、目标文件是坏 JSON、或写盘失败。
pub fn apply_patch(
    paths: &SettingsPaths,
    dest: SettingsDestinationDto,
    patch: Vec<(String, Option<Value>)>,
) -> Result<(), String> {
    for (key, _) in &patch {
        if let Some((reserved, replacement)) =
            RESERVED_KEYS.iter().find(|(reserved, _)| reserved == key)
        {
            return Err(format!(
                "the `{reserved}` key has a dedicated writer and is refused here; \
                 use the `{replacement}` command instead"
            ));
        }
    }
    let path = writable_path(paths, dest)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let mut map = read_settings_map(&path)?;
    for (key, value) in patch {
        match value {
            Some(v) => { map.insert(key, v); }
            None => { map.remove(&key); }
        }
    }
    let serialized = serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|e| format!("failed to serialize settings for {}: {e}", path.display()))?;

    // I2：先标记，再写。settings_watch.rs:343 以 5 秒窗口消费该标记；
    // 漏掉这一步会让桌面自己的每次保存都触发一轮 ConfigChange hook。
    permission::mark_internal_write(&path);
    std::fs::write(&path, serialized + "\n")
        .map_err(|e| format!("failed to write settings to {}: {e}", path.display()))
}
```

`ClientCommand` 加入：

```rust
    /// 对一个可写层应用一批浅层设置补丁。`None` 即删除该键。
    UpdateSettings {
        destination: SettingsDestinationDto,
        patch: Vec<(String, Option<serde_json::Value>)>,
    },
```

`router.rs` 加入分支（成功后重发快照，兑现 I3）：

```rust
            ClientCommand::UpdateSettings { destination, patch } => {
                let paths = self.settings_paths();
                match crate::settings_bridge::apply_patch(&paths, destination, patch) {
                    Ok(()) => {
                        let snapshot = crate::settings_bridge::build_snapshot(
                            &paths,
                            self.handle.active_settings().await.unwrap_or_default(),
                        );
                        sink.emit(ClientEvent::SettingsSnapshot(snapshot)).await;
                    }
                    Err(message) => {
                        sink.emit(ClientEvent::SettingsOperationFailed { message }).await;
                    }
                }
            }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p bridge-server 2>&1 | tee /tmp/t4.log; grep -E "^test result|failures:" /tmp/t4.log`
Expected: 三条测试全 PASS

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/bridge-server lingxi-code/client-protocol
git commit -m "Write settings through one marked, reserved-key-aware path"
```

---

## Task 5: 权限三命令

**Files:**
- Modify: `lingxi-code/client-protocol/src/commands.rs`
- Modify: `lingxi-code/apps/bridge-server/src/router.rs`
- Test: `lingxi-code/apps/bridge-server/tests/router_test.rs`

**Interfaces:**
- Consumes: `permission::{persist_permission_rule_set, persist_permission_mode, persist_workspace_directories, PermissionPaths, PermissionBehavior, PermissionUpdateDestination}`
- Produces:
  - `ClientCommand::UpdatePermissionRules { destination, behavior: PermissionBehaviorDto, add: Vec<String>, remove: Vec<String> }`
  - `ClientCommand::SetDefaultPermissionMode { destination, mode: String }`
  - `ClientCommand::UpdateWorkspaceDirectories { destination, add: Vec<String>, remove: Vec<String> }`
  - `PermissionBehaviorDto { Allow, Deny, Ask }`

- [ ] **Step 1: 写失败测试**

在 `router_test.rs` 加入：

```rust
#[tokio::test]
async fn update_permission_rules_writes_the_named_layer_and_preserves_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("repo");
    std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();
    let target = project.join(branding::DOT_DIR).join("settings.json");
    std::fs::write(&target, r#"{"outputStyle":"terse"}"#).unwrap();

    let harness = RouterHarness::with_project(&project).await;
    harness.send(ClientCommand::UpdatePermissionRules {
        destination: SettingsDestinationDto::Project,
        behavior: PermissionBehaviorDto::Allow,
        add: vec!["Bash(ls:*)".to_string()],
        remove: vec![],
    }).await;

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(
        written["permissions"]["allow"][0], "Bash(ls:*)",
        "the rule must land in the project layer's permissions.allow"
    );
    assert_eq!(
        written["outputStyle"], "terse",
        "unrelated keys must survive verbatim"
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p bridge-server update_permission_rules 2>&1 | tee /tmp/t5.log`
Expected: 编译失败，`no variant named \`UpdatePermissionRules\``

- [ ] **Step 3: 最小实现**

`client-protocol/src/commands.rs`：

```rust
/// 权限规则的行为桶。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
pub enum PermissionBehaviorDto { Allow, Deny, Ask }
```

```rust
    /// 在一个可写层增删同一行为桶的权限规则。
    UpdatePermissionRules {
        destination: SettingsDestinationDto,
        behavior: PermissionBehaviorDto,
        add: Vec<String>,
        remove: Vec<String>,
    },
    /// 持久化默认权限模式（区别于会话作用域的 `SetPermissionMode`）。
    SetDefaultPermissionMode { destination: SettingsDestinationDto, mode: String },
    /// 在一个可写层增删 `permissions.additionalDirectories`。
    UpdateWorkspaceDirectories {
        destination: SettingsDestinationDto,
        add: Vec<String>,
        remove: Vec<String>,
    },
```

`router.rs` —— 三个分支都走 `permission/src/persist.rs`，成功后重发快照：

```rust
            ClientCommand::UpdatePermissionRules { destination, behavior, add, remove } => {
                let dest = match permission_destination(destination) {
                    Ok(d) => d,
                    Err(message) => {
                        sink.emit(ClientEvent::SettingsOperationFailed { message }).await;
                        return;
                    }
                };
                let paths = self.permission_paths();
                let behavior = match behavior {
                    PermissionBehaviorDto::Allow => PermissionBehavior::Allow,
                    PermissionBehaviorDto::Deny => PermissionBehavior::Deny,
                    PermissionBehaviorDto::Ask => PermissionBehavior::Ask,
                };
                let parse = |raw: &[String]| -> Result<Vec<PermissionRule>, String> {
                    raw.iter()
                        .map(|s| PermissionRule::parse(s, behavior)
                            .map_err(|e| format!("invalid permission rule `{s}`: {e}")))
                        .collect()
                };
                let result = async {
                    let to_add = parse(&add)?;
                    let to_remove = parse(&remove)?;
                    if !to_add.is_empty() {
                        permission::persist_permission_rule_set(&to_add, true, dest, &paths)
                            .await.map_err(|e| e.to_string())?;
                    }
                    if !to_remove.is_empty() {
                        permission::persist_permission_rule_set(&to_remove, false, dest, &paths)
                            .await.map_err(|e| e.to_string())?;
                    }
                    Ok::<(), String>(())
                }.await;
                self.emit_settings_result(sink, result).await;
            }

            ClientCommand::SetDefaultPermissionMode { destination, mode } => {
                let dest = match permission_destination(destination) {
                    Ok(d) => d,
                    Err(message) => {
                        sink.emit(ClientEvent::SettingsOperationFailed { message }).await;
                        return;
                    }
                };
                let result = permission::persist_permission_mode(&mode, dest, &self.permission_paths())
                    .await.map(|_| ()).map_err(|e| e.to_string());
                self.emit_settings_result(sink, result).await;
            }

            ClientCommand::UpdateWorkspaceDirectories { destination, add, remove } => {
                let dest = match permission_destination(destination) {
                    Ok(d) => d,
                    Err(message) => {
                        sink.emit(ClientEvent::SettingsOperationFailed { message }).await;
                        return;
                    }
                };
                let paths = self.permission_paths();
                let result = async {
                    if !add.is_empty() {
                        permission::persist_workspace_directories(&add, true, dest, &paths)
                            .await.map_err(|e| e.to_string())?;
                    }
                    if !remove.is_empty() {
                        permission::persist_workspace_directories(&remove, false, dest, &paths)
                            .await.map_err(|e| e.to_string())?;
                    }
                    Ok::<(), String>(())
                }.await;
                self.emit_settings_result(sink, result).await;
            }
```

辅助函数（放 `settings_bridge.rs`）：

```rust
/// 把可写的线上层映射到 permission 的持久化目标。
pub fn permission_destination(
    dest: SettingsDestinationDto,
) -> Result<permission::result::PermissionUpdateDestination, String> {
    use permission::result::PermissionUpdateDestination as D;
    match dest {
        SettingsDestinationDto::User => Ok(D::UserSettings),
        SettingsDestinationDto::Project => Ok(D::ProjectSettings),
        SettingsDestinationDto::Local => Ok(D::LocalSettings),
        other => Err(format!(
            "settings destination {other:?} is not writable; writable destinations are user, project, local"
        ).to_lowercase()),
    }
}
```

`emit_settings_result` 放 router 的 impl 内：

```rust
    async fn emit_settings_result(&self, sink: &impl EventSink, result: Result<(), String>) {
        match result {
            Ok(()) => {
                let snapshot = crate::settings_bridge::build_snapshot(
                    &self.settings_paths(),
                    self.handle.active_settings().await.unwrap_or_default(),
                );
                sink.emit(ClientEvent::SettingsSnapshot(snapshot)).await;
            }
            Err(message) => {
                sink.emit(ClientEvent::SettingsOperationFailed { message }).await;
            }
        }
    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p bridge-server 2>&1 | tee /tmp/t5.log; grep -E "^test result|failures:" /tmp/t5.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/client-protocol lingxi-code/apps/bridge-server
git commit -m "Route permission edits through the writer that already owns them"
```

---

## Task 6: MCP 三命令

**Files:**
- Modify: `lingxi-code/client-protocol/src/commands.rs`、`events.rs`
- Modify: `lingxi-code/apps/bridge-server/src/router.rs`
- Create: `lingxi-code/apps/bridge-server/src/mcp_bridge.rs`
- Test: `lingxi-code/apps/bridge-server/src/mcp_bridge.rs`

**Interfaces:**
- Consumes: `mcp::{parse_mcp_json_string, ConfigScope, McpServerConfig}`
- Produces:
  - `ClientCommand::{ListMcpServers, UpsertMcpServer { scope, name, config }, RemoveMcpServer { scope, name }}`
  - `ClientEvent::McpServerListing(McpServerListingDto)`
  - `McpScopeDto { User, Local, Project }`（只有这三个可写）

> **实施前置**：spec §0.9 记录了一个未确认项 —— MCP 审批字段（`enableAllProjectMcpServers` / `enabledMcpjsonServers` / `disabledMcpjsonServers`）已被迁移到 `settings.local.json`，但 `migrate_mcp_servers.rs` 的注释写明「No Rust reader consumes these settings keys yet」。**动手前先定位实际读者**：
> `cd lingxi-code && grep -rn "enabledMcpjsonServers\|enableAllProjectMcpServers" --include="*.rs" . --exclude-dir=target`
> 若读者仍读 `~/.lingxi.json`，本任务只做 server 定义的增删查，审批开关留到读者统一后再做，并在计划里记一行。

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_scope_servers_round_trip_through_dot_mcp_json() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_path_buf();
        upsert_server(
            &project,
            McpScopeDto::Project,
            "linear",
            serde_json::json!({ "command": "npx", "args": ["-y", "linear-mcp"] }),
        ).unwrap();

        let raw = std::fs::read_to_string(project.join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            parsed["mcpServers"]["linear"]["command"], "npx",
            "a project-scope server must land in <project>/.mcp.json under mcpServers"
        );

        remove_server(&project, McpScopeDto::Project, "linear").unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(project.join(".mcp.json")).unwrap()).unwrap();
        assert!(
            after["mcpServers"].get("linear").is_none(),
            "removal must delete the entry, got: {after}"
        );
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p bridge-server mcp_bridge 2>&1 | tee /tmp/t6.log`
Expected: 编译失败，`cannot find function \`upsert_server\``

- [ ] **Step 3: 最小实现**

```rust
//! MCP server 定义的读写。**不复用设置的层机制** —— MCP 有自己的三处存储：
//! User → `~/.lingxi.json` 顶层 `mcpServers`；
//! Local → `~/.lingxi.json` 的 `projects[<cwd>]`；
//! Project → `<project>/.mcp.json`。

use std::path::Path;
use serde_json::{Map, Value};

pub use client_protocol::McpScopeDto;

fn read_json_object(path: &Path) -> Result<Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Ok(Map::new()),
        Ok(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|e| format!("{} is not valid JSON: {e}; not overwriting", path.display()))?
            .as_object().cloned()
            .ok_or_else(|| format!("{} is not a JSON object; not overwriting", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(format!("failed to read {}: {e}", path.display())),
    }
}

fn write_json_object(path: &Path, map: Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|e| format!("failed to serialize {}: {e}", path.display()))?;
    permission::mark_internal_write(path);
    std::fs::write(path, text + "\n").map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// 新增或替换一个 server 定义。
pub fn upsert_server(
    project_dir: &Path,
    scope: McpScopeDto,
    name: &str,
    config: Value,
) -> Result<(), String> {
    let path = project_dir.join(".mcp.json");
    match scope {
        McpScopeDto::Project => {
            let mut root = read_json_object(&path)?;
            let servers = root.entry("mcpServers".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            servers.as_object_mut()
                .ok_or_else(|| "mcpServers is not an object; not overwriting".to_string())?
                .insert(name.to_string(), config);
            write_json_object(&path, root)
        }
        McpScopeDto::User | McpScopeDto::Local => Err(
            "user and local MCP scopes live in ~/.lingxi.json and are wired in a later step"
                .to_string()
        ),
    }
}

/// 删除一个 server 定义。
pub fn remove_server(project_dir: &Path, scope: McpScopeDto, name: &str) -> Result<(), String> {
    let path = project_dir.join(".mcp.json");
    match scope {
        McpScopeDto::Project => {
            let mut root = read_json_object(&path)?;
            if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
                servers.remove(name);
            }
            write_json_object(&path, root)
        }
        McpScopeDto::User | McpScopeDto::Local => Err(
            "user and local MCP scopes live in ~/.lingxi.json and are wired in a later step"
                .to_string()
        ),
    }
}
```

> User / Local 域的 `~/.lingxi.json` 读写在本任务的第二轮补齐（同样的 `read_json_object` / `write_json_object`，目标是 `global_config` 路径与 `projects[<key>]` 子对象）。先让 Project 域端到端跑通，再补另外两个域，各自配一条与上面同形的往返测试。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p bridge-server mcp_bridge 2>&1 | tee /tmp/t6.log; grep -E "^test result|failures:" /tmp/t6.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/bridge-server lingxi-code/client-protocol
git commit -m "Read and write MCP servers where they actually live"
```

---

## Task 7: Skills 两命令

**Files:**
- Modify: `lingxi-code/client-protocol/src/commands.rs`、`events.rs`
- Modify: `lingxi-code/apps/bridge-server/src/router.rs`
- Test: `lingxi-code/apps/bridge-server/tests/router_test.rs`

**Interfaces:**
- Consumes: `skill_api::registry`（列举）、`commands_core::reload_skills`
- Produces:
  - `ClientCommand::{ListSkills, ReloadSkills}`
  - `ClientEvent::SkillListing(SkillListingDto)`
  - `SkillListingDto { skills: Vec<SkillEntryDto> }`、`SkillEntryDto { name, source_dir, plugin: Option<String> }`

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn list_skills_reports_name_and_source_for_each_discovered_skill() {
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join(branding::DOT_DIR).join("skills").join("greet");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::write(
        skills.join("SKILL.md"),
        "---\nname: greet\ndescription: says hello\n---\n\nhello\n",
    ).unwrap();

    let harness = RouterHarness::with_project(dir.path()).await;
    let events = harness.send_and_collect(ClientCommand::ListSkills).await;

    let listing = events.iter().find_map(|e| match e {
        ClientEvent::SkillListing(l) => Some(l),
        _ => None,
    }).expect("ListSkills must emit a SkillListing event");

    let entry = listing.skills.iter().find(|s| s.name == "greet")
        .expect("the discovered skill must be listed by name");
    assert!(
        entry.source_dir.contains("skills/greet"),
        "the entry must name where it was found, got: {}",
        entry.source_dir
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p bridge-server list_skills 2>&1 | tee /tmp/t7.log`
Expected: 编译失败，`no variant named \`ListSkills\``

- [ ] **Step 3: 最小实现**

`client-protocol`：

```rust
/// 一个被发现的 skill。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct SkillEntryDto {
    pub name: String,
    pub source_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
}

/// 已发现 skills 的清单。skills 是目录发现制，没有逐个的开关。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct SkillListingDto { pub skills: Vec<SkillEntryDto> }
```

`ClientCommand` 加 `ListSkills` 与 `ReloadSkills`；`ClientEvent` 加 `SkillListing(SkillListingDto)`。

`router.rs`：

```rust
            ClientCommand::ListSkills => {
                let skills = self.handle.list_skills().await.unwrap_or_default();
                sink.emit(ClientEvent::SkillListing(SkillListingDto { skills })).await;
            }
            ClientCommand::ReloadSkills => {
                match self.handle.reload_skills().await {
                    Ok(()) => {
                        let skills = self.handle.list_skills().await.unwrap_or_default();
                        sink.emit(ClientEvent::SkillListing(SkillListingDto { skills })).await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::SettingsOperationFailed {
                            message: format!("reload_skills failed: {e}"),
                        }).await;
                    }
                }
            }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p bridge-server 2>&1 | tee /tmp/t7.log; grep -E "^test result|failures:" /tmp/t7.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/client-protocol lingxi-code/apps/bridge-server
git commit -m "List and reload skills over the protocol"
```

---

## Task 8: 阶段 1-2 的全量门

**Files:** 无改动，纯验证

- [ ] **Step 1: 全工作区构建**

Run: `cd lingxi-code && cargo build --workspace --all-features 2>&1 | tee /tmp/build.log; tail -5 /tmp/build.log`
Expected: 成功。`--all-features` 是必须的——`engine-mobile` 的部分模块是 `#[cfg(feature = "uniffi")]`，默认构建对它们是盲的。

> 若出现形状对不上 diff 的 `could not compile` / `failed to write query cache`，先 `df -h`：磁盘满会伪装成编译错误。

- [ ] **Step 2: 协议快照与版本门**

Run: `cd lingxi-code && cargo test -p client-protocol 2>&1 | tee /tmp/proto.log; grep -E "^test result|failures:" /tmp/proto.log`
Expected: PASS，且 `CLIENT_PROTOCOL_VERSION` 仍为 `8.0.0`，`snapshots/blessed_major.txt` 未被修改（`git diff --stat lingxi-code/client-protocol/snapshots/` 应为空）

- [ ] **Step 3: 提交（若无改动则跳过）**

---

# 阶段 3 —— 客户端管道

## Task 9: `AllowedClientCommand` 单一来源

**Files:**
- Create: `clients/electron/src/shared/clientCommands.ts`
- Modify: `clients/electron/src/preload/index.ts`、`clients/electron/src/renderer/bridge/lingxi.d.ts`
- Test: `clients/electron/test/client-commands.test.ts`

**Interfaces:**
- Produces: `export type AllowedClientCommand`（唯一定义）

> 修一个既有缺陷：这份清单现在有两份拷贝且已经漂了 —— `preload/index.ts` 缺 `get_conversation_controls` / `set_reasoning_selection` / `set_fast_mode`，`lingxi.d.ts` 有。

- [ ] **Step 1: 写失败测试**

```typescript
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '..');

test('the allowed-command list has exactly one definition', () => {
  const preload = readFileSync(join(root, 'src/preload/index.ts'), 'utf8');
  const rendererTypes = readFileSync(join(root, 'src/renderer/bridge/lingxi.d.ts'), 'utf8');
  const declarations = [preload, rendererTypes].filter((source) =>
    /export type AllowedClientCommand\s*=/.test(source));
  assert.equal(
    declarations.length, 0,
    'AllowedClientCommand must be declared only in src/shared/clientCommands.ts; ' +
    'two copies drifted once already and will drift again',
  );
  const shared = readFileSync(join(root, 'src/shared/clientCommands.ts'), 'utf8');
  assert.match(shared, /export type AllowedClientCommand\s*=/);
});

test('the guard can actually fail', () => {
  const seeded = 'export type AllowedClientCommand = never;';
  assert.ok(
    /export type AllowedClientCommand\s*=/.test(seeded),
    'if this regex does not match a known sample, the guard above proves nothing',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e1.log; grep -E "^# (pass|fail)|not ok" /tmp/e1.log`
Expected: FAIL，`src/shared/clientCommands.ts` 不存在

- [ ] **Step 3: 最小实现**

`src/shared/clientCommands.ts`：

```typescript
import type { ClientCommand } from '@lingxi/bridge-client';

/** 主进程接受的、有界的桌面命令集。preload 与渲染进程共用这一份定义。 */
export type AllowedClientCommand =
  | Extract<ClientCommand, {
      type: 'set_model' | 'list_models' | 'new_session' | 'resume_session' | 'list_sessions' |
        'task_list' | 'task_output' | 'task_stop' | 'set_permission_mode' | 'run_slash_command' |
        'get_conversation_controls' | 'set_reasoning_selection' | 'set_fast_mode' |
        'get_settings' | 'update_settings' | 'update_permission_rules' |
        'set_default_permission_mode' | 'update_workspace_directories' |
        'list_mcp_servers' | 'upsert_mcp_server' | 'remove_mcp_server' |
        'list_skills' | 'reload_skills';
    }>
  | { type: 'refresh_listings'; which: Array<{ type: 'status' | 'doctor' | 'slash_commands' }> };
```

`preload/index.ts` 与 `lingxi.d.ts` 删掉各自的本地定义，改为
`import type { AllowedClientCommand } from '../shared/clientCommands.js';`（`lingxi.d.ts` 用相对路径 `../../shared/clientCommands.js`）并 re-export。

- [ ] **Step 4: 运行测试与类型检查确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e1.log; grep -E "^# (pass|fail)" /tmp/e1.log && npm run typecheck`
Expected: 全 PASS，typecheck 无错

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/shared/clientCommands.ts clients/electron/src/preload/index.ts clients/electron/src/renderer/bridge/lingxi.d.ts clients/electron/test/client-commands.test.ts
git commit -m "Give the allowed-command list one home"
```

---

## Task 10: 主进程 IPC 转发

**Files:**
- Modify: `clients/electron/src/main/host.ts`、`clients/electron/src/preload/index.ts`、`clients/electron/src/renderer/bridge/lingxi.d.ts`
- Test: `clients/electron/test/host.test.ts`

**Interfaces:**
- Consumes: Task 9 的 `AllowedClientCommand`
- Produces: `window.lingxi.command(sessionId, { type: 'get_settings' })` 等新命令可通过主进程的白名单校验

- [ ] **Step 1: 写失败测试**

```typescript
test('the settings commands pass the main-process allowlist', async () => {
  const { host, sent } = await hostWithOpenSession();
  await host.invoke(CH_COMMAND, sessionId, { type: 'get_settings' });
  await host.invoke(CH_COMMAND, sessionId, {
    type: 'update_settings',
    destination: 'user',
    patch: [['outputStyle', 'terse']],
  });
  assert.deepEqual(
    sent.map((c) => c.type),
    ['get_settings', 'update_settings'],
    'both settings commands must reach the engine',
  );
});

test('an unlisted command is still rejected', async () => {
  const { host } = await hostWithOpenSession();
  await assert.rejects(
    () => host.invoke(CH_COMMAND, sessionId, { type: 'login' }),
    /not allowed|invalid command/i,
    'the allowlist must still reject commands outside the desktop surface',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e2.log; grep -E "not ok" /tmp/e2.log`
Expected: FAIL，`get_settings` 被白名单拒绝

- [ ] **Step 3: 最小实现**

在 `host.ts` 的命令白名单常量中加入 11 个新 `type` 字符串（与 Task 9 的联合类型逐字一致）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e2.log; grep -E "^# (pass|fail)" /tmp/e2.log`
Expected: 两条都 PASS —— 正向命令通过，反向命令仍被拒

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/main/host.ts clients/electron/test/host.test.ts
git commit -m "Let the settings commands through the desktop allowlist"
```

---

## Task 11: `theme` 支持跟随系统

**Files:**
- Modify: `clients/electron/src/main/host-utils.ts:59,75,132,189`、`clients/electron/src/main/settings.ts`、`clients/electron/src/renderer/theme/tokens.ts:149`
- Test: `clients/electron/test/host-utils.test.ts`

**Interfaces:**
- Produces: `PersistedSettings['theme']` 与 `PublicSettings['theme']` 为 `'dark' | 'light' | 'system'`；`ThemeMode` 不变（仍是 `'dark' | 'light'`，因为渲染只有两种实际配色）

- [ ] **Step 1: 写失败测试**

```typescript
test("parseSettings keeps 'system' and still rejects garbage", () => {
  assert.equal(parseSettings({ version: 1, theme: 'system', projects: [] }).theme, 'system');
  assert.equal(parseSettings({ version: 1, theme: 'dark', projects: [] }).theme, 'dark');
  assert.equal(
    parseSettings({ version: 1, theme: 'chartreuse', projects: [] }).theme, undefined,
    'an unknown theme must still be dropped, not passed through',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e3.log; grep -E "not ok" /tmp/e3.log`
Expected: FAIL —— `'system'` 被当成未知值丢弃

- [ ] **Step 3: 最小实现**

`host-utils.ts:132`：

```typescript
  settings.theme =
    value['theme'] === 'dark' || value['theme'] === 'light' || value['theme'] === 'system'
      ? value['theme']
      : undefined;
```

同文件 `:59` 与 `:75` 的类型标注改为 `theme?: 'dark' | 'light' | 'system';`。

`settings.ts` 的 `update()` 校验：

```typescript
    if ('theme' in patch) {
      if (patch.theme !== 'dark' && patch.theme !== 'light' && patch.theme !== 'system') {
        throw new Error('invalid theme');
      }
      this.settings.theme = patch.theme;
    }
```

渲染进程在 `App.tsx` 中把 `'system'` 解析为实际配色（`window.matchMedia('(prefers-color-scheme: dark)')`），并监听其 `change` 事件。

- [ ] **Step 4: 运行测试与类型检查确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e3.log; grep -E "^# (pass|fail)" /tmp/e3.log && npm run typecheck`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/main clients/electron/src/renderer/theme/tokens.ts clients/electron/test/host-utils.test.ts
git commit -m "Let the theme follow the system"
```

---

# 阶段 4 —— 设置界面

## Task 12: 行原语与卡片

**Files:**
- Create: `clients/electron/src/renderer/components/settings/rows.tsx`
- Test: `clients/electron/test/settings-rows.test.ts`

**Interfaces:**
- Produces：
  - `Card({ title?, children })`
  - `Row({ title, desc?, badge?, align?, children })`
  - `ProvenanceBadge({ destination })` —— `'device'|'user'|'project'|'local'|'managed'` → 中文徽标
  - `OverriddenNotice({ editingLayer, effectiveLayer, onJump })`
  - `LockedBadge()`

- [ ] **Step 1: 写失败测试**

```typescript
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { provenanceLabel } from '../src/renderer/components/settings/rows';

test('every provenance value has a label', () => {
  for (const d of ['device', 'user', 'project', 'local', 'managed'] as const) {
    const label = provenanceLabel(d);
    assert.ok(label && label.length > 0, `${d} must have a badge label`);
  }
});

test('managed is labelled as policy-locked, not as an editable layer', () => {
  assert.notEqual(
    provenanceLabel('managed'), provenanceLabel('user'),
    'a policy-locked value must not read like a user-editable one',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e4.log; grep -E "not ok" /tmp/e4.log`
Expected: FAIL，模块不存在

- [ ] **Step 3: 最小实现**

```tsx
import type { ReactNode } from 'react';
import { useT } from '../../theme/ThemeContext';

export type Provenance = 'device' | 'user' | 'project' | 'local' | 'managed';

const PROVENANCE_LABELS: Record<Provenance, string> = {
  device: '设备',
  user: '用户',
  project: '项目',
  local: '本地',
  managed: '策略',
};

export function provenanceLabel(d: Provenance): string {
  return PROVENANCE_LABELS[d];
}

/** 一组设置行的容器：圆角卡片，行以卡内分隔线相隔。 */
export function Card({ title, children }: { title?: string; children: ReactNode }) {
  const t = useT();
  return (
    <div style={{ marginBottom: 28 }}>
      {title && (
        <div style={{ fontSize: 13, fontWeight: 600, color: t.text2, marginBottom: 10 }}>{title}</div>
      )}
      <div style={{
        border: `0.5px solid ${t.border}`, borderRadius: 12,
        background: t.surface, overflow: 'hidden',
      }}>
        {children}
      </div>
    </div>
  );
}

export function Row({
  title, desc, badge, align = 'start', children,
}: {
  title: ReactNode; desc?: ReactNode; badge?: ReactNode;
  align?: 'start' | 'center'; children: ReactNode;
}) {
  const t = useT();
  return (
    <div style={{
      display: 'flex', gap: 24, padding: '16px 18px',
      alignItems: align === 'center' ? 'center' : 'flex-start',
      borderTop: `0.5px solid ${t.border}`,
    }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{title}</span>
          {badge}
        </div>
        {desc && (
          <div style={{ fontSize: 12.5, color: t.text3, marginTop: 4, lineHeight: 1.5, maxWidth: 620 }}>
            {desc}
          </div>
        )}
      </div>
      <div style={{ flexShrink: 0 }}>{children}</div>
    </div>
  );
}

export function ProvenanceBadge({ destination }: { destination: Provenance }) {
  const t = useT();
  return (
    <span style={{
      fontSize: 10.5, padding: '2px 7px', borderRadius: 5,
      background: t.surfaceHover, color: t.text3, fontWeight: 600,
    }}>
      {provenanceLabel(destination)}
    </span>
  );
}

export function LockedBadge() {
  return <ProvenanceBadge destination="managed" />;
}

/** 编辑层已写入、但生效值来自更高层时的一等状态。 */
export function OverriddenNotice({
  editingLayer, effectiveLayer, onJump,
}: { editingLayer: Provenance; effectiveLayer: Provenance; onJump(): void }) {
  const t = useT();
  return (
    <div style={{ marginTop: 6, fontSize: 12, color: t.warn }}>
      已写入{provenanceLabel(editingLayer)}层；当前生效值来自{provenanceLabel(effectiveLayer)}层。
      <button
        type="button"
        onClick={onJump}
        style={{
          marginLeft: 6, background: 'none', border: 'none', padding: 0,
          color: t.link ?? t.accent, cursor: 'pointer', font: 'inherit', textDecoration: 'underline',
        }}
      >
        前往该层
      </button>
    </div>
  );
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e4.log; grep -E "^# (pass|fail)" /tmp/e4.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/rows.tsx clients/electron/test/settings-rows.test.ts
git commit -m "Give settings rows a card and a home layer"
```

---

## Task 13: 行状态机 `useEngineSettings`

**Files:**
- Create: `clients/electron/src/renderer/components/settings/useEngineSettings.ts`
- Test: `clients/electron/test/engine-settings-state.test.ts`

**Interfaces:**
- Consumes: Task 3 的 `SettingsSnapshotDto`、Task 12 的 `Provenance`
- Produces:
  - `export type RowState = { kind: 'unset' } | { kind: 'set-here' } | { kind: 'overridden'; by: Provenance } | { kind: 'inherited'; from: Provenance } | { kind: 'locked' } | { kind: 'layer-broken'; error: string }`
  - `export function rowState(snapshot: SettingsSnapshotDto, key: string, editingLayer: Provenance): RowState`
  - `export function pendingKeys(snapshot: SettingsSnapshotDto): string[]`

- [ ] **Step 1: 写失败测试 —— 六态穷举 + 待应用的 A/B**

```typescript
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rowState, pendingKeys } from '../src/renderer/components/settings/useEngineSettings';

const files = [
  { destination: 'user', path: '/u', exists: true, writable: true },
  { destination: 'project', path: '/p', exists: true, writable: true },
  { destination: 'local', path: '/l', exists: true, writable: true },
] as const;

function snap(over: Record<string, unknown> = {}) {
  return {
    files: [...files], effective: {}, active: {}, provenance: {}, locked: [], ...over,
  } as never;
}

test('unset when no layer defines the key', () => {
  assert.deepEqual(rowState(snap(), 'outputStyle', 'user'), { kind: 'unset' });
});

test('set-here when the editing layer supplies the effective value', () => {
  const s = snap({ effective: { outputStyle: 'terse' }, provenance: { outputStyle: 'user' } });
  assert.deepEqual(rowState(s, 'outputStyle', 'user'), { kind: 'set-here' });
});

test('overridden when a higher layer wins over the editing layer', () => {
  const s = snap({ effective: { outputStyle: 'loud' }, provenance: { outputStyle: 'project' } });
  assert.deepEqual(
    rowState(s, 'outputStyle', 'user'), { kind: 'overridden', by: 'project' },
    'editing user while project wins is the common case, not an edge case',
  );
});

test('inherited when the editing layer is higher than the winner', () => {
  const s = snap({ effective: { outputStyle: 'terse' }, provenance: { outputStyle: 'user' } });
  assert.deepEqual(rowState(s, 'outputStyle', 'local'), { kind: 'inherited', from: 'user' });
});

test('locked when the managed layer pins the key', () => {
  const s = snap({ effective: { outputStyle: 'x' }, provenance: { outputStyle: 'managed' }, locked: ['outputStyle'] });
  assert.deepEqual(rowState(s, 'outputStyle', 'user'), { kind: 'locked' });
});

test('layer-broken when the editing layer failed to parse', () => {
  const s = snap({
    files: [{ destination: 'user', path: '/u', exists: true, writable: false, parse_error: 'bad json' },
            files[1], files[2]],
  });
  assert.deepEqual(rowState(s, 'outputStyle', 'user'), { kind: 'layer-broken', error: 'bad json' });
});

test('pending names only the keys the running session actually differs on', () => {
  const differing = snap({ effective: { outputStyle: 'terse' }, active: { outputStyle: 'loud' } });
  assert.deepEqual(
    pendingKeys(differing), ['outputStyle'],
    'a key on disk that differs from what the session loaded is pending',
  );
});

test('pending is empty when disk and session agree (the A/B for the test above)', () => {
  const same = snap({ effective: { outputStyle: 'terse' }, active: { outputStyle: 'terse' } });
  assert.deepEqual(
    pendingKeys(same), [],
    'if this also reported pending, the test above would prove nothing',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e5.log; grep -E "not ok" /tmp/e5.log`
Expected: FAIL，模块不存在

- [ ] **Step 3: 最小实现**

```typescript
import type { Provenance } from './rows';

/** 层优先级，高者覆盖低者。与引擎的 env → managed → cli → local → project → user 一致。 */
const LAYER_RANK: Record<string, number> = {
  defaults: 0, user: 1, project: 2, local: 3, cli: 4, managed: 5, env: 6,
};

export interface SettingsFile {
  destination: string; path: string; exists: boolean; writable: boolean; parse_error?: string;
}
export interface SettingsSnapshot {
  files: SettingsFile[];
  effective: Record<string, unknown>;
  active: Record<string, unknown>;
  provenance: Record<string, string>;
  locked: string[];
}

export type RowState =
  | { kind: 'unset' }
  | { kind: 'set-here' }
  | { kind: 'overridden'; by: Provenance }
  | { kind: 'inherited'; from: Provenance }
  | { kind: 'locked' }
  | { kind: 'layer-broken'; error: string };

export function rowState(
  snapshot: SettingsSnapshot, key: string, editingLayer: Provenance,
): RowState {
  const file = snapshot.files.find((f) => f.destination === editingLayer);
  if (file?.parse_error) return { kind: 'layer-broken', error: file.parse_error };
  if (snapshot.locked.includes(key)) return { kind: 'locked' };

  const winner = snapshot.provenance[key];
  if (winner === undefined) return { kind: 'unset' };
  if (winner === editingLayer) return { kind: 'set-here' };

  const winnerRank = LAYER_RANK[winner] ?? 0;
  const editingRank = LAYER_RANK[editingLayer] ?? 0;
  return winnerRank > editingRank
    ? { kind: 'overridden', by: winner as Provenance }
    : { kind: 'inherited', from: winner as Provenance };
}

/**
 * 盘上生效值与运行中会话实际加载值的差集。
 *
 * 这是「待应用」的唯一判据 —— 不是前端记账「用户刚点过保存」，那种记账在
 * 引擎重启后、或用户在终端改了文件后会撒谎。
 */
export function pendingKeys(snapshot: SettingsSnapshot): string[] {
  const keys = new Set([...Object.keys(snapshot.effective), ...Object.keys(snapshot.active)]);
  return [...keys].filter(
    (k) => JSON.stringify(snapshot.effective[k]) !== JSON.stringify(snapshot.active[k]),
  ).sort();
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e5.log; grep -E "^# (pass|fail)" /tmp/e5.log`
Expected: 八条全 PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/useEngineSettings.ts clients/electron/test/engine-settings-state.test.ts
git commit -m "Derive each settings row's state from the engine's snapshot"
```

---

## Task 14: 导航声明与搜索

**Files:**
- Create: `clients/electron/src/renderer/components/settings/nav.ts`
- Test: `clients/electron/test/settings-nav.test.ts`

**Interfaces:**
- Produces:
  - `export interface NavPage { id, label, group, icon, needsEngine: boolean, layered: boolean, searchKeys: string[] }`
  - `export const SETTINGS_NAV: NavPage[]`（15 页）
  - `export function searchNav(query: string): NavPage[]`

- [ ] **Step 1: 写失败测试**

```typescript
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SETTINGS_NAV, searchNav } from '../src/renderer/components/settings/nav';

test('the nav declares all fifteen pages across four groups', () => {
  assert.equal(SETTINGS_NAV.length, 15);
  assert.deepEqual(
    [...new Set(SETTINGS_NAV.map((p) => p.group))],
    ['个人', '模型与服务', '编码', '高级'],
  );
});

test('only the coding group is layered, and MCP opts out', () => {
  for (const page of SETTINGS_NAV) {
    if (page.group !== '编码') {
      assert.equal(page.layered, false, `${page.id} is outside 编码 and must not be layered`);
    }
  }
  assert.equal(
    SETTINGS_NAV.find((p) => p.id === 'mcp')?.layered, false,
    'MCP has its own three-scope storage and must not reuse the settings layer switcher',
  );
});

test('client-owned pages do not need the engine', () => {
  for (const id of ['appearance', 'projects', 'diagnostics', 'about', 'voice']) {
    assert.equal(
      SETTINGS_NAV.find((p) => p.id === id)?.needsEngine, false,
      `${id} lives in the client and must stay usable with no engine`,
    );
  }
});

test('search finds a page by a settings key it owns', () => {
  const hits = searchNav('outputStyle');
  assert.ok(
    hits.some((p) => p.id === 'tools-agent'),
    'searching a settings key must reach the page that owns it',
  );
});

test('search finds nothing for a key no page declares', () => {
  assert.deepEqual(
    searchNav('zzzzz-not-a-setting'), [],
    'if this returned hits, the search test above would prove nothing',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e6.log; grep -E "not ok" /tmp/e6.log`
Expected: FAIL，模块不存在

- [ ] **Step 3: 最小实现**

```typescript
export interface NavPage {
  id: string;
  label: string;
  group: '个人' | '模型与服务' | '编码' | '高级';
  icon: string;
  /** 该页是否依赖运行中的引擎。false 的页在无引擎时仍完全可用。 */
  needsEngine: boolean;
  /** 该页是否受顶部的用户/项目/本地层切换器支配。 */
  layered: boolean;
  /** 该页拥有的设置键与关键词，供搜索建索引。 */
  searchKeys: string[];
}

export const SETTINGS_NAV: NavPage[] = [
  { id: 'general', label: '通用', group: '个人', icon: 'cog', needsEngine: false, layered: false, searchKeys: ['general', '通用'] },
  { id: 'appearance', label: '外观', group: '个人', icon: 'sun', needsEngine: false, layered: false, searchKeys: ['theme', '主题', '外观', 'appearance', 'dark', 'light', 'system'] },
  { id: 'voice', label: '语音', group: '个人', icon: 'mic', needsEngine: false, layered: false, searchKeys: ['voice', '语音', 'tts', 'stt', 'rate', 'autoPlayReplies'] },
  { id: 'projects', label: '项目与信任', group: '个人', icon: 'folder', needsEngine: false, layered: false, searchKeys: ['project', '项目', 'trust', '信任', 'pinned'] },

  { id: 'provider-credentials', label: 'Provider 凭据', group: '模型与服务', icon: 'key', needsEngine: true, layered: false, searchKeys: ['provider', 'credential', '凭据', 'apiBaseUrl', 'keychain'] },
  { id: 'custom-providers', label: '自定义 Provider 与路由', group: '模型与服务', icon: 'plug', needsEngine: true, layered: true, searchKeys: ['providers', 'routing', 'baseUrl', 'apiKeyEnv', 'models', 'aliases', 'fallback', 'retry'] },

  { id: 'permissions', label: '权限', group: '编码', icon: 'shield', needsEngine: true, layered: true, searchKeys: ['permissions', '权限', 'allow', 'deny', 'ask', 'additionalDirectories', 'bypassPermissions'] },
  { id: 'tools-agent', label: '工具与 Agent 行为', group: '编码', icon: 'sliders', needsEngine: true, layered: true, searchKeys: ['enabledTools', 'outputStyle', 'modelOverrides', 'alwaysThinkingEnabled', 'showThinkingSummaries', 'visionDelegationEnabled', 'disableAllHooks', 'skipWebFetchPreflight'] },
  { id: 'skills', label: 'Skills', group: '编码', icon: 'sparkle', needsEngine: true, layered: true, searchKeys: ['skills', 'syncClaudeAiSkills', 'reload'] },
  { id: 'mcp', label: 'MCP 服务器', group: '编码', icon: 'server', needsEngine: true, layered: false, searchKeys: ['mcp', 'mcpServers', 'server', '服务器'] },
  { id: 'hooks', label: 'Hooks', group: '编码', icon: 'anchor', needsEngine: true, layered: true, searchKeys: ['hooks', 'ConfigChange', 'PreToolUse', 'PostToolUse'] },
  { id: 'plugins', label: '插件与市场', group: '编码', icon: 'puzzle', needsEngine: true, layered: true, searchKeys: ['enabledPlugins', 'pluginConfigs', 'marketplace', '插件', '市场'] },

  { id: 'raw-json', label: '原始 JSON', group: '高级', icon: 'braces', needsEngine: true, layered: true, searchKeys: ['json', 'raw', '原始', 'settings.json'] },
  { id: 'diagnostics', label: '诊断', group: '高级', icon: 'activity', needsEngine: false, layered: false, searchKeys: ['diagnostics', '诊断', 'log', '日志', 'export'] },
  { id: 'about', label: '关于', group: '高级', icon: 'info', needsEngine: false, layered: false, searchKeys: ['about', '关于', 'version', '版本'] },
];

/** 按标签与设置键做大小写不敏感的子串匹配。 */
export function searchNav(query: string): NavPage[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  return SETTINGS_NAV.filter((page) =>
    page.label.toLowerCase().includes(needle) ||
    page.id.includes(needle) ||
    page.searchKeys.some((key) => key.toLowerCase().includes(needle)));
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e6.log; grep -E "^# (pass|fail)" /tmp/e6.log`
Expected: 五条全 PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/nav.ts clients/electron/test/settings-nav.test.ts
git commit -m "Declare the settings map as data so search can read it"
```

---

## Task 15: 设置外壳

**Files:**
- Create: `clients/electron/src/renderer/components/settings/SettingsScreen.tsx`
- Test: `clients/electron/test/settings-screen.test.mjs`（交互测试，沿用 `settings-background-interaction.test.mjs` 的形式）

**Interfaces:**
- Consumes: Task 12 `rows.tsx`、Task 13 `useEngineSettings.ts`、Task 14 `nav.ts`
- Produces: `SettingsScreen({ bridge, theme, onTheme, initialProviderId, pendingModelReference, onClose })` —— 与既有 `BetaSettingsProps` 同形，好让 `App.tsx` 平移

- [ ] **Step 1: 写失败测试**

```javascript
test('the layer switcher appears only on layered pages', () => {
  const screen = mountSettings({ page: 'permissions' });
  assert.ok(screen.querySelector('[data-testid="layer-switcher"]'),
    'permissions is layered and must offer user/project/local');

  screen.selectPage('diagnostics');
  assert.equal(screen.querySelector('[data-testid="layer-switcher"]'), null,
    'diagnostics is client-owned and must not show a layer switcher');
});

test('project and local tabs are disabled with no project open', () => {
  const screen = mountSettings({ page: 'permissions', project: null });
  assert.equal(screen.querySelector('[data-layer="project"]').disabled, true);
  assert.equal(screen.querySelector('[data-layer="user"]').disabled, false,
    'the user layer needs no project and must stay editable');
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e7.log; grep -E "not ok" /tmp/e7.log`
Expected: FAIL，组件不存在

- [ ] **Step 3: 最小实现**

`SettingsScreen.tsx` 组成：全屏容器（`position: absolute; inset: 0`）、左侧 240px 导航（顶部搜索框 + 按 `group` 分组渲染 `SETTINGS_NAV`，带图标）、右侧内容区（页标题 + 层切换器 + 页组件）。

层切换器只在 `SETTINGS_NAV.find(p => p.id === page)?.layered` 为真时渲染；`project` / `local` 两个 tab 在无活动项目时 `disabled` 并给出「打开一个项目后可编辑」的说明。

顶部若 `pendingKeys(snapshot).length > 0`，渲染一条提示条：`重启引擎以应用（N 项）`，按钮调用 `bridge.restartBridge(sessionId)`；turn 进行中时禁用并说明原因。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e7.log; grep -E "^# (pass|fail)" /tmp/e7.log && npm run typecheck`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/SettingsScreen.tsx clients/electron/test/settings-screen.test.mjs
git commit -m "Give settings a full-page shell that knows which pages are layered"
```

---

## Task 16: 客户端组的四页

**Files:**
- Create: `clients/electron/src/renderer/components/settings/pages/{General,Appearance,Projects,Diagnostics,About}.tsx`
- Test: `clients/electron/test/settings-client-pages.test.ts`

**Interfaces:**
- Consumes: Task 12 的 `Card` / `Row`，`bridge`（`useBridge` 的返回）
- Produces: 五个页面组件，各自 `({ bridge })` 或 `({ theme, onTheme })`

- [ ] **Step 1: 写失败测试**

```typescript
test('appearance offers exactly dark, light and system', () => {
  const options = appearanceOptions();
  assert.deepEqual(options.map((o) => o.id), ['system', 'light', 'dark']);
});

test('projects lists the persisted projects and marks the active one', () => {
  const rows = projectRows({
    projects: ['/a', '/b'], activeProject: '/b',
  } as never);
  assert.deepEqual(rows.map((r) => r.path), ['/a', '/b']);
  assert.equal(rows.find((r) => r.path === '/b')?.active, true);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e8.log; grep -E "not ok" /tmp/e8.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

- `Appearance.tsx`：一张 `Card`，一行 `Row` 三段式选择（跟随系统 / 浅色 / 深色），写 `bridge.setThemePreference`。
- `Projects.tsx`：项目列表（增删、切换）、workspace trust 与指纹、置顶会话，全部走既有 IPC。
- `Diagnostics.tsx`：日志流 + 复制报告 / 导出 JSON / 刷新 / 重启引擎，逻辑从 `BetaSettings` 的 Diagnostics 段平移。
- `About.tsx`：应用 / Electron / 引擎版本号。
- `General.tsx`：本轮只承载跨页入口，不新增未实现的开关。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e8.log; grep -E "^# (pass|fail)" /tmp/e8.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/pages clients/electron/test/settings-client-pages.test.ts
git commit -m "Land the settings pages that need no engine"
```

---

## Task 17: Provider 两页

**Files:**
- Create: `clients/electron/src/renderer/components/settings/pages/{ProviderCredentials,CustomProviders}.tsx`
- Test: `clients/electron/test/settings-providers.test.ts`

**Interfaces:**
- Consumes: `bridge.{providerCredentials,setProviderCredential,clearProviderCredential}`；Task 13 的 `rowState`
- Produces: 两个页面组件

- [ ] **Step 1: 写失败测试 —— 空 models 必须在写盘前被拒**

```typescript
import { validateCustomProvider } from '../src/renderer/components/settings/pages/CustomProviders';

test('a custom provider with no models is refused before it reaches disk', () => {
  const error = validateCustomProvider({ type: 'openai', baseUrl: 'https://x', models: [] });
  assert.ok(error, 'an empty models list must be refused');
  assert.match(
    error, /models/,
    'the message must name `models` — an absent or empty list is an engine-startup error',
  );
});

test('a custom provider with a model passes (the A/B for the test above)', () => {
  assert.equal(
    validateCustomProvider({ type: 'openai', baseUrl: 'https://x', models: [{ id: 'gpt-x' }] }),
    null,
    'if this also failed, the refusal test above would prove nothing',
  );
});

test('an unsupported provider type is refused and the message lists the supported set', () => {
  const error = validateCustomProvider({ type: 'gopher', baseUrl: 'https://x', models: [{ id: 'm' }] });
  assert.match(error ?? '', /openai/);
  assert.match(error ?? '', /anthropic/);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e9.log; grep -E "not ok" /tmp/e9.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

```typescript
const SUPPORTED_PROVIDER_TYPES = [
  'openai', 'openai-responses', 'anthropic', 'gemini', 'azure-openai',
  'bedrock-claude', 'vertex-claude', 'vertex-gemini', 'foundry-claude',
] as const;

export interface CustomProviderDraft {
  type: string;
  baseUrl?: string;
  apiKeyEnv?: string;
  models: { id: string; aliases?: string[] }[];
}

/**
 * 写盘前的校验。返回 null 表示可写。
 *
 * `models` 是必填的：schema 注明「an absent or empty list is an error at engine
 * startup」，所以一次没校验的保存会让引擎下次起不来。
 */
export function validateCustomProvider(draft: CustomProviderDraft): string | null {
  if (!SUPPORTED_PROVIDER_TYPES.includes(draft.type as never)) {
    return `unsupported provider type \`${draft.type}\`; supported types are ${SUPPORTED_PROVIDER_TYPES.join(', ')}`;
  }
  if (!draft.models || draft.models.length === 0) {
    return 'this provider needs at least one entry in `models`; an empty list makes the engine fail to start';
  }
  if (draft.models.some((m) => !m.id.trim())) {
    return 'every entry in `models` needs a non-empty `id`';
  }
  return null;
}
```

`ProviderCredentials.tsx` 从 `BetaSettings` 的 Providers 段平移：凭据连接 / 替换 / 断开、Keychain 可用性、`runtimeOnly` 状态、`apiBaseUrl`、深链聚焦（`initialProviderId` / `pendingModelReference`）。

`CustomProviders.tsx`：`settings.providers` 的表格编辑 + `settings.routing` 的 aliases / fallback / retry，经 `update_settings` 写入当前编辑层。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e9.log; grep -E "^# (pass|fail)" /tmp/e9.log`
Expected: 三条全 PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/pages clients/electron/test/settings-providers.test.ts
git commit -m "Edit providers without letting an empty model list reach disk"
```

---

## Task 18: 编码组的六页

**Files:**
- Create: `clients/electron/src/renderer/components/settings/pages/{Permissions,ToolsAgent,Skills,McpServers,Hooks,Plugins}.tsx`
- Test: `clients/electron/test/settings-coding-pages.test.ts`

**Interfaces:**
- Consumes: Task 13 的 `rowState` / `pendingKeys`；Task 5/6/7 的命令
- Produces: 六个页面组件

- [ ] **Step 1: 写失败测试**

```typescript
test('permission rule edits go through the dedicated command, never the generic patch', () => {
  const sent = capturePermissionEdit({ behavior: 'allow', add: ['Bash(ls:*)'] });
  assert.equal(sent.type, 'update_permission_rules');
  assert.notEqual(
    sent.type, 'update_settings',
    'permissions has a dedicated writer; the generic patch refuses the key anyway',
  );
});

test('the bypass acceptance row is device-owned, not layered', () => {
  assert.equal(
    bypassRowProvenance(), 'device',
    'bypassPermissionsModeAccepted lives in the Electron store, so switching layers must not move it',
  );
});

test('hooks are read-only and point at the raw JSON page', () => {
  const page = hooksPageModel({ hooks: { PreToolUse: [] } } as never);
  assert.equal(page.editable, false);
  assert.equal(page.escapeHatch, 'raw-json');
});

test('the skills list is not affected by the layer switcher', () => {
  const model = skillsPageModel({ layer: 'user' } as never);
  const other = skillsPageModel({ layer: 'project' } as never);
  assert.deepEqual(
    model.skills, other.skills,
    'skills are discovered from directories; only syncClaudeAiSkills is layered',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e10.log; grep -E "not ok" /tmp/e10.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

- `Permissions.tsx`：allow / ask / deny 三张表（增删，带 pattern 提示）、默认权限模式、`additionalDirectories`、bypass 接受状态（`ProvenanceBadge destination="device"`）。全部经 Task 5 的三条命令。
- `ToolsAgent.tsx`：`enabledTools`、各 `disable_*`、`outputStyle`、`modelOverrides`、思考与视觉委派开关，经 `update_settings`。
- `Skills.tsx`：`ListSkills` 的结果表 + `syncClaudeAiSkills` 开关（唯一受层影响的一行，需在列表区标注）+ `ReloadSkills` 按钮。
- `McpServers.tsx`：页内自带的三域选择器（User / Local / Project），只读域（Dynamic / Enterprise）条目禁用并标来源。
- `Hooks.tsx`：只读罗列 + 「在 JSON 中编辑」跳转到 `raw-json` 页。
- `Plugins.tsx`：`enabledPlugins` / `pluginConfigs` / marketplaces。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e10.log; grep -E "^# (pass|fail)" /tmp/e10.log && npm run typecheck`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/pages clients/electron/test/settings-coding-pages.test.ts
git commit -m "Land the engine-settings pages on the commands that own them"
```

---

## Task 19: 原始 JSON 逃生口

**Files:**
- Create: `clients/electron/src/renderer/components/settings/pages/RawJson.tsx`
- Test: `clients/electron/test/settings-raw-json.test.ts`

**Interfaces:**
- Consumes: `update_settings`
- Produces: `RawJson.tsx`；`export function validateRawLayer(text: string): string | null`

> 这是 I1 的**唯一例外**：整层文件覆写，因此是唯一能以文本方式写 `permissions` 的路径。它仍走同一个校验与同一个 `mark_internal_write`。

- [ ] **Step 1: 写失败测试**

```typescript
test('malformed JSON is refused before it can overwrite a layer', () => {
  const error = validateRawLayer('{ "outputStyle": }');
  assert.ok(error, 'broken JSON must be refused');
  assert.match(error, /JSON/i);
});

test('a JSON array is refused — a settings layer must be an object', () => {
  assert.ok(validateRawLayer('[1,2,3]'));
});

test('valid object JSON passes (the A/B for the tests above)', () => {
  assert.equal(validateRawLayer('{ "outputStyle": "terse" }'), null);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e11.log; grep -E "not ok" /tmp/e11.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

```typescript
/** 提交前的原文校验。返回 null 表示可写。 */
export function validateRawLayer(text: string): string | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    return `not valid JSON: ${(error as Error).message}`;
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return 'a settings layer must be a JSON object';
  }
  return null;
}
```

页面本体：当前编辑层的原文编辑器（校验 + 错误提示），以及四个层文件的路径、存在性、可写性、解析状态一览。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e11.log; grep -E "^# (pass|fail)" /tmp/e11.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/pages/RawJson.tsx clients/electron/test/settings-raw-json.test.ts
git commit -m "Keep an escape hatch for every field the UI does not cover"
```

---

## Task 20: 挂载新壳、删除旧的两套、立防漏门

**Files:**
- Modify: `clients/electron/src/renderer/App.tsx`
- Modify: `clients/electron/src/renderer/components/BetaDesktop.tsx`（删 1843-2130 的 `BetaSettings`）
- Delete: `clients/electron/src/renderer/components/settings/` 的 8 个旧文件
- Test: `clients/electron/test/settings-deadcode-guard.test.ts`

**Interfaces:**
- Consumes: Task 15 的 `SettingsScreen`
- Produces: `App.tsx` 挂载 `SettingsScreen`，`SettingsRoute` / `SettingsBackground` / Composer 深链回焦不变

- [ ] **Step 1: 写失败测试 —— 防漏门必须先证明自己能红**

```typescript
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');

function hits(needle: string): string[] {
  try {
    return execFileSync('git', ['grep', '-l', '-F', needle, '--', 'clients/electron/src'],
      { cwd: root, encoding: 'utf8' }).trim().split('\n').filter(Boolean);
  } catch {
    return [];   // git grep 无命中时退出码为 1
  }
}

test('the grep guard can actually find something', () => {
  assert.ok(
    hits('SettingsScreen').length > 0,
    'if a known-present symbol returns zero hits, every zero below proves nothing',
  );
});

test('the mock settings pages are gone', () => {
  assert.deepEqual(hits('SettingsGenericPage'), []);
  assert.deepEqual(hits('SettingsBillingPage'), []);
  assert.deepEqual(hits('SettingsUsagePage'), []);
});

test('BetaSettings is gone', () => {
  assert.deepEqual(hits('BetaSettings'), []);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e12.log; grep -E "not ok" /tmp/e12.log`
Expected: 第一条 PASS（哨兵能命中），后两条 FAIL（旧代码尚在）

- [ ] **Step 3: 实施删除与挂载**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git rm clients/electron/src/renderer/components/settings/SettingsPage.tsx \
       clients/electron/src/renderer/components/settings/SettingsGeneralPage.tsx \
       clients/electron/src/renderer/components/settings/SettingsGenericPage.tsx \
       clients/electron/src/renderer/components/settings/SettingsAccountPage.tsx \
       clients/electron/src/renderer/components/settings/SettingsBillingPage.tsx \
       clients/electron/src/renderer/components/settings/SettingsUsagePage.tsx \
       clients/electron/src/renderer/components/settings/SettingsPrivacyPage.tsx \
       clients/electron/src/renderer/components/settings/SettingsCodePage.tsx
```

`primitives.tsx` 里 `Segmented` / `Toggle` / `SettingsSelect` 仍被新页面使用，**保留**；`SettingsRow` / `SectionTitle` / `CODE_THEMES` / `CodePreview` 若新页面不再引用则一并删除（以 typecheck 为准）。

`BetaDesktop.tsx`：删除 `BetaSettingsProps` 与 `BetaSettings`（1843-2130），并移除其导出。

`App.tsx`：`import { SettingsScreen } from './components/settings/SettingsScreen';`，把 `<BetaSettings …>` 换成 `<SettingsScreen …>`，props 逐字保持。

- [ ] **Step 4: 运行全部测试与类型检查确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/e12.log; grep -E "^# (pass|fail)" /tmp/e12.log && npm run typecheck`
Expected: 全 PASS，typecheck 无错

- [ ] **Step 5: 提交**

```bash
git add -A clients/electron
git commit -m "Retire the settings mock and the modal it never replaced"
```

---

## Task 21: 真机验收

**Files:** 无改动，纯验收。**这一步不可省** —— 单测与评审抓不到「装起来根本没跑对」。

- [ ] **Step 1: 构建并启动**

```bash
cd lingxi-code && cargo build -p bridge-server --release
cd ../clients/electron && npm run build && npm run dev
```

- [ ] **Step 2: 项目层写入与保真**

在**权限**页切到**项目**层，加一条 `Bash(ls:*)` 到 allow。退出应用，然后：

```bash
cat <你的项目>/.lingxi/settings.json
```

Expected: `permissions.allow` 含 `Bash(ls:*)`，且文件里原有的其他键**逐字未变**。

- [ ] **Step 3: 待应用与重启**

重启应用，确认该规则真的生效，且页顶的「重启引擎以应用」提示**已消失**。

- [ ] **Step 4: 覆盖状态可见**

在**用户**层设一个已被项目层定义的键。Expected: 该行出现「已写入用户层；当前生效值来自项目层」，而不是静默无变化。

- [ ] **Step 5: 坏文件降级**

手动把项目层文件改成坏 JSON。Expected: 该层降级只读、写入被拒、**原文件未被覆写**。

- [ ] **Step 6: watcher 不被自己的写触发**

带 `RUST_LOG=debug` 启动 bridge-server，在设置页保存一次。Expected: 日志中**没有**因这次保存而产生的 `ConfigChange` hook 触发（I2 生效的证据）。

- [ ] **Step 7: 记录结果**

把 2-6 步的实际输出贴进 PR 描述。**不要**只写「验收通过」。

---

## 自查记录

- **Spec 覆盖**：A1 → Task 14/15/16/17/18/19；A2 → Task 3/4/5/6/7/9/10；A3 → Task 13/15；A4 → Task 13（`pendingKeys`）/15（提示条）；A5.1 → Task 11/16；A5.2 → Task 16；A5.3/A5.4 → Task 17；A5.5 → Task 6/18；A5.6 → Task 7/18；A5.7 → Task 5/18；A5.8/A5.9/A5.10 → Task 18；A5.11 → Task 19；A6 → Task 20；T1-T5 分散在各任务；T6 → Task 21。
- **类型一致性**：`SettingsDestinationDto`（Rust）↔ `Provenance`（TS）在 `rowState` 中以字符串对齐；`writable_path` 与 `permission_destination` 共用同一句可写集合错误文案；`AllowedClientCommand` 的 11 个新 `type` 与 Task 3/5/6/7 定义的 `serde(rename_all = "snake_case")` 标签逐字一致。
- **已知缺口**：Task 6 的 MCP User / Local 域留了第二轮补齐，且带一个必须先做的读者定位前置（spec §0.9）。
