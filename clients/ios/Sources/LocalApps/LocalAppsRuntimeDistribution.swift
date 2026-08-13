import Foundation

enum LocalAppsRuntimeDistribution {
    static var usesFullRuntime: Bool {
        #if LINGXI_FULL
            true
        #else
            false
        #endif
    }

    static var runtimeRoot: String? {
        resolveRuntimeRoot(
            resourceURL: Bundle.main.resourceURL,
            manifest: LXISHRuntimeBundleMetadata.current()
        )
    }

    static func resolveRuntimeRoot(
        resourceURL: URL?,
        manifest: LXISHRuntimeBundleManifest,
        fileManager: FileManager = .default
    ) -> String? {
        guard manifest.localAppRuntime,
              let root = resourceURL?
            .appendingPathComponent("local-app-runtime", isDirectory: true),
              fileManager.fileExists(
                atPath: root
                    .appendingPathComponent("node_modules/vite/bin/vite.js")
                    .path
              )
        else { return nil }
        return root.path
    }
}
