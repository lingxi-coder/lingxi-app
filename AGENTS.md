# LingXi repository instructions

## Development checkout

- Use `~/lingxi/lingxi-app` as the canonical product checkout and start repository commands from this real directory.
- `/Users/luolingfeng/Projects/LingXi-Next` is a compatibility symlink to the same checkout; changes and Git configuration are already shared between these paths.
- The product `origin` is `git@github.com-lingxi-coder:lingxi-coder/lingxi-app.git`. Keep the repository-owner SSH alias for fetch and push.

## macOS Electron packaging

- Always package the signed macOS Desktop app through `npm run package:mac:flare` from `apps/electron`, or invoke `apps/electron/scripts/package-macos-flare.sh` directly. Do not bypass this wrapper with `npm run package:mac` during normal development or release preparation.
- Use the same Apple Developer team as iOS: Flare App, Inc., Team ID `AZ4AX7J833`. The wrapper rejects a different Team ID and selects the currently valid `Apple Development: lingfeng luo (KQ7KX8LCYL)` identity by certificate hash.
- Development packaging uses the isolated `development` credential channel and requires Mac App Development profiles authorizing `com.lingxi.code.development` and `com.lingxi.code.credential-broker.development`. An iOS Xcode Managed Profile is not a substitute for a macOS profile.
- When either profile is missing, the wrapper must use the checked-in provisioning bootstrap target with Xcode Automatic Signing and `-allowProvisioningUpdates -allowProvisioningDeviceRegistration`. This reuses the Flare account already configured in Xcode; do not automate the Apple Developer website with a browser.
- Explicit profile paths may be supplied with `LINGXI_MAC_PROVISIONING_PROFILE` and `LINGXI_MAC_BROKER_PROVISIONING_PROFILE`. Use `--no-register` only for CI or a deliberately read-only preflight. Never commit Apple IDs, app-specific passwords, private keys, App Store Connect API keys, or local profile files.
- Run `npm run package:mac:flare -- --check` to create/download missing profiles and validate signing assets without building the real app. Add `-- --launch` to launch the packaged app after all verification passes.
- Do not use `npm run dev` to test macOS credential persistence. The Credential Broker intentionally rejects unpackaged Electron processes.
- The wrapper must retain the release Rust path remapping, inside-out signing performed by `package-mac.mjs`, and `verify:package` static/runtime verification. A successful compilation without those checks is not a completed macOS package.

## Mobile Local App presentation and hosting (user decisions, updated 2026-10-28)

- Mobile opens application pages in the operating system's resolved default browser by default. Users may choose another supported opening method, including Android's browser chooser and iOS's optional system in-app browser; never hard-code an external browser brand, package, or application ID. Opening method is independent of execution host. Preserve the originating browser's Agent pending state during native handoff; disclose unsupported handoff modes instead of silently changing browsers. Desktop keeps its existing application windows. See [the Mobile Local App Agent and lifetime contract](docs/MOBILE_LOCAL_APP_AGENT.md).
- Keep Mobile hosting available, including for long tasks. Recommend Desktop for sustained execution without forcing migration or creating a second data replica. Desktop-hosted apps run through the authenticated [remote access gateway](docs/LOCAL_APP_REMOTE_ACCESS.md); Mobile access does not install or execute their backend locally.
- LingXi owns runtime services, independent Agent tasks, model credentials and trusted native permission/tool approval. Preserve Agent support using an authenticated browser-to-host interface scoped to the installed app and current run; never expose host-administration or approval APIs to app pages.
- Use browser camera/file selection for the application UI. Verify external-browser navigation, photo return, host lifecycle, Agent requests, cancellation and recovery on physical Android and iOS. Historical WebView checks are not external-browser acceptance.
- The default external-browser flow remains the release acceptance baseline. Optional system-browser modes need their own lifecycle, photo/file and Agent handoff acceptance; a default-browser pass does not establish those results.

## Local App Agent protocol standardization

- Use an established protocol for public App-to-Agent messages and events; do not invent another cross-platform message or reply format. The current direction is AG-UI 1.0, recorded in [the shared protocol migration contract](docs/LOCAL_APP_AGENT_PROTOCOL.md).
- Keep provider communication in `llm-client`, connected-tool communication in MCP, and Skill packages aligned with Agent Skills. Host authorization, durable resources, source Project bindings and mobile background leases remain host responsibilities.
- Public Agent input, output and events use the pinned official AG-UI 1.0 Schema and SDK. Preserve historical prototype evidence separately; require official schema/behaviour checks and acceptance of freshly built affected platform packages before claiming completion.

- First-release AG-UI migration is a clean replacement (user decision, 2026-10-24): remove the old public Agent API, old protocol fixtures and obsolete Agent task/chat/pending-store formats. Do not add compatibility aliases, fallback decoders or data conversion. Delete only known obsolete Agent storage; preserve source Projects, immutable releases and application business data.
