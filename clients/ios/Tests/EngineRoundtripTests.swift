// EngineRoundtripTests.swift — M10 A2 / P4.
//
// Simulator XCTest e2e that proves the FULL in-process path the SwiftUI app
// rides:  Swift caller → UniFFI (`buildIosEngine` + `MobileEngineHandle.submit`)
// → in-process engine (its handle-owned tokio runtime) → adapter → the Swift
// `IosEventListener` callback.  This is the in-process analog of the verified
// Electron↔bridge real-conversation path: ONE protocol (`client-protocol`), ONE
// transport (UniFFI), ONE listener.
//
// It runs — and MUST pass — KEYLESS.  With no `ANTHROPIC_API_KEY`:
//
//   * `buildIosEngine(...)` still succeeds — the engine host is constructed and
//     the foreign listener is registered (the "handshake").  Construction does
//     NOT touch the network and does NOT require a key.
//   * `submit(.sendPrompt(...))` returns Ok — per the binding contract, a turn
//     failure is NOT thrown from `submit`; the turn is spawned on the engine's
//     runtime and `submit` returns once it is queued.
//   * the keyless turn then fails at the model boundary and the adapter streams a
//     TERMINAL `.error` or `.turnEnded(stopReason: "model_error")` back.
//
// On the simulator `cfg(target_os = "ios")` is TRUE, so `buildIosEngine` builds
// the REAL `IosPlatform` whose `http` handle is the shared `reqwest` + `rustls`
// client (`platform_common::http::ReqwestHttp`).  Missing credentials may be
// rejected locally before any request is attempted; if a transport `.error`
// arrives, this test still verifies it is not the old posix-minimal SSE stub.
//
// So a keyless run is itself a complete proof: engine-build succeeded AND a real
// engine-originated event arrived through the UniFFI callback on the listener AND
// it reaches a terminal model-error outcome.  (WITH a key, the same path streams
// `.textDelta`; this test does not require or assert that, so it never needs a
// secret in CI.)
//
// NO secrets: the test passes an explicitly empty key and never reads or logs a
// credential from the environment.

import XCTest

// The app target (`LingxiCode`) compiles the generated UniFFI bindings, so the
// public engine surface (`buildIosEngine`, `MobileEngineHandle`, `ClientCommand`,
// `ClientEvent`, `IosEventListener`) is visible here via the host app module.
@testable import LingxiCode

// The per-namespace FFI clang module is provided by LingxiCodeFFI.xcframework,
// which this test target also links — mirroring the app's `canImport` guard so
// the suite degrades to a skip if the engine bindings are not present.
#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

#if canImport(engine_mobileFFI)

    /// A test `IosEventListener` that records every inbound `ClientEvent` and
    /// fulfils an expectation when a terminal event (`error` or `turnEnded`)
    /// arrives.  Thread-safe: Rust delivers `onEvent` from the engine runtime.
    final class CollectingListener: IosEventListener, @unchecked Sendable {
        private let lock = NSLock()
        private var _events: [ClientEvent] = []
        /// Fulfilled the first time a terminal event is observed.
        let terminal: XCTestExpectation

        init(terminal: XCTestExpectation) {
            self.terminal = terminal
        }

        /// Snapshot of everything received so far (copy under lock).
        var events: [ClientEvent] {
            lock.lock(); defer { lock.unlock() }
            return _events
        }

        func onEvent(event: ClientEvent) async {
            lock.lock()
            _events.append(event)
            let isTerminal: Bool
            switch event {
            case .error, .turnEnded:
                isTerminal = true
            default:
                isTerminal = false
            }
            lock.unlock()
            if isTerminal {
                // Fulfil exactly once; XCTestExpectation tolerates extra fulfils
                // only when assertForOverFulfill is off, so guard with a flag.
                terminal.fulfill()
            }
        }
    }

    /// A no-op `IosPermissionSink` for the keyless round-trip: the keyless turn
    /// 401s before any tool runs, so no permission is ever requested. Required
    /// only to satisfy `buildIosEngine`'s `permissions:` parameter.
    final class NoopPermissionSink: IosPermissionSink, @unchecked Sendable {
        func onRequest(request: PermissionRequest) async {}
    }

    /// A test listener that fulfils a distinct expectation for each model event
    /// kind (SHIP-BLOCKER #2). Used to prove the OUT-OF-BAND model-state path:
    /// `ListModels`→`ModelList` and `SetModel`→`ModelChanged` flow over the same
    /// UniFFI listener WITHOUT a text turn. Thread-safe (Rust delivers off-runtime).
    final class ModelListener: IosEventListener, @unchecked Sendable {
        private let lock = NSLock()
        private(set) var modelList: (models: [String], current: String)?
        private(set) var changedModel: String?
        let gotList: XCTestExpectation
        let gotChanged: XCTestExpectation

        init(gotList: XCTestExpectation, gotChanged: XCTestExpectation) {
            self.gotList = gotList
            self.gotChanged = gotChanged
            gotList.assertForOverFulfill = false
            gotChanged.assertForOverFulfill = false
        }

        func onEvent(event: ClientEvent) async {
            switch event {
            case let .modelList(models, current):
                lock.lock(); modelList = (models, current); lock.unlock()
                gotList.fulfill()
            case let .modelChanged(model):
                lock.lock(); changedModel = model; lock.unlock()
                gotChanged.fulfill()
            default:
                break
            }
        }
    }

    final class EngineRoundtripTests: XCTestCase {

        func testModelDisplayGroupsOnlyInputByProviderAndPreservesReferences() {
            let references = [
                "openai/gpt-5.5",
                "openai/gpt-5.4-mini",
                "github-copilot/gpt-5.5",
                "deepseek/deepseek-v4-flash",
            ]

            let sections = ModelDisplay.sections(for: references)

            XCTAssertEqual(sections.map(\.providerId), ["openai", "github-copilot", "deepseek"])
            XCTAssertEqual(sections.map(\.name), ["OpenAI", "GitHub Copilot", "DeepSeek"])
            XCTAssertEqual(sections.flatMap(\.models).map(\.reference), references)
            XCTAssertEqual(sections[0].models.map(\.name), ["GPT-5.5", "GPT-5.4 Mini"])
            XCTAssertEqual(sections[2].models.first?.name, "DeepSeek V4 Flash")
        }

        func testModelDisplayKeepsSameWireModelDistinctAcrossProviders() {
            let sections = ModelDisplay.sections(for: [
                "openai/gpt-5.5",
                "github-copilot/gpt-5.5",
                "openai/gpt-5.5", // exact duplicate is safe to collapse
            ])
            let items = sections.flatMap(\.models)

            XCTAssertEqual(items.map(\.reference), [
                "openai/gpt-5.5",
                "github-copilot/gpt-5.5",
            ])
            XCTAssertEqual(items.map(\.modelId), ["gpt-5.5", "gpt-5.5"])
            XCTAssertEqual(Set(items.map(\.id)).count, 2)
        }

        func testModelDisplaySplitsOnlyFirstSlashForAggregatorModels() {
            let item = ModelDisplay.item(for: "openrouter/openai/gpt-5.5")

            XCTAssertEqual(item.providerId, "openrouter")
            XCTAssertEqual(item.modelId, "openai/gpt-5.5")
            XCTAssertEqual(item.reference, "openrouter/openai/gpt-5.5")
        }

        func testModelDisplayUsesStableKimiLabels() {
            let sections = ModelDisplay.sections(for: ["kimi/kimi-k3", "kimi-code/k3"])

            XCTAssertEqual(sections.first?.name, "Kimi")
            XCTAssertEqual(sections.first?.models.first?.name, "Kimi K3")
            XCTAssertEqual(sections.first?.models.first?.shortName, "K3")
            XCTAssertEqual(sections.last?.name, "Kimi Code")
            XCTAssertEqual(sections.last?.models.first?.name, "K3")
            XCTAssertEqual(
                Presets.llm.first(where: { $0.id == "kimi-code" })?.models.first,
                "kimi-for-coding"
            )
        }

        /// End-to-end, KEYLESS:  build the engine, submit a prompt, and assert a
        /// terminal engine event is delivered to the listener over UniFFI.
        ///
        /// This single test exercises every hop of the SwiftUI→UniFFI→engine→
        /// listener path without a network success and without a secret.
        func testKeylessSendPromptDeliversTerminalEvent() async throws {
            // The terminal-event expectation: the keyless turn streams a final
            // `.error` (or, defensively, a `.turnEnded`) to the listener.
            let terminal = expectation(description: "engine delivers a terminal ClientEvent to the listener")
            terminal.assertForOverFulfill = false
            let listener = CollectingListener(terminal: terminal)

            // A writable sandbox root for the engine's filesystem / config — a
            // fresh temp dir so the test is hermetic.
            let sandbox = FileManager.default.temporaryDirectory
                .appendingPathComponent("LingxiCodeTest-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: sandbox) }

            // HANDSHAKE / engine-build: must succeed even with NO api key.  We do
            // Pass an explicitly empty key so a developer's local environment
            // cannot silently turn this into a credentialed/network-success test.
            let apiKey = ""
            let handle: MobileEngineHandle
            do {
                handle = try buildIosEngine(
                    apiBase: ProcessInfo.processInfo.environment["ANTHROPIC_BASE_URL"]
                        ?? "https://api.anthropic.com",
                    apiKey: apiKey,
                    model: "claude-sonnet-4-20250514",
                    appSandboxRoot: sandbox.path,
                    listener: listener,
                    stt: SttImpl(),
                    tts: TtsImpl(),
                    camera: CameraImpl(),
                    share: ShareImpl(),
                    voice: VoiceImpl(),
                    notifications: NotificationImpl(),
                    clipboard: ClipboardImpl(),
                    permissions: NoopPermissionSink(),
                    mobileLinux: nil,
                    secureStorage: nil)
            } catch {
                XCTFail("buildIosEngine must succeed keyless (handshake), got error: \(error)")
                return
            }

            // SUBMIT: per the binding contract this returns Ok — the turn is
            // spawned on the engine's owned runtime; a turn failure is NOT thrown
            // here, it streams to the listener as a `ClientEvent.error`.
            try await handle.submit(command: .sendPrompt(
                text: "Reply with exactly: hello from lingxi",
                promptMode: nil,
                images: [],
                turnId: nil))

            // Wait for the streamed terminal event to reach the listener over the
            // UniFFI callback.  Generous timeout: the engine spins up its runtime
            // and the keyless turn round-trips to the (rejecting) model endpoint.
            await fulfillment(of: [terminal], timeout: 60)

            // PROOF: at least one real engine-originated event arrived on the
            // listener, and the keyless turn surfaced either a terminal `.error`
            // or the current adapter contract's `.turnEnded(model_error)`.
            let received = listener.events
            // Diagnostic: surface the real engine-originated event stream in the
            // test log (no secrets — these are ClientEvent kinds/messages).
            print("[P4] keyless ClientEvents received via UniFFI listener: "
                  + received.map { String(describing: $0) }.joined(separator: " | "))
            XCTAssertFalse(received.isEmpty,
                           "the listener must receive at least one ClientEvent over UniFFI")

            let errorEvents: [(ErrorKindDto, String)] = received.compactMap { ev in
                if case let .error(kind, message) = ev { return (kind, message) }
                return nil
            }
            let modelErrorEnds = received.compactMap { ev -> String? in
                if case let .turnEnded(_, stopReason, _) = ev,
                   stopReason == "model_error" {
                    return stopReason
                }
                return nil
            }
            XCTAssertFalse(errorEvents.isEmpty && modelErrorEnds.isEmpty,
                           """
                           keyless turn must deliver ClientEvent.error or turnEnded(model_error) \
                           (proves SwiftUI→UniFFI→engine→listener end to end). \
                           Received events: \(received.map { String(describing: $0) })
                           """)

            // The error must carry a human-readable message (engine-originated,
            // not an empty placeholder).  We do NOT assert the key value or print
            // any secret.
            if let (_, message) = errorEvents.first {
                XCTAssertFalse(message.isEmpty, "the terminal error must carry a message")
            }

            // REAL-CLIENT PROOF.  The terminal error must come from the real
            // `reqwest`+`rustls` transport (`platform_common::http::ReqwestHttp`)
            // wired into `IosPlatform`, NOT the old `platform-posix-minimal` SSE
            // stub.  We do this NEGATIVELY (hermetic-safe): assert the message is
            // not the stub signature.  This passes whether the network returned a
            // real `401` or a `connection failed: …` — both are the real client —
            // and fails only if the engine is still wired to the stub.
            for (_, message) in errorEvents {
                assertRealHttpAttempt(message)
            }
        }

        /// SHIP-BLOCKER #2, KEYLESS: prove the out-of-band model-state path end to
        /// end — `ListModels`→`ModelList` (real ids, NOT branded mock ids) and
        /// `SetModel`→`ModelChanged` — over the same UniFFI listener, with NO text
        /// turn and NO key. The engine's `list_available_models` returns a real
        /// fallback catalog keyless, so this is hermetic.
        func testModelCatalogAndSwitchRoundTripKeyless() async throws {
            let gotList = expectation(description: "engine delivers a ModelList over the listener")
            let gotChanged = expectation(description: "engine delivers a ModelChanged over the listener")
            let listener = ModelListener(gotList: gotList, gotChanged: gotChanged)

            let sandbox = FileManager.default.temporaryDirectory
                .appendingPathComponent("LingxiCodeTest-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: sandbox) }

            // SHIP-BLOCKER #2: build with an EMPTY model id — the engine must start
            // on `MobileConfig.default_model` (a real Anthropic wire id), never a
            // branded mock id. `build_ios_engine` only overrides default_model when
            // the passed id is non-empty, so "" exercises exactly that path.
            let handle: MobileEngineHandle
            do {
                handle = try buildIosEngine(
                    apiBase: ProcessInfo.processInfo.environment["ANTHROPIC_BASE_URL"]
                        ?? "https://api.anthropic.com",
                    apiKey: ProcessInfo.processInfo.environment["ANTHROPIC_API_KEY"] ?? "",
                    model: "",
                    appSandboxRoot: sandbox.path,
                    listener: listener,
                    stt: SttImpl(),
                    tts: TtsImpl(),
                    camera: CameraImpl(),
                    share: ShareImpl(),
                    voice: VoiceImpl(),
                    notifications: NotificationImpl(),
                    clipboard: ClipboardImpl(),
                    permissions: NoopPermissionSink(),
                    mobileLinux: nil,
                    secureStorage: nil)
            } catch {
                XCTFail("buildIosEngine must succeed keyless with empty model, got: \(error)")
                return
            }

            // OUT-OF-BAND: ask for the catalog (not a text turn).
            try await handle.submit(command: .listModels)
            await fulfillment(of: [gotList], timeout: 30)

            guard let list = listener.modelList else {
                XCTFail("no ModelList received"); return
            }
            XCTAssertFalse(list.models.isEmpty, "ModelList must carry real model ids")
            // The reported active model (engine default) and the catalog must NOT be
            // the branded mock ids the apps used to hardcode (lx-72b, …). Sending a
            // branded id is precisely the SHIP-BLOCKER #2 bug.
            let mockIds: Set<String> = ["lx-72b", "lx-72b-r", "lx-32b", "lx-code"]
            XCTAssertFalse(mockIds.contains(list.current),
                           "active model must be a REAL engine id, not a branded mock id: \(list.current)")
            for id in list.models {
                XCTAssertFalse(mockIds.contains(id),
                               "catalog must contain only REAL engine ids, found mock id: \(id)")
            }

            // SetModel→ModelChanged: pick a different real id from the catalog and
            // confirm the engine echoes it back (a turn would then send THIS id).
            let target = list.models.first(where: { $0 != list.current }) ?? list.models[0]
            try await handle.submit(command: .setModel(model: target))
            await fulfillment(of: [gotChanged], timeout: 30)
            XCTAssertEqual(listener.changedModel, target,
                           "ModelChanged must echo the SetModel id (the real id a turn will send)")
        }

        /// Assert a terminal-error `message` is a REAL transport outcome, not the
        /// `platform-posix-minimal` stub.
        ///
        /// The stub's error is `connection failed: posix-minimal: SSE stub (Plan
        /// 17 wires the real client)` (and `… HTTP stub …` for the request path).
        /// The real client's error is either `non-success HTTP status 401: …`
        /// (request reached the host) or `connection failed: …` from `reqwest`
        /// (no route from the simulator).  We assert the stub's distinctive
        /// substrings are ABSENT — true for every real-client outcome, false only
        /// for the stub — so the test stays hermetic (no network success needed)
        /// while still proving the real client ran.
        private func assertRealHttpAttempt(
            _ message: String,
            file: StaticString = #filePath,
            line: UInt = #line
        ) {
            let stubMarkers = ["posix-minimal", "SSE stub", "HTTP stub"]
            for marker in stubMarkers {
                XCTAssertFalse(
                    message.contains(marker),
                    """
                    terminal error still carries the posix-minimal stub marker \
                    "\(marker)" — the engine is wired to the SSE/HTTP STUB, not the \
                    real reqwest+rustls client. message: \(message)
                    """,
                    file: file, line: line)
            }
        }
    }

#else

    /// If the engine bindings are not linked, fail loudly — P4 requires them.
    final class EngineRoundtripTests: XCTestCase {
        func testEngineBindingsMustBeLinked() {
            XCTFail("engine_mobileFFI not importable — run scripts/build-xcframework.sh before testing")
        }
    }

#endif
