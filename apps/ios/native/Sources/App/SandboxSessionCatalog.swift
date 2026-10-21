import Foundation
import CoreFoundation
import Darwin

/// An iOS update can move the data container without changing any session data.
/// Rebind its current-format catalogs before starting an engine rooted at the
/// new path. Originals remain intact; divergent copies are never overwritten.
enum SandboxSessionCatalog {
    private static let lock = NSLock()
    private static let uuidPattern = "[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}"

    enum RebindError: LocalizedError {
        case unsafeEntry(String)
        case conflict(String)

        var errorDescription: String? {
            switch self {
            case .unsafeEntry(let path):
                return "Cannot restore conversation storage through a non-regular entry: \(path)"
            case .conflict(let path):
                return "Cannot restore conflicting conversation copies: \(path). Both originals have been preserved."
            }
        }
    }

    static func prepare(appRoot: String) throws {
        lock.lock()
        defer { lock.unlock() }
        // Foundation can prefer /var even after resolving symlinks. Rust's
        // canonicalize uses realpath; its exact spelling is the catalog key.
        guard let resolved = appRoot.withCString({ realpath($0, nil) }) else {
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
        }
        defer { free(resolved) }
        let canonicalPath = String(cString: resolved)
        let root = URL(fileURLWithPath: canonicalPath)
        guard containerPrefix(canonicalPath) != nil else { return }
        let home = root.appendingPathComponent(".lingxi", isDirectory: true)
        guard try kind(home) != nil else { return }
        try requireDirectory(home)
        let marker = home.appendingPathComponent("session-catalog-container")
        if let type = try kind(marker) {
            guard type == .typeRegular else { throw RebindError.unsafeEntry(marker.path) }
            if try Data(contentsOf: marker) == Data(canonicalPath.utf8) { return }
        }
        var suffixes = ["", "/scheduled/workspace"]
        for folder in ["Projects", "projects"] {
            let projects = root.appendingPathComponent(folder, isDirectory: true)
            guard try kind(projects) != nil else { continue }
            try requireDirectory(projects)
            for entry in try FileManager.default.contentsOfDirectory(at: projects, includingPropertiesForKeys: nil) {
                guard UUID(uuidString: entry.lastPathComponent) != nil else { continue }
                try requireDirectory(entry)
                suffixes.append("/\(folder)/\(entry.lastPathComponent)/workspace")
            }
        }
        let catalogs = home.appendingPathComponent("projects", isDirectory: true)
        if try kind(catalogs) != nil {
            try rebind(projects: catalogs, currentAppRoot: canonicalPath, workspaceSuffixes: suffixes)
        }
        try Data(canonicalPath.utf8).write(to: marker, options: .atomic)
    }

    /// The caller supplies only the global, scheduled, and managed workspaces
    /// belonging to this app. No arbitrary cwd or transcript body is rewritten.
    static func rebind(projects: URL, currentAppRoot: String, workspaceSuffixes: [String]) throws {
        guard let prefix = containerPrefix(currentAppRoot) else { return }
        try requireDirectory(projects)
        let directories = try FileManager.default.contentsOfDirectory(at: projects, includingPropertiesForKeys: nil)
        let encodedPrefix = sanitize(prefix)
        let pattern = try NSRegularExpression(pattern: "^" + NSRegularExpression.escapedPattern(for: encodedPrefix) + "(" + uuidPattern + ")")
        var choices: [String: URL] = [:]
        var destinations: [String: URL] = [:]
        var cwds: [String: String] = [:]
        for directory in directories {
            let name = directory.lastPathComponent
            let range = NSRange(name.startIndex..<name.endIndex, in: name)
            guard let match = pattern.firstMatch(in: name, range: range),
                  let idRange = Range(match.range(at: 1), in: name) else { continue }
            let oldRoot = prefix + name[idRange] + "/Library/Application Support/LingxiCode"
            guard oldRoot != currentAppRoot else { continue }
            for suffix in workspaceSuffixes where directoryName(oldRoot + suffix) == name {
                try requireDirectory(directory)
                let cwd = currentAppRoot + suffix
                let destination = projects.appendingPathComponent(directoryName(cwd), isDirectory: true)
                try visit(directory) { source, relative in
                    let target = destination.appendingPathComponent(relative)
                    try validateParents(of: target, under: projects)
                    let key = target.path
                    let previous: URL?
                    if let selected = choices[key] { previous = selected }
                    else { previous = try kind(target) == nil ? nil : target }
                    if let previous {
                        guard try kind(previous) == .typeRegular else { throw RebindError.unsafeEntry(previous.path) }
                        if try isPrefix(source, of: previous) { choices[key] = previous }
                        else if try isPrefix(previous, of: source) { choices[key] = source }
                        else if try supersedesEmptyPlaceholder(source, placeholder: previous) { choices[key] = source }
                        else if try supersedesEmptyPlaceholder(previous, placeholder: source) { choices[key] = previous }
                        else { throw RebindError.conflict(relative) }
                    } else { choices[key] = source }
                    destinations[key] = target
                    if source.deletingLastPathComponent() == directory,
                       source.pathExtension == "jsonl",
                       UUID(uuidString: source.deletingPathExtension().lastPathComponent) != nil {
                        cwds[key] = cwd
                    }
                }
            }
        }
        // Resolve every collision before creating or replacing any destination.
        for key in choices.keys.sorted() {
            guard let source = choices[key], let target = destinations[key], source != target else { continue }
            try validateParents(of: target, under: projects)
            try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
            let temporary = target.deletingLastPathComponent().appendingPathComponent(".container-rebind-" + UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: temporary) }
            try FileManager.default.copyItem(at: source, to: temporary)
            if let cwd = cwds[key] {
                let marker = try JSONSerialization.data(withJSONObject: [
                    "type": "relocated", "relocatedCwd": cwd,
                    "sessionId": target.deletingPathExtension().lastPathComponent,
                ], options: [.sortedKeys, .withoutEscapingSlashes])
                let file = try FileHandle(forUpdating: temporary)
                defer { try? file.close() }
                let size = try file.seekToEnd()
                if size > 0 {
                    try file.seek(toOffset: size - 1)
                    if try file.read(upToCount: 1) != Data([10]) { try file.write(contentsOf: Data([10])) }
                }
                try file.seekToEnd()
                try file.write(contentsOf: marker + Data([10]))
                try file.synchronize()
            }
            if try kind(target) != nil {
                let backup = projects.appendingPathComponent(".container-rebind-originals", isDirectory: true)
                    .appendingPathComponent(target.deletingLastPathComponent().lastPathComponent, isDirectory: true)
                    .appendingPathComponent(UUID().uuidString + "-" + target.lastPathComponent)
                try validateParents(of: backup, under: projects)
                try FileManager.default.createDirectory(at: backup.deletingLastPathComponent(), withIntermediateDirectories: true)
                try FileManager.default.copyItem(at: target, to: backup)
                _ = try FileManager.default.replaceItemAt(target, withItemAt: temporary)
            } else {
                try FileManager.default.moveItem(at: temporary, to: target)
            }
        }
    }

    /// Mirrors session::jsonl::project_dir_name, including its long-path hash.
    static func directoryName(_ cwd: String) -> String {
        let name = sanitize(cwd)
        guard name.count > 200 else { return name }
        let hash = cwd.utf16.reduce(Int32(0)) { ($0 &* 31) &+ Int32($1) }
        return String(name.prefix(200)) + "-" + String(Int64(hash).magnitude, radix: 36)
    }

    private static func sanitize(_ path: String) -> String {
        String(path.unicodeScalars.map { value -> Character in
            let n = value.value
            return (48...57).contains(n) || (65...90).contains(n) || (97...122).contains(n) ? Character(String(value)) : "-"
        })
    }

    private static func containerPrefix(_ path: String) -> String? {
        let pattern = "^(.*?/Containers/Data/Application/)" + uuidPattern + "/Library/Application Support/LingxiCode$"
        guard let regex = try? NSRegularExpression(pattern: pattern),
              let match = regex.firstMatch(in: path, range: NSRange(path.startIndex..<path.endIndex, in: path)),
              let range = Range(match.range(at: 1), in: path) else { return nil }
        return String(path[range])
    }

    private static func kind(_ url: URL) throws -> FileAttributeType? {
        do { return try FileManager.default.attributesOfItem(atPath: url.path)[.type] as? FileAttributeType }
        catch let error as NSError where error.domain == NSCocoaErrorDomain &&
            [NSFileNoSuchFileError, NSFileReadNoSuchFileError].contains(error.code) { return nil }
    }

    private static func requireDirectory(_ url: URL) throws {
        guard try kind(url) == .typeDirectory else { throw RebindError.unsafeEntry(url.path) }
    }

    private static func validateParents(of file: URL, under root: URL) throws {
        var parent = file.deletingLastPathComponent()
        while parent.path != root.path {
            guard parent.path.hasPrefix(root.path + "/") else { throw RebindError.unsafeEntry(parent.path) }
            if let type = try kind(parent), type != .typeDirectory { throw RebindError.unsafeEntry(parent.path) }
            parent.deleteLastPathComponent()
        }
    }

    private static func visit(_ root: URL, relative: String = "", file: (URL, String) throws -> Void) throws {
        let directory = relative.isEmpty ? root : root.appendingPathComponent(relative)
        for entry in try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) {
            let path = relative.isEmpty ? entry.lastPathComponent : relative + "/" + entry.lastPathComponent
            switch try kind(entry) {
            case .typeDirectory: try visit(root, relative: path, file: file)
            case .typeRegular: try file(entry, path)
            default: throw RebindError.unsafeEntry(entry.path)
            }
        }
    }

    private static func isPrefix(_ shorter: URL, of longer: URL) throws -> Bool {
        let left = try FileHandle(forReadingFrom: shorter)
        let right = try FileHandle(forReadingFrom: longer)
        defer { try? left.close(); try? right.close() }
        while let bytes = try left.read(upToCount: 64 * 1024), !bytes.isEmpty {
            guard try right.read(upToCount: bytes.count) == bytes else { return false }
        }
        return true
    }

    /// A current mobile empty-session anchor may have gained settings records
    /// in one container while the real conversation continued in another.
    /// Prefer that continuation only when its original anchor bytes agree.
    private static func supersedesEmptyPlaceholder(_ fuller: URL, placeholder: URL) throws -> Bool {
        guard fuller.pathExtension == "jsonl",
              UUID(uuidString: fuller.deletingPathExtension().lastPathComponent) != nil,
              let anchor = try emptyAnchor(placeholder),
              try emptyAnchor(fuller) == nil else { return false }
        let file = try FileHandle(forReadingFrom: fuller)
        defer { try? file.close() }
        return try file.read(upToCount: anchor.count) == anchor
    }

    private static func emptyAnchor(_ file: URL) throws -> Data? {
        let input = try FileHandle(forReadingFrom: file)
        defer { try? input.close() }
        guard let data = try input.read(upToCount: 64 * 1024 + 1), data.count <= 64 * 1024 else { return nil }
        let lines = data.split(separator: 10)
        guard let first = lines.first,
              let anchor = (try? JSONSerialization.jsonObject(with: Data(first))) as? [String: Any],
              anchor["type"] as? String == "custom-title",
              let version = anchor["mobileEmptySession"] as? NSNumber,
              CFGetTypeID(version) != CFBooleanGetTypeID(), version == 1,
              let id = anchor["sessionId"] as? String,
              id == file.deletingPathExtension().lastPathComponent else { return nil }
        for line in lines {
            guard let row = (try? JSONSerialization.jsonObject(with: Data(line))) as? [String: Any],
                  row["sessionId"] as? String == id,
                  let type = row["type"] as? String,
                  ["custom-title", "session-mode", "permission-mode", "relocated"].contains(type) else { return nil }
            if type == "custom-title", row["mobileEmptySession"] as? Int != 1 { return nil }
        }
        return Data(first) + Data([10])
    }
}
