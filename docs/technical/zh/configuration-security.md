# 配置、凭据与隔离

追踪设置来源，明确模型路由、授权、凭据存储与操作系统隔离的宿主职责。

<a id="settings-layers"></a>

## 设置来源与合并规则

生效值按默认 → 用户 → 项目 → 本地 → CLI／flag 设置 → managed 层 → 环境进行合并。managed 层按优先级升序提供。用户文件为 ~/.lingxi/settings.json；项目和本地文件为项目下的 .lingxi/settings.json 与 .lingxi/settings.local.json。显式宿主可选择不同用户路径和文件层范围。

合并语义按字段区分：已提供标量覆盖，列表拼接／去重，hooks／sandbox 对象深度合并；权限数组累积。格式错误、过大或不可读文件作为整层跳过，后续有效层继续生效。来源 trace 解释最终选定来源，不能仅由最后打开的文件推断。

<a id="credential-lifecycle"></a>

## 凭据与路由标识

宿主先解析供应商 profile、账户与凭据来源，再将请求服务交给 llm-client。秘密值属于安全存储与请求 header，不属于 JSONL、工具参数或公开配置导出。空启动 API key 可能允许命令初始化，但模型请求之后认证失败；构建成功并非登录成功。

macOS 的 NativePreferred 要求随包凭据 broker；打包身份错误时拒绝继续。NativeOrMemory 是显式隔离的进程内模式；PlainTextFixture 是显式测试选项。Linux 安全存储可用性与回退遵循其 factory 路径。明文后端使用严格文件权限，但文件权限不等于加密。

不要记录整个配置或凭据 struct。字段专用 Debug 实现与历史注释不能作为无条件脱敏保证；记录 profile ID、来源名称及失败分类，避免秘密内容。

<a id="authorization"></a>

## 工具授权

PolicyGate 是调度执行约束边界。Allow 继续执行，Deny 返回拒绝的工具结果，Ask 在需要时使用注入的提示传输。会话可见性过滤与逐调用授权是独立控制。同一授权边界适用于子 Agent 与 teammate。

wire 模式名称为 default、plan、acceptEdits、bypassPermissions、dontAsk 和 auto；Bubble 仅内部使用。模式可用性与持久设置可接受性不相同。Auto 还受模型／供应商和宿主策略限制。具体决策以 mode／rule 实现为准；单个模式名称不能概括所有规则、hook 或 managed 上限。

<a id="sandbox-and-watchers"></a>

## 系统沙箱与实时设置

沙箱封装独立于权限模式与项目信任选择。决策考虑实际 runtime 可用性、显式覆盖及非沙箱执行权限、空命令和排除命令，再由平台后端落实文件系统／网络／进程限制。用户授权不会创建缺失的原生沙箱。

桌面设置 watcher 监控用户／项目／本地／managed 目录并触发 ConfigChange。即使未配置 hooks，也启动 watcher，因为 managed 安全更新仍需观测。managed drop-in 按排序读取。在组合生命周期内保留 watcher handle；丢弃它会停止任务。

<a id="safe-example"></a>

## 项目本地示例与验证

以下完整 .lingxi/settings.local.json 示例只声明读操作工具授权，不含凭据。它只是输入层之一：managed 规则与实时授权仍然参与，工具启用取决于宿主。个人选择放在本地文件中，避免随项目配置导出秘密。

### .lingxi/settings.local.json

```json
{
  "permissions": {
    "allow": ["Read"]
  }
}
```

验证方式: 接口摘录

### 定向设置与沙箱检查

```bash
cargo test --locked -p core --test settings_4layer_test
cargo test --locked -p harness-runtime --features desktop --test settings_watch_spawn
cargo test --locked -p harness-runtime --features desktop --test sandbox_startup_test
```

验证方式: 需环境验证

这些测试验证合并、watcher 与启动契约。打包后的 macOS broker 身份、原生沙箱约束、Linux secret service 及移动系统权限提示需在实际目标验证。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/core/src/settings/mod.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/core/src/settings/mod.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/core/src/settings/loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/core/src/settings/loader.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/core/src/settings/merger.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/core/src/settings/merger.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/core/src/settings/schema.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/core/src/settings/schema.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/runtime/src/desktop/settings_watch.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/src/desktop/settings_watch.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/runtime/src/desktop/credentials.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/src/desktop/credentials.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/platforms/posix/src/secure_storage/factory.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/platforms/posix/src/secure_storage/factory.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/platforms/posix/src/secure_storage/plaintext.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/platforms/posix/src/secure_storage/plaintext.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/permission/src/mode.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/permission/src/mode.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/permission/src/policy_gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/permission/src/policy_gate.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/sandbox/src/decision.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/sandbox/src/decision.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)

- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 a39da75f0985d61d90fffa255f7b4d90901b0e27

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
