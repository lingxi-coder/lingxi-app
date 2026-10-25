# 组合 Skills、Hooks 与 MCP

选择合适扩展，配置范围清晰的生命周期检查，并核对外部能力已连接且可调用。

<a id="choose-extension"></a>

## 按职责选择扩展

Skill 增加指令，不绕过工具权限，也不安装进程传输。Hook 处理特定事件，不应替代普通工具。MCP 需要真实认证连接，不能只看配置条目。

| 需求 | 机制 | 例子 |
| --- | --- | --- |
| 教授可重复流程 | Skill／命令 | 仓库审查步骤与明确报告格式。 |
| 在生命周期事件运行代码或决定 | Hook | 校验待写操作或记录完成元数据。 |
| 暴露外部工具／数据 | MCP 服务器 | 拥有自身连接与认证的项目服务。 |
| 共同分发多种扩展 | 插件 | 共同生命周期负责人管理的版本化 Skills、Hooks 和服务器。 |

<a id="write-a-skill"></a>

## 编写小而可验证的 Skill

parse_skill_markdown 将 frontmatter／正文解析为 Skill；SkillRegistry 负责注册／发现。宿主 loader 选择可信根目录与来源层级。先确认 Skill 出现在宿主列表，再用结果可观察的小 fixture 调用。

### Markdown · Skill 源文件示例

```markdown
---
name: inspect-change
description: Review a scoped change and report reproducible findings.
---

1. Read the requested diff and its owning source.
2. Check one normal path and one failure path.
3. Report actionable findings with file locations.
4. State which checks were actually run.

```

验证方式: 需环境验证

示例是文件内容；位置由宿主配置的发现根目录决定。另一产品的配置目录不会在所有嵌入宿主中自动生效。

<a id="add-a-hook"></a>

## 添加有范围的执行前检查

编辑器／服务严格校验事件映射层时使用 parse_hook_layer_strict；parse_hooks_from_settings_json 读取外层 settings 对象。解析只建立定义，不代表 executor 就绪：command、HTTP、prompt、agent 与 MCP handler 需要宿主依赖。应同时测试允许路径及阻止／错误路径。

| 字段 | 用途 |
| --- | --- |
| PreToolUse／matcher | 选择工具执行前事件与待检查规范工具名。 |
| type／command | 使用 command executor 和宿主解析的程序；脚本需自行提供。 |
| timeout | 以秒为单位限制前台执行时长。 |
| onFailure: block | 支持该策略的执行前消费者在 Hook 失败时阻止动作。 |

### JSON · settings Hook 示例

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "python3 scripts/check-write.py",
            "timeout": 5,
            "onFailure": "block"
          }
        ]
      }
    ]
  }
}
```

验证方式: 需环境验证

<a id="connect-mcp"></a>

## 显式检查 MCP 就绪状态

1. 在宿主解析预期服务器配置与认证。
2. 使用平台可用传输连接，检查连接／认证状态。
3. 将通过接纳的工具发布到服务器所属分区。
4. 目录变更或断开时刷新／移除该分区。

### McpRegistry::connect

```text
pub async fn connect(&self, config: McpServerConfig) -> Result<McpConnectionId, McpError>
```

经宿主提供的传输建立连接，返回连接 id 或明确失败。

### McpRegistry::has_callable_server

```text
pub async fn has_callable_server(&self, name: &str) -> bool
```

检查当前可调用性，而不只检查保存配置是否存在。

### McpRegistry::subscribe_catalog_changes

```text
pub fn subscribe_catalog_changes(&self) -> broadcast::Receiver<McpCatalogChanged>
```

观察目录变更，替换所属能力分区。

<a id="package-and-diagnose"></a>

## 打包流程并诊断激活

流程涉及 Skills、Hooks 与 MCP 工具时，应使用插件生命周期归属，不由界面执行互不关联的安装步骤。检查插件加载状态与组件诊断。Manifest 条目或已解析 Hook 不能证明其 runner、进程或凭据就绪。

| 现象 | 优先检查 |
| --- | --- |
| Skill 未出现 | 发现根目录、来源层级、frontmatter 与活动会话 loader。 |
| 已列出的 Hook 不执行 | 事件、matcher、managed 策略与注入 executor／runner。 |
| 已配置 MCP 没有可调用工具 | 连接／认证状态与目录刷新诊断。 |
| 已移除插件工具仍可见 | 按归属的清理与下一次能力快照。 |

- [插件激活](harness-plugins.md)
- [扩展架构](extensions-lifecycle.md)
- [Claude Hooks 主题参考](https://code.claude.com/docs/en/hooks)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/skill/src/frontmatter.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/skill/src/frontmatter.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/skill/src/model.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/skill/src/model.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/skill/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/skill/src/registry.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/hooks/src/loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/hooks/src/loader.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/hooks/src/settings_layer.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/hooks/src/settings_layer.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/hooks/src/definition.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/hooks/src/definition.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/hooks/src/response.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/hooks/src/response.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/mcp/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/mcp/src/registry.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/plugin/src/manager.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/plugin/src/manager.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/desktop/skill_loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/desktop/skill_loader.rs) — 已提交源码 (0e28910fd624638e6c5d1f92eca4899a7938d188)

- harness-runtime: HEAD 0e28910fd624638e6c5d1f92eca4899a7938d188; 所据文件指纹 sha256:b6b36966a8f4c48b8d0813ea03f73cd617387ddaf2531d8254e4a5ac81f48255; 产品固定版本 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
