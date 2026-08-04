import Foundation
import Observation

struct LocalAppSourceFile: Identifiable, Hashable, Sendable {
    var id: String { relativePath }
    let relativePath: String
    let size: Int
}

@Observable
@MainActor
final class LocalAppCodeBrowser {
    private static let excludedDirectories: Set<String> = [
        ".git", ".lingxi", ".next", "build", "node_modules",
    ]
    private static let editableExtensions: Set<String> = [
        "css", "html", "js", "json", "jsx", "md", "mjs", "svg", "ts", "tsx", "txt",
    ]
    private static let editableNames: Set<String> = [
        ".gitignore", ".npmrc", "next.config.js", "next.config.mjs", "package-lock.json", "package.json",
    ]
    private static let maximumEditableBytes = 1_048_576

    private(set) var files: [LocalAppSourceFile] = []
    private(set) var selectedPath: String?
    private(set) var editorText = ""
    private(set) var isLoading = false
    private(set) var isSaving = false
    private(set) var errorMessage: String?
    private(set) var statusMessage: String?

    private let workspaceRelativePath: String

    init(workspaceRelativePath: String) {
        self.workspaceRelativePath = workspaceRelativePath
    }

    func refresh() {
        isLoading = true
        defer { isLoading = false }
        do {
            let root = try workspaceRoot()
            let keys: [URLResourceKey] = [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey]
            guard let enumerator = FileManager.default.enumerator(
                at: root,
                includingPropertiesForKeys: keys,
                options: [],
                errorHandler: { _, _ in true }
            ) else {
                files = []
                return
            }

            var values: [LocalAppSourceFile] = []
            while let url = enumerator.nextObject() as? URL {
                let resource = try url.resourceValues(forKeys: Set(keys))
                if resource.isSymbolicLink == true {
                    if resource.isDirectory == true { enumerator.skipDescendants() }
                    continue
                }
                if resource.isDirectory == true, Self.excludedDirectories.contains(url.lastPathComponent) {
                    enumerator.skipDescendants()
                    continue
                }
                guard resource.isRegularFile == true, resource.isSymbolicLink != true else { continue }
                let relativePath = try safeRelativePath(for: url, root: root)
                guard Self.isEditable(url), (resource.fileSize ?? 0) <= Self.maximumEditableBytes else { continue }
                values.append(LocalAppSourceFile(relativePath: relativePath, size: resource.fileSize ?? 0))
            }
            files = values.sorted { $0.relativePath.localizedStandardCompare($1.relativePath) == .orderedAscending }
            errorMessage = nil
        } catch {
            files = []
            errorMessage = error.localizedDescription
        }
    }

    func open(_ file: LocalAppSourceFile) {
        do {
            let url = try resolvedFileURL(relativePath: file.relativePath)
            let data = try Data(contentsOf: url, options: [.mappedIfSafe])
            guard data.count <= Self.maximumEditableBytes, let text = String(data: data, encoding: .utf8) else {
                throw BrowserError.notEditable
            }
            selectedPath = file.relativePath
            editorText = text
            errorMessage = nil
            statusMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func updateEditorText(_ value: String) {
        editorText = value
        statusMessage = nil
    }

    func closeEditor() {
        selectedPath = nil
        editorText = ""
        statusMessage = nil
    }

    func clearError() {
        errorMessage = nil
    }

    func save() {
        guard let selectedPath else { return }
        isSaving = true
        defer { isSaving = false }
        do {
            guard editorText.utf8.count <= Self.maximumEditableBytes else { throw BrowserError.tooLarge }
            let url = try resolvedFileURL(relativePath: selectedPath)
            try editorText.write(to: url, atomically: true, encoding: .utf8)
            statusMessage = "已保存。下次生成或构建会校验依赖和源码策略。"
            errorMessage = nil
            refresh()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func workspaceRoot() throws -> URL {
        let sandbox = URL(
            fileURLWithPath: ConversationSourceFactory.appSandboxRoot(),
            isDirectory: true
        ).standardizedFileURL.resolvingSymlinksInPath()
        guard !workspaceRelativePath.hasPrefix("/") else { throw BrowserError.pathEscaped }
        let components = workspaceRelativePath.split(separator: "/", omittingEmptySubsequences: false)
        guard components.count == 3,
              components[0] == "apps",
              components[2] == "workspace",
              String(components[1]).range(of: #"^[a-z0-9][a-z0-9-]{0,63}$"#, options: .regularExpression) != nil
        else { throw BrowserError.pathEscaped }
        let candidate = sandbox
            .appendingPathComponent(workspaceRelativePath, isDirectory: true)
            .standardizedFileURL
            .resolvingSymlinksInPath()
        guard Self.isDescendant(candidate, of: sandbox) else { throw BrowserError.pathEscaped }
        return candidate
    }

    private func resolvedFileURL(relativePath: String) throws -> URL {
        let root = try workspaceRoot()
        let candidate = root.appendingPathComponent(relativePath, isDirectory: false).standardizedFileURL
        guard Self.isDescendant(candidate, of: root) else { throw BrowserError.pathEscaped }
        let values = try candidate.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
        guard values.isRegularFile == true, values.isSymbolicLink != true, Self.isEditable(candidate) else {
            throw BrowserError.notEditable
        }
        return candidate
    }

    private func safeRelativePath(for url: URL, root: URL) throws -> String {
        let candidate = url.standardizedFileURL
        guard Self.isDescendant(candidate, of: root) else { throw BrowserError.pathEscaped }
        return String(candidate.path.dropFirst(root.path.count + 1))
    }

    private static func isDescendant(_ candidate: URL, of root: URL) -> Bool {
        candidate.path == root.path || candidate.path.hasPrefix(root.path + "/")
    }

    private static func isEditable(_ url: URL) -> Bool {
        editableNames.contains(url.lastPathComponent) || editableExtensions.contains(url.pathExtension.lowercased())
    }

    private enum BrowserError: LocalizedError {
        case notEditable
        case pathEscaped
        case tooLarge

        var errorDescription: String? {
            switch self {
            case .notEditable: "该文件不是可编辑的 UTF-8 源码。"
            case .pathEscaped: "已拒绝工作区之外的路径。"
            case .tooLarge: "单个可编辑源码文件不能超过 1 MiB。"
            }
        }
    }
}
