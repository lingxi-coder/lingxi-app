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
//   * the keyless turn then fails at the model call and the adapter streams a
//     TERMINAL `ClientEvent.error(kind:message:)` back to the listener.
//
// On the simulator `cfg(target_os = "ios")` is TRUE, so `buildIosEngine` builds
// the REAL `IosPlatform` whose `http` handle is the shared `reqwest` + `rustls`
// client (`platform_common::http::ReqwestHttp`).  The keyless turn therefore
// makes a REAL HTTPS request to the Anthropic-compatible endpoint and the
// terminal error is a real transport outcome — a `401` (`non-success HTTP status
// 401: …`) when the request reaches the host, or a `connection failed: …` when
// the simulator has no route to it.  EITHER is proof the real client ran; what it
// must NOT be is the old `platform-posix-minimal` stub, whose error carries the
// literal `posix-minimal: … SSE stub (Plan 17 wires the real client)`.  This test
// asserts exactly that distinction (see `assertRealHttpAttempt` below).
//
// So a keyless run is itself a complete proof: engine-build succeeded AND a real
// engine-originated event arrived through the UniFFI callback on the listener AND
// it came from a real `reqwest` call, not the stub.  (WITH a key, the same path
// streams `.textDelta` — see README-engine.md for the real-run command; this test
// does not require or assert that, so it never needs a secret in CI.)
//
// NO secrets: the test never sets or reads a hardcoded key.  It asserts the
// keyless behavior; the engine reads `ANTHROPIC_API_KEY` from the environment.

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

    final class EngineRoundtripTests: XCTestCase {

        /// End-to-end, KEYLESS:  build the engine, submit a prompt, and assert a
        /// terminal engine event is delivered to the listener over UniFFI.
        ///
        /// This single test exercises every hop of the SwiftUI→UniFFI→engine→
        /// listener path without a network success and without a secret.
        func testKeylessSendPromptDeliversTerminalErrorEvent() async throws {
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
            // NOT pass a key — read it from the env, which is empty in keyless CI.
            // (We deliberately do not source a hardcoded secret here.)
            let apiKey = ProcessInfo.processInfo.environment["ANTHROPIC_API_KEY"] ?? ""
            let handle: MobileEngineHandle
            do {
                handle = try buildIosEngine(
                    apiBase: ProcessInfo.processInfo.environment["ANTHROPIC_BASE_URL"]
                        ?? "https://api.anthropic.com",
                    apiKey: apiKey,
                    model: "claude-sonnet-4-20250514",
                    appSandboxRoot: sandbox.path,
                    listener: listener)
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
            // listener, and the keyless turn surfaced a terminal `error` (the
            // expected keyless outcome).  WITH a key the same path yields
            // `.textDelta`; keyless it is `.error`.
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
            XCTAssertFalse(errorEvents.isEmpty,
                           """
                           keyless turn must deliver a terminal ClientEvent.error to the listener \
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
