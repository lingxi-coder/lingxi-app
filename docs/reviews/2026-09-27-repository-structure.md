# 项目结构检查与清理（2026-09-27）

## 范围与起点

检查 Git 跟踪文件、Rust workspace / Cargo 来源、客户端构建入口、CI 门禁、
TypeScript 模块引用、重复文件内容、翻译生成源和根目录临时产物。
开始时工作区已有 328 个删除记录；这些已有删除未恢复、未重新计为本次清理。
本次没有提交，也没有改变产品运行逻辑或重排原生构建目录。

## 已实施

- README、CLAUDE.md、客户端说明和架构入口对齐独立 `harness-runtime` 布局；
  修正根目录 Cargo 命令、已不存在的 demo / 本地 runtime 测试命令及过时版本说明。
- 新增文档导航。9 份唯一的 `.gap-notes` 笔记归档到 `docs/archive/gap-notes/`；
  `cost-fix.md` 与已有审计文档逐字节相同，删除重复副本。
  隐藏工具目录内的主循环设计移到 `docs/plans/`。
- 删除已无生产引用的 Electron `PlanTasks.tsx`、`planOverflow.ts` 和
  `engineStatus.ts`，以及只测试这些已废弃辅助函数的两个测试文件。
  当前计划展示继续由计划文档和 Runtime Center 路径负责。
- 删除只认 `0.5.0`、已无构建/CI 调用的 `tools/scripts/check_version.sh`。
- JavaScript 客户端统一采用现有 npm 流程及 package-lock；删除 Electron/shared
  的重复 pnpm 锁文件和含未填写 `allowBuilds` 值的 workspace 配置。
- 回收 12 个平台独有翻译键到 5 语言 JSON，保留现有语音收音提示并补齐翻译。
  重新生成 Android/iOS 资源，删除两个重复的 `strings_permission_sheet_v1.xml`。
- 共享协议快照与 Electron 命令注册测试改用同一个固定来源解析辅助函数，
  不再读取已经迁出的 `lingxi-code/client-protocol`、`commands/core` 等目录。
- 删除 12 个明确的根目录日志、Finder 元数据和临时分析 JSON，共 643,695 字节。
  移除已迁出模板目录的过期 ignore 例外，并补充根目录日志忽略规则。

## 保留依据

- `third_party/mksh`、`third_party/toybox` 和 OpenMinis 参考子模块参与移动端构建。
- 原生平台中的同内容头像、图标和生成 Swift 文件分别由平台资源编译器或生成器消费；
  内容相同不代表冗余。
- `assets/brand` 和 `clients/.design-reference` 保存设计源文件，不能用是否被 import 判废。
- 历史运行时审计保留用于证据追溯；用文档导航区分当前说明和历史布局。
- 签名桌面包、依赖缓存、本地参考仓库、账号/会话状态和备份保持原样。

## 验证

- Electron 和共享 SDK TypeScript 检查通过；Electron 生产前端构建通过。
- 共享 SDK：64 项通过；翻译生成器：16 项通过；固定来源解析器：7 项通过。
- 命令注册与计划文档定向测试：10 项通过。
- 翻译配对与生成一致性门禁通过；Cargo 格式检查、Git diff 空白检查通过。
- 固定运行时依赖身份、3 个协议镜像与宿主边界、绝对符号链接门禁通过。
- Electron 完整测试在允许本地端口、浏览器和文件监听的环境中：
  1,283 项，1,277 通过，4 失败，2 跳过。

## 仍存在的问题

以下实现与测试文件均未在本次清理中修改，不能将全项目描述为全部检查通过：

1. `settings-screen-logic.test.ts` 的 3 个断言仍要求忽略 `active_json`，
   现有 SettingsScreen 实现会保留 active 快照，预期与当前契约不一致。
2. `compact-progress-interaction.test.mjs` 检测到压缩图标与子代理图标横坐标相差
   12 像素（122 / 110）；这是布局断言失败，不是构建失败。
3. 品牌门禁在清理开始前即有 7 个新增匹配和 2 个失效基线条目，涉及测试、
   移动资源校验脚本、桌面 Stage 与 README；没有通过扩大豁免掩盖问题。

本次进行了全仓结构与引用检查，并运行相关门禁及客户端测试；没有重新构建
Android/iOS 原生应用，也不代表逐行完成所有源码的功能、安全与性能审计。
