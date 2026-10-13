import type { DocPage } from './types';

// Authored bilingual content; source baselines are recorded by docs:sync.
export const mobilePages: DocPage[] = [
  {
    "id": "mobile-linux",
    "group": "mobile",
    "title": {
      "en": "Mobile Linux Runtime",
      "zh": "移动 Linux 运行时"
    },
    "description": {
      "en": "Embed a Linux userspace in Android and iOS apps, with commands, streaming, PTY, raw stdio and verified rootfs lifecycle.",
      "zh": "在 Android 与 iOS 应用内嵌入 Linux 用户空间，提供命令、流式输出、PTY、原始标准输入输出与经过校验的 rootfs 生命周期。"
    },
    "packageName": "mobile-linux-runtime",
    "sourceUrl": "https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/Cargo.toml",
    "sections": [
      {
        "id": "platforms",
        "title": {
          "en": "Supported platforms",
          "zh": "支持的平台"
        },
        "paragraphs": [
          {
            "en": "Mobile Linux Runtime is a standalone SDK. The application supplies paths, verified rootfs assets, mount and network policy, and lifecycle. Rust contracts and platform backends do not depend on Harness, an Agent, an LLM, or application preferences.",
            "zh": "Mobile Linux Runtime 是独立 SDK。应用负责提供路径、经过校验的 rootfs 资源、挂载与网络策略，以及生命周期。Rust 契约与平台后端不依赖 Harness、Agent、LLM 或应用偏好设置。"
          }
        ],
        "bullets": [
          {
            "en": "Android: PRoot backend; arm64-v8a and x86_64; application minimum API 26. Optional legacy host-shell helpers require API 29.",
            "zh": "Android：PRoot 后端；arm64-v8a 与 x86_64；应用最低 API 26。可选的旧版宿主 shell 辅助接口要求 API 29。"
          },
          {
            "en": "iOS: iSH arm64 backend on physical devices; minimum iOS 18.0. Simulator arm64 and x86_64 slices support linking and API integration checks, with no guest execution.",
            "zh": "iOS：真机使用 iSH arm64 后端；最低 iOS 18.0。arm64 与 x86_64 模拟器切片用于链接和 API 集成检查，不能执行 guest 命令。"
          }
        ]
      },
      {
        "id": "integration-mode",
        "title": {
          "en": "Choose your integration",
          "zh": "选择接入方式"
        },
        "bullets": [
          {
            "en": "Kotlin apps use mobile-linux-runtime with transitive installer and native-support AARs. Swift apps select MobileLinuxRuntime, which links the FFI and native-support XCFrameworks.",
            "zh": "Kotlin 应用使用 mobile-linux-runtime，以及传递依赖的 installer 和 native-support AAR。Swift 应用选择 MobileLinuxRuntime 产品，它链接 FFI 与 native-support XCFramework。"
          },
          {
            "en": "Apps that already embed Rust use SDK Rust crates from one Git revision and native-support-only artifacts. Android also needs mobile-linux-installer; iOS selects MobileLinuxNativeSupport.",
            "zh": "已嵌入 Rust 的应用使用同一个 Git 修订版本的 SDK Rust crates 与仅含原生支持的产物。Android 还需 mobile-linux-installer；iOS 选择 MobileLinuxNativeSupport。"
          },
          {
            "en": "Keep exactly one Rust runtime in the process. Native-support distributions contain platform glue and helpers rather than a second Rust library.",
            "zh": "进程内保留一份 Rust 运行时。native-support 分发包含平台桥接与辅助程序，不包含第二份 Rust 库。"
          }
        ]
      },
      {
        "id": "prerequisites",
        "title": {
          "en": "Build prerequisites",
          "zh": "构建前提"
        },
        "paragraphs": [
          {
            "en": "Source builds pin Rust 1.94.0 and require Python 3.12 or newer. Android uses the Gradle wrapper, JDK 21, SDK platform 37, NDK 27.2.12479018 and cargo-ndk 4.1.2. iOS requires macOS, Xcode with the iOS SDK, Meson and Ninja; SwiftPM wrappers use Swift tools 5.9.",
            "zh": "源码构建固定 Rust 1.94.0，并要求 Python 3.12 或更新版本。Android 使用 Gradle wrapper、JDK 21、SDK platform 37、NDK 27.2.12479018 与 cargo-ndk 4.1.2。iOS 需要 macOS、带 iOS SDK 的 Xcode、Meson 与 Ninja；SwiftPM 封装使用 Swift tools 5.9。"
          },
          {
            "en": "Run source checks from the SDK repository root. SDK_BUILD_DIR must be an absolute caller-owned directory outside the checkout; the SDK does not require a sibling application or Harness checkout.",
            "zh": "从 SDK 仓库根目录运行源码检查。SDK_BUILD_DIR 必须是源码检出目录以外、由调用方拥有的绝对路径；SDK 不要求相邻的应用或 Harness 仓库。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "SDK-INTEGRATION.md · source-check excerpt",
              "zh": "源码检查摘录"
            },
            "language": "bash",
            "code": ": \"${SDK_BUILD_DIR:?Set an absolute directory outside the source checkout}\"\nexport CARGO_TARGET_DIR=\"$SDK_BUILD_DIR/rust\"\ncargo test --locked -p mobile-linux-api -p mobile-linux-core -p platform-pty\nbash scripts/checks/check-dependencies.sh\nbash scripts/checks/check-resource-contracts.sh\nbash scripts/checks/check-resource-tests.sh",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "runtime-handle",
        "title": {
          "en": "Create one persistent handle",
          "zh": "创建持久运行时句柄"
        },
        "paragraphs": [
          {
            "en": "The UniFFI namespace is mobile_linux_runtime. Kotlin and Swift expose RuntimeHandle through their MobileLinuxRuntime wrapper. Retain that object while task, PTY or raw-stdio handles remain in use.",
            "zh": "UniFFI 命名空间为 mobile_linux_runtime。Kotlin 与 Swift 通过 MobileLinuxRuntime 封装暴露 RuntimeHandle。任务、PTY 或 raw-stdio 句柄仍在使用时，必须保留运行时对象。"
          }
        ],
        "apis": [
          {
            "name": "create_runtime",
            "signature": "pub fn create_runtime(config: RuntimeConfig) -> Result<Arc<RuntimeHandle>, MobileLinuxApiErrorFfi>",
            "description": {
              "en": "Creates a platform runtime. managed_root and app_sandbox_root must be absolute; ABI and rootfs version must be non-empty. iOS additionally requires workspace_host_path and stable_workspace_id.",
              "zh": "创建平台运行时。managed_root 与 app_sandbox_root 必须是绝对路径；ABI 与 rootfs 版本不能为空。iOS 还要求 workspace_host_path 和 stable_workspace_id。"
            }
          }
        ],
        "code": [
          {
            "label": {
              "en": "types.rs · RuntimeConfig declaration excerpt",
              "zh": "配置声明摘录"
            },
            "language": "rust",
            "code": "pub struct RuntimeConfig {\n    pub platform: RuntimePlatform,\n    pub managed_root: String,\n    pub app_sandbox_root: String,\n    pub abi: String,\n    pub rootfs_version: String,\n    pub archive_sha256: Option<String>,\n    pub native_library_dir: Option<String>,\n    pub workspace_host_path: Option<String>,\n    pub stable_workspace_id: Option<String>,\n    pub authorization_file: Option<String>,\n    pub rootfs_archive_path: Option<String>,\n    #[uniffi(default = None)]\n    pub rootfs_patch_path: Option<String>,\n    pub default_mount_path: Option<String>,\n    pub protected_host_roots: Vec<String>,\n    pub allowed_mount_roots: Vec<String>,\n    pub allowed_guest_roots: Vec<String>,\n}",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "rootfs-lifecycle",
        "title": {
          "en": "Verified rootfs lifecycle",
          "zh": "经过校验的 rootfs 生命周期"
        },
        "paragraphs": [
          {
            "en": "Rootfs archives are separate caller inputs and are not silently downloaded or bundled into the SDK. Pin the archive SHA-256 independently and supply the matching manifest and SBOM. Android stages verified tar.gz assets before repair and boot; iOS uses a converted iSH fakefs ZIP and explicit resource paths.",
            "zh": "rootfs 压缩包是调用方单独提供的输入，SDK 不会静默下载或内置它们。独立固定压缩包 SHA-256，并提供匹配的 manifest 与 SBOM。Android 先暂存经验证的 tar.gz，再 repair 和 boot；iOS 使用转换后的 iSH fakefs ZIP 与显式资源路径。"
          }
        ],
        "apis": [
          {
            "name": "RuntimeHandle::status",
            "signature": "pub async fn status(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi>",
            "description": {
              "en": "Read the rootfs lifecycle snapshot, version, paths and last error.",
              "zh": "读取 rootfs 生命周期快照、版本、路径与最近错误。"
            }
          },
          {
            "name": "RuntimeHandle::verify_rootfs",
            "signature": "pub async fn verify_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi>",
            "description": {
              "en": "Recompute integrity and return the current state.",
              "zh": "重新计算完整性并返回当前状态。"
            }
          },
          {
            "name": "RuntimeHandle::repair_rootfs",
            "signature": "pub async fn repair_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi>",
            "description": {
              "en": "Attempt non-destructive repair; Android activates a valid staged image.",
              "zh": "尝试非破坏性修复；Android 会激活有效的暂存镜像。"
            }
          },
          {
            "name": "RuntimeHandle::reset_rootfs",
            "signature": "pub async fn reset_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi>",
            "description": {
              "en": "Reset managed rootfs state while preserving external user data; handle RestartRequired on a booted iOS kernel.",
              "zh": "重置托管 rootfs 状态并保留外部用户数据；已启动的 iOS 内核可能返回 RestartRequired。"
            }
          }
        ]
      },
      {
        "id": "release-identity",
        "title": {
          "en": "Release identity",
          "zh": "发布产物身份"
        },
        "paragraphs": [
          {
            "en": "Use checksums and release-provenance.json from the specific release. Source builds, archive presence and simulator linking do not establish physical-device acceptance. Validation-only artifacts are deliberately not release artifacts.",
            "zh": "使用对应发布版本的 checksums 与 release-provenance.json。源码构建成功、压缩包存在或模拟器链接成功，不等同于真机验收。validation-only 产物不属于发布产物。"
          }
        ]
      },
      {
        "id": "lifecycle-flow",
        "title": {
          "en": "Installation and session flow",
          "zh": "安装与会话流程"
        },
        "paragraphs": [
          {
            "en": "Rootfs identity belongs to the embedding app. A version label describes the intended image; it does not substitute for the archive digest, manifest inventory or platform ABI. Keep immutable image assets separate from the writable workspace and guest home. The runtime owns execution handles after the host has admitted a request.",
            "zh": "rootfs 身份由嵌入应用决定。版本标签描述预期镜像，不能替代压缩包摘要、manifest 文件清单或平台 ABI。不可变镜像资源应与可写工作区和 guest home 分离；宿主准入请求后，由运行时拥有执行句柄。"
          },
          {
            "en": "Boot establishes readiness, while verify explicitly refreshes integrity. A status read is diagnostic data; repair and reset are deliberate lifecycle actions. An app should show the returned state and last_error, preserve user workspace data, and ask the runtime to repair only the managed image.",
            "zh": "boot 建立可执行状态，verify 显式刷新完整性。status 读取是诊断数据；repair 与 reset 是明确的生命周期操作。应用应展示返回的状态及 last_error、保留用户工作区，并只对托管镜像执行修复。"
          }
        ],
        "steps": [
          {
            "en": "Acquire the platform resource bundle and pin its archive digest and revision.",
            "zh": "获取平台资源包，固定其压缩包摘要与修订版本。"
          },
          {
            "en": "Stage and verify the rootfs; construct RuntimeConfig with app-private absolute roots.",
            "zh": "暂存并验证 rootfs；用应用私有绝对路径创建 RuntimeConfig。"
          },
          {
            "en": "Retain one runtime, inspect capability, boot, then issue commands or open sessions.",
            "zh": "保留运行时，检查 capability 并 boot，再执行命令或打开会话。"
          },
          {
            "en": "Consume output and terminal state; close PTY/raw/background handles before shutdown.",
            "zh": "消费输出及终态；在 shutdown 前关闭 PTY、raw stdio 与后台句柄。"
          }
        ],
        "diagram": {
          "src": "/docs-assets/platform-mobile-lifecycle.svg",
          "alt": {
            "en": "Resource verification leads to a ready runtime, then owned execution sessions and cleanup.",
            "zh": "资源校验进入就绪运行时，之后创建有归属的执行会话并清理。"
          },
          "caption": {
            "en": "Repair targets managed rootfs; iOS process-wide kernel changes may require a restart.",
            "zh": "修复针对托管 rootfs；iOS 进程级内核变更可能要求重启。"
          }
        }
      },
      {
        "id": "validation-levels",
        "title": {
          "en": "Implementation, evidence and open acceptance",
          "zh": "实现、证据与开放验收"
        },
        "paragraphs": [
          {
            "en": "The public trait, UniFFI handle and Kotlin/Swift wrappers implement the described integration surfaces. The source repository includes unit/contract checks and device-smoke applications. A documented test command is a repeatable check, not a claim that it has run on every supported device.",
            "zh": "公共 trait、UniFFI 句柄及 Kotlin/Swift 包装实现本页接入接口。源码仓库包含单元／契约检查和真机 smoke 应用；文档命令提供可重复验证方法，不表示已经在所有受支持设备上执行。"
          },
          {
            "en": "Acceptance for a new distribution still requires archive corruption and recovery tests, ABI packaging validation, permission denial, network/memory enforcement, process-tree cancellation and actual guest execution on a physical device. The simulator is suitable for API and linking tests only.",
            "zh": "新分发版本仍须验收压缩包损坏与恢复、ABI 打包、权限拒绝、网络／内存约束、进程树取消及真机 guest 执行。模拟器仅用于 API 和链接测试。"
          }
        ],
        "links": [
          {
            "label": {
              "en": "Portable Rust contracts",
              "zh": "可移植 Rust 契约"
            },
            "href": "/docs/mobile-rust"
          },
          {
            "label": {
              "en": "Mount, network and rootfs policy",
              "zh": "挂载、网络与 rootfs 策略"
            },
            "href": "/docs/mobile-authorization"
          },
          {
            "label": {
              "en": "Product native host",
              "zh": "产品原生宿主"
            },
            "href": "/docs/platform-integration"
          }
        ]
      }
    ],
    "sources": [
      {
        "repo": "mobile-linux-runtime",
        "path": "Cargo.toml"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "rust-toolchain.toml"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ffi/src/runtime.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ffi/src/types.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "android/runtime/build.gradle.kts"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "ios/Package.swift"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "scripts/checks/check-resource-contracts.sh"
      }
    ]
  },
  {
    "id": "mobile-rust",
    "group": "mobile",
    "title": {
      "en": "Rust API",
      "zh": "Rust API"
    },
    "description": {
      "en": "Shared contracts for commands, task lifecycle, PTY, raw stdio, mounts and rootfs verification.",
      "zh": "命令、任务生命周期、PTY、原始标准输入输出、挂载与 rootfs 校验的共享契约。"
    },
    "packageName": "mobile-linux-api",
    "sourceUrl": "https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/crates/mobile-linux-api/src/mobile_linux.rs",
    "sections": [
      {
        "id": "crates",
        "title": {
          "en": "Rust integration",
          "zh": "Rust 接入"
        },
        "paragraphs": [
          {
            "en": "Select mobile-linux-api and mobile-linux-core with mobile-linux-android or mobile-linux-ios from the same Git revision. The SDK-owned platform-pty crate supplies shared PTY support. Embed native-support artifacts alongside your existing Rust library to keep one runtime in the process.",
            "zh": "从同一个 Git 修订版本选择 mobile-linux-api、mobile-linux-core，以及 mobile-linux-android 或 mobile-linux-ios。SDK 自有的 platform-pty crate 提供共享 PTY 支持。将 native-support 产物配合已有 Rust 库使用，保持进程内只有一份运行时。"
          }
        ],
        "apis": [
          {
            "name": "AndroidProotRuntimeConfig",
            "signature": "pub struct AndroidProotRuntimeConfig",
            "description": {
              "en": "Caller-supplied managed root, app sandbox root, ABI, rootfs version/hash, native library directory and optional isolated build profile.",
              "zh": "调用方提供托管根目录、应用沙箱根目录、ABI、rootfs 版本与哈希、原生库目录，以及可选的隔离构建配置。"
            }
          },
          {
            "name": "IosIshRuntime::new",
            "signature": "pub fn new(mut config: IosIshRuntimeConfig) -> Result<Self, MobileLinuxError>",
            "description": {
              "en": "Constructs a session without mutating filesystem or native state and validates the process-wide kernel identity.",
              "zh": "构造会话时不改变文件系统或原生状态，并校验进程级内核身份。"
            }
          }
        ]
      },
      {
        "id": "working-tree-consumer",
        "title": {
          "en": "Consumer workspace and source identity",
          "zh": "消费者工作区与源码身份"
        },
        "paragraphs": [
          {
            "en": "Create a standalone consumer directory beside the mobile-linux-runtime checkout and use this complete Cargo.toml. The empty workspace keeps the consumer out of a parent product workspace; the direct path dependency binds it to the current SDK working tree. Save the Rust lifecycle function from this page in src/lib.rs and run cargo check from the consumer root. Replace the SDK path only when deliberately choosing another source snapshot.",
            "zh": "在 mobile-linux-runtime checkout 旁创建独立消费者目录，并使用这份完整 Cargo.toml。空 workspace 使消费者脱离父产品工作区；直接 path 依赖绑定当前 SDK 工作树。将本页 Rust 生命周期函数保存到 src/lib.rs，并从消费者根目录运行 cargo check。只有明确选择其他源码快照时才替换 SDK 路径。"
          },
          {
            "en": "This example depends only on the portable mobile-linux-api contract. Constructing a platform runtime additionally requires its Android or iOS backend and matching native-support artifacts from one revision. Compilation of this consumer does not boot Linux, install rootfs assets, exercise UniFFI or prove physical-device execution.",
            "zh": "此示例仅依赖可移植 mobile-linux-api 契约。构造平台运行时还需要相同 revision 的 Android 或 iOS 后端及匹配 native-support 产物。编译此消费者不会启动 Linux、安装 rootfs、执行 UniFFI 或证明真机执行。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Cargo.toml · complete working-tree consumer",
              "zh": "Cargo.toml · 完整工作树消费者"
            },
            "language": "toml",
            "code": "[package]\nname = \"handbook-mobile-consumer\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[dependencies]\nmobile-linux-api = { path = \"../mobile-linux-runtime/crates/mobile-linux-api\" }\n",
            "exampleId": "mobile-working-tree-manifest",
            "verification": "manual"
          }
        ]
      },
      {
        "id": "commands",
        "title": {
          "en": "Command requests and results",
          "zh": "命令请求与结果"
        },
        "paragraphs": [
          {
            "en": "LinuxCommandRequest describes the executable, arguments, guest cwd, environment, optional text stdin and timeout, explicit network policy, resource limits and mounts. LinuxCommandResult returns collected text, exit code, timeout/cancellation flags and an enforcement receipt.",
            "zh": "LinuxCommandRequest 包含可执行文件、参数、guest 工作目录、环境变量、可选文本 stdin 与超时、显式网络策略、资源限制及挂载。LinuxCommandResult 返回收集的文本、退出码、超时与取消标记，以及策略执行回执。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "mobile_linux.rs · request declaration excerpt",
              "zh": "请求声明摘录"
            },
            "language": "rust",
            "code": "pub struct LinuxCommandRequest {\n    pub command: String,\n    pub args: Vec<String>,\n    pub cwd: Option<String>,\n    pub env: BTreeMap<String, String>,\n    pub stdin: Option<String>,\n    pub timeout_ms: Option<u64>,\n    pub network: NetworkPolicy,\n    #[serde(default)]\n    pub resource_limits: ResourceLimits,\n    pub mounts: Vec<MountSpec>,\n}",
            "verification": "excerpt"
          }
        ],
        "apis": [
          {
            "name": "MobileLinuxRuntime::run",
            "signature": "async fn run(&self, request: LinuxCommandRequest) -> Result<LinuxCommandResult, MobileLinuxError>",
            "description": {
              "en": "Run a command to completion inside the mobile backend.",
              "zh": "在移动后端运行命令直至完成。"
            }
          },
          {
            "name": "MobileLinuxRuntime::run_streaming",
            "signature": "async fn run_streaming(&self, request: LinuxCommandRequest, sink: Arc<dyn ProcessStreamSink>) -> Result<LinuxCommandResult, MobileLinuxError>",
            "description": {
              "en": "Deliver stdout lines and stderr byte chunks through the sink and return the final result.",
              "zh": "通过 sink 传递 stdout 行与 stderr 字节块，并返回最终结果。"
            }
          }
        ]
      },
      {
        "id": "task-lifecycle",
        "title": {
          "en": "Runtime and background tasks",
          "zh": "运行时与后台任务"
        },
        "apis": [
          {
            "name": "MobileLinuxRuntime::probe_capability",
            "signature": "async fn probe_capability(&self) -> MobileLinuxCapability",
            "description": {
              "en": "Probe availability and support for streaming, background processes, PTY, mounts and integrity without changing state.",
              "zh": "在不改变状态的情况下探测可用性，以及流式输出、后台进程、PTY、挂载和完整性支持。"
            }
          },
          {
            "name": "MobileLinuxRuntime::boot",
            "signature": "async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError>",
            "description": {
              "en": "Ensure that the backend is ready to accept requests.",
              "zh": "确保后端已准备好接收请求。"
            }
          },
          {
            "name": "MobileLinuxRuntime::shutdown",
            "signature": "async fn shutdown(&self) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Stop the session and release transient resources; iOS native kernel lifetime remains process-wide.",
              "zh": "停止会话并释放临时资源；iOS 原生内核仍具有进程级生命周期。"
            }
          },
          {
            "name": "MobileLinuxRuntime::spawn_background",
            "signature": "async fn spawn_background(&self, request: LinuxCommandRequest) -> Result<LinuxProcessHandle, MobileLinuxError>",
            "description": {
              "en": "Start a managed background command and retain its opaque handle.",
              "zh": "启动托管后台命令并保留其不透明句柄。"
            }
          },
          {
            "name": "MobileLinuxRuntime::kill",
            "signature": "async fn kill(&self, handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Success guarantees that the complete guest process group/tree has been reaped and no task descendant remains runnable.",
              "zh": "成功返回表示整个 guest 进程组或进程树已被回收，该任务的后代不再可运行。"
            }
          }
        ]
      },
      {
        "id": "pty",
        "title": {
          "en": "PTY sessions",
          "zh": "PTY 会话"
        },
        "paragraphs": [
          {
            "en": "PTY input and output are byte buffers. Set terminal size in cells with PtySize { cols, rows }; drain PtyOutput and PtyClosed events through read_events.",
            "zh": "PTY 输入输出使用字节缓冲区。通过 PtySize { cols, rows } 设置以字符单元为单位的终端尺寸；通过 read_events 消费 PtyOutput 与 PtyClosed 事件。"
          }
        ],
        "apis": [
          {
            "name": "MobileLinuxRuntime::open_pty",
            "signature": "async fn open_pty(&self, request: PtyOpenRequest) -> Result<PtySessionHandle, MobileLinuxError>",
            "description": {
              "en": "Open an interactive terminal process.",
              "zh": "打开交互式终端进程。"
            }
          },
          {
            "name": "MobileLinuxRuntime::write_pty",
            "signature": "async fn write_pty(&self, handle: &PtySessionHandle, input: Vec<u8>) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Write terminal input bytes.",
              "zh": "写入终端输入字节。"
            }
          },
          {
            "name": "MobileLinuxRuntime::resize_pty",
            "signature": "async fn resize_pty(&self, handle: &PtySessionHandle, size: PtySize) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Resize the terminal.",
              "zh": "调整终端尺寸。"
            }
          },
          {
            "name": "MobileLinuxRuntime::close_pty",
            "signature": "async fn close_pty(&self, handle: &PtySessionHandle) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Close the PTY and reap its complete guest process tree before returning success.",
              "zh": "关闭 PTY，在成功返回前回收它的整个 guest 进程树。"
            }
          }
        ]
      },
      {
        "id": "raw-stdio",
        "title": {
          "en": "Raw stdio sessions",
          "zh": "原始标准输入输出会话"
        },
        "paragraphs": [
          {
            "en": "Use RawStdioOpenRequest for a long-lived process with independent binary stdin/stdout/stderr. This surface preserves exact bytes for protocols such as LSP. Unsupported implementations fail closed instead of substituting a PTY.",
            "zh": "使用 RawStdioOpenRequest 创建具有独立二进制 stdin/stdout/stderr 的长期运行进程。此接口为 LSP 等协议保留精确字节。不支持该能力的实现会返回错误，不会替换成 PTY。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "mobile_linux.rs · raw stdio request excerpt",
              "zh": "原始 IO 请求摘录"
            },
            "language": "rust",
            "code": "pub struct RawStdioOpenRequest {\n    pub command: String,\n    pub args: Vec<String>,\n    pub cwd: Option<String>,\n    pub env: BTreeMap<String, String>,\n    pub network: NetworkPolicy,\n    #[serde(default)]\n    pub resource_limits: ResourceLimits,\n    pub mounts: Vec<MountSpec>,\n}",
            "verification": "excerpt"
          }
        ],
        "apis": [
          {
            "name": "MobileLinuxRuntime::open_raw_stdio",
            "signature": "async fn open_raw_stdio(&self, request: RawStdioOpenRequest) -> Result<RawStdioSessionHandle, MobileLinuxError>",
            "description": {
              "en": "Open a binary stdio process.",
              "zh": "打开二进制标准 IO 进程。"
            }
          },
          {
            "name": "MobileLinuxRuntime::write_raw_stdio",
            "signature": "async fn write_raw_stdio(&self, handle: &RawStdioSessionHandle, input: Vec<u8>) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Write raw stdin bytes.",
              "zh": "写入原始 stdin 字节。"
            }
          },
          {
            "name": "MobileLinuxRuntime::read_raw_stdio",
            "signature": "async fn read_raw_stdio(&self, handle: &RawStdioSessionHandle, max_bytes: usize) -> Result<RawStdioReadResult, MobileLinuxError>",
            "description": {
              "en": "Poll bounded stdout/stderr bytes and inspect closed and exit_code.",
              "zh": "轮询有上限的 stdout/stderr 字节，并检查 closed 与 exit_code。"
            }
          },
          {
            "name": "MobileLinuxRuntime::close_raw_stdio",
            "signature": "async fn close_raw_stdio(&self, handle: &RawStdioSessionHandle) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Close the session and synchronously reap its complete guest process tree.",
              "zh": "关闭会话并同步回收其整个 guest 进程树。"
            }
          }
        ],
        "note": {
          "en": "The FFI form takes MobileLinuxCommandRequestFfi, rejects any provided stdin or timeout_ms, and requires read max_bytes in 1..1048576. Control writes and lifetime through the returned session ID.",
          "zh": "FFI 版本接收 MobileLinuxCommandRequestFfi，拒绝已设置的 stdin 或 timeout_ms，并要求读取 max_bytes 在 1..1048576 范围内。通过返回的会话 ID 控制写入与生命周期。"
        }
      },
      {
        "id": "events",
        "title": {
          "en": "Events and task snapshots",
          "zh": "事件与任务快照"
        },
        "apis": [
          {
            "name": "MobileLinuxRuntime::read_events",
            "signature": "async fn read_events(&self, after_sequence: Option<u64>, limit: usize) -> Result<Vec<MobileLinuxEvent>, MobileLinuxError>",
            "description": {
              "en": "Read events strictly after a sequence cursor. MAX_MOBILE_LINUX_EVENT_BATCH is 512.",
              "zh": "读取严格晚于指定序号游标的事件。MAX_MOBILE_LINUX_EVENT_BATCH 为 512。"
            }
          },
          {
            "name": "MobileLinuxRuntime::list_tasks",
            "signature": "async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError>",
            "description": {
              "en": "Enumerate known tasks and sessions with their status and completion details.",
              "zh": "列举已知任务与会话，以及它们的状态和完成详情。"
            }
          },
          {
            "name": "MobileLinuxRuntime::task_status",
            "signature": "async fn task_status(&self, task_id: &str) -> Result<Option<MobileLinuxTaskSnapshot>, MobileLinuxError>",
            "description": {
              "en": "Read one task by ID; None means no matching task.",
              "zh": "按 ID 读取任务；None 表示没有匹配任务。"
            }
          }
        ]
      },
      {
        "id": "mounts",
        "title": {
          "en": "Mounts and path translation",
          "zh": "挂载与路径转换"
        },
        "apis": [
          {
            "name": "MobileLinuxRuntime::configure_mounts",
            "signature": "async fn configure_mounts(&self, mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Replace the active bind-mount set after platform policy validation.",
              "zh": "通过平台策略校验后替换活动 bind mount 集合。"
            }
          },
          {
            "name": "MobileLinuxRuntime::current_mounts",
            "signature": "fn current_mounts(&self) -> Vec<MountSpec>",
            "description": {
              "en": "Return the current guest-to-host mount table.",
              "zh": "返回当前 guest 到 host 的挂载表。"
            }
          },
          {
            "name": "map_guest_path_to_host",
            "signature": "pub fn map_guest_path_to_host(path: &str, mounts: &[MountSpec]) -> Option<std::path::PathBuf>",
            "description": {
              "en": "Use the longest matching mount prefix. Pure lexical mapping does not provide filesystem fencing or read-only enforcement.",
              "zh": "使用最长匹配挂载前缀。纯词法映射不提供文件系统边界或只读执行约束。"
            }
          },
          {
            "name": "map_host_path_to_guest",
            "signature": "pub fn map_host_path_to_guest(path: &std::path::Path, mounts: &[MountSpec]) -> Option<String>",
            "description": {
              "en": "Translate an absolute clean host path to its guest coordinate without filesystem access.",
              "zh": "无需访问文件系统，将合法绝对 host 路径转换为 guest 坐标。"
            }
          }
        ]
      },
      {
        "id": "rootfs-store",
        "title": {
          "en": "RootfsStore",
          "zh": "RootfsStore"
        },
        "paragraphs": [
          {
            "en": "mobile-linux-core owns inventory verification, safe staging and atomic activation. RootfsManifest identifies platform/ABI, archive identity, immutable files, writable paths, executable allowlists and package/SBOM data.",
            "zh": "mobile-linux-core 负责清单验证、安全暂存与原子激活。RootfsManifest 描述平台与 ABI、压缩包身份、不可变文件、可写路径、可执行文件白名单以及包与 SBOM 数据。"
          }
        ],
        "apis": [
          {
            "name": "RootfsStore::stage_prepared_rootfs",
            "signature": "pub fn stage_prepared_rootfs(&self, prepared_root: &Path, manifest: &RootfsManifest) -> Result<PathBuf, RootfsStoreError>",
            "description": {
              "en": "Verify a prepared directory against the manifest before publishing staging.",
              "zh": "在发布暂存目录前，根据 manifest 验证准备好的目录。"
            }
          },
          {
            "name": "RootfsStore::activate_staged_rootfs",
            "signature": "pub fn activate_staged_rootfs(&self, manifest: &RootfsManifest) -> Result<RootfsStatus, RootfsStoreError>",
            "description": {
              "en": "Recover interrupted activation, verify staging and activate the image atomically.",
              "zh": "恢复中断的激活流程、验证暂存内容，并原子激活镜像。"
            }
          },
          {
            "name": "RootfsStore::authorize_manifest_file",
            "signature": "pub fn authorize_manifest_file(&self, guest_path: &str, manifest: &RootfsManifest) -> Result<PathBuf, RootfsStoreError>",
            "description": {
              "en": "Validate an allowlisted immutable executable; writable guest paths and absent allowlist entries are rejected.",
              "zh": "验证白名单中的不可变可执行文件；拒绝可写 guest 路径和白名单以外的条目。"
            }
          }
        ]
      },
      {
        "id": "complete-command",
        "title": {
          "en": "Complete command lifecycle example",
          "zh": "完整命令生命周期示例"
        },
        "paragraphs": [
          {
            "en": "This reusable function accepts the already configured platform trait, boots it, executes a bounded command, validates the requested network receipt and always attempts shutdown after execution. It returns nonzero exit codes as command results; callers must inspect exit_code, timed_out and cancelled before declaring success. Construction and rootfs distribution remain host inputs.",
            "zh": "此复用函数接收已经配置的平台 trait，启动后执行有时间上限的命令、验证网络执行回执，并在执行后始终尝试 shutdown。非零退出码仍作为命令结果返回，调用方须检查 exit_code、timed_out 与 cancelled 后才能判断成功。构造和 rootfs 分发仍由宿主提供。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Rust consumer function",
              "zh": "Rust 消费者函数"
            },
            "language": "rust",
            "code": "use mobile_linux_api::{\n    LinuxCommandRequest, LinuxCommandResult, MobileLinuxError,\n    MobileLinuxRuntime, NetworkPolicy, ResourceLimits,\n};\nuse std::collections::BTreeMap;\n\npub async fn execute_once(\n    runtime: &dyn MobileLinuxRuntime,\n) -> Result<LinuxCommandResult, MobileLinuxError> {\n    runtime.boot().await?;\n    let network = NetworkPolicy::Disabled;\n    let limits = ResourceLimits::default();\n    let execution = runtime.run(LinuxCommandRequest {\n        command: \"/bin/sh\".into(),\n        args: vec![\"-c\".into(), \"printf 'ready\\n'\".into()],\n        cwd: Some(\"/root\".into()),\n        env: BTreeMap::new(),\n        stdin: None,\n        timeout_ms: Some(10_000),\n        network,\n        resource_limits: limits,\n        mounts: Vec::new(),\n    }).await;\n    let cleanup = runtime.shutdown().await;\n    let result = execution?;\n    cleanup?;\n    result.enforcement.ensure_for(network, limits)?;\n    Ok(result)\n}",
            "verification": "compile",
            "exampleId": "mobile-command-lifecycle"
          }
        ]
      },
      {
        "id": "io-ownership",
        "title": {
          "en": "IO selection, cancellation and event cursors",
          "zh": "IO 选择、取消与事件游标"
        },
        "paragraphs": [
          {
            "en": "Use run for collected text, run_streaming for incremental stdout lines and stderr byte chunks, PTY for a terminal, and raw stdio for byte-framed protocols. PTY terminal processing changes bytes, so LSP or a binary protocol needs raw stdio. A zero-length raw read is rejected by the FFI rather than used as a readiness probe.",
            "zh": "run 用于收集文本，run_streaming 用于增量 stdout 行与 stderr 字节块，PTY 用于终端，raw stdio 用于字节分帧协议。PTY 的终端处理会改变字节，因此 LSP 或二进制协议需要 raw stdio。FFI 拒绝长度为零的 raw read，不能用它探测就绪。"
          },
          {
            "en": "Keep sequence cursors only for read_events, advancing to the largest returned sequence. The UniFFI streaming sink synthesizes per-call events with sequence 0 and a run-N task identifier; those callback sequences are not the backend event-log cursor. A task snapshot reconciles current state after a UI reconnect; output is not durable conversation history.",
            "zh": "只为 read_events 保留序号游标，并推进到本批最大 sequence。UniFFI 流式 sink 合成每次调用的事件，sequence 为 0，任务标识为 run-N；这些回调序号不是后端事件日志游标。界面重连后通过任务快照对齐当前状态；进程输出并非持久会话历史。"
          },
          {
            "en": "Managed background, PTY and raw handles own guest processes. Explicit kill/close returns only after the owned process tree is reaped. Foreground drop, timeout and shutdown paths have dedicated iOS transport tests, but a future cancellation alone is not an app-level promise of suspended or resumable mobile execution.",
            "zh": "托管后台、PTY 与 raw 句柄拥有 guest 进程。显式 kill/close 在归属进程树回收后才返回。iOS 传输层具有前台 future 丢弃、超时及 shutdown 专项测试，但仅取消 future 不能承诺应用层移动执行可暂停或恢复。"
          }
        ],
        "tables": [
          {
            "headers": [
              {
                "en": "Surface",
                "zh": "接口"
              },
              {
                "en": "Output / ownership",
                "zh": "输出／归属"
              },
              {
                "en": "Finish",
                "zh": "结束方式"
              }
            ],
            "rows": [
              [
                {
                  "en": "run / run_streaming",
                  "zh": "run / run_streaming"
                },
                {
                  "en": "One awaited operation; final result carries enforcement.",
                  "zh": "单次等待操作，终态结果携带执行回执。"
                },
                {
                  "en": "Completion, timeout, or owner cancellation.",
                  "zh": "完成、超时或归属取消。"
                }
              ],
              [
                {
                  "en": "spawn_background",
                  "zh": "spawn_background"
                },
                {
                  "en": "Opaque LinuxProcessHandle plus task snapshots.",
                  "zh": "不透明 LinuxProcessHandle 加任务快照。"
                },
                {
                  "en": "kill(handle), then inspect terminal task state.",
                  "zh": "kill(handle)，再检查任务终态。"
                }
              ],
              [
                {
                  "en": "open_pty",
                  "zh": "open_pty"
                },
                {
                  "en": "Terminal bytes and PtyOutput/PtyClosed events.",
                  "zh": "终端字节及 PtyOutput/PtyClosed 事件。"
                },
                {
                  "en": "close_pty(handle).",
                  "zh": "close_pty(handle)。"
                }
              ],
              [
                {
                  "en": "open_raw_stdio",
                  "zh": "open_raw_stdio"
                },
                {
                  "en": "Separate binary streams and closed/exit_code reads.",
                  "zh": "分离的二进制流与 closed/exit_code 读取。"
                },
                {
                  "en": "close_raw_stdio(handle).",
                  "zh": "close_raw_stdio(handle)。"
                }
              ]
            ]
          }
        ]
      }
    ],
    "sources": [
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-api/src/mobile_linux.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-api/src/execution.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ffi/src/runtime.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-core/src/store/install.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-core/src/store/lifecycle.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-android/src/runtime/implementation.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/implementation.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/tests.rs"
      }
    ]
  },
  {
    "id": "mobile-android",
    "group": "mobile",
    "title": {
      "en": "Android · Kotlin",
      "zh": "Android · Kotlin"
    },
    "description": {
      "en": "Install the Maven bundle, stage a verified rootfs, and operate a persistent Kotlin runtime.",
      "zh": "安装 Maven 分发包、暂存经过验证的 rootfs，并使用持久 Kotlin 运行时。"
    },
    "packageName": "io.github.lingxi-coder:mobile-linux-runtime",
    "sourceUrl": "https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt",
    "sections": [
      {
        "id": "maven-bundle",
        "title": {
          "en": "Install a release bundle",
          "zh": "安装发布分发包"
        },
        "paragraphs": [
          {
            "en": "When a release provides mobile-linux-maven-<version>.zip, use its published SHA-256, exact version and full binary source revision. The bundle installer verifies release identity and the recorded file inventory; validation-only bundles are rejected.",
            "zh": "如果发布版本提供 mobile-linux-maven-<version>.zip，请使用发布的 SHA-256、精确版本与完整二进制源码修订号。分发包安装器会验证发布身份与文件清单；validation-only 分发包会被拒绝。"
          },
          {
            "en": "Set SDK_MAVEN_ARCHIVE, SDK_MAVEN_SHA256, SDK_VERSION and SDK_SOURCE_REVISION from that release, with SDK_BUILD_DIR outside the checkout. Use --url with an explicit HTTPS release URL when downloading.",
            "zh": "根据对应发布版本设置 SDK_MAVEN_ARCHIVE、SDK_MAVEN_SHA256、SDK_VERSION 和 SDK_SOURCE_REVISION，并把 SDK_BUILD_DIR 放在源码目录以外。下载时使用 --url 指定明确的 HTTPS 发布地址。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "SDK-INTEGRATION.md · Maven installer excerpt",
              "zh": "Maven 安装摘录"
            },
            "language": "bash",
            "code": "python3 scripts/release/install-maven-bundle.py \\\n  --archive \"$SDK_MAVEN_ARCHIVE\" --sha256 \"$SDK_MAVEN_SHA256\" \\\n  --version \"$SDK_VERSION\" --source-revision \"$SDK_SOURCE_REVISION\" \\\n  --cache-dir \"$SDK_BUILD_DIR/download-cache\" \\\n  --output-dir \"$SDK_BUILD_DIR/maven\"",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "gradle",
        "title": {
          "en": "Configure Gradle",
          "zh": "配置 Gradle"
        },
        "paragraphs": [
          {
            "en": "Add the installed Maven directory or release Maven URL to both pluginManagement and dependencyResolutionManagement in settings.gradle.kts. The 0.1.0 coordinate in the source guide is a local build example; it does not claim a Maven Central publication.",
            "zh": "在 settings.gradle.kts 的 pluginManagement 与 dependencyResolutionManagement 中加入已安装的 Maven 目录或发布 Maven 地址。源码指南中的 0.1.0 坐标是本地构建示例，并不表示已发布到 Maven Central。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "SDK-INTEGRATION.md · local-coordinate excerpt",
              "zh": "本地坐标摘录"
            },
            "language": "kotlin",
            "code": "plugins {\n    id(\"io.github.lingxi-coder.mobile-linux\") version \"0.1.0\"\n}\ndependencies {\n    implementation(\"io.github.lingxi-coder:mobile-linux-runtime:0.1.0\")\n}",
            "verification": "excerpt"
          }
        ],
        "bullets": [
          {
            "en": "Set minSdk = 26 and include arm64-v8a and x86_64. Set android:extractNativeLibs=\"true\" in the application manifest.",
            "zh": "设置 minSdk = 26，并包含 arm64-v8a 与 x86_64。在应用 manifest 中设置 android:extractNativeLibs=\"true\"。"
          },
          {
            "en": "The packaging plugin enables legacy JNI packaging and verifies native helper presence and ELF ABI in the APK. Optional legacy host-shell APIs retain their API 29 minimum.",
            "zh": "打包插件开启 legacy JNI packaging，并在 APK 中校验原生辅助程序存在及 ELF ABI。可选的旧版宿主 shell API 仍要求最低 API 29。"
          },
          {
            "en": "An existing Rust host uses installer and native-support coordinates with mobileLinuxPackaging=native-support. Supply applicationInfo.nativeLibraryDir explicitly.",
            "zh": "已有 Rust 宿主使用 installer 与 native-support 坐标，并设置 mobileLinuxPackaging=native-support。显式提供 applicationInfo.nativeLibraryDir。"
          }
        ]
      },
      {
        "id": "install-rootfs",
        "title": {
          "en": "Stage a verified rootfs",
          "zh": "暂存经过验证的 rootfs"
        },
        "paragraphs": [
          {
            "en": "Call RootfsInstaller.stage on an I/O worker with an app-private managedRoot, the pinned archive digest, matching full manifest and SBOM, and a copyArchive callback. The manifest ABI is arm64 or x86_64; Android RuntimeConfig uses arm64-v8a or x86_64.",
            "zh": "在 IO 工作线程中调用 RootfsInstaller.stage，提供应用私有 managedRoot、固定的压缩包摘要、匹配的完整 manifest 与 SBOM，以及 copyArchive 回调。manifest ABI 使用 arm64 或 x86_64；Android RuntimeConfig 使用 arm64-v8a 或 x86_64。"
          }
        ],
        "apis": [
          {
            "name": "RootfsInstaller.stage",
            "signature": "fun stage(managedRoot: File, expectedRootfsVersion: String, expectedArchiveSha: String, expectedManifestAbi: String, archiveName: String, manifestJson: String, sbomJson: String, copyArchive: (File) -> Unit, persistManifest: ((String) -> Unit)? = null): StageResult",
            "description": {
              "en": "Verifies the archive, immutable inventory, symlinks and SBOM, then atomically publishes staging and the full manifest. StageResult reports stagedRoot, manifestPath, verifiedFiles and verifiedBytes.",
              "zh": "验证压缩包、不可变清单、符号链接与 SBOM，然后原子发布暂存内容和完整 manifest。StageResult 返回 stagedRoot、manifestPath、verifiedFiles 与 verifiedBytes。"
            }
          }
        ],
        "code": [
          {
            "label": {
              "en": "RealRuntimeSmokeTest.kt · partial staging excerpt",
              "zh": "部分暂存摘录"
            },
            "language": "kotlin",
            "code": "val result = RootfsInstaller.stage(\n            managedRoot = managed, expectedRootfsVersion = version,\n            expectedArchiveSha = archiveSha, expectedManifestAbi = manifest.getString(\"abi\"),\n            archiveName = archiveRecord.getString(\"filename\"), manifestJson = manifestText,\n            sbomJson = File(inputs, \"rootfs.spdx.json\").readText(),\n            copyArchive = { destination -> archive.copyTo(destination, overwrite = true) },\n        )",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "wrapper",
        "title": {
          "en": "Kotlin runtime wrapper",
          "zh": "Kotlin 运行时封装"
        },
        "code": [
          {
            "label": {
              "en": "MobileLinuxRuntime.kt · wrapper source",
              "zh": "封装源码"
            },
            "language": "kotlin",
            "code": "package io.lingxi.mobilelinux\n\nimport io.lingxi.mobilelinux.bindings.*\n\n/** One persistent runtime. The embedding app owns policy, paths and rootfs distribution. */\nclass MobileLinuxRuntime private constructor(val handle: RuntimeHandle) {\n    companion object {\n        fun create(config: RuntimeConfig): MobileLinuxRuntime = MobileLinuxRuntime(createRuntime(config))\n    }\n    suspend fun boot(): MobileLinuxStatusFfi = handle.boot()\n    suspend fun shutdown() = handle.shutdown()\n    suspend fun capability(): MobileLinuxCapabilityFfi = handle.capability()\n    suspend fun execute(request: MobileLinuxCommandRequestFfi): MobileLinuxCommandResultFfi = handle.runCommand(request)\n    // Raw stdio and PTY are deliberately exposed on handle without text conversion.\n}",
            "verification": "excerpt"
          }
        ],
        "apis": [
          {
            "name": "MobileLinuxRuntime.create",
            "signature": "fun create(config: RuntimeConfig): MobileLinuxRuntime",
            "description": {
              "en": "Create the runtime with caller-owned configuration.",
              "zh": "通过调用方拥有的配置创建运行时。"
            }
          },
          {
            "name": "MobileLinuxRuntime.boot",
            "signature": "suspend fun boot(): MobileLinuxStatusFfi",
            "description": {
              "en": "Boot after activating staged rootfs with handle.repairRootfs().",
              "zh": "通过 handle.repairRootfs() 激活暂存 rootfs 后启动。"
            }
          },
          {
            "name": "MobileLinuxRuntime.execute",
            "signature": "suspend fun execute(request: MobileLinuxCommandRequestFfi): MobileLinuxCommandResultFfi",
            "description": {
              "en": "Run a command and collect its result.",
              "zh": "运行命令并收集结果。"
            }
          },
          {
            "name": "MobileLinuxRuntime.shutdown",
            "signature": "suspend fun shutdown()",
            "description": {
              "en": "Close runtime operations when the host lifecycle ends.",
              "zh": "宿主生命周期结束时关闭运行时操作。"
            }
          },
          {
            "name": "MobileLinuxRuntime.capability",
            "signature": "suspend fun capability(): MobileLinuxCapabilityFfi",
            "description": {
              "en": "Inspect live backend availability and supported features.",
              "zh": "检查当前后端可用性与支持的能力。"
            }
          }
        ]
      },
      {
        "id": "command-example",
        "title": {
          "en": "Command request",
          "zh": "命令请求"
        },
        "paragraphs": [
          {
            "en": "This helper is copied from the Android device smoke test. It creates a disabled-network shell request by default and clears the timeout for raw stdio. Install and boot the runtime before calling execute or handle.openRawStdio.",
            "zh": "下面的辅助函数摘自 Android 真机 smoke test。默认创建禁用网络的 shell 请求，raw stdio 模式清除超时。调用 execute 或 handle.openRawStdio 前，先安装并启动运行时。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "RealRuntimeSmokeTest.kt · helper excerpt",
              "zh": "辅助函数摘录"
            },
            "language": "kotlin",
            "code": "private fun command(script: String, network: NetworkPolicyFfi = NetworkPolicyFfi.DISABLED,\n                        python: Boolean = false, raw: Boolean = false) = MobileLinuxCommandRequestFfi(\n        command = if (python) \"/usr/bin/python3\" else \"/bin/sh\",\n        args = if (python) listOf(\"-u\", \"-c\", script) else listOf(\"-c\", script),\n        cwd = \"/root\", env = emptyList(), stdin = null,\n        timeoutMs = if (raw) null else 15_000uL, network = network,\n        resourceLimits = ResourceLimitsFfi(null, null, null, null), mounts = emptyList(),\n    )",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "advanced-handle",
        "title": {
          "en": "Streaming, sessions and native support",
          "zh": "流式输出、会话与原生支持"
        },
        "paragraphs": [
          {
            "en": "Use runtime.handle for runCommandStreaming, spawnBackground/killProcess, PTY, raw stdio, events and task snapshots. Byte arrays pass through without text conversion. Retain runtime for the lifetime of every returned handle.",
            "zh": "通过 runtime.handle 使用 runCommandStreaming、spawnBackground/killProcess、PTY、raw stdio、事件与任务快照。字节数组不经过文本转换。每个返回句柄的整个生命周期内都应保留 runtime。"
          },
          {
            "en": "Android native support contains the network policy launcher, PRoot overlay and pinned talloc sources. It contains no Rust runtime core or FFI cdylib. ARM64 helper builds enforce 16 KiB ELF page alignment and verify PT_LOAD segments before publication.",
            "zh": "Android 原生支持包含网络策略启动器、PRoot overlay 与固定版本的 talloc 源码。它不包含 Rust runtime core 或 FFI cdylib。ARM64 辅助程序构建强制 16 KiB ELF 页对齐，并在发布前检查 PT_LOAD 段。"
          }
        ],
        "note": {
          "en": "MainActivity demonstrates object creation only. RealRuntimeSmokeTest supplies verified assets and exercises guest execution; running neither has been claimed by this documentation build.",
          "zh": "MainActivity 仅演示对象创建。RealRuntimeSmokeTest 提供经验证的资源并测试 guest 执行；本次文档构建不声称运行过这两个示例。"
        }
      },
      {
        "id": "android-end-to-end",
        "title": {
          "en": "Execute and release a Kotlin runtime",
          "zh": "执行并释放 Kotlin 运行时"
        },
        "paragraphs": [
          {
            "en": "Pass a RuntimeConfig whose rootfs has already been staged by RootfsInstaller and whose nativeLibraryDir is Context.applicationInfo.nativeLibraryDir. This complete consumer function handles boot, a bounded command, result checks and cleanup. Run it from a coroutine off the UI thread; display typed integrity or policy errors instead of silently retrying a rejected request.",
            "zh": "传入已由 RootfsInstaller 暂存 rootfs 的 RuntimeConfig，nativeLibraryDir 来自 Context.applicationInfo.nativeLibraryDir。此完整消费者函数包含 boot、有超时上限的命令、结果检查及清理。通过协程在 UI 线程之外执行；展示类型化完整性或策略错误，不要静默重试被拒绝的请求。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Kotlin lifecycle function",
              "zh": "Kotlin 生命周期函数"
            },
            "language": "kotlin",
            "code": "import io.lingxi.mobilelinux.MobileLinuxRuntime\nimport io.lingxi.mobilelinux.bindings.*\n\nsuspend fun executeOnce(config: RuntimeConfig): MobileLinuxCommandResultFfi {\n    val runtime = MobileLinuxRuntime.create(config)\n    runtime.boot()\n    try {\n        val result = runtime.execute(MobileLinuxCommandRequestFfi(\n            command = \"/bin/sh\", args = listOf(\"-c\", \"printf 'ready\\n'\"),\n            cwd = \"/root\", env = emptyList(), stdin = null,\n            timeoutMs = 10_000uL, network = NetworkPolicyFfi.DISABLED,\n            resourceLimits = ResourceLimitsFfi(null, null, null, null),\n            mounts = emptyList(),\n        ))\n        check(result.enforcement.networkPolicyEnforced) { \"Network isolation not proven\" }\n        check(result.exitCode == 0 && !result.timedOut && !result.cancelled)\n        return result\n    } finally {\n        runtime.shutdown()\n    }\n}",
            "verification": "manual",
            "exampleId": "android-runtime-lifecycle"
          }
        ]
      },
      {
        "id": "android-recovery",
        "title": {
          "en": "Packaging, recovery and device acceptance",
          "zh": "打包、恢复与设备验收"
        },
        "paragraphs": [
          {
            "en": "Missing helper libraries or an ABI mismatch are packaging failures, before model or shell diagnosis. Verify the selected APK flavor contains its native libraries and extracted executable helpers. Rootfs installation errors belong to the archive/manifest/SBOM boundary; repair uses validated staging, and reset must preserve external user workspaces.",
            "zh": "辅助库缺失或 ABI 不匹配属于打包故障，应先于模型或 shell 排查。确认选定 APK flavor 含有原生库和已提取的可执行辅助程序。rootfs 安装错误属于压缩包／manifest／SBOM 边界；repair 使用已验证的暂存镜像，reset 必须保留外部用户工作区。"
          },
          {
            "en": "The SDK exposes background processes but Android can stop an application process. Host foreground-service and app lifecycle policy are separate from Linux task state. Keep resource paths stable across activity recreation, retain handles in an application-owned component, and inspect task snapshots rather than inferring task survival from a screen.",
            "zh": "SDK 暴露后台进程，但 Android 可能终止应用进程。宿主前台服务与应用生命周期策略独立于 Linux 任务状态。Activity 重建时保持资源路径稳定，在应用拥有的组件中保留句柄，并读取任务快照，不能根据页面状态推断任务存活。"
          },
          {
            "en": "RealRuntimeSmokeTest exercises actual device requests and policy cases. It is the acceptance entry point, not evidence that this documentation edit ran instrumentation. Validate both arm64-v8a and x86_64 where supported, timeout descendants, binary stdio, terminal resize and denied network/memory policies on the artifact to be distributed.",
            "zh": "RealRuntimeSmokeTest 覆盖真实设备请求和策略用例，是验收入口，不表示此文档修改已经执行 instrumentation。对待分发产物验证支持的 arm64-v8a 与 x86_64、超时后代进程、二进制 stdio、终端尺寸调整及拒绝网络／内存策略。"
          }
        ]
      }
    ],
    "sources": [
      {
        "repo": "mobile-linux-runtime",
        "path": "android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "android/installer/src/main/kotlin/io/lingxi/mobilelinux/RootfsInstaller.kt"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "android/gradle-plugin/src/main/java/io/lingxi/mobilelinux/gradle/MobileLinuxPlugin.java"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-android/src/runtime/config.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ffi/src/runtime.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "examples/android/app/src/androidTest/java/io/lingxi/mobilelinux/sample/RealRuntimeSmokeTest.kt"
      }
    ]
  },
  {
    "id": "mobile-ios",
    "group": "mobile",
    "title": {
      "en": "iOS · Swift",
      "zh": "iOS · Swift"
    },
    "description": {
      "en": "Consume SwiftPM and XCFramework products, provide iSH fakefs resources, and control lifecycle through Swift.",
      "zh": "接入 SwiftPM 与 XCFramework 产品、提供 iSH fakefs 资源，并通过 Swift 控制生命周期。"
    },
    "packageName": "MobileLinuxRuntime",
    "sourceUrl": "https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/ios/SDK/MobileLinuxRuntime.swift",
    "sections": [
      {
        "id": "swiftpm",
        "title": {
          "en": "SwiftPM products",
          "zh": "SwiftPM 产品"
        },
        "paragraphs": [
          {
            "en": "The package exposes MobileLinuxRuntime for Swift applications and MobileLinuxNativeSupport for applications already embedding Rust. The full product links MobileLinuxRuntimeFFI and MobileLinuxNativeSupport; the support product adds sqlite3, resolv, z and c++ linker libraries.",
            "zh": "该 package 为 Swift 应用暴露 MobileLinuxRuntime，为已嵌入 Rust 的应用暴露 MobileLinuxNativeSupport。完整产品链接 MobileLinuxRuntimeFFI 与 MobileLinuxNativeSupport；支持产品加入 sqlite3、resolv、z 和 c++ 链接库。"
          },
          {
            "en": "The checked-in repository-root Package.swift points to v0.1.0-rc.2 binary assets built from source 9e8e19a473728722183fcbfa0814398c1dd8a8ff and contains their actual checksums. Asset availability and release device acceptance must be checked against that release. Do not replace checksums with placeholder values.",
            "zh": "仓库根目录中已提交的 Package.swift 指向 v0.1.0-rc.2 二进制资源，声明源码版本为 9e8e19a473728722183fcbfa0814398c1dd8a8ff，并包含实际 checksums。资源是否可用及发布版本的真机验收需核对该次发布。不要用占位值替换 checksums。"
          },
          {
            "en": "For local SwiftPM, stage the two built frameworks in ios/Artifacts and matching generated bindings in ios/Bindings, then add the local ios package. The local manifest cannot resolve from a clean checkout without those binary artifacts.",
            "zh": "本地 SwiftPM 接入时，将两个构建完成的 frameworks 放到 ios/Artifacts，并把匹配的生成绑定放到 ios/Bindings，然后添加本地 ios package。缺少这些二进制产物时，本地 manifest 无法从干净检出目录解析。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "ios/Package.swift · local target excerpt",
              "zh": "本地 target 摘录"
            },
            "language": "swift",
            "code": "    targets: [\n        .binaryTarget(name: \"MobileLinuxRuntimeFFI\", path: \"Artifacts/MobileLinuxRuntimeFFI.xcframework\"),\n        .binaryTarget(name: \"MobileLinuxNativeSupport\", path: \"Artifacts/MobileLinuxNativeSupport.xcframework\"),\n        .target(name: \"MobileLinuxNativeSupportLink\", dependencies: [\"MobileLinuxNativeSupport\"], path: \"NativeSupportLink\", linkerSettings: [.linkedLibrary(\"sqlite3\"), .linkedLibrary(\"resolv\"), .linkedLibrary(\"z\"), .linkedLibrary(\"c++\")]),\n        .target(name: \"MobileLinuxRuntimeBindings\", dependencies: [\"MobileLinuxRuntimeFFI\"], path: \"Bindings\"),\n        .target(name: \"MobileLinuxRuntime\", dependencies: [\"MobileLinuxRuntimeBindings\", \"MobileLinuxNativeSupportLink\"], path: \"SDK\"),",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "build-frameworks",
        "title": {
          "en": "Build frameworks from source",
          "zh": "从源码构建 frameworks"
        },
        "paragraphs": [
          {
            "en": "Build native support and FFI independently from the same source revision. Simulator slices support linking and API checks; real guest execution requires an arm64 iOS device.",
            "zh": "从同一源码修订版本分别构建原生支持与 FFI。模拟器切片支持链接与 API 检查；真正的 guest 执行要求 arm64 iOS 真机。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "SDK-INTEGRATION.md · XCFramework build excerpt",
              "zh": "XCFramework 构建摘录"
            },
            "language": "bash",
            "code": "rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios\nbash scripts/build/build-ios-xcframework.sh \\\n  --output \"$SDK_BUILD_DIR/ios-native\" --cache \"$SDK_BUILD_DIR/ios-cache\" \\\n  --kind native-support --configuration Release\npython3 scripts/build/build-ffi.py --platform ios --release \\\n  --output-dir \"$SDK_BUILD_DIR/ios-ffi\" --target-dir \"$SDK_BUILD_DIR/ios-rust\"",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "ios-rootfs",
        "title": {
          "en": "Prepare iSH fakefs resources",
          "zh": "准备 iSH fakefs 资源"
        },
        "paragraphs": [
          {
            "en": "iOS consumes an iSH fakefs ZIP rather than the Android/Linux tarball. Convert a verified aarch64 rootfs with the pinned SDK tool, then distribute its ZIP, manifest, RootfsPatch.bundle, libvdso.so.elf and default mount resources. RuntimeConfig must use the converted ZIP digest.",
            "zh": "iOS 使用 iSH fakefs ZIP，而不是 Android/Linux tarball。使用 SDK 固定的工具转换经过验证的 aarch64 rootfs，然后分发 ZIP、manifest、RootfsPatch.bundle、libvdso.so.elf 与默认挂载资源。RuntimeConfig 必须使用转换后 ZIP 的摘要。"
          },
          {
            "en": "Supply absolute app-private managedRoot and appSandboxRoot, workspaceHostPath, persistent stableWorkspaceId, rootfsArchivePath, rootfsPatchPath and defaultMountPath. The app also chooses protectedHostRoots, allowedMountRoots and allowedGuestRoots.",
            "zh": "提供应用私有的绝对 managedRoot 与 appSandboxRoot、workspaceHostPath、持久 stableWorkspaceId、rootfsArchivePath、rootfsPatchPath 与 defaultMountPath。应用还负责选择 protectedHostRoots、allowedMountRoots 与 allowedGuestRoots。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "SDK-INTEGRATION.md · rootfs conversion excerpt",
              "zh": "rootfs 转换摘录"
            },
            "language": "bash",
            "code": ": \"${ROOTFS_ARCHIVE:?Set the aarch64 archive path}\"\n: \"${ROOTFS_ARCHIVE_SHA256:?Set its pinned SHA-256}\"\nbash scripts/build/prepare-ios-rootfs.sh \\\n  --archive \"$ROOTFS_ARCHIVE\" --expected-archive-sha256 \"$ROOTFS_ARCHIVE_SHA256\" \\\n  --profile toolchain --native-output \"$SDK_BUILD_DIR/ios-native/native\" \\\n  --output \"$SDK_BUILD_DIR/ios-rootfs\" --cache \"$SDK_BUILD_DIR/ios-cache\"",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "swift-wrapper",
        "title": {
          "en": "Swift runtime wrapper",
          "zh": "Swift 运行时封装"
        },
        "code": [
          {
            "label": {
              "en": "MobileLinuxRuntime.swift · wrapper source",
              "zh": "封装源码"
            },
            "language": "swift",
            "code": "import MobileLinuxRuntimeBindings\n\n/// The app chooses paths, authorization, mount policy and rootfs distribution.\n/// Keep this object alive while using its task, PTY and raw-stdio handles.\npublic final class MobileLinuxRuntime {\n    public let handle: RuntimeHandle\n    public init(configuration: RuntimeConfig) throws {\n        self.handle = try createRuntime(config: configuration)\n    }\n    public func boot() async throws -> MobileLinuxStatusFfi { try await handle.boot() }\n    public func shutdown() async throws { try await handle.shutdown() }\n    public func capability() async -> MobileLinuxCapabilityFfi { await handle.capability() }\n}",
            "verification": "excerpt"
          }
        ],
        "apis": [
          {
            "name": "MobileLinuxRuntime.init",
            "signature": "public init(configuration: RuntimeConfig) throws",
            "description": {
              "en": "Create a runtime. Retain it while using handle tasks, PTY and raw stdio.",
              "zh": "创建运行时。使用 handle 上的任务、PTY 与 raw stdio 时保留该对象。"
            }
          },
          {
            "name": "MobileLinuxRuntime.boot",
            "signature": "public func boot() async throws -> MobileLinuxStatusFfi",
            "description": {
              "en": "Install/boot the configured backend and return status.",
              "zh": "安装并启动已配置的后端，返回状态。"
            }
          },
          {
            "name": "MobileLinuxRuntime.shutdown",
            "signature": "public func shutdown() async throws",
            "description": {
              "en": "Close logical runtime tasks. The iSH kernel remains alive for the app process lifetime.",
              "zh": "关闭逻辑运行时任务。iSH 内核仍存活到应用进程结束。"
            }
          },
          {
            "name": "MobileLinuxRuntime.capability",
            "signature": "public func capability() async -> MobileLinuxCapabilityFfi",
            "description": {
              "en": "Probe live availability before execution.",
              "zh": "执行前探测当前可用性。"
            }
          }
        ]
      },
      {
        "id": "swift-handle",
        "title": {
          "en": "Commands and byte streams",
          "zh": "命令与字节流"
        },
        "paragraphs": [
          {
            "en": "Command execution is exposed on runtime.handle. Generated Swift bindings use camelCase names and Data for byte streams. The following signatures are from the checked-in RuntimeHandleProtocol.",
            "zh": "命令执行通过 runtime.handle 暴露。生成的 Swift 绑定使用 camelCase 名称，并用 Data 表示字节流。以下签名来自已提交的 RuntimeHandleProtocol。"
          }
        ],
        "apis": [
          {
            "name": "RuntimeHandle.runCommand",
            "signature": "func runCommand(request: MobileLinuxCommandRequestFfi) async throws -> MobileLinuxCommandResultFfi",
            "description": {
              "en": "Run a command and collect stdout/stderr and enforcement metadata.",
              "zh": "运行命令并收集 stdout/stderr 与执行回执。"
            }
          },
          {
            "name": "RuntimeHandle.runCommandStreaming",
            "signature": "func runCommandStreaming(request: MobileLinuxCommandRequestFfi, sink: RuntimeEventSink) async throws -> MobileLinuxCommandResultFfi",
            "description": {
              "en": "Stream structured runtime events while the command runs.",
              "zh": "命令运行时流式接收结构化运行时事件。"
            }
          },
          {
            "name": "RuntimeHandle.openRawStdio",
            "signature": "func openRawStdio(request: MobileLinuxCommandRequestFfi) async throws -> RawStdioHandle",
            "description": {
              "en": "Open exact binary stdin/stdout/stderr with no request stdin or timeout.",
              "zh": "打开精确的二进制 stdin/stdout/stderr；请求中不设置 stdin 或 timeout。"
            }
          },
          {
            "name": "RuntimeHandle.writeRawStdio",
            "signature": "func writeRawStdio(id: String, data: Data) async throws",
            "description": {
              "en": "Write binary stdin without text conversion.",
              "zh": "写入二进制 stdin，不做文本转换。"
            }
          },
          {
            "name": "RuntimeHandle.readRawStdio",
            "signature": "func readRawStdio(id: String, maxBytes: UInt32) async throws -> RawStdioOutput",
            "description": {
              "en": "Read bounded output and inspect closed and exitCode.",
              "zh": "读取有上限的输出，并检查 closed 与 exitCode。"
            }
          },
          {
            "name": "RuntimeHandle.openPty",
            "signature": "func openPty(request: MobileLinuxPtyOpenRequestFfi) async throws -> MobileLinuxPtySessionHandleFfi",
            "description": {
              "en": "Open an interactive PTY process.",
              "zh": "打开交互式 PTY 进程。"
            }
          },
          {
            "name": "RuntimeHandle.writePty",
            "signature": "func writePty(handle: MobileLinuxPtySessionHandleFfi, input: Data) async throws",
            "description": {
              "en": "Write terminal bytes.",
              "zh": "写入终端字节。"
            }
          }
        ]
      },
      {
        "id": "swift-example",
        "title": {
          "en": "Command helper",
          "zh": "命令辅助函数"
        },
        "paragraphs": [
          {
            "en": "This partial source excerpt from the device smoke supplies a disabled-network command and optional memory/timeout limits. Create and boot a configured runtime before passing the request to handle.runCommand.",
            "zh": "这段真机 smoke 的部分源码摘录提供禁用网络的命令，以及可选内存与超时限制。把请求传给 handle.runCommand 之前，先创建并启动配置好的运行时。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "SDKDeviceSmoke.swift · helper excerpt",
              "zh": "辅助函数摘录"
            },
            "language": "swift",
            "code": "    private func command(_ script: String, network: NetworkPolicyFfi = .disabled, memory: UInt32? = nil, timeout: UInt64? = 10_000) -> MobileLinuxCommandRequestFfi {\n        MobileLinuxCommandRequestFfi(command: \"/bin/sh\", args: [\"-c\", script], cwd: nil, env: [], stdin: nil,\n            timeoutMs: timeout, network: network,\n            resourceLimits: ResourceLimitsFfi(maxCpuSeconds: nil, maxMemoryMb: memory, maxProcesses: nil, maxOpenFiles: nil), mounts: [])\n    }",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "ios-lifetime",
        "title": {
          "en": "Kernel lifetime and restart",
          "zh": "内核生命周期与重启"
        },
        "paragraphs": [
          {
            "en": "An app process has one immutable iSH kernel/rootfs configuration, while sessions may use different workspace paths and stable IDs. Changing an initialized kernel configuration or repairing/resetting after boot can return RestartRequired. The host owns the app restart experience.",
            "zh": "一个应用进程具有一份不可变的 iSH 内核与 rootfs 配置，会话可以使用不同工作区路径和稳定 ID。修改已初始化的内核配置，或在启动后 repair/reset，可能返回 RestartRequired。宿主负责应用重启体验。"
          }
        ]
      },
      {
        "id": "ios-end-to-end",
        "title": {
          "en": "Execute and close a Swift session",
          "zh": "执行并关闭 Swift 会话"
        },
        "paragraphs": [
          {
            "en": "This complete function receives a host-created configuration, boots the logical session, executes a bounded request and closes it on both success and error. The host must additionally inspect exitCode, timedOut, cancelled and enforcement.networkPolicyEnforced before accepting output. Device archive, overlay, VDSO and mount resources must match the selected SDK build.",
            "zh": "此完整函数接收宿主创建的配置，启动逻辑会话、执行有时间上限的请求，并在成功或失败时关闭会话。宿主还须检查 exitCode、timedOut、cancelled 与 enforcement.networkPolicyEnforced 后才接受输出。真机压缩包、overlay、VDSO 与挂载资源必须匹配选定 SDK 构建。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Swift lifecycle function",
              "zh": "Swift 生命周期函数"
            },
            "language": "swift",
            "code": "import MobileLinuxRuntime\nimport MobileLinuxRuntimeBindings\n\nfunc executeOnce(config: RuntimeConfig) async throws -> MobileLinuxCommandResultFfi {\n    let runtime = try MobileLinuxRuntime(configuration: config)\n    _ = try await runtime.boot()\n    do {\n        let request = MobileLinuxCommandRequestFfi(\n            command: \"/bin/sh\", args: [\"-c\", \"printf 'ready\\n'\"], cwd: \"/root\",\n            env: [], stdin: nil, timeoutMs: 10_000, network: .disabled,\n            resourceLimits: ResourceLimitsFfi(maxCpuSeconds: nil, maxMemoryMb: nil,\n                maxProcesses: nil, maxOpenFiles: nil), mounts: [])\n        let result = try await runtime.handle.runCommand(request: request)\n        try await runtime.shutdown()\n        return result\n    } catch {\n        try? await runtime.shutdown()\n        throw error\n    }\n}",
            "verification": "manual",
            "exampleId": "ios-runtime-lifecycle"
          }
        ]
      },
      {
        "id": "ios-recovery-and-evidence",
        "title": {
          "en": "Process identity and acceptance boundary",
          "zh": "进程身份与验收边界"
        },
        "paragraphs": [
          {
            "en": "Stable workspace identity is independent of the immutable process kernel identity. Switching workspaces must preserve the kernel/rootfs configuration. When repair, reset or configuration change returns RestartRequired, persist the requested state and surface a deliberate host restart action; do not loop boot attempts in the same process.",
            "zh": "稳定工作区身份与进程不可变内核身份相互独立。切换工作区须保留内核／rootfs 配置。当 repair、reset 或配置变更返回 RestartRequired，保存所请求状态并提供明确的宿主重启动作，不要在同一进程内循环 boot。"
          },
          {
            "en": "Swift concurrency does not itself grant background execution time. The application owns OS suspension, scene transitions and resource cleanup. An iOS simulator native-support slice deliberately reports unavailable guest execution; API generation and simulator linking cannot replace a signed physical-device run.",
            "zh": "Swift 并发本身不授予后台执行时间。应用负责系统挂起、scene 切换及资源清理。iOS 模拟器 native-support 切片会明确报告 guest 执行不可用；API 生成和模拟器链接不能替代已签名真机运行。"
          },
          {
            "en": "SDKDeviceSmoke is the physical-device acceptance source. Validate the exact framework and resource bundle together, including interrupted install, integrity refresh, restart-required behavior, raw byte preservation and whole-tree cancellation. Unit tests with fake native transport establish lifecycle logic only.",
            "zh": "SDKDeviceSmoke 是真机验收源码。框架与资源包须作为一组验证，包括安装中断、完整性刷新、要求重启的行为、raw 字节保持及完整进程树取消。使用伪原生传输的单元测试仅证明生命周期逻辑。"
          }
        ]
      }
    ],
    "sources": [
      {
        "repo": "mobile-linux-runtime",
        "path": "ios/SDK/MobileLinuxRuntime.swift"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "ios/Package.swift"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/config.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/lifecycle.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/implementation.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "examples/ios/App/SDKDeviceSmoke.swift"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "scripts/build/prepare-ios-rootfs.sh"
      }
    ]
  },
  {
    "id": "mobile-authorization",
    "group": "mobile",
    "title": {
      "en": "Permissions and integrity",
      "zh": "权限与完整性"
    },
    "description": {
      "en": "Host-owned mount policy, network and resource enforcement, rootfs trust and explicit platform limits.",
      "zh": "宿主拥有的挂载策略、网络与资源执行约束、rootfs 信任及明确的平台限制。"
    },
    "packageName": "mobile-linux-runtime",
    "sourceUrl": "https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/crates/mobile-linux-api/src/mobile_linux.rs",
    "sections": [
      {
        "id": "host-policy",
        "title": {
          "en": "Host-owned authorization",
          "zh": "宿主拥有授权决策"
        },
        "paragraphs": [
          {
            "en": "The application owns admission, filesystem access, rootfs trust and lifecycle. PRoot is a compatibility layer, not a security boundary. Validate requests at the host boundary and use platform capabilities and enforcement receipts when an operation requires a policy.",
            "zh": "应用负责准入、文件系统访问、rootfs 信任与生命周期。PRoot 是兼容层，不是安全边界。在宿主边界校验请求；操作要求特定策略时，检查平台能力和策略执行回执。"
          },
          {
            "en": "RuntimeConfig.authorization_file is an optional iOS value forwarded to the native bridge and included in runtime identity. The field is not a universal SDK authentication service, token format or account system. Android create_runtime does not consume it.",
            "zh": "RuntimeConfig.authorization_file 是可选的 iOS 配置值，会转发给原生桥接并纳入运行时身份。该字段不是通用的 SDK 身份认证服务、token 格式或账号系统。Android create_runtime 不消费此字段。"
          }
        ]
      },
      {
        "id": "mount-policy",
        "title": {
          "en": "iOS mount policy",
          "zh": "iOS 挂载策略"
        },
        "bullets": [
          {
            "en": "Request mounts cannot overlap managed runtime storage or configured protectedHostRoots, including ancestors. Mounting the app sandbox root itself is rejected.",
            "zh": "请求挂载不能与托管运行时存储或 protectedHostRoots 重叠，包括它们的祖先目录。直接挂载应用沙箱根目录会被拒绝。"
          },
          {
            "en": "Workspace mounts must match workspaceHostPath and /workspace/<stableWorkspaceId>. Request mounts cannot replace persistent guest /root.",
            "zh": "工作区挂载必须匹配 workspaceHostPath 和 /workspace/<stableWorkspaceId>。请求挂载不能替换持久 guest /root。"
          },
          {
            "en": "Non-workspace mounts require both an allowedMountRoots host prefix and an allowedGuestRoots guest prefix. Empty allowlists therefore admit no extra request mounts.",
            "zh": "非工作区挂载必须同时匹配 allowedMountRoots 的 host 前缀与 allowedGuestRoots 的 guest 前缀。因此，空白名单不允许额外请求挂载。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "types.rs · mount record excerpt",
              "zh": "挂载记录摘录"
            },
            "language": "rust",
            "code": "pub struct MobileLinuxMountSpecFfi {\n    pub host_path: String,\n    pub guest_path: String,\n    pub read_only: bool,\n    pub purpose: MobileLinuxMountPurposeFfi,\n}",
            "verification": "excerpt"
          }
        ],
        "note": {
          "en": "These explicit host/guest allowlist fields are consumed by the iOS backend. Android has a different runtime configuration and validation path; do not assume iOS policy fields configure Android.",
          "zh": "这些显式 host/guest 白名单字段由 iOS 后端消费。Android 使用不同的运行时配置与验证路径；不要假定 iOS 策略字段会配置 Android。"
        }
      },
      {
        "id": "network-and-limits",
        "title": {
          "en": "Network and resource enforcement",
          "zh": "网络与资源限制执行"
        },
        "paragraphs": [
          {
            "en": "NetworkPolicy has Disabled, LoopbackOnly and Allowed. ResourceLimits includes optional CPU seconds, resident memory, process count and open-file ceilings. Requested values do not prove enforcement; inspect the receipt or handle typed errors when the backend cannot apply a required constraint.",
            "zh": "NetworkPolicy 包含 Disabled、LoopbackOnly 与 Allowed。ResourceLimits 包含可选的 CPU 秒数、驻留内存、进程数及打开文件数上限。请求值不代表已执行约束；检查回执，或处理后端无法应用所需约束时返回的类型化错误。"
          }
        ],
        "apis": [
          {
            "name": "LinuxEnforcementReceipt::ensure_for",
            "signature": "pub fn ensure_for(self, network: NetworkPolicy, limits: ResourceLimits) -> Result<(), MobileLinuxError>",
            "description": {
              "en": "Requires network_policy_enforced for policies stricter than Allowed and memory_limit_enforced for any configured memory ceiling. Missing proof returns NetworkPolicyUnavailable or ResourceLimitExceeded.",
              "zh": "对比 Allowed 更严格的策略要求 network_policy_enforced；配置内存上限时要求 memory_limit_enforced。缺少证明会返回 NetworkPolicyUnavailable 或 ResourceLimitExceeded。"
            }
          }
        ],
        "code": [
          {
            "label": {
              "en": "mobile_linux.rs · enforcement receipt excerpt",
              "zh": "执行回执摘录"
            },
            "language": "rust",
            "code": "pub struct LinuxEnforcementReceipt {\n    pub network_policy_enforced: bool,\n    pub memory_limit_enforced: bool,\n}",
            "verification": "excerpt"
          }
        ]
      },
      {
        "id": "rootfs-trust",
        "title": {
          "en": "Archive trust and executable authorization",
          "zh": "压缩包信任与可执行文件授权"
        },
        "paragraphs": [
          {
            "en": "Pin the expected archive SHA-256 at the application boundary. The manifest is not a replacement for that trust decision. Verify archive identity, immutable inventory, symlink paths and SBOM before activation. RootfsStore.authorize_manifest_file validates immutable executable allowlist entries and rejects writable guest locations.",
            "zh": "在应用边界固定预期压缩包 SHA-256。manifest 不能替代这一信任决策。激活前验证压缩包身份、不可变清单、符号链接路径与 SBOM。RootfsStore.authorize_manifest_file 校验不可变可执行文件白名单，并拒绝可写 guest 位置。"
          },
          {
            "en": "Default guest writable roots are /root, /tmp, /var/tmp and /workspace. Keep managed runtime storage app-private and separate from externally mounted user workspaces.",
            "zh": "默认 guest 可写根目录为 /root、/tmp、/var/tmp 与 /workspace。托管运行时存储应保持应用私有，并与外部挂载的用户工作区分开。"
          }
        ]
      },
      {
        "id": "errors",
        "title": {
          "en": "Errors and platform limits",
          "zh": "错误与平台限制"
        },
        "apis": [
          {
            "name": "MobileLinuxError::Unsupported",
            "signature": "Unsupported",
            "description": {
              "en": "The selected backend does not implement the requested operation.",
              "zh": "所选后端没有实现请求的操作。"
            }
          },
          {
            "name": "MobileLinuxError::Unavailable",
            "signature": "Unavailable(String)",
            "description": {
              "en": "The backend cannot execute now; inspect the detail and capability reason.",
              "zh": "后端当前不能执行；检查详细信息与 capability 的原因。"
            }
          },
          {
            "name": "MobileLinuxError::RestartRequired",
            "signature": "RestartRequired(String)",
            "description": {
              "en": "A fresh host process is required because the native kernel is already initialized.",
              "zh": "原生内核已初始化，需要新的宿主进程。"
            }
          },
          {
            "name": "MobileLinuxError::Integrity",
            "signature": "Integrity(String)",
            "description": {
              "en": "Rootfs verification failed.",
              "zh": "rootfs 验证失败。"
            }
          },
          {
            "name": "MobileLinuxError::InvalidRequest",
            "signature": "InvalidRequest(String)",
            "description": {
              "en": "A path, ABI, identity or operation argument failed validation.",
              "zh": "路径、ABI、身份或操作参数未通过校验。"
            }
          },
          {
            "name": "MobileLinuxError::NetworkPolicyUnavailable",
            "signature": "NetworkPolicyUnavailable(String)",
            "description": {
              "en": "Required network policy could not be enforced.",
              "zh": "无法执行所需网络策略。"
            }
          },
          {
            "name": "MobileLinuxError::ResourceLimitExceeded",
            "signature": "ResourceLimitExceeded(String)",
            "description": {
              "en": "A requested limit could not be guaranteed or was exceeded.",
              "zh": "无法保证所需资源限制，或资源已超限。"
            }
          }
        ],
        "note": {
          "en": "An iOS simulator uses an unavailable backend. App-level operating-system permissions and physical-device acceptance remain host responsibilities.",
          "zh": "iOS 模拟器使用不可用后端。应用级操作系统权限与真机验收仍由宿主负责。"
        }
      },
      {
        "id": "attribution",
        "title": {
          "en": "Distribution notices",
          "zh": "分发声明"
        },
        "paragraphs": [
          {
            "en": "SDK Rust source retains MIT OR Apache-2.0, with explicit crate/source exceptions. Full distributions contain third-party components under separate terms, including GPL/LGPL code. Preserve the component notices, source revisions, patch provenance and rootfs SBOM licenses when distributing SDK artifacts.",
            "zh": "SDK Rust 源码保留 MIT OR Apache-2.0，部分 crate 或源码有明确例外。完整分发包含适用独立条款的第三方组件，包括 GPL/LGPL 代码。分发 SDK 产物时保留组件声明、源码版本、补丁来源与 rootfs SBOM 许可证信息。"
          }
        ]
      },
      {
        "id": "policy-decision-flow",
        "title": {
          "en": "Admission and enforcement flow",
          "zh": "准入与执行约束流程"
        },
        "paragraphs": [
          {
            "en": "An application permission decision admits an operation; runtime validation then checks its concrete path, mount, network and resource request. These are distinct decisions. When a required guarantee is unavailable, return a typed failure before accepting output rather than interpreting compatibility-layer execution as isolation.",
            "zh": "应用权限决定准入操作；运行时校验随后检查具体路径、挂载、网络与资源请求，两者是不同决定。当所需保证不可用时，在接受输出前返回类型化错误，不能把兼容层执行解释成隔离。"
          },
          {
            "en": "For finite commands inspect the returned enforcement receipt. Long-lived process/PTY/raw handles also carry enforcement identity in the portable contract; validate it when admitting a session. ResourceLimits lists requested ceilings, but its portable proof currently has only network_policy_enforced and memory_limit_enforced. Do not invent receipt bits for CPU, process count or file descriptors.",
            "zh": "有限命令检查返回的执行回执。可移植契约中的长期进程／PTY／raw 句柄也携带执行约束身份，准入会话时进行验证。ResourceLimits 列出所请求上限，但当前可移植证明仅包含 network_policy_enforced 和 memory_limit_enforced，不能臆造 CPU、进程数或文件描述符的回执位。"
          }
        ],
        "steps": [
          {
            "en": "Authorize the user action and choose explicit policy.",
            "zh": "授权用户动作并选择明确策略。"
          },
          {
            "en": "Validate absolute paths and platform mount boundaries.",
            "zh": "校验绝对路径与平台挂载边界。"
          },
          {
            "en": "Verify the image and admit execution through the selected backend.",
            "zh": "验证镜像，并通过选定后端准入执行。"
          },
          {
            "en": "Check the typed result/handle receipt, output and terminal state.",
            "zh": "检查类型化结果／句柄回执、输出及终态。"
          }
        ]
      },
      {
        "id": "negative-acceptance",
        "title": {
          "en": "Failure scenarios to test",
          "zh": "需要验证的失败场景"
        },
        "paragraphs": [
          {
            "en": "Corrupt the pinned archive or an immutable file: staging/verify must report integrity failure, and no unverified image may become active. Request a writable guest executable through immutable authorization: it must be rejected. Restore valid staging and test repair without deleting user workspace data.",
            "zh": "破坏固定的压缩包或不可变文件：暂存／verify 必须报告完整性失败，未经验证的镜像不能成为活动镜像。通过不可变授权请求可写 guest 可执行文件时，必须拒绝。恢复有效暂存镜像后，验证 repair 不删除用户工作区数据。"
          },
          {
            "en": "Request mount ancestors of protected paths, a mismatched stable workspace, missing allowlists, a zero-byte raw read, or raw stdin/timeout fields. Check typed rejection. Separately test network-disabled escape attempts and memory limits on actual devices, because host-only tests cannot establish native policy enforcement.",
            "zh": "请求保护路径的祖先挂载、不匹配的稳定工作区、缺少白名单、零字节 raw read，或 raw stdin／timeout 字段，检查类型化拒绝。另在真机测试禁用网络的逃逸尝试与内存限制，因为宿主测试不能证明原生策略执行。"
          }
        ]
      }
    ],
    "sources": [
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-api/src/mobile_linux.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-api/src/execution.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-core/src/store/verification.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-core/src/store/install.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/validation.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-android/src/runtime/mounts.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "scripts/tests/test_resource_contracts.py"
      }
    ]
  }
];
