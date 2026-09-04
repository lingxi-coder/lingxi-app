import Foundation
import Security

let brokerProtocolVersion = 1
let maxBrokerMessageBytes = 128 * 1024

struct BrokerManifest: Codable {
    let version: String
    let protocolVersion: Int
    let channel: String

    enum CodingKeys: String, CodingKey {
        case version
        case protocolVersion = "protocol_version"
        case channel
    }
}

enum BrokerSecurity {
    static let productionBrokerBundleId = "com.lingxi.code.credential-broker"
    static let brokerBundleName = "LingXiCredentialBroker.app"
    static let productionClientIdentifier = "com.lingxi.code.credential-client"
    static let brokerExecutable = "LingXiCredentialBroker"

    static func scopedIdentifier(_ productionIdentifier: String, channel: String) -> String {
        channel == "production" ? productionIdentifier : "\(productionIdentifier).development"
    }

    static func brokerBundleId(channel: String) -> String {
        scopedIdentifier(productionBrokerBundleId, channel: channel)
    }

    static func clientIdentifier(channel: String) -> String {
        scopedIdentifier(productionClientIdentifier, channel: channel)
    }

    static func machService(channel: String) -> String {
        brokerBundleId(channel: channel)
    }

    static func allowedCallerIdentifiers(channel: String) -> [String] {
        [
            "com.lingxi.code",
            "com.lingxi.code.cli",
        ].map { scopedIdentifier($0, channel: channel) }
    }
}

struct BrokerRequest: Codable {
    let op: String
    let service: String?
    let account: String?
    let payload: String?
}

struct BrokerResponse: Codable {
    let ok: Bool
    let present: Bool?
    let payload: String?
    let accounts: [String]?
    let protocolVersion: Int?
    let buildVersion: String?
    let errorKind: String?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case ok
        case present
        case payload
        case accounts
        case protocolVersion = "protocol_version"
        case buildVersion = "build_version"
        case errorKind = "error_kind"
        case error
    }
}

enum BrokerFailure: Error {
    case invalidRequest(String)
    case permission(String)
    case locked(String)
    case unavailable(String)
    case internalError(String)

    var response: BrokerResponse {
        let kind: String
        let message: String
        switch self {
        case .invalidRequest(let value):
            kind = "invalid_request"
            message = value
        case .permission(let value):
            kind = "permission"
            message = value
        case .locked(let value):
            kind = "locked"
            message = value
        case .unavailable(let value):
            kind = "unavailable"
            message = value
        case .internalError(let value):
            kind = "internal"
            message = value
        }
        return BrokerResponse(
            ok: false,
            present: nil,
            payload: nil,
            accounts: nil,
            protocolVersion: brokerProtocolVersion,
            buildVersion: nil,
            errorKind: kind,
            error: message
        )
    }
}

func successResponse(
    present: Bool? = nil,
    payload: String? = nil,
    accounts: [String]? = nil,
    buildVersion: String? = nil
) -> BrokerResponse {
    BrokerResponse(
        ok: true,
        present: present,
        payload: payload,
        accounts: accounts,
        protocolVersion: brokerProtocolVersion,
        buildVersion: buildVersion,
        errorKind: nil,
        error: nil
    )
}

func validateStorageComponent(_ raw: String, label: String) throws -> String {
    let trimmed = raw.trimmingCharacters(in: .newlines)
    guard !trimmed.isEmpty, trimmed.count <= 256, !trimmed.contains("\0") else {
        throw BrokerFailure.invalidRequest("invalid \(label)")
    }
    return trimmed
}

func loadManifest(from root: URL) throws -> BrokerManifest {
    let data = try Data(contentsOf: root.appendingPathComponent("broker-manifest.json"))
    let manifest = try JSONDecoder().decode(BrokerManifest.self, from: data)
    try validateManifest(manifest)
    return manifest
}

func loadManifestForBrokerBundle() throws -> BrokerManifest {
    guard let url = Bundle.main.url(forResource: "broker-manifest", withExtension: "json") else {
        throw BrokerFailure.unavailable("credential broker manifest is missing")
    }
    let data = try Data(contentsOf: url)
    let manifest = try JSONDecoder().decode(BrokerManifest.self, from: data)
    try validateManifest(manifest)
    return manifest
}

func requirementString(teamId: String, identifiers: [String]) -> String {
    let identityClause = identifiers
        .map { #"identifier "\#($0)""# }
        .joined(separator: " or ")
    return #"anchor apple generic and certificate leaf[subject.OU] = "\#(teamId)" and (\#(identityClause))"#
}

func requirementString(teamId: String, identifier: String) -> String {
    requirementString(teamId: teamId, identifiers: [identifier])
}

func currentTeamIdentifier() throws -> String {
    var selfCode: SecCode?
    let selfStatus = SecCodeCopySelf(SecCSFlags(), &selfCode)
    guard selfStatus == errSecSuccess, let selfCode else {
        throw BrokerFailure.permission(securityMessage(selfStatus, action: "resolve current code signature"))
    }
    var staticCode: SecStaticCode?
    let staticStatus = SecCodeCopyStaticCode(selfCode, SecCSFlags(), &staticCode)
    guard staticStatus == errSecSuccess, let staticCode else {
        throw BrokerFailure.permission(securityMessage(staticStatus, action: "resolve current static code"))
    }
    var info: CFDictionary?
    let infoStatus = SecCodeCopySigningInformation(
        staticCode,
        SecCSFlags(rawValue: kSecCSSigningInformation),
        &info
    )
    guard infoStatus == errSecSuccess, let info = info as? [String: Any] else {
        throw BrokerFailure.permission(securityMessage(infoStatus, action: "resolve TeamIdentifier"))
    }
    guard let teamId = info[kSecCodeInfoTeamIdentifier as String] as? String,
          !teamId.isEmpty else {
        throw BrokerFailure.permission("current code signature does not contain a TeamIdentifier")
    }
    return teamId
}

func validateManifest(_ manifest: BrokerManifest) throws {
    guard manifest.protocolVersion == brokerProtocolVersion else {
        throw BrokerFailure.unavailable("credential broker protocol version is incompatible")
    }
    if manifest.channel != "production" && manifest.channel != "development" {
        throw BrokerFailure.unavailable("credential broker channel is invalid")
    }
}

func validateProcessIdentifier(
    _ pid: pid_t,
    expectedIdentifiers: [String],
    teamId: String,
    action: String
) throws {
    let attributes = [kSecGuestAttributePid as String: NSNumber(value: pid)] as CFDictionary
    var code: SecCode?
    let guestStatus = SecCodeCopyGuestWithAttributes(nil, attributes, SecCSFlags(), &code)
    guard guestStatus == errSecSuccess, let code else {
        throw BrokerFailure.permission(securityMessage(guestStatus, action: action))
    }
    var requirement: SecRequirement?
    let requirementStatus = SecRequirementCreateWithString(
        requirementString(teamId: teamId, identifiers: expectedIdentifiers) as CFString,
        SecCSFlags(),
        &requirement
    )
    guard requirementStatus == errSecSuccess, let requirement else {
        throw BrokerFailure.internalError(securityMessage(requirementStatus, action: "compile \(action) requirement"))
    }
    let validityStatus = SecCodeCheckValidity(code, SecCSFlags(), requirement)
    guard validityStatus == errSecSuccess else {
        throw BrokerFailure.permission(securityMessage(validityStatus, action: action))
    }
}

func validateStaticCode(
    at url: URL,
    expectedIdentifier: String,
    teamId: String,
    action: String
) throws {
    var staticCode: SecStaticCode?
    let createStatus = SecStaticCodeCreateWithPath(url as CFURL, SecCSFlags(), &staticCode)
    guard createStatus == errSecSuccess, let staticCode else {
        throw BrokerFailure.permission(securityMessage(createStatus, action: action))
    }
    var requirement: SecRequirement?
    let requirementStatus = SecRequirementCreateWithString(
        requirementString(teamId: teamId, identifier: expectedIdentifier) as CFString,
        SecCSFlags(),
        &requirement
    )
    guard requirementStatus == errSecSuccess, let requirement else {
        throw BrokerFailure.internalError(securityMessage(requirementStatus, action: "compile \(action) requirement"))
    }
    let validityStatus = SecStaticCodeCheckValidity(staticCode, SecCSFlags(), requirement)
    guard validityStatus == errSecSuccess else {
        throw BrokerFailure.permission(securityMessage(validityStatus, action: action))
    }
}

func codeDirectoryHash(at url: URL) throws -> Data {
    var staticCode: SecStaticCode?
    let createStatus = SecStaticCodeCreateWithPath(url as CFURL, SecCSFlags(), &staticCode)
    guard createStatus == errSecSuccess, let staticCode else {
        throw BrokerFailure.permission(securityMessage(createStatus, action: "read code directory hash"))
    }
    var info: CFDictionary?
    let infoStatus = SecCodeCopySigningInformation(
        staticCode,
        SecCSFlags(rawValue: kSecCSSigningInformation),
        &info
    )
    guard infoStatus == errSecSuccess,
          let info = info as? [String: Any],
          let hash = info[kSecCodeInfoUnique as String] as? Data,
          !hash.isEmpty else {
        throw BrokerFailure.permission(securityMessage(infoStatus, action: "read code directory hash"))
    }
    return hash
}

func compareSemanticVersions(_ lhs: String, _ rhs: String) -> ComparisonResult {
    let left = parseSemanticVersion(lhs)
    let right = parseSemanticVersion(rhs)
    let count = max(left.count, right.count)
    for index in 0..<count {
        let leftValue = index < left.count ? left[index] : 0
        let rightValue = index < right.count ? right[index] : 0
        if leftValue < rightValue { return .orderedAscending }
        if leftValue > rightValue { return .orderedDescending }
    }
    return lhs.compare(rhs, options: .literal)
}

private func parseSemanticVersion(_ raw: String) -> [Int] {
    raw
        .split(separator: ".", omittingEmptySubsequences: false)
        .map { component in
            Int(component.prefix { $0.isNumber }) ?? 0
        }
}

func securityMessage(_ status: OSStatus, action: String) -> String {
    if let text = SecCopyErrorMessageString(status, nil) as String? {
        return "\(action) failed (\(status)): \(text)"
    }
    return "\(action) failed (\(status))"
}
