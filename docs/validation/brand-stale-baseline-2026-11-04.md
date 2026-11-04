# 已消失的品牌基线项复核（2026-11-04）

以下 20 个键是扫描器在当前源码树中报告不再存在的既有基线项。本次对每个相关文件重新运行未加载任何冻结收据的原始规则，20 项均未命中；其中 5 项涉及 3 个已删除的旧文件。针对每个键，在独立临时合成文件中重新引入相应字面量或环境变量读操作，20 项全部再次触发原规则。因而只删除以下 20 条失效记录，未添加基线；任何被重新引入的命中仍会成为新增错误。扫描器自测 62 项同时通过。

| 规则 | 文件 | 检查项 | 当前 Git blob |
| --- | --- | --- | --- |
| `G1` | `apps/android/native/app/src/main/java/com/lingxi/code/settings/EngineSettingsPages.kt` | `\.lingxi(?![A-Za-z0-9_.\-])` | `be584c15ce0cdea7469b83366894c8f816a364f3` |
| `G1` | `apps/android/native/app/src/main/java/com/lingxi/code/settings/ProviderSettingsRepository.kt` | `LINGXI_` | `6e2f3ed52be07782dcaf6cae787548fbd23e63c0` |
| `G1` | `apps/cli/host/src/background_launch.rs` | `\.lingxi` | `0ac6bbbb20badcac5d4b62bd9299ff8f4d76499b` |
| `G1` | `apps/cli/host/src/startup_resources.rs` | `LINGXI` | `ef174737ff2657aa4c7c5946a392640f96bb579e` |
| `G1` | `apps/cli/host/tests/mcp_serve.rs` | `\.lingxi` | `496155beb7207b838b88eb3457dfc313745b86c5` |
| `G1` | `apps/cli/tui-core/src/collapse/group.rs` | `\.lingxi` | `6b386fd239c7a095d24dd5a0a6bd8acdd32013f4` |
| `G1` | `apps/ios/ffi/src/configuration.rs` | `\.lingxi` | `9fc8c0724f6d0d4f60bda5d40d75135503c79483` |
| `G1` | `apps/ios/native/Sources/Settings/MCPConfigurationRepository.swift` | `\.lingxi(?![A-Za-z0-9_.\-])` | `419c09b968fddf9fb93c7a1760e330a6474726f3` |
| `G2` | `apps/cli/host/src/commands/auth.rs` | `CLAUDE_CODE_USE_BEDROCK` | `文件已删除` |
| `G2` | `apps/cli/host/src/commands/auth.rs` | `CLAUDE_CODE_USE_FOUNDRY` | `文件已删除` |
| `G2` | `apps/cli/host/src/commands/auth.rs` | `CLAUDE_CODE_USE_VERTEX` | `文件已删除` |
| `G2` | `apps/cli/host/src/init.rs` | `CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR` | `8fb74268a6d5838bed4f0eb8d322b10ab5c7d3de` |
| `G3` | `apps/android/native/app/src/androidTest/java/com/lingxi/code/apps/ChatAppThemeTest.kt` | `.lingxi-*` | `文件已删除` |
| `G3` | `apps/android/native/app/src/main/java/com/lingxi/code/apps/ChatAppRuntime.kt` | `.lingxi-*` | `文件已删除` |
| `G3` | `apps/android/native/app/src/main/java/com/lingxi/code/settings/EngineSettingsPages.kt` | `.lingxi` | `be584c15ce0cdea7469b83366894c8f816a364f3` |
| `G3` | `apps/cli/host/src/background_launch.rs` | `.lingxi` | `0ac6bbbb20badcac5d4b62bd9299ff8f4d76499b` |
| `G3` | `apps/cli/host/tests/mcp_serve.rs` | `.lingxi.json` | `496155beb7207b838b88eb3457dfc313745b86c5` |
| `G3` | `apps/cli/tui-core/src/collapse/group.rs` | `.lingxi` | `6b386fd239c7a095d24dd5a0a6bd8acdd32013f4` |
| `G3` | `apps/ios/ffi/src/configuration.rs` | `.lingxi` | `9fc8c0724f6d0d4f60bda5d40d75135503c79483` |
| `G3` | `apps/ios/native/Sources/Settings/MCPConfigurationRepository.swift` | `.lingxi` | `419c09b968fddf9fb93c7a1760e330a6474726f3` |
