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
        "id": "first-execution",
        "title": {
          "en": "Get from installation to a successful command",
          "zh": "从安装到第一个成功命令"
        },
        "paragraphs": [
          {
            "en": "Use this order when embedding the SDK for the first time. Constructing a wrapper creates a handle; it does not mean a rootfs has been installed or that the selected device can execute Linux. Keep installation, readiness and command success as three separate results in your application.",
            "zh": "首次嵌入 SDK 时按以下顺序接入。创建包装对象只会得到句柄，不表示 rootfs 已安装，也不表示当前设备可以执行 Linux。应用应分别记录安装、就绪与命令执行的结果。"
          }
        ],
        "steps": [
          {
            "en": "Select either the full Kotlin/Swift distribution or the native-support distribution for an existing Rust host, with bindings and resources from the same source revision.",
            "zh": "选择完整 Kotlin／Swift 分发，或供已有 Rust 宿主使用的 native-support 分发；绑定与资源必须来自同一源码版本。"
          },
          {
            "en": "Verify and stage the platform image. On Android, RootfsInstaller.stage prepares staged data and handle.repairRootfs activates it; iOS takes explicit converted fakefs ZIP and overlay paths.",
            "zh": "校验并暂存平台镜像。Android 由 RootfsInstaller.stage 准备 staged 数据，再由 handle.repairRootfs 激活；iOS 提供转换后的 fakefs ZIP 与 overlay 的显式路径。"
          },
          {
            "en": "Create one retained runtime with app-private absolute paths. Probe capability and show its reason when available is false; simulator linking is not guest readiness.",
            "zh": "使用应用私有绝对路径创建并持有运行时。检查 capability；available 为 false 时展示 reason，不能把模拟器链接成功当作 guest 已就绪。"
          },
          {
            "en": "Await boot, then execute a small bounded command with an explicit guest working directory and network policy.",
            "zh": "等待 boot 完成，再以明确的 guest 工作目录和网络策略执行一个有时间上限的小命令。"
          },
          {
            "en": "Accept the result only after checking exit_code, timed_out, cancelled and the enforcement receipt. Close owned sessions and await shutdown when the host no longer needs the runtime.",
            "zh": "检查 exit_code、timed_out、cancelled 与执行回执后再接受结果。宿主不再需要运行时时，关闭其拥有的会话并等待 shutdown。"
          }
        ],
        "links": [
          {
            "label": {
              "en": "Complete portable command example",
              "zh": "完整可移植命令示例"
            },
            "href": "/docs/mobile-rust#complete-command"
          },
          {
            "label": {
              "en": "Android installation and activation",
              "zh": "Android 安装与激活"
            },
            "href": "/docs/mobile-android#install-rootfs"
          },
          {
            "label": {
              "en": "Swift session lifecycle",
              "zh": "Swift 会话生命周期"
            },
            "href": "/docs/mobile-ios#ios-end-to-end"
          }
        ]
      },
      {
        "id": "guest-identity",
        "title": {
          "en": "Guest user, home and project paths",
          "zh": "Guest 用户、主目录与项目路径"
        },
        "paragraphs": [
          {
            "en": "Pass Linux guest paths to command cwd and guest-side tools. Pass native host paths to runtime storage and host_path mount fields. The same project has two coordinates: a host backing directory and the guest path exposed by a mount. Do not send an Android filesDir or iOS application-container path as a Linux cwd.",
            "zh": "命令 cwd 与 guest 工具使用 Linux guest 路径；运行时存储和挂载 host_path 使用原生宿主路径。同一项目具有两个坐标：宿主 backing 目录与挂载后可见的 guest 路径。不要把 Android filesDir 或 iOS 应用容器路径当作 Linux cwd。"
          }
        ],
        "tables": [
          {
            "headers": [
              {
                "en": "Value",
                "zh": "项目"
              },
              {
                "en": "Default",
                "zh": "默认值"
              },
              {
                "en": "How to use it",
                "zh": "使用方式"
              }
            ],
            "rows": [
              [
                {
                  "en": "Account / UID / GID",
                  "zh": "账号／UID／GID"
                },
                {
                  "en": "lingxi / 1000 / 1000",
                  "zh": "lingxi / 1000 / 1000"
                },
                {
                  "en": "Default unprivileged guest identity; /root is the administrator home.",
                  "zh": "默认非特权 guest 身份；/root 是管理员主目录。"
                }
              ],
              [
                {
                  "en": "HOME",
                  "zh": "HOME"
                },
                {
                  "en": "/home/lingxi",
                  "zh": "/home/lingxi"
                },
                {
                  "en": "Use for user configuration and a first command that needs no project mount.",
                  "zh": "存放用户配置；不需要项目挂载的首个命令可使用此 cwd。"
                }
              ],
              [
                {
                  "en": "Workspace root",
                  "zh": "工作区根"
                },
                {
                  "en": "/home/lingxi/lingxi",
                  "zh": "/home/lingxi/lingxi"
                },
                {
                  "en": "guest_paths::workspace(id) appends a stable project identifier.",
                  "zh": "guest_paths::workspace(id) 在此追加稳定的项目标识。"
                }
              ],
              [
                {
                  "en": "Project path",
                  "zh": "项目路径"
                },
                {
                  "en": "/home/lingxi/lingxi/<stable-id>",
                  "zh": "/home/lingxi/lingxi/<stable-id>"
                },
                {
                  "en": "Choose a normalized ID and configure its host directory before executing project commands.",
                  "zh": "选择规范化 ID；执行项目命令前配置对应宿主目录。"
                }
              ],
              [
                {
                  "en": "Persistent host home",
                  "zh": "持久化宿主 HOME"
                },
                {
                  "en": "managed_root/persistent/lingxi",
                  "zh": "managed_root/persistent/lingxi"
                },
                {
                  "en": "Independent of replaceable rootfs images; do not place it inside an extracted active image.",
                  "zh": "独立于可替换 rootfs 镜像；不要放入解压后的 active 镜像内部。"
                }
              ],
              [
                {
                  "en": "Scratch",
                  "zh": "临时目录"
                },
                {
                  "en": "/tmp, /var/tmp",
                  "zh": "/tmp、/var/tmp"
                },
                {
                  "en": "Temporary guest files, rather than stable project identity.",
                  "zh": "用于 guest 临时文件，不作为稳定项目身份。"
                }
              ]
            ]
          }
        ]
      },
      {
        "id": "execution-selection",
        "title": {
          "en": "Choose the execution API for your feature",
          "zh": "按功能选择执行接口"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Feature",
                "zh": "功能"
              },
              {
                "en": "Entry point",
                "zh": "入口"
              },
              {
                "en": "Result and cleanup",
                "zh": "结果与清理"
              }
            ],
            "rows": [
              [
                {
                  "en": "Run a formatter, build step or finite command",
                  "zh": "运行格式化、构建步骤或有限命令"
                },
                {
                  "en": "run / run_command / Kotlin execute",
                  "zh": "run／run_command／Kotlin execute"
                },
                {
                  "en": "Collected stdout/stderr and exit state; set timeout_ms deliberately.",
                  "zh": "收集 stdout／stderr 与退出状态；主动设置 timeout_ms。"
                }
              ],
              [
                {
                  "en": "Show command output as it arrives",
                  "zh": "执行中展示命令输出"
                },
                {
                  "en": "run_streaming / run_command_streaming",
                  "zh": "run_streaming／run_command_streaming"
                },
                {
                  "en": "Consume stdout text and stderr bytes with bounded UI queues; still inspect the final result.",
                  "zh": "通过有界 UI 队列消费 stdout 文本与 stderr 字节；仍需检查最终结果。"
                }
              ],
              [
                {
                  "en": "Keep a service running under host ownership",
                  "zh": "在宿主管理下保持服务运行"
                },
                {
                  "en": "spawn_background",
                  "zh": "spawn_background"
                },
                {
                  "en": "Retain the process handle; use kill and observe terminal state.",
                  "zh": "保留进程句柄；使用 kill 并确认终态。"
                }
              ],
              [
                {
                  "en": "Build a terminal",
                  "zh": "构建终端"
                },
                {
                  "en": "open_pty / write_pty / resize_pty",
                  "zh": "open_pty／write_pty／resize_pty"
                },
                {
                  "en": "Decode terminal bytes across chunks, handle resize, and call close_pty.",
                  "zh": "跨 chunk 解码终端字节、处理尺寸变化，并调用 close_pty。"
                }
              ],
              [
                {
                  "en": "Connect an LSP or byte-framed child protocol",
                  "zh": "连接 LSP 或按字节分帧的子进程协议"
                },
                {
                  "en": "open_raw_stdio / read_raw_stdio / write_raw_stdio",
                  "zh": "open_raw_stdio／read_raw_stdio／write_raw_stdio"
                },
                {
                  "en": "Own the framing buffer and read loop; call close_raw_stdio to reap the process tree.",
                  "zh": "宿主维护分帧缓冲与读取循环；调用 close_raw_stdio 回收进程树。"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "The SDK supplies execution primitives. It does not create an Agent, grant application permissions or keep an app alive after the operating system suspends it.",
          "zh": "SDK 提供执行原语，不会自动创建 Agent、授予应用权限，或在操作系统挂起应用后保持宿主存活。"
        }
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
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-api/src/mobile_linux.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-android/src/runtime/config.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/config.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-core/src/user_workspace.rs"
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
        "id": "command-fields",
        "title": {
          "en": "Command parameters and result checks",
          "zh": "命令参数与结果检查"
        },
        "paragraphs": [
          {
            "en": "The request is an executable plus an argument vector. Shell operators, variable expansion and redirection are interpreted only when you explicitly invoke a shell, for example command=/bin/sh with args=[-c, script]. Keep user-selected arguments separate from shell source when the feature does not need a shell.",
            "zh": "请求由可执行文件与参数数组组成。只有显式启动 shell，例如 command=/bin/sh、args=[-c, script]，才会解释管道、变量展开和重定向。不需要 shell 的功能应将用户选择的参数与 shell 源码分开。"
          }
        ],
        "tables": [
          {
            "headers": [
              {
                "en": "Parameter / result",
                "zh": "参数／结果"
              },
              {
                "en": "Contract",
                "zh": "契约"
              },
              {
                "en": "Caller decision",
                "zh": "调用方决策"
              }
            ],
            "rows": [
              [
                {
                  "en": "command, args",
                  "zh": "command、args"
                },
                {
                  "en": "Non-empty command; arguments are individual values, not a joined command string.",
                  "zh": "command 非空；args 是独立参数，不是拼接后的命令字符串。"
                },
                {
                  "en": "Prefer an installed guest executable and a concrete argument list.",
                  "zh": "优先使用已安装的 guest 可执行文件与明确参数数组。"
                }
              ],
              [
                {
                  "en": "cwd",
                  "zh": "cwd"
                },
                {
                  "en": "Optional absolute guest path; it is not a native host path.",
                  "zh": "可选的 guest 绝对路径，不是原生宿主路径。"
                },
                {
                  "en": "Use /home/lingxi for a first command or the configured guest project path.",
                  "zh": "首个命令可使用 /home/lingxi；项目命令使用已配置的 guest 项目路径。"
                }
              ],
              [
                {
                  "en": "env",
                  "zh": "env"
                },
                {
                  "en": "Rust BTreeMap; FFI uses key/value records. Environment keys must be non-empty and cannot contain = or NUL.",
                  "zh": "Rust 使用 BTreeMap；FFI 使用 key／value 记录。环境变量名不能为空，不能含 = 或 NUL。"
                },
                {
                  "en": "Supply values explicitly; do not assume shell expansion occurs inside a value.",
                  "zh": "显式传入值；不要假定值内部会执行 shell 展开。"
                }
              ],
              [
                {
                  "en": "stdin, timeout_ms",
                  "zh": "stdin、timeout_ms"
                },
                {
                  "en": "Finite command input is optional text; a provided timeout must be positive.",
                  "zh": "有限命令可携带文本输入；指定的 timeout 必须大于零。"
                },
                {
                  "en": "Use raw stdio for binary or ongoing input, and set a budget appropriate to the operation.",
                  "zh": "二进制或持续输入使用 raw stdio；按操作设置合理时间预算。"
                }
              ],
              [
                {
                  "en": "network, resource_limits, mounts",
                  "zh": "network、resource_limits、mounts"
                },
                {
                  "en": "Requested policy and filesystem exposure; success still needs the matching enforcement receipt.",
                  "zh": "请求的策略与文件系统暴露范围；成功后仍需核对对应执行回执。"
                },
                {
                  "en": "Choose the necessary policy before admission; a policy error is not permission to retry with weaker isolation.",
                  "zh": "准入前选择所需策略；策略错误不表示可以自动改为更弱隔离后重试。"
                }
              ],
              [
                {
                  "en": "exit_code, timed_out, cancelled",
                  "zh": "exit_code、timed_out、cancelled"
                },
                {
                  "en": "A returned result can describe a failed or interrupted command. SDK errors are a separate result path.",
                  "zh": "返回结果仍可能描述命令失败或中断；SDK 错误是另一条返回路径。"
                },
                {
                  "en": "Check all three before marking the operation successful; retain stderr for diagnosis.",
                  "zh": "标记成功前检查这三个字段，并保留 stderr 用于诊断。"
                }
              ]
            ]
          }
        ]
      },
      {
        "id": "stream-consumption",
        "title": {
          "en": "Consume live output without blocking execution",
          "zh": "消费实时输出而不阻塞执行"
        },
        "paragraphs": [
          {
            "en": "ProcessStreamSink has separate stdout_line(String) and stderr_chunk(Vec<u8>) callbacks. Ordinary stdout line endings are removed; stderr chunks do not promise line boundaries or complete UTF-8 characters. The Android reader also emits bounded stdout fragments for an unusually long line, so a callback is not a general-purpose binary framing boundary.",
            "zh": "ProcessStreamSink 分别提供 stdout_line(String) 与 stderr_chunk(Vec<u8>) 回调。普通 stdout 行末会被移除；stderr chunk 不保证按行或完整 UTF-8 字符划分。Android 读取器还会为异常长行发送有界 stdout 片段，因此回调不能充当通用二进制协议分帧边界。"
          },
          {
            "en": "Drain stdout and stderr together. Forward output into a bounded host queue, then render on the UI executor. Returning a ProcessError from a sink stops delivery and causes the platform runner to tear down the owned command; a slow sink applies backpressure rather than producing unlimited host memory.",
            "zh": "同时消费 stdout 与 stderr。先将输出送入有界宿主队列，再由 UI 执行器渲染。sink 返回 ProcessError 会停止交付，并使平台运行器清理其拥有的命令；缓慢 sink 会施加背压，而不是无限占用宿主内存。"
          },
          {
            "en": "The portable trait provides a run-then-replay fallback. Probe streaming_output and use the selected backend implementation when real-time output is part of your feature. Receiving some output does not establish successful completion; await the command result and check its terminal fields.",
            "zh": "可移植 trait 提供先 run、再回放输出的默认实现。功能要求实时输出时，应检查 streaming_output 并使用所选后端的实现。收到部分输出不表示执行成功；仍须等待命令结果并检查终态字段。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Reusable streaming helper · host supplies runtime and sink",
              "zh": "可复用流式辅助函数 · 宿主提供运行时与 sink"
            },
            "language": "rust",
            "verification": "compile",
            "code": "use mobile_linux_api::{\n    LinuxCommandRequest, LinuxCommandResult, MobileLinuxError,\n    MobileLinuxRuntime, ProcessStreamSink,\n};\nuse std::sync::Arc;\n\npub async fn stream_command(\n    runtime: &dyn MobileLinuxRuntime,\n    request: LinuxCommandRequest,\n    sink: Arc<dyn ProcessStreamSink>,\n) -> Result<LinuxCommandResult, MobileLinuxError> {\n    let network = request.network;\n    let limits = request.resource_limits;\n    let result = runtime.run_streaming(request, sink).await?;\n    result.enforcement.ensure_for(network, limits)?;\n    Ok(result)\n}",
            "exampleId": "mobile-stream-command"
          }
        ],
        "note": {
          "en": "This helper assumes the host already owns and booted the runtime. Its caller still checks the command exit state and owns shutdown.",
          "zh": "此辅助函数假定宿主已拥有并启动运行时。调用方仍检查命令退出状态并负责 shutdown。"
        }
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
        "id": "raw-read-loop",
        "title": {
          "en": "Implement the raw stdio read loop",
          "zh": "实现 raw stdio 读取循环"
        },
        "steps": [
          {
            "en": "Open a RawStdioOpenRequest with explicit command, arguments and policy. Retain the returned session handle with the runtime owner.",
            "zh": "使用明确的命令、参数和策略创建 RawStdioOpenRequest；由运行时所有者保留返回的会话句柄。"
          },
          {
            "en": "Write complete protocol frames with write_raw_stdio. Keep framing independent of how the SDK splits later reads.",
            "zh": "通过 write_raw_stdio 写入协议帧；协议分帧不应依赖 SDK 随后的读取分块方式。"
          },
          {
            "en": "Read a bounded amount, for example 65,536 bytes. Append stdout to the protocol parser and drain stderr to diagnostics; neither buffer is guaranteed to contain a complete message.",
            "zh": "每次读取有界数据，例如 65,536 字节。把 stdout 追加到协议解析器，同时将 stderr 送往诊断通道；两个缓冲区都不保证包含完整消息。"
          },
          {
            "en": "A read with empty buffers and closed=false is not EOF. Yield or wait according to the host scheduling policy before polling again. Finish only when closed is true, handling the optional exit_code.",
            "zh": "缓冲区为空且 closed=false 不表示 EOF。再次轮询前按宿主调度策略让出执行权或等待。仅在 closed 为 true 时结束，并处理可选 exit_code。"
          },
          {
            "en": "On normal completion, protocol failure or user cancellation, await close_raw_stdio. Closing owns process-tree cleanup; dropping a screen or stopping a read loop does not replace it.",
            "zh": "正常完成、协议失败或用户取消时，都要等待 close_raw_stdio。关闭操作负责进程树清理；离开页面或停止读取循环不能替代关闭。"
          }
        ],
        "note": {
          "en": "FFI raw sessions reject command-request stdin and timeout_ms. Their input and lifetime are controlled through write/close calls. FFI max_bytes is 1..1048576; do not issue a zero-byte readiness probe.",
          "zh": "FFI raw 会话拒绝命令请求中的 stdin 与 timeout_ms，输入和生命周期由 write／close 调用管理。FFI max_bytes 范围是 1..1048576，不要用零字节读取探测就绪状态。"
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
        "id": "event-cursor",
        "title": {
          "en": "Resume monitoring with event cursors and task snapshots",
          "zh": "使用事件游标与任务快照恢复监控"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Operation",
                "zh": "操作"
              },
              {
                "en": "Meaning",
                "zh": "语义"
              },
              {
                "en": "Host handling",
                "zh": "宿主处理"
              }
            ],
            "rows": [
              [
                {
                  "en": "read_events(None, limit)",
                  "zh": "read_events(None, limit)"
                },
                {
                  "en": "Read currently retained events after sequence zero. The maximum batch is 512.",
                  "zh": "读取当前保留且序号大于零的事件；单批最多 512 条。"
                },
                {
                  "en": "Use a nonzero bounded limit and store the largest sequence you processed.",
                  "zh": "使用非零有界 limit，保存已处理事件中的最大 sequence。"
                }
              ],
              [
                {
                  "en": "read_events(Some(last), limit)",
                  "zh": "read_events(Some(last), limit)"
                },
                {
                  "en": "Returns events with sequence strictly greater than last.",
                  "zh": "返回 sequence 严格大于 last 的事件。"
                },
                {
                  "en": "Advance after processing the batch; do not add one to last before sending the cursor.",
                  "zh": "处理完一批再推进；传递游标前不要给 last 加一。"
                }
              ],
              [
                {
                  "en": "TaskStatusChanged / PtyClosed",
                  "zh": "TaskStatusChanged／PtyClosed"
                },
                {
                  "en": "Terminal state and exit detail are distinct from output frames.",
                  "zh": "终态和退出细节独立于输出帧。"
                },
                {
                  "en": "Stop the activity indicator from terminal state, not from an idle stdout channel.",
                  "zh": "根据终态停止运行指示，不要根据 stdout 暂时安静推断结束。"
                }
              ],
              [
                {
                  "en": "list_tasks / task_status",
                  "zh": "list_tasks／task_status"
                },
                {
                  "en": "Current task snapshots; task_status can return None for an unknown ID.",
                  "zh": "当前任务快照；未知 ID 的 task_status 可以返回 None。"
                },
                {
                  "en": "Reconcile a recreated UI with these snapshots. Do not treat None as a successful command.",
                  "zh": "用快照重建 UI；不要把 None 当作命令成功。"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "Streaming callbacks from RuntimeHandle.run_command_streaming use a separate per-call run-N identifier and sequence 0. Do not overwrite a backend read_events cursor with callback metadata. The runtime event buffer is not a durable transcript store.",
          "zh": "RuntimeHandle.run_command_streaming 回调使用单次调用的 run-N 标识与 sequence 0。不要用回调元数据覆盖后端 read_events 游标；运行时事件缓冲区不是持久化会话存储。"
        }
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
            "code": "use mobile_linux_api::{\n    LinuxCommandRequest, LinuxCommandResult, MobileLinuxError,\n    MobileLinuxRuntime, NetworkPolicy, ResourceLimits,\n};\nuse std::collections::BTreeMap;\n\npub async fn execute_once(\n    runtime: &dyn MobileLinuxRuntime,\n) -> Result<LinuxCommandResult, MobileLinuxError> {\n    runtime.boot().await?;\n    let network = NetworkPolicy::Disabled;\n    let limits = ResourceLimits::default();\n    let execution = runtime.run(LinuxCommandRequest {\n        command: \"/bin/sh\".into(),\n        args: vec![\"-c\".into(), \"printf 'ready\\n'\".into()],\n        cwd: Some(\"/home/lingxi\".into()),\n        env: BTreeMap::new(),\n        stdin: None,\n        timeout_ms: Some(10_000),\n        network,\n        resource_limits: limits,\n        mounts: Vec::new(),\n    }).await;\n    let cleanup = runtime.shutdown().await;\n    let result = execution?;\n    cleanup?;\n    result.enforcement.ensure_for(network, limits)?;\n    Ok(result)\n}",
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
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-android/src/runtime/process.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-android/src/runtime/streaming.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/validation.rs"
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
        "id": "android-config-fields",
        "title": {
          "en": "Fill RuntimeConfig from the Android host",
          "zh": "从 Android 宿主填写 RuntimeConfig"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Field",
                "zh": "字段"
              },
              {
                "en": "Host value",
                "zh": "宿主值"
              },
              {
                "en": "Common mistake",
                "zh": "常见问题"
              }
            ],
            "rows": [
              [
                {
                  "en": "managedRoot",
                  "zh": "managedRoot"
                },
                {
                  "en": "A stable app-private runtime directory below filesDir.",
                  "zh": "filesDir 下稳定的应用私有运行时目录。"
                },
                {
                  "en": "Using a cache directory that the OS may evict as the only copy of persistent runtime data.",
                  "zh": "把可能被系统清理的 cache 目录作为持久化运行时数据的唯一副本。"
                }
              ],
              [
                {
                  "en": "appSandboxRoot",
                  "zh": "appSandboxRoot"
                },
                {
                  "en": "The app-owned files root used to validate managed paths.",
                  "zh": "用于校验托管路径的应用文件根目录。"
                },
                {
                  "en": "Passing a guest path such as /home/lingxi.",
                  "zh": "传入 /home/lingxi 等 guest 路径。"
                }
              ],
              [
                {
                  "en": "abi",
                  "zh": "abi"
                },
                {
                  "en": "arm64-v8a or x86_64, matching the packaged native helpers.",
                  "zh": "arm64-v8a 或 x86_64，与打包的原生辅助程序一致。"
                },
                {
                  "en": "Copying the installer manifest ABI arm64 into this field without mapping it.",
                  "zh": "未经映射便把 installer manifest 的 arm64 填入此字段。"
                }
              ],
              [
                {
                  "en": "rootfsVersion / archiveSha256",
                  "zh": "rootfsVersion／archiveSha256"
                },
                {
                  "en": "The exact image version and expected archive digest used for staging.",
                  "zh": "暂存时使用的精确镜像版本与预期压缩包摘要。"
                },
                {
                  "en": "Treating a version label as proof that archive contents match.",
                  "zh": "把版本标签当作压缩包内容一致的证明。"
                }
              ],
              [
                {
                  "en": "nativeLibraryDir",
                  "zh": "nativeLibraryDir"
                },
                {
                  "en": "Context.applicationInfo.nativeLibraryDir.",
                  "zh": "Context.applicationInfo.nativeLibraryDir。"
                },
                {
                  "en": "Omitting extracted native helper libraries or packaging a different ABI.",
                  "zh": "遗漏提取后的原生辅助库，或打包了不同 ABI。"
                }
              ]
            ]
          }
        ],
        "paragraphs": [
          {
            "en": "The cross-platform FFI record also contains iOS resource and mount-policy fields. Android create_runtime does not consume the iOS authorization_file or iOS allowlist fields. Configure Android mounts and request policy through the Android runtime path rather than relying on those fields.",
            "zh": "跨平台 FFI 记录也包含 iOS 资源和挂载策略字段。Android create_runtime 不消费 iOS authorization_file 或 iOS 白名单字段。应通过 Android 运行时路径配置挂载和请求策略，不要依赖这些字段。"
          }
        ]
      },
      {
        "id": "android-handle-lifetime",
        "title": {
          "en": "Keep execution alive across Activity recreation",
          "zh": "跨 Activity 重建管理执行归属"
        },
        "paragraphs": [
          {
            "en": "Keep MobileLinuxRuntime in an application-owned component with a lifetime appropriate to the task. An Activity can be recreated while its process remains alive; creating another screen must not discard the only process/PTY/raw handle needed for cleanup. A host coroutine scope and any foreground-service policy remain application choices.",
            "zh": "将 MobileLinuxRuntime 保存在生命周期与任务匹配的应用级组件中。进程存活期间 Activity 也可能重建；新建页面时，不能丢失用于清理的唯一进程／PTY／raw 句柄。宿主 coroutine scope 和 foreground-service 策略仍由应用决定。"
          }
        ],
        "steps": [
          {
            "en": "Retain runtime plus the returned session or process handle, and route output to a bounded state holder.",
            "zh": "保留 runtime 与返回的会话或进程句柄，并将输出路由到有界状态容器。"
          },
          {
            "en": "After screen recreation, use listTasks or taskStatus to reconcile state; continue readEvents with the backend cursor if you are monitoring runtime events.",
            "zh": "页面重建后使用 listTasks 或 taskStatus 对齐状态；监控运行时事件时，沿用后端游标调用 readEvents。"
          },
          {
            "en": "On a deliberate stop, await killProcess, closePty or closeRawStdio for the selected handle, then release the UI subscription.",
            "zh": "主动停止时，等待对应句柄的 killProcess、closePty 或 closeRawStdio，再释放 UI 订阅。"
          },
          {
            "en": "When the application no longer needs any runtime work, await shutdown. Android process death is a separate OS event and does not preserve in-memory SDK handles.",
            "zh": "应用不再需要任何运行时任务时，等待 shutdown。Android 进程死亡是独立的操作系统事件，不会保留内存中的 SDK 句柄。"
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
            "en": "This helper adapts the Android device smoke request shape to the unprivileged guest home. It creates a disabled-network shell request by default and clears the timeout for raw stdio. Install and boot the runtime before calling execute or handle.openRawStdio.",
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
            "code": "private fun command(script: String, network: NetworkPolicyFfi = NetworkPolicyFfi.DISABLED,\n                        python: Boolean = false, raw: Boolean = false) = MobileLinuxCommandRequestFfi(\n        command = if (python) \"/usr/bin/python3\" else \"/bin/sh\",\n        args = if (python) listOf(\"-u\", \"-c\", script) else listOf(\"-c\", script),\n        cwd = \"/home/lingxi\", env = emptyList(), stdin = null,\n        timeoutMs = if (raw) null else 15_000uL, network = network,\n        resourceLimits = ResourceLimitsFfi(null, null, null, null), mounts = emptyList(),\n    )",
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
            "code": "import io.lingxi.mobilelinux.MobileLinuxRuntime\nimport io.lingxi.mobilelinux.bindings.*\n\nsuspend fun executeOnce(config: RuntimeConfig): MobileLinuxCommandResultFfi {\n    val runtime = MobileLinuxRuntime.create(config)\n    runtime.boot()\n    try {\n        val result = runtime.execute(MobileLinuxCommandRequestFfi(\n            command = \"/bin/sh\", args = listOf(\"-c\", \"printf 'ready\\n'\"),\n            cwd = \"/home/lingxi\", env = emptyList(), stdin = null,\n            timeoutMs = 10_000uL, network = NetworkPolicyFfi.DISABLED,\n            resourceLimits = ResourceLimitsFfi(null, null, null, null),\n            mounts = emptyList(),\n        ))\n        check(result.enforcement.networkPolicyEnforced) { \"Network isolation not proven\" }\n        check(result.exitCode == 0 && !result.timedOut && !result.cancelled)\n        return result\n    } finally {\n        runtime.shutdown()\n    }\n}",
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
      },
      {
        "id": "android-troubleshooting",
        "title": {
          "en": "Diagnose an Android integration failure",
          "zh": "排查 Android 接入失败"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Symptom",
                "zh": "现象"
              },
              {
                "en": "Inspect first",
                "zh": "先检查"
              },
              {
                "en": "Next action",
                "zh": "处理方式"
              }
            ],
            "rows": [
              [
                {
                  "en": "Runtime unavailable or PRoot cannot start",
                  "zh": "运行时不可用或 PRoot 无法启动"
                },
                {
                  "en": "capability.reason, runtime ABI, nativeLibraryDir and packaged helper files.",
                  "zh": "capability.reason、运行时 ABI、nativeLibraryDir 与已打包的辅助文件。"
                },
                {
                  "en": "Correct the package/ABI before changing command or model configuration.",
                  "zh": "先修正打包／ABI，再排查命令或模型配置。"
                }
              ],
              [
                {
                  "en": "Missing or corrupt rootfs",
                  "zh": "rootfs 缺失或损坏"
                },
                {
                  "en": "status.state, status.last_error and the staged image version/digest.",
                  "zh": "status.state、status.last_error 以及暂存镜像版本／摘要。"
                },
                {
                  "en": "Stage the expected verified image, then repairRootfs and boot; preserve the external workspace.",
                  "zh": "暂存预期的已校验镜像，再 repairRootfs 与 boot；保留外部工作区。"
                }
              ],
              [
                {
                  "en": "Command returns but the feature fails",
                  "zh": "命令已返回但功能失败"
                },
                {
                  "en": "exitCode, stderr, timedOut and cancelled.",
                  "zh": "exitCode、stderr、timedOut 与 cancelled。"
                },
                {
                  "en": "Show the command result separately from an FFI exception; do not equate a returned record with exit success.",
                  "zh": "分别展示命令结果与 FFI 异常；返回记录不等于退出成功。"
                }
              ],
              [
                {
                  "en": "Terminal text or protocol frames are damaged",
                  "zh": "终端文字或协议帧损坏"
                },
                {
                  "en": "PTY versus raw stdio selection and decoder buffering across chunks.",
                  "zh": "PTY／raw stdio 的选择，以及跨 chunk 的解码缓冲。"
                },
                {
                  "en": "Use a terminal decoder for PTY and a byte-framed parser for raw stdout.",
                  "zh": "PTY 使用终端解码器；raw stdout 使用按字节分帧的解析器。"
                }
              ],
              [
                {
                  "en": "Cancellation leaves work running",
                  "zh": "取消后仍有任务运行"
                },
                {
                  "en": "Whether the owner awaited the handle-specific close/kill and checked its error.",
                  "zh": "所有者是否等待了对应句柄的 close／kill，并检查其错误。"
                },
                {
                  "en": "Exercise whole-process-tree cancellation on the exact packaged device build.",
                  "zh": "在实际打包的设备版本上验证整个进程树的取消。"
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
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-android/src/runtime/process.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "android/installer/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxGuestPaths.kt"
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
            "en": "The checked-in repository-root Package.swift points to v0.1.0-rc.3 binary assets built from source 6b033d41a2b6b736fda1e8f996f55a4b20bd885d and contains their actual checksums. Asset availability and release device acceptance must be checked against that release. Do not replace checksums with placeholder values.",
            "zh": "仓库根目录中已提交的 Package.swift 指向 v0.1.0-rc.3 二进制资源，声明源码版本为 6b033d41a2b6b736fda1e8f996f55a4b20bd885d，并包含实际 checksums。资源是否可用及发布版本的真机验收需核对该次发布。不要用占位值替换 checksums。"
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
        "id": "ios-config-fields",
        "title": {
          "en": "Choose stable Swift configuration values",
          "zh": "选择稳定的 Swift 配置值"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Input",
                "zh": "输入"
              },
              {
                "en": "Rule",
                "zh": "规则"
              },
              {
                "en": "Why it matters",
                "zh": "作用"
              }
            ],
            "rows": [
              [
                {
                  "en": "managedRoot / appSandboxRoot",
                  "zh": "managedRoot／appSandboxRoot"
                },
                {
                  "en": "App-private absolute host paths, normalized before the session is created.",
                  "zh": "应用私有的宿主绝对路径，创建会话前进行规范化。"
                },
                {
                  "en": "Separate replaceable image state from persistent guest user storage.",
                  "zh": "分离可替换镜像状态与持久 guest 用户存储。"
                }
              ],
              [
                {
                  "en": "workspaceHostPath / stableWorkspaceId",
                  "zh": "workspaceHostPath／stableWorkspaceId"
                },
                {
                  "en": "A configured host directory and a non-empty ID with no /. The values . and .. are rejected.",
                  "zh": "已配置的宿主目录与非空稳定 ID；ID 不能含 /，也不能为 . 或 ..。"
                },
                {
                  "en": "The guest project path is /home/lingxi/lingxi/<stable-id>; changing a screen title should not change project identity.",
                  "zh": "guest 项目路径为 /home/lingxi/lingxi/<stable-id>；修改页面标题不应改变项目身份。"
                }
              ],
              [
                {
                  "en": "rootfsArchivePath / archiveSha256",
                  "zh": "rootfsArchivePath／archiveSha256"
                },
                {
                  "en": "The explicit converted fakefs ZIP path and its digest.",
                  "zh": "转换后的 fakefs ZIP 的显式路径及其摘要。"
                },
                {
                  "en": "The original tarball digest does not authenticate a converted ZIP.",
                  "zh": "源 tarball 的摘要不能校验转换后的 ZIP。"
                }
              ],
              [
                {
                  "en": "rootfsPatchPath / defaultMountPath",
                  "zh": "rootfsPatchPath／defaultMountPath"
                },
                {
                  "en": "Explicit resource locations matching the framework build.",
                  "zh": "与 framework 构建匹配的显式资源位置。"
                },
                {
                  "en": "The native bridge does not infer the application bundle or search environment variables for them.",
                  "zh": "原生桥接不会推断应用 bundle 或通过环境变量查找它们。"
                }
              ],
              [
                {
                  "en": "protectedHostRoots / allowedMountRoots / allowedGuestRoots",
                  "zh": "protectedHostRoots／allowedMountRoots／allowedGuestRoots"
                },
                {
                  "en": "Protected host trees plus explicit host and guest prefixes for additional mounts.",
                  "zh": "受保护宿主目录，以及额外挂载允许使用的 host／guest 前缀。"
                },
                {
                  "en": "An empty extra allowlist admits no extra non-workspace mounts.",
                  "zh": "额外白名单为空时，不允许额外的非工作区挂载。"
                }
              ]
            ]
          }
        ]
      },
      {
        "id": "ios-session-identity",
        "title": {
          "en": "Understand kernel identity and workspace sessions",
          "zh": "区分内核身份与工作区会话"
        },
        "paragraphs": [
          {
            "en": "The process-wide iSH registry compares immutable configuration separately from workspaceHostPath and stableWorkspaceId. A second session can therefore target another configured workspace while keeping the same kernel/rootfs identity. A different immutable configuration is rejected at construction with InvalidRequest; it is not a request to silently replace an existing kernel.",
            "zh": "进程级 iSH registry 将不可变配置与 workspaceHostPath、stableWorkspaceId 分开比较。因此，第二个会话可以使用另一个已配置工作区，同时保持相同内核／rootfs 身份。构建时发现不同不可变配置会返回 InvalidRequest，不会静默替换已存在内核。"
          },
          {
            "en": "After the kernel starts, repair/reset cannot unload it and return RestartRequired. shutdown closes the logical session and its transient work; it does not create a fresh process. Persist the user’s requested recovery action and let the host restart deliberately rather than repeatedly constructing a new wrapper.",
            "zh": "内核启动后，repair／reset 无法卸载它，会返回 RestartRequired。shutdown 关闭逻辑会话及其临时任务，并不会创建新进程。保存用户请求的恢复动作，由宿主主动重启，不要反复创建新的包装对象。"
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
        "id": "ios-terminal-and-protocols",
        "title": {
          "en": "Handle terminals and child protocols in Swift",
          "zh": "在 Swift 中处理终端与子进程协议"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Surface",
                "zh": "接口"
              },
              {
                "en": "Input / output",
                "zh": "输入／输出"
              },
              {
                "en": "Lifecycle detail",
                "zh": "生命周期细节"
              }
            ],
            "rows": [
              [
                {
                  "en": "PTY",
                  "zh": "PTY"
                },
                {
                  "en": "Generated bindings use Data; output contains terminal control sequences as well as text.",
                  "zh": "生成绑定使用 Data；输出既包含文字，也包含终端控制序列。"
                },
                {
                  "en": "The iSH runtime supports one PTY at a time. Close the existing session before opening another.",
                  "zh": "iSH 运行时同时支持一个 PTY；新建前关闭现有会话。"
                }
              ],
              [
                {
                  "en": "resizePty",
                  "zh": "resizePty"
                },
                {
                  "en": "Terminal dimensions are rows and columns in cells, not pixel sizes.",
                  "zh": "终端尺寸是字符单元的行列数，不是像素尺寸。"
                },
                {
                  "en": "Use nonzero dimensions and discard the old handle after close.",
                  "zh": "使用非零尺寸；关闭后丢弃旧句柄。"
                }
              ],
              [
                {
                  "en": "Raw stdio",
                  "zh": "Raw stdio"
                },
                {
                  "en": "Separate stdout and stderr Data buffers; bounded read maxBytes is 1...1048576.",
                  "zh": "stdout 与 stderr 使用分开的 Data 缓冲；有界读取 maxBytes 为 1...1048576。"
                },
                {
                  "en": "No PTY byte translation; accumulate protocol frames across reads and await closeRawStdio.",
                  "zh": "不会执行 PTY 字节转换；跨读取累积协议帧，并等待 closeRawStdio。"
                }
              ],
              [
                {
                  "en": "Background process",
                  "zh": "后台进程"
                },
                {
                  "en": "spawnBackground returns a process handle, not an OS background-execution entitlement.",
                  "zh": "spawnBackground 返回进程句柄，不代表获得操作系统后台执行权限。"
                },
                {
                  "en": "The app owns scene/OS lifetime and explicitly awaits killProcess when stopping the task.",
                  "zh": "应用负责 scene／操作系统生命周期，停止任务时显式等待 killProcess。"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "Keep terminal rendering and UI state on the UI executor, and keep execution ownership outside a transient view. A Swift Task cancellation is not a substitute for checking the SDK close/kill result.",
          "zh": "终端渲染和 UI 状态在 UI 执行器上处理，执行所有权应位于临时视图之外。取消 Swift Task 不能替代检查 SDK close／kill 的返回结果。"
        }
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
            "en": "An app process has one immutable iSH kernel/rootfs configuration, while sessions may use different workspace paths and stable IDs. Constructing a conflicting immutable configuration returns InvalidRequest. Repairing or resetting after the kernel has started returns RestartRequired; the host owns the application restart experience.",
            "zh": "应用进程只能使用一个不可变 iSH 内核／rootfs 配置，会话可以使用不同工作区路径与稳定 ID。构建冲突的不可变配置会返回 InvalidRequest；内核启动后修复或重置会返回 RestartRequired。应用重启体验由宿主负责。"
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
            "code": "import MobileLinuxRuntime\nimport MobileLinuxRuntimeBindings\n\nfunc executeOnce(config: RuntimeConfig) async throws -> MobileLinuxCommandResultFfi {\n    let runtime = try MobileLinuxRuntime(configuration: config)\n    _ = try await runtime.boot()\n    do {\n        let request = MobileLinuxCommandRequestFfi(\n            command: \"/bin/sh\", args: [\"-c\", \"printf 'ready\\n'\"], cwd: \"/home/lingxi\",\n            env: [], stdin: nil, timeoutMs: 10_000, network: .disabled,\n            resourceLimits: ResourceLimitsFfi(maxCpuSeconds: nil, maxMemoryMb: nil,\n                maxProcesses: nil, maxOpenFiles: nil), mounts: [])\n        let result = try await runtime.handle.runCommand(request: request)\n        try await runtime.shutdown()\n        return result\n    } catch {\n        try? await runtime.shutdown()\n        throw error\n    }\n}",
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
            "en": "Stable workspace identity is independent of the immutable process kernel identity. Switching workspaces must preserve the kernel/rootfs configuration. A conflicting constructor configuration is InvalidRequest; repair/reset after kernel initialization is RestartRequired. Persist the requested recovery state and surface a deliberate host restart action instead of looping boot attempts in the same process.",
            "zh": "稳定工作区身份与进程不可变内核身份相互独立。切换工作区须保留内核／rootfs 配置。构建冲突配置返回 InvalidRequest；内核初始化后的 repair／reset 返回 RestartRequired。保存所请求的恢复状态，提供明确的宿主重启动作，不要在同一进程内循环 boot。"
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
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/validation.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-api/src/mobile_linux.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "Package.swift"
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
            "en": "Workspace mounts must match workspaceHostPath and /home/lingxi/lingxi/<stableWorkspaceId>. Request mounts cannot replace persistent guest /home/lingxi or administrator /root.",
            "zh": "工作区挂载必须匹配 workspaceHostPath 和 /home/lingxi/lingxi/<stableWorkspaceId>。请求挂载不能替换持久 guest /home/lingxi 或管理员 /root。"
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
        "id": "mount-recipes",
        "title": {
          "en": "Mount a project without exposing runtime storage",
          "zh": "挂载项目而不暴露运行时存储"
        },
        "steps": [
          {
            "en": "Choose a stable guest coordinate with guest_paths::workspace(id), and supply the matching host workspace in runtime configuration.",
            "zh": "使用 guest_paths::workspace(id) 选择稳定 guest 坐标，并在运行时配置中提供对应宿主工作区。"
          },
          {
            "en": "Use MountPurpose::Workspace for the exact configured workspace pair. Additional external data needs its own mount purpose and both host and guest allowlist coverage on iOS.",
            "zh": "精确匹配已配置的工作区 host／guest 对时使用 MountPurpose::Workspace。额外外部数据需要独立挂载用途；iOS 还要求 host 与 guest 白名单同时覆盖。"
          },
          {
            "en": "Reject requests that replace /home/lingxi, /root or protected managed storage. The managed-home workspace exception applies only to the exact canonical host twin of that workspace.",
            "zh": "拒绝替换 /home/lingxi、/root 或受保护托管存储的请求。托管 HOME 中工作区的例外，仅适用于该工作区精确、规范化的宿主 backing 目录。"
          },
          {
            "en": "Use the current mount snapshot for presentation translation. map_guest_path_to_host and map_host_path_to_guest are lexical longest-prefix mappings; they do not themselves authorize file writes.",
            "zh": "展示时使用当前挂载快照进行路径转换。map_guest_path_to_host 与 map_host_path_to_guest 是词法最长前缀映射，本身不会授权文件写入。"
          }
        ],
        "note": {
          "en": "For host file access, resolve_guest_path additionally honors a matching mount’s read_only bit and rejects guest-owned paths without a safe host mapping. Do not fall back from an unmapped guest /home or /root path to an unrestricted native filesystem path.",
          "zh": "访问宿主文件时，resolve_guest_path 还会遵守匹配挂载的 read_only 位，并拒绝没有安全宿主映射的 guest 路径。不要把未映射的 guest /home 或 /root 路径降级为不受约束的原生文件系统路径。"
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
            "en": "Default guest writable roots are /home/lingxi, /root, /tmp, /var/tmp and /home/lingxi/lingxi. Keep managed runtime storage app-private and separate from externally mounted user workspaces.",
            "zh": "默认 guest 可写根目录为 /home/lingxi、/root、/tmp、/var/tmp 与 /home/lingxi/lingxi。托管运行时存储应保持应用私有，并与外部挂载的用户工作区分开。"
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
        "id": "recovery-decisions",
        "title": {
          "en": "Choose recovery from the typed error",
          "zh": "根据类型化错误选择恢复方式"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Error / state",
                "zh": "错误／状态"
              },
              {
                "en": "Interpretation",
                "zh": "含义"
              },
              {
                "en": "Recovery",
                "zh": "恢复方式"
              }
            ],
            "rows": [
              [
                {
                  "en": "InvalidRequest",
                  "zh": "InvalidRequest"
                },
                {
                  "en": "The concrete request or runtime identity is invalid.",
                  "zh": "具体请求或运行时身份无效。"
                },
                {
                  "en": "Fix the indicated argument, path, mount pair, raw read size or immutable iOS configuration; retrying unchanged inputs will not help.",
                  "zh": "修正提示的参数、路径、挂载对、raw 读取大小或 iOS 不可变配置；原样重试没有帮助。"
                }
              ],
              [
                {
                  "en": "Integrity / Corrupt",
                  "zh": "Integrity／Corrupt"
                },
                {
                  "en": "The selected image or immutable inventory failed verification.",
                  "zh": "选定镜像或不可变文件清单未通过校验。"
                },
                {
                  "en": "Restore the pinned verified resource, then repair the managed rootfs. Keep project data outside image replacement.",
                  "zh": "恢复固定的已校验资源，再修复托管 rootfs；替换镜像时保持项目数据独立。"
                }
              ],
              [
                {
                  "en": "Unavailable / Unsupported",
                  "zh": "Unavailable／Unsupported"
                },
                {
                  "en": "The current backend/device cannot provide execution or the selected surface.",
                  "zh": "当前后端／设备不能执行或不支持所选接口。"
                },
                {
                  "en": "Inspect capability.reason and choose a supported device/surface explicitly. A simulator does not become supported by retrying boot.",
                  "zh": "检查 capability.reason，并显式选择受支持设备／接口。反复 boot 不会让模拟器获得 guest 执行能力。"
                }
              ],
              [
                {
                  "en": "NetworkPolicyUnavailable / ResourceLimitExceeded",
                  "zh": "NetworkPolicyUnavailable／ResourceLimitExceeded"
                },
                {
                  "en": "A required policy cannot be guaranteed, or its resource budget was exceeded.",
                  "zh": "无法保证所需策略，或已超出资源预算。"
                },
                {
                  "en": "Report the named guarantee and keep the chosen admission policy. Ask the host to change the operation or budget deliberately.",
                  "zh": "报告具体保证，并保持既定准入策略；由宿主主动调整操作或预算。"
                }
              ],
              [
                {
                  "en": "RestartRequired",
                  "zh": "RestartRequired"
                },
                {
                  "en": "The initialized iSH kernel cannot perform the requested repair/reset in this process.",
                  "zh": "本进程中已初始化的 iSH 内核不能执行所请求的修复／重置。"
                },
                {
                  "en": "Persist the recovery intent and restart the host process before modifying rootfs.",
                  "zh": "保存恢复意图，在修改 rootfs 前重启宿主进程。"
                }
              ],
              [
                {
                  "en": "Timeout or timed_out result",
                  "zh": "Timeout 或 timed_out 结果"
                },
                {
                  "en": "A configured operation budget expired; the exact return form belongs to the selected API.",
                  "zh": "操作时间预算耗尽；具体返回形式取决于所选 API。"
                },
                {
                  "en": "Show timeout separately from a nonzero exit, await cleanup, and inspect the command budget before retrying.",
                  "zh": "分别展示超时与非零退出，等待清理，并在重试前检查命令预算。"
                }
              ]
            ]
          }
        ],
        "links": [
          {
            "label": {
              "en": "Android diagnosis",
              "zh": "Android 排查"
            },
            "href": "/docs/mobile-android#android-troubleshooting"
          },
          {
            "label": {
              "en": "iOS kernel and session identity",
              "zh": "iOS 内核与会话身份"
            },
            "href": "/docs/mobile-ios#ios-session-identity"
          }
        ]
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
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-core/src/guest_path.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-core/src/user_workspace.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/lifecycle.rs"
      },
      {
        "repo": "mobile-linux-runtime",
        "path": "crates/mobile-linux-ios/src/runtime/config.rs"
      }
    ]
  }
];
