import Foundation

final class FakeLaunchd {
    var registered = false
    var failNextBootstrap = false
    var failNextKickstart = false
    var calls: [[String]] = []

    func run(_ arguments: [String]) throws {
        calls.append(arguments)
        switch arguments[0] {
        case "print":
            if !registered { throw BrokerFailure.unavailable("fake service is not registered") }
        case "bootout":
            registered = false
        case "bootstrap":
            if failNextBootstrap {
                failNextBootstrap = false
                throw BrokerFailure.unavailable("injected bootstrap failure")
            }
            precondition(!registered, "bootstrap attempted on a registered service")
            registered = true
        case "kickstart":
            precondition(registered)
            if failNextKickstart {
                failNextKickstart = false
                throw BrokerFailure.unavailable("injected kickstart failure")
            }
        default:
            preconditionFailure("unexpected launchctl command")
        }
    }

    func count(_ command: String) -> Int { calls.filter { $0[0] == command }.count }
}

@main
struct CredentialRegistrationRegression {
    static func main() throws {
        let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let launchd = FakeLaunchd()
        let service = "com.lingxi.fake-broker.development"
        var executable = directory.appendingPathComponent("fake-broker").path
        func register(reinstall: Bool = false) throws {
            try registerBrokerLaunchAgent(
                machService: service,
                executablePath: executable,
                launchAgentsDirectory: directory,
                reinstall: reinstall,
                launchctl: launchd.run
            )
        }

        launchd.failNextBootstrap = true
        do {
            try register()
            preconditionFailure("bootstrap failure was hidden")
        } catch let failure as BrokerFailure {
            guard case .unavailable = failure else { throw failure }
        }
        let plist = directory.appendingPathComponent("\(service).plist")
        precondition(FileManager.default.fileExists(atPath: plist.path))
        precondition(!launchd.registered)
        try register()
        precondition(launchd.registered && launchd.count("bootstrap") == 2,
                     "matching plist prevented retry after failed bootstrap")

        let callsBeforeHealthyRetry = launchd.calls.count
        try register()
        precondition(launchd.calls.count == callsBeforeHealthyRetry + 1,
                     "healthy retry restarted the broker")
        precondition(launchd.calls.last?[0] == "print")

        // Same on-disk installation, new login or externally removed service.
        launchd.registered = false
        try register()
        precondition(launchd.registered && launchd.count("bootstrap") == 3)

        executable += "-new"
        try register()
        precondition(launchd.count("bootout") == 1 && launchd.count("bootstrap") == 4,
                     "changed configuration did not replace registered service")
        let changedPlist = try String(contentsOf: plist, encoding: .utf8)
        precondition(changedPlist.contains(executable))

        launchd.failNextKickstart = true
        do {
            try register(reinstall: true)
            preconditionFailure("kickstart failure was hidden")
        } catch let failure as BrokerFailure {
            guard case .unavailable = failure else { throw failure }
        }
        let bootstrapsAfterKickstartFailure = launchd.count("bootstrap")
        try register()
        precondition(launchd.count("bootstrap") == bootstrapsAfterKickstartFailure,
                     "already registered on-demand service was unnecessarily re-bootstrapped")
        // A prior installation may still be registered with the old background
        // clamp. Replace it once so on-demand XPC work can receive caller QoS.
        let adaptivePlist = try String(contentsOf: plist, encoding: .utf8)
        precondition(adaptivePlist.contains("<key>ProcessType</key><string>Adaptive</string>"))
        let backgroundPlist = adaptivePlist.replacingOccurrences(
            of: "<key>ProcessType</key><string>Adaptive</string>",
            with: "<key>ProcessType</key><string>Background</string>"
        )
        try backgroundPlist.write(to: plist, atomically: true, encoding: .utf8)
        let bootoutsBeforeUpgrade = launchd.count("bootout")
        let bootstrapsBeforeUpgrade = launchd.count("bootstrap")
        try register()
        precondition(launchd.count("bootout") == bootoutsBeforeUpgrade + 1)
        precondition(launchd.count("bootstrap") == bootstrapsBeforeUpgrade + 1)
        let upgradedPlist = try String(contentsOf: plist, encoding: .utf8)
        precondition(upgradedPlist == adaptivePlist)
        let callsBeforeReuse = launchd.calls.count
        try register()
        precondition(launchd.calls.count == callsBeforeReuse + 1,
                     "adaptive service was restarted on every call")
        print("registration retry, login recovery, configuration replacement and healthy reuse checks passed")
    }
}
