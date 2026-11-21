import Foundation
import Network

/// Test-runner-only loopback adapters. Mac workers arrive via a paired USB
/// connection. Each listener has exactly one preconfigured staging service.
final class DataStagingUSB {
    private let queue = DispatchQueue(label: "data.staging.usb")
    private var listeners: [NWListener] = []
    private var pools: [Pool] = []

    private final class Pool {
        var clients: [NWConnection] = []
        var hosts: [NWConnection] = []
        var active: [UUID: (NWConnection, NWConnection)] = [:]
    }

    init(ports: [UInt16], secret: Data) throws {
        guard (3...5).contains(ports.count), secret.count == 32, Set(ports).count == ports.count else {
            throw CocoaError(.coderInvalidValue)
        }
        for (i, port) in ports.enumerated() {
            let pool = Pool()
            pools.append(pool)
            try listen(port, pool: pool, secret: nil)
            try listen(UInt16(61231 + i), pool: pool, secret: secret)
        }
    }

    private func listen(_ port: UInt16, pool: Pool, secret: Data?) throws {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!)
        let listener = try NWListener(using: parameters)
        listener.stateUpdateHandler = { [weak listener] state in
            switch state {
            case .ready: print("Data staging listener ready", port, listener?.port?.rawValue ?? 0)
            case .failed(let error): print("Data staging listener failed", port, error)
            default: break
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            guard let self else { connection.cancel(); return }
            connection.start(queue: queue)
            if let secret {
                connection.receive(minimumIncompleteLength: secret.count, maximumLength: secret.count) { data, _, _, error in
                    guard error == nil, data == secret, pool.hosts.count < 12 else { connection.cancel(); return }
                    connection.send(content: Data([65]), completion: .contentProcessed { error in
                        guard error == nil else { connection.cancel(); return }
                        pool.hosts.append(connection)
                        self.pair(pool)
                    })
                }
            } else if pool.clients.count < 24 {
                pool.clients.append(connection)
                self.pair(pool)
            } else { connection.cancel() }
        }
        listeners.append(listener)
        listener.start(queue: queue)
    }

    private func pair(_ pool: Pool) {
        while !pool.clients.isEmpty && !pool.hosts.isEmpty {
            let id = UUID(), client = pool.clients.removeFirst(), host = pool.hosts.removeFirst()
            pool.active[id] = (client, host)
            var ended = 0
            func done(_ failed: Bool) {
                ended += 1
                if failed || ended == 2 {
                    client.cancel(); host.cancel(); pool.active[id] = nil
                }
            }
            host.send(content: Data([82]), completion: .contentProcessed { error in
                guard error == nil else { done(true); return }
                self.pump(client, host, done: done)
                self.pump(host, client, done: done)
            })
        }
    }

    private func pump(_ from: NWConnection, _ to: NWConnection, done: @escaping (Bool) -> Void) {
        from.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { data, _, eof, error in
            if error != nil { done(true); return }
            to.send(content: data, contentContext: eof ? .finalMessage : .defaultMessage, isComplete: true,
                    completion: .contentProcessed { error in
                if error != nil || eof { done(error != nil) }
                else { self.pump(from, to, done: done) }
            })
        }
    }

    func close() {
        queue.sync {
            listeners.forEach { $0.cancel() }
            for pool in pools {
                (pool.hosts + pool.clients).forEach { $0.cancel() }
                for (client, host) in pool.active.values { client.cancel(); host.cancel() }
                pool.active.removeAll()
            }
        }
    }
}
