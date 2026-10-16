# 权限与完整性

宿主拥有的挂载策略、网络与资源执行约束、rootfs 信任及明确的平台限制。

<a id="host-policy"></a>

## 宿主拥有授权决策

应用负责准入、文件系统访问、rootfs 信任与生命周期。PRoot 是兼容层，不是安全边界。在宿主边界校验请求；操作要求特定策略时，检查平台能力和策略执行回执。

RuntimeConfig.authorization_file 是可选的 iOS 配置值，会转发给原生桥接并纳入运行时身份。该字段不是通用的 SDK 身份认证服务、token 格式或账号系统。Android create_runtime 不消费此字段。

<a id="mount-policy"></a>

## iOS 挂载策略

- 请求挂载不能与托管运行时存储或 protectedHostRoots 重叠，包括它们的祖先目录。直接挂载应用沙箱根目录会被拒绝。
- 工作区挂载必须匹配 workspaceHostPath 和 /workspace/<stableWorkspaceId>。请求挂载不能替换持久 guest /root。
- 非工作区挂载必须同时匹配 allowedMountRoots 的 host 前缀与 allowedGuestRoots 的 guest 前缀。因此，空白名单不允许额外请求挂载。

### 挂载记录摘录

```rust
pub struct MobileLinuxMountSpecFfi {
    pub host_path: String,
    pub guest_path: String,
    pub read_only: bool,
    pub purpose: MobileLinuxMountPurposeFfi,
}
```

验证方式: 接口摘录

这些显式 host/guest 白名单字段由 iOS 后端消费。Android 使用不同的运行时配置与验证路径；不要假定 iOS 策略字段会配置 Android。

<a id="network-and-limits"></a>

## 网络与资源限制执行

NetworkPolicy 包含 Disabled、LoopbackOnly 与 Allowed。ResourceLimits 包含可选的 CPU 秒数、驻留内存、进程数及打开文件数上限。请求值不代表已执行约束；检查回执，或处理后端无法应用所需约束时返回的类型化错误。

### 执行回执摘录

```rust
pub struct LinuxEnforcementReceipt {
    pub network_policy_enforced: bool,
    pub memory_limit_enforced: bool,
}
```

验证方式: 接口摘录

### LinuxEnforcementReceipt::ensure_for

```text
pub fn ensure_for(self, network: NetworkPolicy, limits: ResourceLimits) -> Result<(), MobileLinuxError>
```

对比 Allowed 更严格的策略要求 network_policy_enforced；配置内存上限时要求 memory_limit_enforced。缺少证明会返回 NetworkPolicyUnavailable 或 ResourceLimitExceeded。

<a id="rootfs-trust"></a>

## 压缩包信任与可执行文件授权

在应用边界固定预期压缩包 SHA-256。manifest 不能替代这一信任决策。激活前验证压缩包身份、不可变清单、符号链接路径与 SBOM。RootfsStore.authorize_manifest_file 校验不可变可执行文件白名单，并拒绝可写 guest 位置。

默认 guest 可写根目录为 /root、/tmp、/var/tmp 与 /workspace。托管运行时存储应保持应用私有，并与外部挂载的用户工作区分开。

<a id="errors"></a>

## 错误与平台限制

### MobileLinuxError::Unsupported

```text
Unsupported
```

所选后端没有实现请求的操作。

### MobileLinuxError::Unavailable

```text
Unavailable(String)
```

后端当前不能执行；检查详细信息与 capability 的原因。

### MobileLinuxError::RestartRequired

```text
RestartRequired(String)
```

原生内核已初始化，需要新的宿主进程。

### MobileLinuxError::Integrity

```text
Integrity(String)
```

rootfs 验证失败。

### MobileLinuxError::InvalidRequest

```text
InvalidRequest(String)
```

路径、ABI、身份或操作参数未通过校验。

### MobileLinuxError::NetworkPolicyUnavailable

```text
NetworkPolicyUnavailable(String)
```

无法执行所需网络策略。

### MobileLinuxError::ResourceLimitExceeded

```text
ResourceLimitExceeded(String)
```

无法保证所需资源限制，或资源已超限。

iOS 模拟器使用不可用后端。应用级操作系统权限与真机验收仍由宿主负责。

<a id="attribution"></a>

## 分发声明

SDK Rust 源码保留 MIT OR Apache-2.0，部分 crate 或源码有明确例外。完整分发包含适用独立条款的第三方组件，包括 GPL/LGPL 代码。分发 SDK 产物时保留组件声明、源码版本、补丁来源与 rootfs SBOM 许可证信息。

<a id="policy-decision-flow"></a>

## 准入与执行约束流程

应用权限决定准入操作；运行时校验随后检查具体路径、挂载、网络与资源请求，两者是不同决定。当所需保证不可用时，在接受输出前返回类型化错误，不能把兼容层执行解释成隔离。

有限命令检查返回的执行回执。可移植契约中的长期进程／PTY／raw 句柄也携带执行约束身份，准入会话时进行验证。ResourceLimits 列出所请求上限，但当前可移植证明仅包含 network_policy_enforced 和 memory_limit_enforced，不能臆造 CPU、进程数或文件描述符的回执位。

1. 授权用户动作并选择明确策略。
2. 校验绝对路径与平台挂载边界。
3. 验证镜像，并通过选定后端准入执行。
4. 检查类型化结果／句柄回执、输出及终态。

<a id="negative-acceptance"></a>

## 需要验证的失败场景

破坏固定的压缩包或不可变文件：暂存／verify 必须报告完整性失败，未经验证的镜像不能成为活动镜像。通过不可变授权请求可写 guest 可执行文件时，必须拒绝。恢复有效暂存镜像后，验证 repair 不删除用户工作区数据。

请求保护路径的祖先挂载、不匹配的稳定工作区、缺少白名单、零字节 raw read，或 raw stdin／timeout 字段，检查类型化拒绝。另在真机测试禁用网络的逃逸尝试与内存限制，因为宿主测试不能证明原生策略执行。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/crates/mobile-linux-api/src/mobile_linux.rs)

- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/crates/mobile-linux-api/src/mobile_linux.rs) — 已提交源码 (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)
- [mobile-linux-runtime/crates/mobile-linux-api/src/execution.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/crates/mobile-linux-api/src/execution.rs) — 已提交源码 (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/verification.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/crates/mobile-linux-core/src/store/verification.rs) — 已提交源码 (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/install.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/crates/mobile-linux-core/src/store/install.rs) — 已提交源码 (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/validation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/crates/mobile-linux-ios/src/runtime/validation.rs) — 已提交源码 (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/mounts.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/crates/mobile-linux-android/src/runtime/mounts.rs) — 已提交源码 (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)
- [mobile-linux-runtime/scripts/tests/test_resource_contracts.py](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/scripts/tests/test_resource_contracts.py) — 已提交源码 (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)

- mobile-linux-runtime: HEAD 6b033d41a2b6b736fda1e8f996f55a4b20bd885d; 所据文件指纹 sha256:435a888b1a310f357f18311df5f96418fa72b1a900d78e987af282bb3e91d7ca; 产品固定版本 6b033d41a2b6b736fda1e8f996f55a4b20bd885d

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
