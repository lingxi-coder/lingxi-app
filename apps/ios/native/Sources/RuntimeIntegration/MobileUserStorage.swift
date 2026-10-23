import Foundation

/// Native backing locations of the Linux runtime account. Stable project and
/// session identities use guest paths; host paths are only filesystem adapters.
enum MobileUserStorage {
    static func directories(managedRoot: String = LXISHDefaultWorkspace.managedRootPath()) -> MobileUserDirectories {
        // The shared resolver is pure and can fail only for a malformed host
        // root. Filesystem availability is reported by repository/engine I/O.
        // iOS can spell the same container through /var and /private/var.
        // Normalize the managed root before deriving owned child paths, so
        // repository containment checks do not reject the system's alias.
        guard (try? mobileUserDirectories(managedRoot: managedRoot)) != nil else {
            preconditionFailure("Application Support must provide an absolute runtime storage root")
        }
        let canonicalRoot = URL(fileURLWithPath: managedRoot, isDirectory: true)
            .resolvingSymlinksInPath().standardizedFileURL.path
        guard let paths = try? mobileUserDirectories(managedRoot: canonicalRoot) else {
            preconditionFailure("Application Support must provide an absolute runtime storage root")
        }
        return paths
    }

    static var projectsURL: URL {
        URL(fileURLWithPath: directories().projects, isDirectory: true)
    }
}
