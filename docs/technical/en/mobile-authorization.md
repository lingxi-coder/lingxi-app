# Permissions and integrity

Host-owned mount policy, network and resource enforcement, rootfs trust and explicit platform limits.

<a id="host-policy"></a>

## Host-owned authorization

The application owns admission, filesystem access, rootfs trust and lifecycle. PRoot is a compatibility layer, not a security boundary. Validate requests at the host boundary and use platform capabilities and enforcement receipts when an operation requires a policy.

RuntimeConfig.authorization_file is an optional iOS value forwarded to the native bridge and included in runtime identity. The field is not a universal SDK authentication service, token format or account system. Android create_runtime does not consume it.

<a id="mount-policy"></a>

## iOS mount policy

- Request mounts cannot overlap managed runtime storage or configured protectedHostRoots, including ancestors. Mounting the app sandbox root itself is rejected.
- Workspace mounts must match workspaceHostPath and /home/lingxi/lingxi/<stableWorkspaceId>. Request mounts cannot replace persistent guest /home/lingxi or administrator /root.
- Non-workspace mounts require both an allowedMountRoots host prefix and an allowedGuestRoots guest prefix. Empty allowlists therefore admit no extra request mounts.

### types.rs · mount record excerpt

```rust
pub struct MobileLinuxMountSpecFfi {
    pub host_path: String,
    pub guest_path: String,
    pub read_only: bool,
    pub purpose: MobileLinuxMountPurposeFfi,
}
```

Verification method: API excerpt

These explicit host/guest allowlist fields are consumed by the iOS backend. Android has a different runtime configuration and validation path; do not assume iOS policy fields configure Android.

<a id="mount-recipes"></a>

## Mount a project without exposing runtime storage

1. Choose a stable guest coordinate with guest_paths::workspace(id), and supply the matching host workspace in runtime configuration.
2. Use MountPurpose::Workspace for the exact configured workspace pair. Additional external data needs its own mount purpose and both host and guest allowlist coverage on iOS.
3. Reject requests that replace /home/lingxi, /root or protected managed storage. The managed-home workspace exception applies only to the exact canonical host twin of that workspace.
4. Use the current mount snapshot for presentation translation. map_guest_path_to_host and map_host_path_to_guest are lexical longest-prefix mappings; they do not themselves authorize file writes.

For host file access, resolve_guest_path additionally honors a matching mount’s read_only bit and rejects guest-owned paths without a safe host mapping. Do not fall back from an unmapped guest /home or /root path to an unrestricted native filesystem path.

<a id="network-and-limits"></a>

## Network and resource enforcement

NetworkPolicy has Disabled, LoopbackOnly and Allowed. ResourceLimits includes optional CPU seconds, resident memory, process count and open-file ceilings. Requested values do not prove enforcement; inspect the receipt or handle typed errors when the backend cannot apply a required constraint.

### mobile_linux.rs · enforcement receipt excerpt

```rust
pub struct LinuxEnforcementReceipt {
    pub network_policy_enforced: bool,
    pub memory_limit_enforced: bool,
}
```

Verification method: API excerpt

### LinuxEnforcementReceipt::ensure_for

```text
pub fn ensure_for(self, network: NetworkPolicy, limits: ResourceLimits) -> Result<(), MobileLinuxError>
```

Requires network_policy_enforced for policies stricter than Allowed and memory_limit_enforced for any configured memory ceiling. Missing proof returns NetworkPolicyUnavailable or ResourceLimitExceeded.

<a id="rootfs-trust"></a>

## Archive trust and executable authorization

Pin the expected archive SHA-256 at the application boundary. The manifest is not a replacement for that trust decision. Verify archive identity, immutable inventory, symlink paths and SBOM before activation. RootfsStore.authorize_manifest_file validates immutable executable allowlist entries and rejects writable guest locations.

Default guest writable roots are /home/lingxi, /root, /tmp, /var/tmp and /home/lingxi/lingxi. Keep managed runtime storage app-private and separate from externally mounted user workspaces.

<a id="errors"></a>

## Errors and platform limits

### MobileLinuxError::Unsupported

```text
Unsupported
```

The selected backend does not implement the requested operation.

### MobileLinuxError::Unavailable

```text
Unavailable(String)
```

The backend cannot execute now; inspect the detail and capability reason.

### MobileLinuxError::RestartRequired

```text
RestartRequired(String)
```

A fresh host process is required because the native kernel is already initialized.

### MobileLinuxError::Integrity

```text
Integrity(String)
```

Rootfs verification failed.

### MobileLinuxError::InvalidRequest

```text
InvalidRequest(String)
```

A path, ABI, identity or operation argument failed validation.

### MobileLinuxError::NetworkPolicyUnavailable

```text
NetworkPolicyUnavailable(String)
```

Required network policy could not be enforced.

### MobileLinuxError::ResourceLimitExceeded

```text
ResourceLimitExceeded(String)
```

A requested limit could not be guaranteed or was exceeded.

An iOS simulator uses an unavailable backend. App-level operating-system permissions and physical-device acceptance remain host responsibilities.

<a id="recovery-decisions"></a>

## Choose recovery from the typed error

| Error / state | Interpretation | Recovery |
| --- | --- | --- |
| InvalidRequest | The concrete request or runtime identity is invalid. | Fix the indicated argument, path, mount pair, raw read size or immutable iOS configuration; retrying unchanged inputs will not help. |
| Integrity / Corrupt | The selected image or immutable inventory failed verification. | Restore the pinned verified resource, then repair the managed rootfs. Keep project data outside image replacement. |
| Unavailable / Unsupported | The current backend/device cannot provide execution or the selected surface. | Inspect capability.reason and choose a supported device/surface explicitly. A simulator does not become supported by retrying boot. |
| NetworkPolicyUnavailable / ResourceLimitExceeded | A required policy cannot be guaranteed, or its resource budget was exceeded. | Report the named guarantee and keep the chosen admission policy. Ask the host to change the operation or budget deliberately. |
| RestartRequired | The initialized iSH kernel cannot perform the requested repair/reset in this process. | Persist the recovery intent and restart the host process before modifying rootfs. |
| Timeout or timed_out result | A configured operation budget expired; the exact return form belongs to the selected API. | Show timeout separately from a nonzero exit, await cleanup, and inspect the command budget before retrying. |

- [Android diagnosis](mobile-android.md#android-troubleshooting)
- [iOS kernel and session identity](mobile-ios.md#ios-session-identity)

<a id="attribution"></a>

## Distribution notices

SDK Rust source retains MIT OR Apache-2.0, with explicit crate/source exceptions. Full distributions contain third-party components under separate terms, including GPL/LGPL code. Preserve the component notices, source revisions, patch provenance and rootfs SBOM licenses when distributing SDK artifacts.

<a id="policy-decision-flow"></a>

## Admission and enforcement flow

An application permission decision admits an operation; runtime validation then checks its concrete path, mount, network and resource request. These are distinct decisions. When a required guarantee is unavailable, return a typed failure before accepting output rather than interpreting compatibility-layer execution as isolation.

For finite commands inspect the returned enforcement receipt. Long-lived process/PTY/raw handles also carry enforcement identity in the portable contract; validate it when admitting a session. ResourceLimits lists requested ceilings, but its portable proof currently has only network_policy_enforced and memory_limit_enforced. Do not invent receipt bits for CPU, process count or file descriptors.

1. Authorize the user action and choose explicit policy.
2. Validate absolute paths and platform mount boundaries.
3. Verify the image and admit execution through the selected backend.
4. Check the typed result/handle receipt, output and terminal state.

<a id="negative-acceptance"></a>

## Failure scenarios to test

Corrupt the pinned archive or an immutable file: staging/verify must report integrity failure, and no unverified image may become active. Request a writable guest executable through immutable authorization: it must be rejected. Restore valid staging and test repair without deleting user workspace data.

Request mount ancestors of protected paths, a mismatched stable workspace, missing allowlists, a zero-byte raw read, or raw stdin/timeout fields. Check typed rejection. Separately test network-disabled escape attempts and memory limits on actual devices, because host-only tests cannot establish native policy enforcement.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/crates/mobile-linux-api/src/mobile_linux.rs)

- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-api/src/execution.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-api/src/execution.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/verification.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-core/src/store/verification.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/install.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-core/src/store/install.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/validation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/validation.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/mounts.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-android/src/runtime/mounts.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/scripts/tests/test_resource_contracts.py](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/scripts/tests/test_resource_contracts.py) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-core/src/guest_path.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-core/src/guest_path.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-core/src/user_workspace.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-core/src/user_workspace.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/lifecycle.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/lifecycle.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/config.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/config.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)

- mobile-linux-runtime: HEAD d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9; Referenced input fingerprint sha256:c85076a3035f8a036929b8e4a66cfae06a1e6eac952ef5315ba1cba3470d2139; Product pin d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
