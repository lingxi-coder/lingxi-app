# 插件

加载插件组件、检查生命周期并处理卸载失败。

<a id="components"></a>

## 插件组件

plugin crate 导出 PluginManifest、PluginComponents、PluginSource、PluginState、PluginManager、严格策略、信任与发现辅助接口。可声明组件包括命令、智能体、skills、hooks、输出样式、MCP 服务器、LSP 服务器和工作流。管理器将其装配到宿主注册表。

构建 PluginManager 时，宿主提供注册表、文件系统及相关依赖。这是配套 crate 接口；Harness 不提供可变插件注册表。

<a id="local-install"></a>

## 安装已准备的本地插件

PluginManager::install 当前支持 PluginSource::LocalPath：原地发现现有目录，读取 manifest 与组件，启用插件并返回 PluginId。管理器中的 Git、市场、MCP bundle 和内置插件安装分支返回类型化错误。网络来源的落盘由宿主生产安装流程负责。

### Rust · host-assembled PluginManager required

```rust
use std::path::PathBuf;
use plugin::{PluginManager, PluginManagerError, PluginSource};

async fn install_prepared(
    manager: &PluginManager,
    directory: PathBuf,
) -> Result<(), PluginManagerError> {
    let id = manager.install(
        PluginSource::LocalPath { path: directory },
    ).await?;
    let _state = manager.plugin_state(&id).await;
    Ok(())
}
```

### PluginManager::install

```text
pub async fn install(&self, source: PluginSource) -> Result<PluginId, PluginManagerError>
```

通过既有宿主注册表发现并启用本地插件目录。

### PluginManager::enable

```text
pub async fn enable(&self, id: &PluginId, manifest: PluginManifest, install_dir: PathBuf) -> Result<(), PluginManagerError>
```

将已落盘的 manifest 和安装目录加载到注册表。

<a id="lifecycle"></a>

## 跟踪插件生命周期

- Declared → Fetching → Fetched → Loaded 表示准备与激活过程。
- Disabled 表示已移除插件贡献的组件。Failed 与 Blocked 保留失败或策略上下文。
- DisablingFailed 保留部分卸载失败，让后续 disable 继续尝试剩余清理。

### PluginManager::plugin_state

```text
pub async fn plugin_state(&self, id: &PluginId) -> Option<PluginState>
```

读取某一插件的生命周期状态快照。

### PluginManager::loaded_plugin_ids

```text
pub async fn loaded_plugin_ids(&self) -> Vec<PluginId>
```

包含 Loaded 和 DisablingFailed 插件，让卸载失败对重试流程保持可见。

### PluginManager::disable

```text
pub async fn disable(&self, id: &PluginId) -> Result<(), PluginManagerError>
```

卸载插件贡献组件并进入 Disabled，清理失败时保留 DisablingFailed。

<a id="policy"></a>

## 应用宿主策略

插件信任、受管市场限制、严格插件策略、组件校验和用户配置解析参与加载。使用既有管理器与发现路径，让插件组件在修改注册表前经过这些控制。

manifest 字段可被解析不代表其可选获取或运行行为已经实现。链接源码列明已启用组件路径与明确不支持的分支。

<a id="teardown-recovery"></a>

## 更新注册表并恢复其归属

启用经生产管理器贡献组件；禁用必须删除这些对应组件并停止所属 worker。DisablingFailed 插件继续出现在 loaded_plugin_ids 中以便重试，但不具备重放活动 Mod 的权限。保留失败并重试清理，之后才能宣称插件已卸载。

更新后的 MCP 或 LSP 目录是活动注册表更新，不是复制一组内置工具。插件工作流进入运行时 launcher 使用的同一个工作流注册表。市场解析、信任策略与运行时成功激活是独立检查点。

### 插件单元与 LSP 注册检查

```bash
cargo test --locked -p plugin --lib
cargo test --locked -p lsp --test plugin_only_registration_test
```

验证方式: 需环境验证

现有测试覆盖组件清理、失败保留及仅允许已验证插件注册 LSP。安装真实第三方包还需验收宿主信任、传输与进程清理。

<a id="plugin-directory"></a>

## 准备本地插件目录

使用 PluginSource::LocalPath 时，提供已经落盘、包含 .lingxi-plugin/plugin.json 的目录。发现路径可以识别可选的 commands/、agents/、skills/、output-styles/ 与 hooks/hooks.json。运行时使用发现的组件期间，应保持这些文件可用。

1. 准备 manifest 以及其声明的组件文件，激活前校验内容。
2. 将路径交给宿主组装的 PluginManager。记录返回的 PluginId，再通过 plugin_state 查看实际状态。
3. 检查贡献到注册表的项目，不要假定每个可选组件都已激活。
4. 移除插件时，对同一个管理器调用 disable，并重试保留为 DisablingFailed 的清理。

### 本地插件目录

```text
report-helper/
  .lingxi-plugin/plugin.json
  commands/
  agents/
  skills/
  hooks/hooks.json
```

验证方式: 接口摘录

<a id="plugin-install-paths"></a>

## 选择正确的安装路径

| 来源 | PluginManager::install | 宿主操作 |
| --- | --- | --- |
| LocalPath | 原地发现并启用已准备目录。 | 提供可读 manifest，并管理目录生命周期。 |
| Git／Marketplace／OfficialMarketplace | 返回类型化的不支持／获取路径错误。 | 先使用生产安装／落盘流程，再激活。 |
| Mcpb | 此方法不会解包 bundle。 | 使用宿主支持的 bundle 安装流程。 |
| BuiltIn | 不经此方法安装。 | 使用组合中已编译的内置注册路径。 |

PluginSource 变体用于描述来源输入。变体存在不代表管理器的此方法实现了网络获取、认证或归档落盘。

<a id="plugin-troubleshooting"></a>

## 诊断激活与移除

| 现象 | 解释 | 恢复方式 |
| --- | --- | --- |
| 找不到 manifest | LocalPath 的平层发现没有找到预期插件目录。 | 检查准确目录与 .lingxi-plugin/plugin.json，不要把顶层版本缓存当作单个插件。 |
| Blocked | 策略在激活前拒绝来源。 | 检查信任与受管策略，不要手动注册组件来绕过策略。 |
| Failed／Validation／Loader | manifest、配置或加载失败。 | 修正已报告原因后再启用候选插件。 |
| DisablingFailed 仍然出现在已加载列表 | 仍有所属资源需要清理。 | 保留负责人并重试 disable，组件移除后再报告成功。 |

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/plugin/src/manager.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/plugin/src/manager.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/plugin/src/lifecycle.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/plugin/src/lifecycle.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/plugin/src/manifest.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/plugin/src/manifest.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/plugin/src/discovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/plugin/src/discovery.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
