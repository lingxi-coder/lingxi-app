import Foundation
import Security
import Darwin

@objc protocol LingXiCredentialBrokerXPC {
    func perform(_ requestData: NSData, withReply reply: @escaping (NSData?, NSString?) -> Void)
}

struct PackagedBrokerResources {
    let manifest: BrokerManifest
    let appBundle: URL
}

func clientResourceRoot(executablePath: String = CommandLine.arguments[0]) -> URL {
    URL(fileURLWithPath: executablePath)
        .resolvingSymlinksInPath()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
}

func loadPackagedBrokerResources() throws -> PackagedBrokerResources {
    let root = clientResourceRoot()
    let appBundle = root.appendingPathComponent(BrokerSecurity.brokerBundleName, isDirectory: true)
    guard FileManager.default.fileExists(atPath: appBundle.path) else {
        throw BrokerFailure.unavailable("packaged credential broker app is missing")
    }
    let manifest = try loadInstalledManifest(bundleURL: appBundle)
    try validateManifest(manifest)
    return PackagedBrokerResources(
        manifest: manifest,
        appBundle: appBundle
    )
}

func readRequestFromStdin() throws -> Data {
    let handle = FileHandle.standardInput
    let firstChunk = handle.readData(ofLength: maxBrokerMessageBytes + 1)
    guard !firstChunk.isEmpty else {
        throw BrokerFailure.invalidRequest("empty credential broker request")
    }
    guard firstChunk.count <= maxBrokerMessageBytes else {
        throw BrokerFailure.invalidRequest("credential broker request exceeds the size limit")
    }
    let trailing = handle.readDataToEndOfFile()
    guard trailing.isEmpty else {
        throw BrokerFailure.invalidRequest("credential broker request exceeds the size limit")
    }
    return firstChunk
}

func authorizedParentChannel(teamId: String) throws -> String {
    for channel in ["production", "development"] {
        do {
            try validateProcessIdentifier(
                getppid(),
                expectedIdentifiers: BrokerSecurity.allowedCallerIdentifiers(channel: channel),
                teamId: teamId,
                action: "authorize broker caller"
            )
            return channel
        } catch {
            continue
        }
    }
    throw BrokerFailure.permission("credential broker caller has an unauthorized code signature")
}

func brokerInstallDirectory(for channel: String) throws -> URL {
    guard let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
        throw BrokerFailure.unavailable("Application Support directory is unavailable")
    }
    return appSupport
        .appendingPathComponent("LingXi", isDirectory: true)
        .appendingPathComponent("CredentialBroker", isDirectory: true)
        .appendingPathComponent(channel, isDirectory: true)
}

func brokerAppExecutableURL(bundleURL: URL) -> URL {
    bundleURL
        .appendingPathComponent("Contents", isDirectory: true)
        .appendingPathComponent("MacOS", isDirectory: true)
        .appendingPathComponent(BrokerSecurity.brokerExecutable)
}

func brokerManifestURL(bundleURL: URL) -> URL {
    bundleURL
        .appendingPathComponent("Contents", isDirectory: true)
        .appendingPathComponent("Resources", isDirectory: true)
        .appendingPathComponent("broker-manifest.json")
}

func loadInstalledManifest(bundleURL: URL) throws -> BrokerManifest {
    let data = try Data(contentsOf: brokerManifestURL(bundleURL: bundleURL))
    let manifest = try JSONDecoder().decode(BrokerManifest.self, from: data)
    guard manifest.channel == "production" || manifest.channel == "development" else {
        throw BrokerFailure.unavailable("installed credential broker channel is invalid")
    }
    return manifest
}

func fileExists(_ url: URL) -> Bool {
    FileManager.default.fileExists(atPath: url.path)
}

func copyDirectoryAtomically(from source: URL, to destination: URL) throws {
    let fileManager = FileManager.default
    let parent = destination.deletingLastPathComponent()
    try fileManager.createDirectory(
        at: parent,
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700]
    )
    let stagingRoot = parent.appendingPathComponent(".install-\(UUID().uuidString)", isDirectory: true)
    try fileManager.createDirectory(
        at: stagingRoot,
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700]
    )
    let stagedBundle = stagingRoot.appendingPathComponent(destination.lastPathComponent, isDirectory: true)
    do {
        try fileManager.copyItem(at: source, to: stagedBundle)
        if fileExists(destination) {
            _ = try fileManager.replaceItemAt(destination, withItemAt: stagedBundle)
        } else {
            try fileManager.moveItem(at: stagedBundle, to: destination)
        }
        try? fileManager.removeItem(at: stagingRoot)
    } catch {
        try? fileManager.removeItem(at: stagingRoot)
        throw error
    }
}

func withInstallLock<T>(at installRoot: URL, _ body: () throws -> T) throws -> T {
    let fileManager = FileManager.default
    try fileManager.createDirectory(
        at: installRoot,
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700]
    )
    let lockPath = installRoot.appendingPathComponent(".install.lock").path
    let descriptor = Darwin.open(lockPath, O_CREAT | O_RDWR | O_CLOEXEC, S_IRUSR | S_IWUSR)
    guard descriptor >= 0 else {
        throw BrokerFailure.unavailable("credential broker installation lock is unavailable")
    }
    var lock = Darwin.flock()
    lock.l_type = Int16(F_WRLCK)
    lock.l_whence = Int16(SEEK_SET)
    defer {
        var unlock = Darwin.flock()
        unlock.l_type = Int16(F_UNLCK)
        unlock.l_whence = Int16(SEEK_SET)
        _ = Darwin.fcntl(descriptor, F_SETLK, &unlock)
        _ = Darwin.close(descriptor)
    }
    guard Darwin.fcntl(descriptor, F_SETLKW, &lock) != -1 else {
        throw BrokerFailure.unavailable("credential broker installation lock could not be acquired")
    }
    return try body()
}

func xmlEscaped(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
        .replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&apos;")
}

func renderedLaunchAgentPlist(machService: String, executablePath: String) -> String {
    """
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0"><dict>
      <key>Label</key><string>\(xmlEscaped(machService))</string>
      <key>MachServices</key><dict><key>\(xmlEscaped(machService))</key><true/></dict>
      <key>ProgramArguments</key><array><string>\(xmlEscaped(executablePath))</string></array>
      <key>RunAtLoad</key><false/>
      <key>ProcessType</key><string>Background</string>
    </dict></plist>
    """
}

func runLaunchctl(_ arguments: [String]) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
    process.arguments = arguments
    let stderr = Pipe()
    process.standardOutput = FileHandle.nullDevice
    process.standardError = stderr
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        let data = stderr.fileHandleForReading.readDataToEndOfFile()
        let message = String(data: data, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? "unknown launchctl error"
        throw BrokerFailure.unavailable("launchctl \(arguments.joined(separator: " ")) failed: \(message)")
    }
}

func installOrResolveBroker(
    packaged: PackagedBrokerResources,
    teamId: String
) throws -> URL {
    try validateStaticCode(
        at: packaged.appBundle,
        expectedIdentifier: BrokerSecurity.brokerBundleId(channel: packaged.manifest.channel),
        teamId: teamId,
        action: "validate packaged credential broker"
    )

    let installRoot = try brokerInstallDirectory(for: packaged.manifest.channel)
    return try withInstallLock(at: installRoot) {
        try installOrResolveBrokerLocked(packaged: packaged, teamId: teamId, installRoot: installRoot)
    }
}

private func installOrResolveBrokerLocked(
    packaged: PackagedBrokerResources,
    teamId: String,
    installRoot: URL
) throws -> URL {
    let fileManager = FileManager.default
    let installedBundle = installRoot.appendingPathComponent(BrokerSecurity.brokerBundleName, isDirectory: true)
    var shouldInstall = !fileExists(installedBundle)

    if fileExists(installedBundle) {
        do {
            try validateStaticCode(
                at: installedBundle,
                expectedIdentifier: BrokerSecurity.brokerBundleId(channel: packaged.manifest.channel),
                teamId: teamId,
                action: "validate installed credential broker"
            )
            let installedManifest = try loadInstalledManifest(bundleURL: installedBundle)
            if installedManifest.protocolVersion != packaged.manifest.protocolVersion {
                if compareSemanticVersions(installedManifest.version, packaged.manifest.version) == .orderedDescending {
                    throw BrokerFailure.unavailable(
                        "installed credential broker protocol \(installedManifest.protocolVersion) is newer and incompatible; upgrade LingXi"
                    )
                }
                shouldInstall = true
            } else {
                let versionComparison = compareSemanticVersions(
                    installedManifest.version,
                    packaged.manifest.version
                )
                switch versionComparison {
                case .orderedAscending:
                    shouldInstall = true
                case .orderedSame:
                    shouldInstall = try codeDirectoryHash(at: installedBundle)
                        != codeDirectoryHash(at: packaged.appBundle)
                case .orderedDescending:
                    shouldInstall = false
                }
            }
        } catch let failure as BrokerFailure {
            if case .unavailable(let message) = failure, message.contains("newer and incompatible") {
                throw failure
            }
            shouldInstall = true
        } catch {
            shouldInstall = true
        }
    }

    if shouldInstall {
        try copyDirectoryAtomically(from: packaged.appBundle, to: installedBundle)
        try validateStaticCode(
            at: installedBundle,
            expectedIdentifier: BrokerSecurity.brokerBundleId(channel: packaged.manifest.channel),
            teamId: teamId,
            action: "validate installed credential broker"
        )
        let installedManifest = try loadInstalledManifest(bundleURL: installedBundle)
        if installedManifest.protocolVersion != packaged.manifest.protocolVersion {
            throw BrokerFailure.unavailable(
                "installed credential broker protocol \(installedManifest.protocolVersion) is incompatible with packaged protocol \(packaged.manifest.protocolVersion)"
            )
        }
    }

    let launchAgentsDirectory = try fileManager
        .url(for: .libraryDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        .appendingPathComponent("LaunchAgents", isDirectory: true)
    try fileManager.createDirectory(
        at: launchAgentsDirectory,
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700]
    )
    try registerBrokerLaunchAgent(
        machService: BrokerSecurity.machService(channel: packaged.manifest.channel),
        executablePath: brokerAppExecutableURL(bundleURL: installedBundle).path,
        launchAgentsDirectory: launchAgentsDirectory,
        reinstall: shouldInstall
    )

    return installedBundle
}

// A matching plist records desired configuration, not registration in the
// current login's launchd domain. Probe the service so an interrupted or failed
// bootstrap can be retried without changing the packaged app or saved plist.
func registerBrokerLaunchAgent(
    machService: String,
    executablePath: String,
    launchAgentsDirectory: URL,
    reinstall: Bool,
    launchctl: ([String]) throws -> Void = runLaunchctl
) throws {
    let launchAgentPath = launchAgentsDirectory.appendingPathComponent("\(machService).plist")
    let rendered = renderedLaunchAgentPlist(machService: machService, executablePath: executablePath)
    let existing = try? String(contentsOf: launchAgentPath, encoding: .utf8)
    let domain = "gui/\(getuid())"
    let serviceTarget = "\(domain)/\(machService)"
    let registered: Bool
    do {
        try launchctl(["print", serviceTarget])
        registered = true
    } catch {
        registered = false
    }
    guard reinstall || existing != rendered || !registered else { return }

    try rendered.write(to: launchAgentPath, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: launchAgentPath.path)
    if registered {
        _ = try? launchctl(["bootout", serviceTarget])
    }
    try launchctl(["bootstrap", domain, launchAgentPath.path])
    try launchctl(["kickstart", "-k", serviceTarget])
}

func performXpcCall(requestData: Data, teamId: String, channel: String) throws -> Data {
    let connection = NSXPCConnection(machServiceName: BrokerSecurity.machService(channel: channel), options: [])
    connection.remoteObjectInterface = NSXPCInterface(with: LingXiCredentialBrokerXPC.self)
    // On macOS 13+, NSXPCConnection evaluates this against the peer's audit
    // token, avoiding PID-reuse races in client-side broker authentication.
    connection.setCodeSigningRequirement(
        requirementString(teamId: teamId, identifier: BrokerSecurity.brokerBundleId(channel: channel))
    )
    connection.resume()
    let semaphore = DispatchSemaphore(value: 0)
    let replyLock = NSLock()
    var completed = false
    var replyData: Data?
    var replyError: Error?
    let proxy = connection.remoteObjectProxyWithErrorHandler { error in
        replyLock.lock()
        guard !completed else { replyLock.unlock(); return }
        completed = true
        replyError = error
        replyLock.unlock()
        semaphore.signal()
    } as? LingXiCredentialBrokerXPC
    guard let proxy else {
        connection.invalidate()
        throw BrokerFailure.unavailable("credential broker proxy is unavailable")
    }
    proxy.perform(requestData as NSData) { responseData, errorText in
        replyLock.lock()
        guard !completed else { replyLock.unlock(); return }
        completed = true
        if let errorText {
            replyError = BrokerFailure.unavailable(errorText as String)
        } else {
            replyData = responseData as Data?
        }
        replyLock.unlock()
        semaphore.signal()
    }
    let waitResult = semaphore.wait(timeout: .now() + 15)
    // A timeout or invalidate may race a reply on XPC's private queue. Close
    // the result under the same lock before invalidating; late callbacks must
    // not mutate data that the calling thread is reading.
    replyLock.lock()
    completed = true
    let capturedError = replyError
    let capturedData = replyData
    replyLock.unlock()
    connection.invalidate()
    guard waitResult == .success else {
        throw BrokerFailure.unavailable("credential broker XPC request timed out")
    }
    if let capturedError {
        throw capturedError
    }
    guard let replyData = capturedData else {
        throw BrokerFailure.unavailable("credential broker did not return a response")
    }
    guard replyData.count <= maxBrokerMessageBytes else {
        throw BrokerFailure.unavailable("credential broker response exceeds the size limit")
    }
    return replyData
}

#if !CREDENTIAL_BROKER_TESTING
@main
struct CredentialClientEntry {
    static func main() {
        do {
            let teamId = try currentTeamIdentifier()
            let channel = try authorizedParentChannel(teamId: teamId)
            try validateStaticCode(
                at: URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath(),
                expectedIdentifier: BrokerSecurity.clientIdentifier(channel: channel),
                teamId: teamId,
                action: "validate credential broker client"
            )
            let packaged = try loadPackagedBrokerResources()
            guard packaged.manifest.channel == channel else {
                throw BrokerFailure.permission("credential broker package channel does not match its caller")
            }
            _ = try installOrResolveBroker(packaged: packaged, teamId: teamId)
            let request = try readRequestFromStdin()
            let reply = try performXpcCall(
                requestData: request,
                teamId: teamId,
                channel: channel
            )
            FileHandle.standardOutput.write(reply)
        } catch let failure as BrokerFailure {
            let data = try! JSONEncoder().encode(failure.response)
            FileHandle.standardOutput.write(data)
            exit(0)
        } catch {
            let response = BrokerFailure.internalError(error.localizedDescription).response
            let data = try! JSONEncoder().encode(response)
            FileHandle.standardOutput.write(data)
            exit(0)
        }
    }
}

#endif
