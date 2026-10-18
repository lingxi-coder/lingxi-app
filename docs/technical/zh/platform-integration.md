# 原生宿主集成

了解产品的 iOS 与 Android UniFFI 包装、生成绑定、原生回调及共享移动会话宿主。

<a id="host-boundary"></a>

## 产品包装与独立 SDK

apps/ios/ffi 与 apps/android/ffi 围绕 Harness Runtime 装配 LingXi 产品的平台能力。它们是由产品维护的 Rust 包 ios-framework 和 android-aar，不是单独发布的通用 Swift 或 Kotlin SDK。

两端都从 harness_runtime::mobile 重导出 MobileEngineHandle 与 MobileEngineError。共享宿主拥有自己的 Tokio runtime，接收 ClientCommand，并通过已注册的监听器发送结果；平台包装提供原生回调与启动配置。

新建独立应用时，先选择 Harness facade 或独立 Mobile Linux SDK；扩展或嵌入 LingXi 产品宿主时使用本页。桌面与 Node 宿主使用 Bridge 传输，原生移动宿主调用生成的 FFI 绑定。

<a id="generate-bindings"></a>

## 与宿主一起构建绑定

- iOS 生成器将 Swift 绑定、FFI 头文件和 module map 写入 apps/ios/native/Generated，再生成包含真机与模拟器切片的 apps/ios/native/Frameworks/LingxiCodeFFI.xcframework。
- Android 生成器为 arm64-v8a 与 x86_64 构建 libandroid_aar.so，写入选定的 play 或 direct JNI source set，并按各组件的 UniFFI 配置生成 Kotlin 绑定。
- 绑定与原生库必须来自同一次源码构建。先修改 Rust 契约，再重新生成当前原生绑定；不要手工编辑生成文件，也不要复制历史 apps/ios/ffi/swift 与 apps/android/ffi/kotlin scaffold。

### iOS／Xcode

```bash
bash apps/ios/native/scripts/build-xcframework.sh
cd apps/ios/native
xcodegen generate
```

验证方式: 接口摘录

### Android／Gradle

```bash
bash apps/android/native/scripts/build-jni.sh --variant play
```

验证方式: 接口摘录

从产品仓库根目录运行构建命令，使用仓库固定的 Rust 工具链及对应的 Xcode 或 Android SDK/NDK。生成或链接模拟器绑定不能证明 Linux guest 已在真机执行。

<a id="construct-host"></a>

## 创建原生宿主

当前 iOS 构造函数是 apps/ios/ffi/src/host.rs 中的 build_ios_engine_with_config；Android 构造函数是 apps/android/ffi/src/host.rs 中的 build_android_engine。配置记录包含 provider 选择、应用私有存储、项目上下文、宿主环境与可选 Mobile Linux 配置。

下方签名是 Rust 源码契约，Swift 与 Kotlin 的名称由 UniFFI 生成。原生宿主负责构造回调，在提交命令与消费监听事件期间保留返回的 engine handle。

### iOS 构造函数契约

```rust
pub fn build_ios_engine_with_config(
    config: IosEngineLaunchConfigFfi,
    listener: Box<dyn IosEventListener>,
    audio: Box<dyn IosAudioService>,
    camera: Box<dyn IosCamera>,
    share: Box<dyn IosShare>,
    notifications: Box<dyn IosNotification>,
    clipboard: Box<dyn IosClipboard>,
    permissions: Box<dyn IosPermissionSink>,
    secure_storage: Option<Box<dyn IosSecureStorage>>,
    device_control: Option<Box<dyn IosDeviceControl>>,
    location: Option<Box<dyn IosLocation>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError>
```

验证方式: 接口摘录

### Android 构造函数契约

```rust
pub fn build_android_engine(
    config: AndroidEngineLaunchConfigFfi,
    listener: Box<dyn AndroidEventListener>,
    audio: Box<dyn AndroidAudioService>,
    camera: Box<dyn AndroidCamera>,
    share: Box<dyn AndroidShare>,
    location: Box<dyn AndroidLocation>,
    notifications: Box<dyn AndroidNotification>,
    clipboard: Box<dyn AndroidClipboard>,
    permissions: Box<dyn AndroidPermissionSink>,
    computer_use: Option<Box<dyn AndroidComputerUseHost>>,
    shell: Option<AndroidShellConfigFfi>,
    git: Option<AndroidGitConfigFfi>,
    git_credential_provider: Option<Box<dyn AndroidGitCredentialProvider>>,
    secure_storage: Option<Box<dyn AndroidSecureStorage>>,
    device_control: Option<Box<dyn AndroidDeviceControl>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError>
```

验证方式: 接口摘录

<a id="callbacks-and-commands"></a>

## 原生回调与命令流

IosAudioService 与 AndroidAudioService 使用相同的操作模型。capabilities() 只返回当前可用性快照，不发起权限请求；execute(request) 返回结构化终态结果；cancel(identity) 只取消匹配的等待操作。原生实现负责音频设备与系统权限展示。

相机、分享、通知、剪贴板、安全存储、设备控制与定位各有回调契约。事件监听器与权限回调将原生界面连接到共享会话宿主；凭证在运行时通过宿主配置或安全存储提供，不编译进绑定。

### Swift／现有 MobileEngineHandle

```swift
try await handle.submit(command: .listModels)
try await handle.submit(command: .getConversationControls)
```

验证方式: 接口摘录

### Kotlin／现有 MobileEngineHandle

```kotlin
handle.submit(ClientCommand.ListModels)
handle.submit(ClientCommand.ListSessions(limit = null))
```

验证方式: 接口摘录

示例假定已经创建配置完整的 handle，并导入原生应用使用的生成组件。submit 接收命令，界面输出通过已注册的事件监听器异步到达；原生宿主负责界面挂接、后台生命周期、权限决定与恢复。

<a id="native-facade"></a>

## 公开原生 facade 与共享归属

生成绑定暴露带注解的 Rust facade，不是产品仓库中的每个声明。Engine 构造函数建立 Arc<MobileEngineHandle>，并将产品回调转换为共享 Harness 服务。build_android_cron_store/build_ios_cron_store 创建计划任务存储时不构造 LLM client、不读取 provider 凭证，但仍执行托管项目路径校验。

Engine 启动配置指定 provider、模型、项目、应用私有路径与宿主环境。Android 构造会准备所要求的 PRoot 运行时；旧 shell 与 Git 配置字段不能替代该运行时。原生安全存储回调提供凭证访问，UI 接收可展示状态。宿主能力注册决定暴露的工具，缺少定位／相机／自动化服务时不能报告为支持。

| Facade | 结果／消费者职责 |
| --- | --- |
| build_ios_engine_with_config / build_android_engine | 创建并保留 engine 及所有原生回调实现。 |
| MobileEngineHandle.submit(ClientCommand) | 等待调用结果 ClientError；已接受命令输出通过监听回调到达。 |
| MobileEngineHandle.mobile_linux_status / verify_mobile_linux_rootfs / repair_mobile_linux_rootfs / reset_mobile_linux_rootfs | 读取或改变托管镜像生命周期；展示类型化错误及重启要求。 |
| IosEventListener / AndroidEventListener | on_event 将 DTO 入队；on_workflow_progress 携带归属于会话／任务／运行的结构化进度。 |
| IosPermissionSink / AndroidPermissionSink | 将提示入队并及时返回；随后通过 submit 处理决定。 |
| IosAudioService / AndroidAudioService | 读取能力快照、执行结构化操作、按完整身份取消。 |

<a id="native-lifecycle"></a>

## 界面挂接、音频身份与销毁

原生监听器须向 UI executor 入队并及时返回；在 on_request 内等待用户决定会阻塞调用边界。与 engine 一起注册并保留权限 sink，再通过 ClientCommand 回答。UI 替换使用新的 source generation 并关闭旧 source，迟到回调必须仍归属于原始会话／source，不能更新替换后的对话。

AudioOperationIdDto 组合 UUID id、单调 generation 与 AudioService service_epoch。AudioOwnerDto 表示稳定会话、UI 或系统资源归属，initiator 调用归因独立。capabilities 不能弹出权限请求。execute 在剩余超时预算内返回一个结构化终态结果，cancel 必须匹配完整身份，避免过期请求停止替换服务的新操作。

释放 engine 会终止任务通知 watcher，并在进程 automation runtime 上安排有预算限制的归属／音频、会话 Cron 与 LSP 清理。Drop 不能同步等待需要 Swift MainActor 的回调；原生 source 先负责关闭和解除 UI 挂接。应用进程与 iSH 内核生命周期仍独立于会话句柄。

1. 在 UI 线程之外初始化应用私有存储、资源路径及运行时配置。
2. 构造原生服务、事件监听器及权限 sink；建立会话 engine。
3. 提交类型化命令、将监听事件入队，并按原始 source 关联交互。
4. 替换／销毁时关闭 source、结束归属音频，并丢弃过期回调 generation。

<a id="native-failures-and-acceptance"></a>

## 原生失败处理与验收

绑定／库不匹配属于构建契约故障，应从同一源码修订重新生成二者。非目标平台构造函数返回 PlatformUnavailable；模拟器可实例化产品 API 路径，但 Linux 后端仍不可用。rootfs 缺失、系统权限拒绝、provider 认证失败及命令拒绝是不同故障，应保留其类型化上下文。

宿主单元测试验证 adapter 与共享会话行为；生成 Swift／Kotlin 编译检查跨组件转换器。已签名设备验收须覆盖回合流式事件、权限拒绝／允许／取消、问卷响应、相机／分享回调、音频 epoch 替换、会话销毁及 rootfs 重启行为。本手册提供契约与可重复命令，不声称文档示例已经验收这些实体设备。

### 原生 facade 宿主编译

```bash
cargo check --locked -p ios-framework --features uniffi
cargo check --locked -p android-aar --features uniffi
```

验证方式: 需环境验证

- [原生构建与发布矩阵](platform-builds.md)
- [Mobile Linux 生命周期](mobile-linux.md)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/lingxi-app/tree/main/apps)

- [lingxi-app/apps/ios/ffi/src/host.rs](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/ios/ffi/src/host.rs) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [lingxi-app/apps/android/ffi/src/host.rs](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/android/ffi/src/host.rs) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [lingxi-app/apps/ios/ffi/src/callbacks.rs](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/ios/ffi/src/callbacks.rs) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [lingxi-app/apps/android/ffi/src/callbacks.rs](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/android/ffi/src/callbacks.rs) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [lingxi-app/apps/ios/ffi/src/configuration.rs](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/ios/ffi/src/configuration.rs) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [lingxi-app/apps/android/ffi/src/configuration.rs](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/android/ffi/src/configuration.rs) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [lingxi-app/apps/ios/native/scripts/build-xcframework.sh](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/ios/native/scripts/build-xcframework.sh) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [lingxi-app/apps/android/native/scripts/build-jni.sh](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/android/native/scripts/build-jni.sh) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- lingxi-app/apps/ios/native/Sources/Conversation/EngineConversationSource.swift — 未提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [lingxi-app/apps/android/native/app/src/main/java/com/lingxi/code/conversation/EngineConversationSource.kt](https://github.com/lingxi-coder/lingxi-app/blob/412953aaf0354442447172e229f8e8f52b67b90d/apps/android/native/app/src/main/java/com/lingxi/code/conversation/EngineConversationSource.kt) — 已提交源码 (412953aaf0354442447172e229f8e8f52b67b90d)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/mobile/host.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/client/src/protocol/audio.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/client/src/protocol/audio.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)

- lingxi-app: HEAD 412953aaf0354442447172e229f8e8f52b67b90d; 所据文件指纹 sha256:9599eff51b5078b439c85bc3d8d22b4780569bb284e75cc3b493c047a79aa30f
- harness-runtime: HEAD 6548b9d5e702f8e9d42772412cd65b72881e24b4; 所据文件指纹 sha256:92c43ec1558ef6090bd3e0c28f21110761b7855079f22272b843555f562fe25b; 产品固定版本 6548b9d5e702f8e9d42772412cd65b72881e24b4

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
