import Foundation

protocol ProjectBookmarkResolving: Sendable {
    func withInitialAccess<T>(to directoryURL: URL, _ body: (URL) throws -> T) throws -> T
    func makeBookmark(for directoryURL: URL) throws -> ProjectExternalBookmark
    func refreshBookmark(for resolved: ResolvedProjectBookmark) throws -> ProjectExternalBookmark
    func resolve(_ bookmark: ProjectExternalBookmark) throws -> ResolvedProjectBookmark
}

final class ResolvedProjectBookmark: @unchecked Sendable {
    let url: URL
    let isStale: Bool
    private let stopAccessing: @Sendable () -> Void

    init(url: URL, isStale: Bool, stopAccessing: @escaping @Sendable () -> Void = {}) {
        self.url = url
        self.isStale = isStale
        self.stopAccessing = stopAccessing
    }

    deinit {
        stopAccessing()
    }
}

struct SecurityScopedProjectBookmarkResolver: ProjectBookmarkResolving {
    func withInitialAccess<T>(to directoryURL: URL, _ body: (URL) throws -> T) throws -> T {
        let didStart = directoryURL.startAccessingSecurityScopedResource()
        defer {
            if didStart {
                directoryURL.stopAccessingSecurityScopedResource()
            }
        }
        return try body(directoryURL)
    }

    func makeBookmark(for directoryURL: URL) throws -> ProjectExternalBookmark {
        let bookmarkData = try directoryURL.bookmarkData(
            options: [],
            includingResourceValuesForKeys: nil,
            relativeTo: nil
        )
        return ProjectExternalBookmark(
            data: bookmarkData,
            displayName: directoryURL.lastPathComponent,
            pathHint: directoryURL.path,
            isStale: false
        )
    }

    func refreshBookmark(for resolved: ResolvedProjectBookmark) throws -> ProjectExternalBookmark {
        try makeBookmark(for: resolved.url)
    }

    func resolve(_ bookmark: ProjectExternalBookmark) throws -> ResolvedProjectBookmark {
        var isStale = false
        let url = try URL(
            resolvingBookmarkData: bookmark.data,
            options: [],
            relativeTo: nil,
            bookmarkDataIsStale: &isStale
        )
        let didStart = url.startAccessingSecurityScopedResource()
        guard didStart else { throw ProjectBookmarkError.authorizationDenied }
        return ResolvedProjectBookmark(url: url, isStale: isStale) {
            url.stopAccessingSecurityScopedResource()
        }
    }
}

enum ProjectBookmarkError: LocalizedError {
    case authorizationDenied

    var errorDescription: String? {
        switch self {
        case .authorizationDenied:
            return "cannot access the bookmarked external directory"
        }
    }
}
