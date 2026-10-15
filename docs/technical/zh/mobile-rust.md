# Rust API

命令、任务生命周期、PTY、原始标准输入输出、挂载与 rootfs 校验的共享契约。

<a id="crates"></a>

## Rust 接入

从同一个 Git 修订版本选择 mobile-linux-api、mobile-linux-core，以及 mobile-linux-android 或 mobile-linux-ios。SDK 自有的 platform-pty crate 提供共享 PTY 支持。将 native-support 产物配合已有 Rust 库使用，保持进程内只有一份运行时。

### AndroidProotRuntimeConfig

```text
pub struct AndroidProotRuntimeConfig
```

调用方提供托管根目录、应用沙箱根目录、ABI、rootfs 版本与哈希、原生库目录，以及可选的隔离构建配置。

### IosIshRuntime::new

```text
pub fn new(mut config: IosIshRuntimeConfig) -> Result<Self, MobileLinuxError>
```

构造会话时不改变文件系统或原生状态，并校验进程级内核身份。

<a id="working-tree-consumer"></a>

## 消费者工作区与源码身份

在 mobile-linux-runtime checkout 旁创建独立消费者目录，并使用这份完整 Cargo.toml。空 workspace 使消费者脱离父产品工作区；直接 path 依赖绑定当前 SDK 工作树。将本页 Rust 生命周期函数保存到 src/lib.rs，并从消费者根目录运行 cargo check。只有明确选择其他源码快照时才替换 SDK 路径。

此示例仅依赖可移植 mobile-linux-api 契约。构造平台运行时还需要相同 revision 的 Android 或 iOS 后端及匹配 native-support 产物。编译此消费者不会启动 Linux、安装 rootfs、执行 UniFFI 或证明真机执行。

### Cargo.toml · 完整工作树消费者

```toml
[package]
name = "handbook-mobile-consumer"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
mobile-linux-api = { path = "../mobile-linux-runtime/crates/mobile-linux-api" }

```

验证方式: 需环境验证 (mobile-working-tree-manifest)

<a id="commands"></a>

## 命令请求与结果

LinuxCommandRequest 包含可执行文件、参数、guest 工作目录、环境变量、可选文本 stdin 与超时、显式网络策略、资源限制及挂载。LinuxCommandResult 返回收集的文本、退出码、超时与取消标记，以及策略执行回执。

### 请求声明摘录

```rust
pub struct LinuxCommandRequest {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub stdin: Option<String>,
    pub timeout_ms: Option<u64>,
    pub network: NetworkPolicy,
    #[serde(default)]
    pub resource_limits: ResourceLimits,
    pub mounts: Vec<MountSpec>,
}
```

验证方式: 接口摘录

### MobileLinuxRuntime::run

```text
async fn run(&self, request: LinuxCommandRequest) -> Result<LinuxCommandResult, MobileLinuxError>
```

在移动后端运行命令直至完成。

### MobileLinuxRuntime::run_streaming

```text
async fn run_streaming(&self, request: LinuxCommandRequest, sink: Arc<dyn ProcessStreamSink>) -> Result<LinuxCommandResult, MobileLinuxError>
```

通过 sink 传递 stdout 行与 stderr 字节块，并返回最终结果。

<a id="task-lifecycle"></a>

## 运行时与后台任务

### MobileLinuxRuntime::probe_capability

```text
async fn probe_capability(&self) -> MobileLinuxCapability
```

在不改变状态的情况下探测可用性，以及流式输出、后台进程、PTY、挂载和完整性支持。

### MobileLinuxRuntime::boot

```text
async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError>
```

确保后端已准备好接收请求。

### MobileLinuxRuntime::shutdown

```text
async fn shutdown(&self) -> Result<(), MobileLinuxError>
```

停止会话并释放临时资源；iOS 原生内核仍具有进程级生命周期。

### MobileLinuxRuntime::spawn_background

```text
async fn spawn_background(&self, request: LinuxCommandRequest) -> Result<LinuxProcessHandle, MobileLinuxError>
```

启动托管后台命令并保留其不透明句柄。

### MobileLinuxRuntime::kill

```text
async fn kill(&self, handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError>
```

成功返回表示整个 guest 进程组或进程树已被回收，该任务的后代不再可运行。

<a id="pty"></a>

## PTY 会话

PTY 输入输出使用字节缓冲区。通过 PtySize { cols, rows } 设置以字符单元为单位的终端尺寸；通过 read_events 消费 PtyOutput 与 PtyClosed 事件。

### MobileLinuxRuntime::open_pty

```text
async fn open_pty(&self, request: PtyOpenRequest) -> Result<PtySessionHandle, MobileLinuxError>
```

打开交互式终端进程。

### MobileLinuxRuntime::write_pty

```text
async fn write_pty(&self, handle: &PtySessionHandle, input: Vec<u8>) -> Result<(), MobileLinuxError>
```

写入终端输入字节。

### MobileLinuxRuntime::resize_pty

```text
async fn resize_pty(&self, handle: &PtySessionHandle, size: PtySize) -> Result<(), MobileLinuxError>
```

调整终端尺寸。

### MobileLinuxRuntime::close_pty

```text
async fn close_pty(&self, handle: &PtySessionHandle) -> Result<(), MobileLinuxError>
```

关闭 PTY，在成功返回前回收它的整个 guest 进程树。

<a id="raw-stdio"></a>

## 原始标准输入输出会话

使用 RawStdioOpenRequest 创建具有独立二进制 stdin/stdout/stderr 的长期运行进程。此接口为 LSP 等协议保留精确字节。不支持该能力的实现会返回错误，不会替换成 PTY。

### 原始 IO 请求摘录

```rust
pub struct RawStdioOpenRequest {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub network: NetworkPolicy,
    #[serde(default)]
    pub resource_limits: ResourceLimits,
    pub mounts: Vec<MountSpec>,
}
```

验证方式: 接口摘录

### MobileLinuxRuntime::open_raw_stdio

```text
async fn open_raw_stdio(&self, request: RawStdioOpenRequest) -> Result<RawStdioSessionHandle, MobileLinuxError>
```

打开二进制标准 IO 进程。

### MobileLinuxRuntime::write_raw_stdio

```text
async fn write_raw_stdio(&self, handle: &RawStdioSessionHandle, input: Vec<u8>) -> Result<(), MobileLinuxError>
```

写入原始 stdin 字节。

### MobileLinuxRuntime::read_raw_stdio

```text
async fn read_raw_stdio(&self, handle: &RawStdioSessionHandle, max_bytes: usize) -> Result<RawStdioReadResult, MobileLinuxError>
```

轮询有上限的 stdout/stderr 字节，并检查 closed 与 exit_code。

### MobileLinuxRuntime::close_raw_stdio

```text
async fn close_raw_stdio(&self, handle: &RawStdioSessionHandle) -> Result<(), MobileLinuxError>
```

关闭会话并同步回收其整个 guest 进程树。

FFI 版本接收 MobileLinuxCommandRequestFfi，拒绝已设置的 stdin 或 timeout_ms，并要求读取 max_bytes 在 1..1048576 范围内。通过返回的会话 ID 控制写入与生命周期。

<a id="events"></a>

## 事件与任务快照

### MobileLinuxRuntime::read_events

```text
async fn read_events(&self, after_sequence: Option<u64>, limit: usize) -> Result<Vec<MobileLinuxEvent>, MobileLinuxError>
```

读取严格晚于指定序号游标的事件。MAX_MOBILE_LINUX_EVENT_BATCH 为 512。

### MobileLinuxRuntime::list_tasks

```text
async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError>
```

列举已知任务与会话，以及它们的状态和完成详情。

### MobileLinuxRuntime::task_status

```text
async fn task_status(&self, task_id: &str) -> Result<Option<MobileLinuxTaskSnapshot>, MobileLinuxError>
```

按 ID 读取任务；None 表示没有匹配任务。

<a id="mounts"></a>

## 挂载与路径转换

### MobileLinuxRuntime::configure_mounts

```text
async fn configure_mounts(&self, mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError>
```

通过平台策略校验后替换活动 bind mount 集合。

### MobileLinuxRuntime::current_mounts

```text
fn current_mounts(&self) -> Vec<MountSpec>
```

返回当前 guest 到 host 的挂载表。

### map_guest_path_to_host

```text
pub fn map_guest_path_to_host(path: &str, mounts: &[MountSpec]) -> Option<std::path::PathBuf>
```

使用最长匹配挂载前缀。纯词法映射不提供文件系统边界或只读执行约束。

### map_host_path_to_guest

```text
pub fn map_host_path_to_guest(path: &std::path::Path, mounts: &[MountSpec]) -> Option<String>
```

无需访问文件系统，将合法绝对 host 路径转换为 guest 坐标。

<a id="rootfs-store"></a>

## RootfsStore

mobile-linux-core 负责清单验证、安全暂存与原子激活。RootfsManifest 描述平台与 ABI、压缩包身份、不可变文件、可写路径、可执行文件白名单以及包与 SBOM 数据。

### RootfsStore::stage_prepared_rootfs

```text
pub fn stage_prepared_rootfs(&self, prepared_root: &Path, manifest: &RootfsManifest) -> Result<PathBuf, RootfsStoreError>
```

在发布暂存目录前，根据 manifest 验证准备好的目录。

### RootfsStore::activate_staged_rootfs

```text
pub fn activate_staged_rootfs(&self, manifest: &RootfsManifest) -> Result<RootfsStatus, RootfsStoreError>
```

恢复中断的激活流程、验证暂存内容，并原子激活镜像。

### RootfsStore::authorize_manifest_file

```text
pub fn authorize_manifest_file(&self, guest_path: &str, manifest: &RootfsManifest) -> Result<PathBuf, RootfsStoreError>
```

验证白名单中的不可变可执行文件；拒绝可写 guest 路径和白名单以外的条目。

<a id="complete-command"></a>

## 完整命令生命周期示例

此复用函数接收已经配置的平台 trait，启动后执行有时间上限的命令、验证网络执行回执，并在执行后始终尝试 shutdown。非零退出码仍作为命令结果返回，调用方须检查 exit_code、timed_out 与 cancelled 后才能判断成功。构造和 rootfs 分发仍由宿主提供。

### Rust 消费者函数

```rust
use mobile_linux_api::{
    LinuxCommandRequest, LinuxCommandResult, MobileLinuxError,
    MobileLinuxRuntime, NetworkPolicy, ResourceLimits,
};
use std::collections::BTreeMap;

pub async fn execute_once(
    runtime: &dyn MobileLinuxRuntime,
) -> Result<LinuxCommandResult, MobileLinuxError> {
    runtime.boot().await?;
    let network = NetworkPolicy::Disabled;
    let limits = ResourceLimits::default();
    let execution = runtime.run(LinuxCommandRequest {
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "printf 'ready\n'".into()],
        cwd: Some("/root".into()),
        env: BTreeMap::new(),
        stdin: None,
        timeout_ms: Some(10_000),
        network,
        resource_limits: limits,
        mounts: Vec::new(),
    }).await;
    let cleanup = runtime.shutdown().await;
    let result = execution?;
    cleanup?;
    result.enforcement.ensure_for(network, limits)?;
    Ok(result)
}
```

验证方式: 编译检查 (mobile-command-lifecycle)

<a id="io-ownership"></a>

## IO 选择、取消与事件游标

run 用于收集文本，run_streaming 用于增量 stdout 行与 stderr 字节块，PTY 用于终端，raw stdio 用于字节分帧协议。PTY 的终端处理会改变字节，因此 LSP 或二进制协议需要 raw stdio。FFI 拒绝长度为零的 raw read，不能用它探测就绪。

只为 read_events 保留序号游标，并推进到本批最大 sequence。UniFFI 流式 sink 合成每次调用的事件，sequence 为 0，任务标识为 run-N；这些回调序号不是后端事件日志游标。界面重连后通过任务快照对齐当前状态；进程输出并非持久会话历史。

托管后台、PTY 与 raw 句柄拥有 guest 进程。显式 kill/close 在归属进程树回收后才返回。iOS 传输层具有前台 future 丢弃、超时及 shutdown 专项测试，但仅取消 future 不能承诺应用层移动执行可暂停或恢复。

| 接口 | 输出／归属 | 结束方式 |
| --- | --- | --- |
| run / run_streaming | 单次等待操作，终态结果携带执行回执。 | 完成、超时或归属取消。 |
| spawn_background | 不透明 LinuxProcessHandle 加任务快照。 | kill(handle)，再检查任务终态。 |
| open_pty | 终端字节及 PtyOutput/PtyClosed 事件。 | close_pty(handle)。 |
| open_raw_stdio | 分离的二进制流与 closed/exit_code 读取。 | close_raw_stdio(handle)。 |

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/crates/mobile-linux-api/src/mobile_linux.rs)

- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-api/src/mobile_linux.rs) — 已提交源码 (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-api/src/execution.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-api/src/execution.rs) — 已提交源码 (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-ffi/src/runtime.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-ffi/src/runtime.rs) — 已提交源码 (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/install.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-core/src/store/install.rs) — 已提交源码 (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/lifecycle.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-core/src/store/lifecycle.rs) — 已提交源码 (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-android/src/runtime/implementation.rs) — 已提交源码 (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-ios/src/runtime/implementation.rs) — 已提交源码 (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/tests.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-ios/src/runtime/tests.rs) — 已提交源码 (b65f8856d773d27ebf73b0cade907884d3080704)

- mobile-linux-runtime: HEAD b65f8856d773d27ebf73b0cade907884d3080704; 所据文件指纹 sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; 产品固定版本 b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
