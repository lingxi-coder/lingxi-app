# 品牌 workspace 根目录修正

最新目录约定：`~/.<品牌>` 为品牌容器及配置根；Global Chat 默认 cwd 为其 `chat` 子目录，Global Code 为其 `code` 子目录。灵犀分别为 `~/.lingxi/chat` 和 `~/.lingxi/code`。项目、调度、应用等管理目录继续归于共同容器，显式项目 cwd 不变。移动端 `~` 为 Linux 用户 home，Desktop 为 OS 用户 home；Desktop 显式 `LINGXI_CONFIG_DIR` 覆盖容器根。CLI 继续使用 shell 启动 cwd 和显式 `--cwd` / `--project`。逻辑范围由已认证的 owner 决定，不能由 cwd 推导。不迁移、删除旧业务目录或添加旧根 fallback。

Local App 的创建、编辑源码、开发预览、发布、remix 与恢复只在 Code 准入；Chat 中明确提示切换到 Code，不能自动切换或自动创建 Code 会话并继续原操作。Chat 仍可浏览和运行已安装应用；先前由 Code owner 接纳的操作可以按该 owner 完成或停放。

以下为此前单根方案的历史记录；其测试结果仅证明当时源码，不能作为当前按模式目录或重新生成 native 绑定的验收。

本次局部修改（Harness 路径修复已独立提交 `f286ff5cd05c720b82451a66dc01e88bca213ade`；App 依赖 pin 尚未更新）：

- Harness `crates/branding/src/lib.rs`：workspace root 从品牌配置目录派生。
- Harness `crates/runtime/src/mobile/directories.rs`：host default cwd 与 config_home 共根，projects/scheduled 从该根派生，guest/host 映射一致。
- Harness `crates/runtime/src/mobile/mobile_lsp.rs`：动态 LSP workspace 不再硬编码旧的 `~/lingxi` 路径。
- App `apps/electron/src/main/conversationSpaceStore.ts`：Global 默认 `~/.lingxi`，支持显式 `LINGXI_CONFIG_DIR`；导航元数据仍在 userData。非绝对路径与 symlink workspace 仍拒绝。
- 对应品牌/目录/Desktop 用例及架构说明已更新。没有添加旧路径 fallback。

实际验证：branding Cargo unit 7/7；直接引入实际 directories.rs 的轻量隔离 Rust harness 5/5（使用替代错误类型，不能代替完整 runtime/native 编译）；Desktop conversation spaces 6/6。未重建 JNI、iOS archive、Bridge/CLI 或更新 target-derived 绑定。

## 主线程整合要求

此为侧对话的局部源码修复，仅提交路径修复，不包含其他进行中的 Cron、媒体生命周期修改。App 当前 immutable SDK pin 尚未包含该 Harness 提交。主线程需以包含此提交的下一次已验证 successor 统一更新 App pins/lock，并使用新源重新验证两端实际 native 库/绑定。已有旧源码产物和验收结果不能冒充新路径验收。mobile-linux 上游默认路径常量仍是独立依赖的约定；产品实际路径由当前 profile/resolver 显式提供，不能为了旧常量添加旧路径别名。
