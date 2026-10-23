#if DEBUG
import FirebaseCore
import FirebaseCrashlytics
import Foundation

/// Explicit device-only acceptance. No launch environment can enable this in Release.
@MainActor
enum CrashlyticsDeviceVerification {
    static func runIfRequested() {
        let environment = ProcessInfo.processInfo.environment
        guard environment["LINGXI_CRASHLYTICS_PROJECT"] == "lingxi-5e4b2",
              FirebaseApp.app()?.options.projectID == "lingxi-5e4b2",
              let rawID = environment["LINGXI_CRASHLYTICS_VERIFICATION_ID"],
              let id = UUID(uuidString: rawID),
              let phase = environment["LINGXI_CRASHLYTICS_PHASE"],
              ["capture", "send"].contains(phase) else { return }
        let reporter = Crashlytics.crashlytics()
        guard !reporter.isCrashlyticsCollectionEnabled() else { return }
        let directory = URL.cachesDirectory.appending(path: "CrashlyticsDeviceVerification")
        let receipt = directory.appending(path: "receipt.json")
        let previous = (try? Data(contentsOf: receipt)).flatMap {
            try? JSONSerialization.jsonObject(with: $0) as? [String: String]
        }
        if phase == "capture", previous?["verification_id"] == id.uuidString { return }
        if phase == "send" {
            guard previous?["verification_id"] == id.uuidString,
                  previous?["phase"] == "capture-ready",
                  reporter.didCrashDuringPreviousExecution() else { return }
        }
        reporter.checkForUnsentReports { hasReports in
            Task { @MainActor in
                // Never send pre-existing reports as part of a fresh verification.
                guard phase == "capture" ? !hasReports : hasReports else { return }
                do {
                    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                    let data = try JSONSerialization.data(withJSONObject: [
                        "verification_id": id.uuidString,
                        "phase": phase == "capture" ? "capture-ready" : "send-enqueued",
                        "automatic_collection": "false",
                    ], options: [.sortedKeys])
                    try data.write(to: receipt, options: .atomic)
                } catch { return }
                if phase == "send" {
                    reporter.sendUnsentReports()
                } else {
                    reporter.setCustomValue(id.uuidString, forKey: "verification_id")
                    // Allow SDK handlers and custom-key storage to finish without a debugger.
                    try? await Task.sleep(for: .seconds(5))
                    NSException(name: NSExceptionName("CrashlyticsDeviceVerification"),
                                reason: "Intentional Crashlytics device verification",
                                userInfo: nil).raise()
                }
            }
        }
    }
}
#endif
