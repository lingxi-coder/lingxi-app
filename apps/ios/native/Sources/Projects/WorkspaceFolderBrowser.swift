import SwiftUI

enum WorkspaceBrowserStrings {
    static func text(_ key: String) -> String {
        String(localized: String.LocalizationValue(key), table: "WorkspaceBrowser")
    }
}

/// One browser for picking an existing directory, saving a location and
/// managing actual folders in the persistent runtime account.
struct WorkspaceFolderBrowser: View {
    @Environment(\.dismiss) private var dismiss
    let repository: WorkspaceDirectoryRepository
    let paths: WorkspaceDirectoryPaths
    let initialPath: String
    let protectedPaths: () -> [String]
    let canMutate: () -> Bool
    let selectionTitle: String
    let onSelect: (String) async throws -> Void
    var body: some View {
        WorkspaceFilesView(repository: repository, paths: paths, initialPath: initialPath,
            protectedPaths: protectedPaths, canMutate: canMutate,
            selectionTitle: selectionTitle, onSelect: onSelect)
    }
}
