# Native host integration

Understand the product’s iOS and Android UniFFI wrappers, generated bindings, native callbacks, and shared mobile session host.

<a id="host-boundary"></a>

## Product wrappers and standalone SDKs

apps/ios/ffi and apps/android/ffi assemble the LingXi product’s platform capabilities around Harness Runtime. They are product-owned Rust packages, ios-framework and android-aar, not separately published general-purpose Swift or Kotlin SDKs.

Both wrappers re-export MobileEngineHandle and MobileEngineError from harness_runtime::mobile. The shared host owns its Tokio runtime, accepts ClientCommand, and sends results through the registered listener. The platform wrapper supplies the native callbacks and launch configuration.

For a new independent application, start with the Harness facade or the standalone Mobile Linux SDK. Follow this page when extending or embedding the LingXi product hosts. Desktop and Node hosts use the Bridge transport; native mobile hosts call their generated FFI bindings.

<a id="generate-bindings"></a>

## Build bindings with the host

- iOS generation writes Swift bindings, FFI headers, and module maps under apps/ios/native/Generated, then creates apps/ios/native/Frameworks/LingxiCodeFFI.xcframework with device and simulator slices.
- Android generation builds libandroid_aar.so for arm64-v8a and x86_64 in the selected play or direct JNI source set and generates the Kotlin component bindings using each component’s UniFFI configuration.
- Keep bindings and libraries from the same source build. Change the Rust contract first, then regenerate active native bindings; do not hand-edit generated files or copy the historical apps/ios/ffi/swift and apps/android/ffi/kotlin scaffolds.

### iOS / Xcode

```bash
bash apps/ios/native/scripts/build-xcframework.sh
cd apps/ios/native
xcodegen generate
```

Verification method: API excerpt

### Android / Gradle

```bash
bash apps/android/native/scripts/build-jni.sh --variant play
```

Verification method: API excerpt

Run build commands from the product repository root with its pinned Rust toolchain and the platform’s Xcode or Android SDK/NDK prerequisites. Generating or linking simulator bindings does not validate Linux guest execution on a physical device.

<a id="construct-host"></a>

## Construct the native host

The actual iOS constructor is build_ios_engine_with_config in apps/ios/ffi/src/host.rs. The Android constructor is build_android_engine in apps/android/ffi/src/host.rs. Their configuration records carry provider selection, app-private storage, project context, host environment, and optional Mobile Linux settings.

The signatures below are the Rust source contracts; UniFFI generates the Swift and Kotlin spellings. Construct callbacks in the native host and retain the returned engine handle while submitting commands and consuming listener events.

### iOS constructor contract

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

Verification method: API excerpt

### Android constructor contract

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

Verification method: API excerpt

<a id="callbacks-and-commands"></a>

## Native callbacks and command flow

IosAudioService and AndroidAudioService expose the same operation model. capabilities() returns the current readiness snapshot without requesting permission; execute(request) returns a structured terminal result; cancel(identity) cancels only the matching pending operation. Native implementations own audio devices and OS permission presentation.

Camera, sharing, notifications, clipboard, secure storage, device control, and location have their own callback contracts. Listener and permission callbacks connect the native UI to the shared session host. Credentials are provided at runtime through host configuration or secure storage, not compiled into bindings.

### Swift / existing MobileEngineHandle

```swift
try await handle.submit(command: .listModels)
try await handle.submit(command: .getConversationControls)
```

Verification method: API excerpt

### Kotlin / existing MobileEngineHandle

```kotlin
handle.submit(ClientCommand.ListModels)
handle.submit(ClientCommand.ListSessions(limit = null))
```

Verification method: API excerpt

These snippets assume an existing configured handle and the generated component imports used by the native app. submit accepts a command; UI output arrives asynchronously through the registered event listener. The native host owns screen attachment, background lifecycle, permission decisions, and recovery.

<a id="native-facade"></a>

## Public native facade and shared ownership

Generated bindings expose the annotated Rust facade, not every declaration in the product repository. Engine constructors build an Arc<MobileEngineHandle> and translate product callbacks into the shared Harness services. build_android_cron_store/build_ios_cron_store create scheduled-task storage without constructing an LLM client or reading provider credentials; their managed project-path validation still applies.

Engine launch configuration names provider, model, project, app-private paths and host environment. Android construction prepares its required PRoot runtime; legacy shell and Git configuration fields do not replace that runtime. Native secure-storage callbacks provide credential access while the UI receives display-safe state. Host capability registration determines which tools are exposed, so an absent location/camera/automation service must not be reported as supported.

| Facade | Result / consumer responsibility |
| --- | --- |
| build_ios_engine_with_config / build_android_engine | Create and retain the engine plus every native callback implementation. |
| MobileEngineHandle.submit(ClientCommand) | Await call-result ClientError; receive accepted command output through listener callbacks. |
| MobileEngineHandle.mobile_linux_status / verify_mobile_linux_rootfs / repair_mobile_linux_rootfs / reset_mobile_linux_rootfs | Read or change the managed image lifecycle; expose typed errors and restart requirements. |
| IosEventListener / AndroidEventListener | on_event enqueues DTOs; on_workflow_progress carries session/task/run-owned structured progress. |
| IosPermissionSink / AndroidPermissionSink | Enqueue the prompt and return promptly; resolve later through submit. |
| IosAudioService / AndroidAudioService | Snapshot capabilities, execute a structured operation, cancel its full identity. |

<a id="native-lifecycle"></a>

## Screen attachment, audio identity and disposal

Native listeners must enqueue onto the UI executor and return promptly; waiting for a user decision inside on_request would block the call boundary. Register and retain the permission sink with the engine, then answer through ClientCommand. UI replacement uses a new source generation and closes the previous source; late callbacks must remain attached to their original session/source rather than updating the replacement conversation.

AudioOperationIdDto combines a UUID id, monotonic generation and AudioService service_epoch. AudioOwnerDto describes stable session, UI or system resource ownership; initiator attribution is separate. Capabilities must not prompt for permission. Execute returns one structured terminal result within the remaining timeout budget, and cancel must match the complete identity so an obsolete request cannot stop a replacement service operation.

Releasing the engine aborts its task notification watcher and schedules bounded owner/audio, session Cron and LSP cleanup on the process automation runtime. Drop must not synchronously join a callback that needs Swift MainActor; the native source owns closing and detaching its UI first. An app process lifetime and the iSH kernel lifetime remain independent of a conversation handle.

1. Initialize app-private storage, resource paths and runtime configuration off the UI thread.
2. Construct native services, event listener and permission sink; build one session engine.
3. Submit typed commands, enqueue listener events, and correlate interactions with their original source.
4. Close the source on replacement/disposal, end owned audio and discard stale callback generations.

<a id="native-failures-and-acceptance"></a>

## Native failure handling and acceptance

Treat binding/library mismatch as a build contract failure and regenerate both from the same source revision. Off-platform constructors return PlatformUnavailable; a simulator can instantiate product API paths while its Linux backend remains unavailable. Missing rootfs, denied OS permission, provider authentication failure and command rejection are different failures and should preserve their typed context.

Host unit tests verify adapters and shared-session behavior; generated Swift/Kotlin compilation checks cross-component converters. A signed-device acceptance run must cover turn streaming, permission deny/allow/cancel, question responses, camera/share callbacks, audio epoch replacement, session disposal and rootfs restart behavior. This handbook provides contracts and repeatable commands, without claiming that documentation examples have validated those physical devices.

### Native facade host compilation

```bash
cargo check --locked -p ios-framework --features uniffi
cargo check --locked -p android-aar --features uniffi
```

Verification method: Environment validation

- [Native build and release matrix](platform-builds.md)
- [Mobile Linux lifecycle](mobile-linux.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app/tree/main/apps)

- [lingxi-app/apps/ios/ffi/src/host.rs](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/ios/ffi/src/host.rs) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/android/ffi/src/host.rs](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/android/ffi/src/host.rs) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/ios/ffi/src/callbacks.rs](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/ios/ffi/src/callbacks.rs) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/android/ffi/src/callbacks.rs](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/android/ffi/src/callbacks.rs) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/ios/ffi/src/configuration.rs](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/ios/ffi/src/configuration.rs) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/android/ffi/src/configuration.rs](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/android/ffi/src/configuration.rs) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/ios/native/scripts/build-xcframework.sh](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/ios/native/scripts/build-xcframework.sh) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/android/native/scripts/build-jni.sh](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/android/native/scripts/build-jni.sh) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/ios/native/Sources/Conversation/EngineConversationSource.swift](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/ios/native/Sources/Conversation/EngineConversationSource.swift) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/android/native/app/src/main/java/com/lingxi/code/conversation/EngineConversationSource.kt](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/android/native/app/src/main/java/com/lingxi/code/conversation/EngineConversationSource.kt) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/e3e34efba151a8219e2b05406c3d229b5cc6314f/crates/runtime/src/mobile/host.rs) — Committed source (e3e34efba151a8219e2b05406c3d229b5cc6314f)
- [harness-runtime/crates/client/src/protocol/audio.rs](https://github.com/lingxi-coder/harness-runtime/blob/e3e34efba151a8219e2b05406c3d229b5cc6314f/crates/client/src/protocol/audio.rs) — Committed source (e3e34efba151a8219e2b05406c3d229b5cc6314f)

- lingxi-app: HEAD a7eacf9587076dfd9571b17f6640bc4ead1bba6b; Referenced input fingerprint sha256:b37c12336706c41006c29d34cf09f0c9a109330ac15afacc8fa7a858e2df4eab
- harness-runtime: HEAD e3e34efba151a8219e2b05406c3d229b5cc6314f; Referenced input fingerprint sha256:2d439c83e9c12bcb5aacfcf0a5da70a9d8beba68e9eba861d817bcf67c9bb51b; Product pin e3e34efba151a8219e2b05406c3d229b5cc6314f

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
