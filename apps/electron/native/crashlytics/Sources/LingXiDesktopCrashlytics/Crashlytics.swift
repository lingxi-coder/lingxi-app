import Foundation
import FirebaseCore
import FirebaseCrashlytics

// Loaded into the Electron MAIN process, never a separate helper process.
// The SDK's signal/Mach handlers therefore capture the process that crashed.
@_cdecl("lingxi_crashlytics_start")
public func start(_ path: UnsafePointer<CChar>, _ distribution: UnsafePointer<CChar>) -> Int32 {
    guard Thread.isMainThread, FirebaseApp.app() == nil,
          let options = FirebaseOptions(contentsOfFile: String(cString: path)),
          options.projectID == "lingxi-5e4b2",
          options.bundleID == Bundle.main.bundleIdentifier else { return 0 }
    if ProcessInfo.processInfo.environment["LINGXI_CRASHLYTICS_TEST_ROOT"] != nil {
        FirebaseConfiguration.shared.setLoggerLevel(.debug)
    }
    FirebaseApp.configure(options: options)
    let reporter = Crashlytics.crashlytics()
    reporter.setCustomValue("desktop-" + String(cString: distribution), forKey: "distribution")
    reporter.setCustomValue("release", forKey: "build_type")
    return 1
}

// Only fixed event categories reach the SDK. Exception messages, IPC payloads,
// project paths, prompts and credentials must never become report annotations.
@_cdecl("lingxi_crashlytics_event")
public func event(_ category: UnsafePointer<CChar>, _ code: Int32) {
    guard FirebaseApp.app() != nil else { return }
    let name = String(cString: category)
    let allowed = ["main-js-uncaught", "renderer-crashed", "renderer-oom", "gpu-crashed", "utility-crashed"]
    guard allowed.contains(name) else { return }
    Crashlytics.crashlytics().record(error: NSError(domain: "LingXiDesktop." + name, code: Int(code)))
}

@_cdecl("lingxi_crashlytics_previous_crash")
public func previousCrash() -> Int32 {
    guard FirebaseApp.app() != nil else { return 0 }
    return Crashlytics.crashlytics().didCrashDuringPreviousExecution() ? 1 : 0
}
