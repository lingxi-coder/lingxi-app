# LingXi Code

LingXi 的产品仓库，包含命令行与 TUI、Electron 桌面端、iOS、Android 和 Web 客户端。
共享代理运行时位于独立的 [harness-runtime](https://github.com/lingxi-coder/harness-runtime)
仓库，通过根目录 `Cargo.toml` 和 `Cargo.lock` 固定到同一 Git 提交。
日常多仓开发目录为 `~/lingxi/`，本产品仓库名为 `lingxi-app`，放在 `~/lingxi/lingxi-app/`，
`app` 表示它负责产品宿主与多端应用装配；产品名称仍为 LingXi Code。
各 SDK 保持独立 checkout、工具链和锁文件。开发方式见
[多仓开发指南](docs/development/multi-repo-workflow.md)。

## 项目结构

| 目录 | 职责 |
|---|---|
| [`Cargo.toml`](Cargo.toml) | 根 Rust 产品 workspace，统一锁定 8 个成员与上游 SDK 来源 |
| [`apps/`](apps/README.md) | 按产品入口组织 Electron、iOS、Android、CLI/TUI、Bridge 和 Web |
| [`packages/`](packages/) | 产品共享包：TypeScript Bridge 客户端与 Rust 配置要求 |
| [`tools/ios-use/`](tools/ios-use/) | 产品使用的 iOS 控制工具 |
| [`resources/`](resources/) | 跨平台语音配置、模型目录与翻译源 |
| [`build-support/`](build-support/) | Rust 产品入口共用的构建辅助代码 |
| [`packaging/`](packaging/) | CLI 的 npm / PyPI 发行包装 |
| [`docs/`](docs/README.md) | 架构、平台说明、集成指南和历史审计记录 |
| [`assets/brand/`](assets/brand/) | 品牌设计源文件 |
| [`scripts/`](scripts/) | 仓库级工具 |
| [`.github/workflows/`](.github/workflows/) | CI、客户端验证及发行流程 |

```text
apps/
├── electron/           # 桌面界面、原生 helper 与签名打包
├── ios/
│   ├── native/         # SwiftUI、Xcode 工程与平台构建脚本
│   └── ffi/            # ios-framework Rust 包
├── android/
│   ├── native/         # Kotlin、Gradle 工程与平台构建脚本
│   └── ffi/            # android-aar Rust 包
├── cli/
│   ├── host/           # cli Rust 包
│   ├── tui/            # tui Rust 包
│   └── tui-core/       # tui-core Rust 包
├── bridge-server/      # 多客户端共享的 Rust Bridge 服务入口
└── web/                # Web 界面
```

同一产品入口的原生界面与 Rust FFI 按平台放在一起，仍各自通过 Cargo、Xcode 或
Gradle 增量构建。目录统一不改变包名、协议、FFI 标识或 SDK 的独立仓库边界；
共享包与资源留在 `packages/` 和 `resources/`。

运行时源码不在本仓库内维护。产品与运行时的边界、固定来源解析和资源归属见
[运行时迁移说明](docs/architecture/harness-runtime-extraction.md)。

## 构建 CLI

Rust 工具链由根目录 `rust-toolchain.toml` 固定。以下命令从仓库根目录执行：

```sh
cargo build --locked -p cli --release
cargo run --locked -p cli -- --help
```

发行安装入口为 `npm install -g lingxi` 或 `uv tool install lingxi`，由
[release workflow](.github/workflows/lingxi-release.yml) 发布。

## 桌面和移动端

```sh
# 初始化 Rust Bridge、共享 SDK 和客户端依赖；移动工具链可选
./apps/setup.sh

# macOS：签名预检、打包、静态/运行验证并启动
cd apps/electron
npm run package:mac:flare -- --check
npm run package:mac:flare -- --launch
```

macOS 签名要求见 [AGENTS.md](AGENTS.md)。凭据和真实会话验证使用签名应用；
`npm run dev` 仅适用于不依赖 Credential Broker 的界面开发。

客户端统一使用 npm 和各自的 `package-lock.json`。共享 SDK 必须先构建，
Electron 才能使用它的 `dist/` 导出。详见 [客户端设置](apps/README.md)、
[iOS](apps/ios/native/README.md) 和 [Android](apps/android/native/README.md)。

## 检查

```sh
# Rust 产品与固定上游边界
./scripts/check-all.sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-features --no-fail-fast
cargo clippy --locked --workspace --all-targets -- -D warnings
```

```sh
# 从仓库根目录执行
npm --prefix packages/bridge-client ci
npm --prefix packages/bridge-client run build
npm --prefix apps/electron ci
npm --prefix apps/electron run typecheck
npm --prefix apps/electron test
python3 resources/translations/generate.py --check
```

运行时与移动 Linux SDK 的固定源码路径分别通过
`python3 scripts/lib/runtime_source.py --root` 和
`python3 scripts/lib/mobile_linux_source.py --root` 解析。
相邻 SDK checkout 不会自动替换固定依赖；本地联调 overlay 的设计与当前限制见
[多仓开发指南](docs/development/multi-repo-workflow.md)。旧里程碑和历史审计记录不代表当前验证状态。

## 文档

- [文档导航](docs/README.md)
- [多仓开发与 SDK 独立性](docs/development/multi-repo-workflow.md)
- [产品架构](docs/ARCHITECTURE.md)
- [平台说明](docs/PLATFORMS.md)
- [模型配置](docs/LLM_PROVIDERS.md)
- [安全模型](docs/SECURITY.md)
- [版本记录](CHANGELOG.md)

## License

Rust workspace 声明为 MIT OR Apache-2.0；客户端及第三方组件以各自许可声明为准。
