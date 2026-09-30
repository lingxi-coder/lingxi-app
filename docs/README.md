# 文档导航

## 当前入口

- [架构与运行时边界](ARCHITECTURE.md)
- [Harness 首次提取的设计与验证记录](architecture/harness-runtime-extraction.md)
- [Harness 验证记录](architecture/harness-runtime-verification.md)
- [多仓开发、lingxi-app 布局与 SDK 独立性](development/multi-repo-workflow.md)
- [平台](PLATFORMS.md)、[安全](SECURITY.md)、[模型配置](LLM_PROVIDERS.md)
- [产品平台布局与构建](../apps/README.md)、[翻译源](../resources/translations/README.md)、[语音配置](../resources/voice/README.md)
- [桌面 Git](desktop-git/)、[桌面终端](desktop-terminal/)、[移动 Linux](mobile-linux/README.md)
- [集成说明](integrations/)、[定时任务](cron-task-center.md)

## 设计与历史记录

- `plans/`：设计和实施计划，不代表已经实现。
- `control-proto/`：早期协议调研与规范，包含迁移前路径；使用时需核对当前 SDK 契约。
- `reviews/`、`gap-audit-2026-06-24/` 及带日期的审计文件：保留当时的证据与结论。
- `archive/gap-notes/`：从根目录 `.gap-notes/` 归档的工作记录；重复的 `cost-fix.md` 仅保留在 `gap-audit-2026-06-24/`。
- [`archive/lingxi-code/`](archive/lingxi-code/README.md)：原 `lingxi-code/` workspace 的历史运行时交接和对齐记录，其中路径可能指向迁移前布局。
- `superpowers/references/OpenMinis/`：保留的只读参考子模块。当前移动 Linux 构建通过固定 SDK 的源码解析器取源码，归档记录不能替代当前构建来源。

产品 checkout 名为 `lingxi-app`，当前根目录为 `~/lingxi/lingxi-app/`。
根 README、多仓开发指南和 `ARCHITECTURE.md` 的当前边界章节是当前布局入口。
当前产品入口统一位于 `apps/`，移动界面和 FFI 分别位于平台的 `native/` 与 `ffi/`；共享包位于 `packages/`，配置资源位于 `resources/`。
历史文件中的相对路径、测试数量与完成状态应结合记录日期解读。
