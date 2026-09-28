# LingXi Code

LingXi 的产品仓库，包含命令行与 TUI、Electron 桌面端、iOS、Android 和 Web 客户端。
共享代理运行时位于独立的 [harness-runtime](https://github.com/lingxi-coder/harness-runtime)
仓库，通过 `lingxi-code/Cargo.toml` 和 `Cargo.lock` 固定到同一 Git 提交。

## 项目结构

| 目录 | 职责 |
|---|---|
| [`lingxi-code/`](lingxi-code/) | Rust 产品 workspace：CLI、Bridge、移动 FFI、TUI 和宿主平台适配 |
| [`clients/`](clients/README.md) | Electron、iOS、Android、Web、共享 TypeScript SDK、翻译和语音配置 |
| [`docs/`](docs/README.md) | 架构、平台说明、集成指南和历史审计记录 |
| [`assets/brand/`](assets/brand/) | 品牌设计源文件 |
| [`third_party/`](third_party/) | Android shell 构建使用的 mksh / toybox 源码 |
| [`scripts/`](scripts/) | 仓库级工具 |
| [`.github/workflows/`](.github/workflows/) | CI、客户端验证及发行流程 |

运行时源码不在本仓库内维护。产品与运行时的边界、固定来源解析和资源归属见
[运行时迁移说明](docs/architecture/harness-runtime-extraction.md)。

## 构建 CLI

Rust 工具链由 `lingxi-code/rust-toolchain.toml` 固定。以下命令从仓库根目录执行：

```sh
cd lingxi-code
cargo build --locked -p cli --release
cargo run --locked -p cli -- --help
```

发行安装入口为 `npm install -g lingxi` 或 `uv tool install lingxi`，由
[release workflow](.github/workflows/lingxi-release.yml) 发布。

## 桌面和移动端

```sh
# 初始化 Rust Bridge、共享 SDK 和客户端依赖；移动工具链可选
./clients/setup.sh

# macOS：签名预检、打包、静态/运行验证并启动
cd clients/electron
npm run package:mac:flare -- --check
npm run package:mac:flare -- --launch
```

macOS 签名要求见 [AGENTS.md](AGENTS.md)。凭据和真实会话验证使用签名应用；
`npm run dev` 仅适用于不依赖 Credential Broker 的界面开发。

客户端统一使用 npm 和各自的 `package-lock.json`。共享 SDK 必须先构建，
Electron 才能使用它的 `dist/` 导出。详见 [客户端设置](clients/README.md)、
[iOS](clients/ios/README.md) 和 [Android](clients/android/README.md)。

## 检查

```sh
# Rust 产品与固定上游边界
cd lingxi-code
./scripts/check-all.sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-features --no-fail-fast
cargo clippy --locked --workspace --all-targets -- -D warnings
```

```sh
# 从仓库根目录执行
npm --prefix clients/shared ci
npm --prefix clients/shared run build
npm --prefix clients/electron ci
npm --prefix clients/electron run typecheck
npm --prefix clients/electron test
python3 clients/translations/generate.py --check
```

本地运行时源码路径通过 `python3 lingxi-code/scripts/runtime_source.py --root`
解析；不要使用相邻 checkout 替换固定依赖。旧里程碑和历史审计记录不代表当前验证状态。

## 文档

- [文档导航](docs/README.md)
- [产品架构](docs/ARCHITECTURE.md)
- [平台说明](docs/PLATFORMS.md)
- [模型配置](docs/LLM_PROVIDERS.md)
- [安全模型](docs/SECURITY.md)
- [版本记录](CHANGELOG.md)

## License

Rust workspace 声明为 MIT OR Apache-2.0；客户端及第三方组件以各自许可声明为准。
