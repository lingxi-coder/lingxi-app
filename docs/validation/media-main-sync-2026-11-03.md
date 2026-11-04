# Media main 同步记录（2026-11-03）

本轮将最新远端 main 合入 Media 开发分支；没有将尚未验收的 WIP 反向合入或推送 main。

| 仓库 | 合入的 main | 合并后提交 |
| --- | --- | --- |
| lingxi-app | 06d205b58bf313ed3cfbc0808002c7d462c23385 | ca128678c（本记录之前的合并提交） |
| harness-runtime | e0a8ae09607d361fc0b94d92fe51756238347302 | 89032106bd35a74a95f1acb19e110f9d55cd5bc7 |
| llm-client | 5a4fc0d40e64879dbfffdb36bfcb74e1510259d9 | e59c8fb33c73461fd5cd6865ebde623eca517a7e |

三个仓库的远端检查点目标为 `codex/media-main-sync-20261103`；应查看本地回执核对推送结果。

## 冲突处理

保留 main 新增的 Teams、Bot、账号、MCP、图片预览功能和本分支品牌常量。移除 main 已删除的 WebMCP fixture 断言，不恢复旧实现。合并 Local App MCP 配置文件监测与品牌配置路径。App 引用以上合并后 Harness/LLM 完整 SHA，Harness 引用合并后 LLM SHA；不再引用旧的本地依赖预览 SHA。WebRTC 0.21.1 和 JSONSchema 修复保留。

## 本轮验证

- App、Harness 的 `cargo metadata --locked --offline --format-version 1` 通过，App 解析到 1008 个包和 13 个 workspace members。旧的 1009 包审计数量不能直接用于新图。
- LLM 相同 metadata 检查因离线缓存缺少 `cast 0.3.0` 中止，不能算通过。
- Electron product namespace 和 remote policy 聚焦测试 24/24 通过。
- verify-package.mjs 语法检查通过；App 合并暂存区 whitespace 检查通过。
- 原 Node 类型检查使用的共享 node_modules 指向旧 checkout，缺少 MCP SDK 和新协议类型，未通过。临时配置显式映射当前 bridge-client 源码和 canonical checkout 已安装的 MCP SDK 1.32.2 后，Node 类型检查通过。临时配置未提交；正式依赖安装和标准类型检查仍待执行。
- 未重新执行 Electron 全套、产品构建、设备测试或完整 CI。

## 后续阻断

全量源码审计未完成；764/775 是旧冻结图的历史结果，须基于本轮新图重算。原生 TLS CA 生命周期和 wrap 所有权问题仍未修复，独立 native WIP 未进入 integrated Harness。原生源码库存、审计、stamp 和最终门禁仍待完成。此前子 agent 执行服务安全限制没有在本轮重试或绕过。

原有详细交接文件：`/Users/luolingfeng/lingxi/worktrees/media-architecture/HANDOFF.md`。以该文件的历史证据和本记录的增量状态一起接手。保留原恢复工作树；不要重置或复制整目录备份。
