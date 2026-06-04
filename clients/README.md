# LingXi clients

Native shells for LingXi Code — a desktop **Electron** app, an **Android** app,
and an **iOS** app — all driving the same Rust engine over the local
`bridge-server` (Electron) or an in-process FFI binding (mobile). They share the
`@lingxi/bridge-client` TypeScript SDK in [`shared/`](shared/).

| Dir | What it is |
|---|---|
| [`shared/`](shared/) | `@lingxi/bridge-client` — wire-protocol types + the Node-side `BridgeClient`. **Must be built before electron** (electron depends on it via `file:../shared`). |
| [`electron/`](electron/) | Desktop shell (Electron + Vite + React + TS). Spawns the Rust `bridge-server` and streams events to the renderer. |
| [`android/`](android/) | Android app (Gradle + Kotlin). Links the engine via UniFFI JNI bindings. |
| [`ios/`](ios/) | iOS app (XcodeGen + Swift). Links the engine via a UniFFI `.xcframework`. |

---

## One-command setup (fresh clone)

```sh
./clients/setup.sh
```

That's the whole bootstrap. From a fresh clone it:

1. **Engine** — `cargo build -p bridge-server` → `lingxi-code/target/debug/bridge-server`
   (the path electron's repo-relative `resolveServerBin()` discovers automatically).
2. **Shared SDK** — `(cd clients/shared && npm install && npm run build)`.
   The build is **required before electron**: electron resolves
   `@lingxi/bridge-client` through `file:../shared`, which serves the compiled
   `dist/` — without it the electron install / typecheck cannot resolve the dep.
3. **Electron** — `(cd clients/electron && npm install)`.
4. **Android** *(optional)* — if `cargo-ndk` **and** an Android NDK are present,
   runs [`android/scripts/build-jni.sh`](android/scripts/build-jni.sh) to
   cross-compile the JNI libs + generate the Kotlin bindings. Otherwise it
   **skips** with guidance (install `cargo-ndk` + the NDK, then re-run).
5. **iOS** *(optional)* — on macOS with `xcodebuild` **and** `xcodegen`, runs
   [`ios/scripts/build-xcframework.sh`](ios/scripts/build-xcframework.sh) and
   `xcodegen generate`. Otherwise it **skips** with guidance.

The script is **idempotent** — re-run it any time (e.g. after installing a
missing mobile toolchain). A missing *optional* mobile toolchain only skips that
one step; it never fails the whole run. The required core is `cargo` + `node`/`npm`.

> **API key.** The LLM key (`ANTHROPIC_API_KEY`) is read at runtime from the
> environment or entered in-app (Settings). It is never committed, logged, or
> baked into any artifact by this script.

### After setup

```sh
cd clients/electron && npm run dev   # desktop dev server
```

- **Android** — open `clients/android` in Android Studio, build & run.
- **iOS** — open `clients/ios/LingxiCode.xcodeproj` in Xcode, build & run.

Then in the app: open **Settings**, enter your Anthropic API key, and pick a
model.

---

## Manual fallback (per step)

If you prefer to run the steps by hand (or `setup.sh` is unavailable), the
equivalents are:

```sh
# 1. Engine binary (electron discovers it under lingxi-code/target/debug/)
cd lingxi-code && cargo build -p bridge-server --bin bridge-server

# 2. Shared SDK — install AND build (build is REQUIRED before electron)
cd clients/shared && npm install && npm run build

# 3. Electron deps (consumes the freshly built clients/shared/dist via file:../shared)
cd clients/electron && npm install

# 4. Android bindings (needs cargo-ndk + an Android NDK)
cd clients/android && bash scripts/build-jni.sh

# 5. iOS framework + project (macOS + Xcode; needs xcodegen)
cd clients/ios && bash scripts/build-xcframework.sh && xcodegen generate
```

> **shared must build before electron.** Step 3's `npm install` resolves
> `@lingxi/bridge-client` from `file:../shared`, which exposes the compiled
> `dist/`. If you skip step 2's `npm run build`, electron's install/typecheck
> cannot find the package's types/entry. Always run step 2 fully before step 3.

---

## More detail

- **Bridge run path (Electron ↔ engine, keyed vs keyless):** [`README-bridge.md`](README-bridge.md)
- **Electron shell:** [`electron/README.md`](electron/README.md)
- **Android app:** [`android/README.md`](android/README.md)
- **iOS app:** [`ios/README.md`](ios/README.md) and [`ios/README-engine.md`](ios/README-engine.md)
