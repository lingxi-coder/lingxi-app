import FirebaseCrashlytics

/// Only static build context is attached; never account IDs, prompts or credentials.
@MainActor
enum CrashReporting {
    static func configure() {
        let reporter = Crashlytics.crashlytics()
        // The expanded Info.plist sets collection before Firebase initializes.
        // Avoid a sticky runtime override that survives switching build types.
        #if DEBUG
        reporter.setCustomValue("debug", forKey: "build_type")
        #else
        reporter.setCustomValue("release", forKey: "build_type")
        #endif
        #if LINGXI_FULL
        reporter.setCustomValue("full", forKey: "distribution")
        #else
        reporter.setCustomValue("store", forKey: "distribution")
        #endif
        #if DEBUG
        CrashlyticsDeviceVerification.runIfRequested()
        #endif
    }
}
