import Foundation

final class DelayedService: NSObject, LingXiCredentialBrokerXPC {
    var mutations = 0
    func perform(_ requestData: NSData, withReply reply: @escaping (NSData?, NSString?) -> Void) {
        let op = try! JSONDecoder().decode(BrokerRequest.self, from: requestData as Data).op
        let delay: Double = op == "health" ? 16 : (op == "late" ? 0.2 : 0)
        if op != "health" { mutations += 1 }
        DispatchQueue.global().asyncAfter(deadline: .now() + delay) {
            reply(Data(#"{"ok":true,"protocol_version":1,"build_version":"delayed"}"#.utf8) as NSData, nil)
        }
    }
}

final class TestListener: NSObject, NSXPCListenerDelegate {
    let service = DelayedService()
    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection connection: NSXPCConnection) -> Bool {
        connection.exportedInterface = NSXPCInterface(with: LingXiCredentialBrokerXPC.self)
        connection.exportedObject = service
        connection.resume()
        return true
    }
}

@main
struct ReadinessRegression {
    static func main() throws {
        let mutation = Data(#"{"op":"delete","service":"example","account":"example"}"#.utf8)
        let health = Data(#"{"op":"health"}"#.utf8)
        let ready = Data(#"{"ok":true,"protocol_version":1,"build_version":"test"}"#.utf8)
        let result = Data(#"{"ok":true,"protocol_version":1}"#.utf8)
        var calls: [(String, TimeInterval)] = []
        let reply = try performReadyBrokerRequest(requestData: mutation) { data, timeout in
            let op = try JSONDecoder().decode(BrokerRequest.self, from: data).op
            calls.append((op, timeout))
            return op == "health" ? ready : result
        }
        precondition(reply == result && calls.map(\.0) == ["health", "delete"])
        precondition(calls.map(\.1) == [60, 15])
        var count = 0
        _ = try performReadyBrokerRequest(requestData: health) { _, _ in count += 1; return ready }
        precondition(count == 1)
        for response in [Data(#"{"ok":false}"#.utf8), Data(#"{"ok":true,"protocol_version":2}"#.utf8), Data("bad-json".utf8)] {
            count = 0
            do {
                _ = try performReadyBrokerRequest(requestData: mutation) { _, _ in count += 1; return response }
                preconditionFailure("invalid readiness must reject")
            } catch { precondition(count == 1) }
        }
        count = 0
        do {
            _ = try performReadyBrokerRequest(requestData: mutation) { _, _ in
                count += 1
                throw BrokerFailure.unavailable("simulated readiness timeout")
            }
            preconditionFailure("readiness timeout must reject")
        } catch { precondition(count == 1) }
        count = 0
        do {
            _ = try performReadyBrokerRequest(requestData: mutation) { _, _ in
                count += 1
                if count == 1 { return ready }
                throw BrokerFailure.unavailable("simulated ambiguous operation timeout")
            }
            preconditionFailure("operation timeout must reject")
        } catch { precondition(count == 2) }
        // Real XPC callback delay exceeds the old 15s operation deadline.
        // This endpoint is test-only; the signed package exercises peer identity.
        let delegate = TestListener()
        let listener = NSXPCListener.anonymous()
        listener.delegate = delegate
        listener.resume()
        let connection = NSXPCConnection(listenerEndpoint: listener.endpoint)
        connection.remoteObjectInterface = NSXPCInterface(with: LingXiCredentialBrokerXPC.self)
        connection.resume()
        defer { connection.invalidate(); listener.invalidate() }
        _ = try performReadyBrokerRequest(requestData: mutation) { data, timeout in
            try exchangeXpcRequest(connection: connection, requestData: data, timeout: timeout)
        }
        precondition(delegate.service.mutations == 1)
        // An exchange that has timed out must ignore its late callback, even
        // when a later exchange on the endpoint has already completed.
        do {
            _ = try exchangeXpcRequest(connection: connection,
                requestData: Data(#"{"op":"late"}"#.utf8), timeout: 0.01)
            preconditionFailure("delayed exchange must time out")
        } catch {}
        let subsequent = try exchangeXpcRequest(connection: connection, requestData: mutation, timeout: 1)
        let subsequentReply = try JSONDecoder().decode(BrokerResponse.self, from: subsequent)
        precondition(subsequentReply.ok)
        Thread.sleep(forTimeInterval: 0.3)
        print("readiness gates operations and never replays mutations")
    }
}
