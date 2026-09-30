// Health-only authorization probe. This executable never sends credential operations.
import Foundation

@objc protocol LingXiCredentialBrokerXPC {
    func perform(_ requestData: NSData, withReply reply: @escaping (NSData?, NSString?) -> Void)
}

private final class ProbeResult: @unchecked Sendable {
    enum Outcome {
        case healthy
        case rejected(Int)
        case invalidReply
    }
    private let lock = NSLock()
    private var value: Outcome?
    let ready = DispatchSemaphore(value: 0)

    func finish(_ outcome: Outcome) {
        lock.lock()
        guard value == nil else { lock.unlock(); return }
        value = outcome
        lock.unlock()
        ready.signal()
    }

    func read() -> Outcome? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

@main
private struct CredentialBrokerXpcProbeMain {
    static func main() {
        guard CommandLine.arguments.count == 2,
              ["--expect-allowed", "--expect-rejected"].contains(CommandLine.arguments[1]) else {
            print("usage: probe --expect-allowed|--expect-rejected")
            exit(2)
        }
        let expectAllowed = CommandLine.arguments[1] == "--expect-allowed"
        let start = DispatchTime.now().uptimeNanoseconds
        let result = ProbeResult()
        let connection = NSXPCConnection(
            machServiceName: "com.lingxi.code.credential-broker.development", options: []
        )
        connection.remoteObjectInterface = NSXPCInterface(with: LingXiCredentialBrokerXPC.self)
        // Authenticate the server even for negative client-identity probes.
        connection.setCodeSigningRequirement(
            #"anchor apple generic and certificate leaf[subject.OU] = "AZ4AX7J833" and identifier "com.lingxi.code.credential-broker.development""#
        )
        connection.invalidationHandler = {
            result.finish(.rejected(NSXPCConnectionInvalid))
        }
        connection.resume()
        let proxy = connection.remoteObjectProxyWithErrorHandler { error in
            let error = error as NSError
            print("XPC error domain=\(error.domain) code=\(error.code)")
            // macOS may surface a peer requirement refusal as 4097. This code
            // alone cannot distinguish authorization from service failure:
            // run negative identities between successful allowed probes.
            if error.domain == NSCocoaErrorDomain,
               [NSXPCConnectionInterrupted, NSXPCConnectionInvalid, NSXPCConnectionCodeSigningRequirementFailure].contains(error.code) {
                result.finish(.rejected(error.code))
            } else {
                result.finish(.invalidReply)
            }
        } as? LingXiCredentialBrokerXPC
        guard let proxy else {
            connection.invalidate()
            print("FAIL: proxy unavailable")
            exit(1)
        }
        proxy.perform(Data(#"{"op":"health"}"#.utf8) as NSData) { data, error in
            guard error == nil, let data,
                  let object = try? JSONSerialization.jsonObject(with: data as Data) as? [String: Any],
                  object["ok"] as? Bool == true,
                  object["protocol_version"] as? Int == 1 else {
                result.finish(.invalidReply)
                return
            }
            result.finish(.healthy)
        }
        let completed = result.ready.wait(timeout: .now() + 30) == .success
        // Read before invalidating: our own teardown must never count as rejection.
        let outcome = completed ? result.read() : nil
        connection.invalidate()
        let elapsed = Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000_000
        switch outcome {
        case .healthy:
            print("\(expectAllowed ? "PASS" : "FAIL"): health accepted elapsed=\(elapsed)")
            exit(expectAllowed ? 0 : 1)
        case .rejected(let code):
            print("\(expectAllowed ? "FAIL" : "PASS"): XPC refused or invalidated code=\(code) elapsed=\(elapsed)")
            exit(expectAllowed ? 1 : 0)
        case .invalidReply:
            print("FAIL: unexpected XPC response elapsed=\(elapsed)")
            exit(1)
        case nil:
            print("FAIL: XPC timed out elapsed=\(elapsed)")
            exit(1)
        }
    }
}
