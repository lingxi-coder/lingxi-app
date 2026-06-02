// EngineModule.swift — M10 A2 / P2 linkage smoke.
//
// This file PROVES that the generated UniFFI Swift bindings
// (clients/ios/Generated/*.swift, compiled into this app target) and the
// `LingxiCodeFFI.xcframework` static library are wired into the XcodeGen
// project and LINK on the simulator — not merely that they compile.
//
// Two layers of proof:
//   1. Type-level: references the generated `MobileEngineHandle` /
//      `ClientCommand` / `ClientError` / `MobileEngineError` Swift types, so a
//      missing/mis-imported binding (the per-namespace `<ns>FFI` clang modules
//      from the xcframework's bundled module.modulemap) fails to compile.
//   2. Link-level: calls the C ABI function `ffi_engine_mobile_uniffi_contract_version()`
//      (exported by the engine_mobile namespace of the static archive and
//      surfaced through the `engine_mobileFFI` clang module). Because this is
//      invoked from a REACHABLE path (`EngineModuleLinkageSmoke.verify()` is
//      called from the app's startup), the linker cannot dead-strip it and must
//      resolve the symbol from `libios_framework.a` — proving the framework is
//      actually linked, not just present.
//
// The generated bindings are produced from a host build by
// `scripts/build-xcframework.sh` (in the gitignored `Generated/` dir). Its
// dedup pass keeps the shared FfiConverter scaffolding in a single module so
// cross-namespace references resolve; the per-namespace C/FFI clang modules
// (engine_mobileFFI, client_protocolFFI, …) are imported automatically by the
// generated Swift via `#if canImport(...)`, so there is intentionally NO
// `import LingxiCodeFFI` here — the public Swift surface lives in this module.
//
// This is a link smoke ONLY: it does NOT drive the engine, build a handle, or
// touch the UI (that is later work). No secrets — the contract-version call
// takes no config and performs no I/O; the engine reads `ANTHROPIC_API_KEY`
// from the runtime environment, never from this code.

import Foundation

// The engine_mobile FFI clang module (from LingxiCodeFFI.xcframework) is what
// exports `ffi_engine_mobile_uniffi_contract_version`. The generated Swift
// imports it the same way, behind a `canImport` guard.
#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

/// Namespace whose sole job is to force-link the engine static archive.
enum EngineModuleLinkageSmoke {
    /// Calls a no-argument C ABI function from the engine_mobile namespace of
    /// `libios_framework.a`. Reachable from app startup so the linker keeps the
    /// reference; harmless at runtime (returns the UniFFI contract version, no
    /// I/O, no engine). The result is intentionally discarded.
    @discardableResult
    static func verify() -> UInt32 {
        #if canImport(engine_mobileFFI)
            // Load-bearing link reference: resolves a symbol from the linked
            // static archive shipped in LingxiCodeFFI.xcframework.
            return ffi_engine_mobile_uniffi_contract_version()
        #else
            return 0
        #endif
    }

    /// Type-level anchor: naming the generated binding types here makes their
    /// absence a COMPILE error, guarding the Swift-bindings half of the wiring.
    /// Never executed; exists purely so the type references are emitted.
    private static func _typeAnchors() {
        let _: MobileEngineHandle.Type = MobileEngineHandle.self
        let _: ClientCommand = .cancel(turnId: nil)
        let _: ClientError.Type = ClientError.self
        let _: MobileEngineError.Type = MobileEngineError.self
    }
}
