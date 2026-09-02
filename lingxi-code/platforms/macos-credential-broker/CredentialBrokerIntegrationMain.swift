import Foundation

private struct IntegrationRequest: Codable {
    let op: String
    let service: String?
    let account: String?
    let payload: String?
}

private struct IntegrationResponse: Codable {
    let ok: Bool
    let present: Bool?
    let payload: String?
    let protocolVersion: Int?
    let errorKind: String?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case ok
        case present
        case payload
        case protocolVersion = "protocol_version"
        case errorKind = "error_kind"
        case error
    }
}

private func rawCallClient(_ client: URL, request: IntegrationRequest) throws -> IntegrationResponse {
    let process = Process()
    process.executableURL = client
    let input = Pipe()
    let output = Pipe()
    let errors = Pipe()
    process.standardInput = input
    process.standardOutput = output
    process.standardError = errors
    try process.run()
    try input.fileHandleForWriting.write(contentsOf: JSONEncoder().encode(request))
    try input.fileHandleForWriting.close()
    process.waitUntilExit()
    let responseData = output.fileHandleForReading.readDataToEndOfFile()
    guard process.terminationStatus == 0 else {
        throw NSError(domain: "LingXiCredentialBrokerIntegration", code: Int(process.terminationStatus))
    }
    return try JSONDecoder().decode(IntegrationResponse.self, from: responseData)
}

private func callClient(_ client: URL, request: IntegrationRequest) throws -> IntegrationResponse {
    let response = try rawCallClient(client, request: request)
    guard response.ok, response.protocolVersion == 1 else {
        throw NSError(
            domain: "LingXiCredentialBrokerIntegration",
            code: 2,
            userInfo: [NSLocalizedDescriptionKey: response.error ?? "broker request failed"]
        )
    }
    return response
}

@main
private struct CredentialBrokerIntegrationMain {
    static func main() throws {
        guard (2...4).contains(CommandLine.arguments.count) else {
            throw NSError(
                domain: "LingXiCredentialBrokerIntegration",
                code: 3,
                userInfo: [NSLocalizedDescriptionKey: "expected credential client path and optional channel"]
            )
        }
        let client = URL(fileURLWithPath: CommandLine.arguments[1])
        let channel = CommandLine.arguments.count >= 3 ? CommandLine.arguments[2] : "production"
        guard channel == "production" || channel == "development" else {
            throw NSError(domain: "LingXiCredentialBrokerIntegration", code: 8)
        }
        if CommandLine.arguments.count == 4 {
            guard CommandLine.arguments[3] == "--expect-rejected" else {
                throw NSError(domain: "LingXiCredentialBrokerIntegration", code: 9)
            }
            let response = try rawCallClient(client, request: IntegrationRequest(
                op: "health", service: nil, account: nil, payload: nil
            ))
            guard !response.ok, response.errorKind == "permission" else {
                throw NSError(domain: "LingXiCredentialBrokerIntegration", code: 10)
            }
            print("unauthorized credential broker caller rejected")
            return
        }
        let service = channel == "production"
            ? "com.lingxi.provider-credentials.v1"
            : "com.lingxi.provider-credentials.v1.development"
        let account = "integration-test-\(ProcessInfo.processInfo.processIdentifier)"
        let firstSecret = "integration-\(UUID().uuidString)"
        let replacementSecret = "replacement-\(UUID().uuidString)"
        let request: (String, String?) -> IntegrationRequest = { operation, payload in
            IntegrationRequest(op: operation, service: service, account: account, payload: payload)
        }

        _ = try callClient(client, request: IntegrationRequest(
            op: "health", service: nil, account: nil, payload: nil
        ))
        defer { _ = try? callClient(client, request: request("delete", nil)) }

        _ = try callClient(client, request: request("store", firstSecret))
        let firstRead = try callClient(client, request: request("retrieve", nil))
        guard firstRead.present == true, firstRead.payload == firstSecret else {
            throw NSError(domain: "LingXiCredentialBrokerIntegration", code: 4)
        }

        _ = try callClient(client, request: request("store", replacementSecret))
        let replacementRead = try callClient(client, request: request("retrieve", nil))
        guard replacementRead.payload == replacementSecret else {
            throw NSError(domain: "LingXiCredentialBrokerIntegration", code: 5)
        }
        let preview = try callClient(client, request: request("preview", nil))
        guard preview.payload == "••••\(replacementSecret.suffix(4))" else {
            throw NSError(domain: "LingXiCredentialBrokerIntegration", code: 6)
        }

        _ = try callClient(client, request: request("delete", nil))
        let missing = try callClient(client, request: request("retrieve", nil))
        guard missing.present == false, missing.payload == nil else {
            throw NSError(domain: "LingXiCredentialBrokerIntegration", code: 7)
        }
        print("signed Data Protection Keychain integration passed")
    }
}
