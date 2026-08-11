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
        guard let root = Bundle.main.resourceURL?
            .appendingPathComponent("local-app-runtime", isDirectory: true),
              FileManager.default.fileExists(
                atPath: root
                    .appendingPathComponent("node_modules/next/dist/bin/next")
                    .path
              ),
              FileManager.default.fileExists(
                atPath: root
                    .appendingPathComponent("node_modules/vite/bin/vite.js")
                    .path
              )
        else { return nil }
        return root.path
    }
}
