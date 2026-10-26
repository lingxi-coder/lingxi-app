import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class PublicExtensionDirectoryTests: XCTestCase {
    func testParsesIOSItemsAndKeepsUnknownMCPConfigFields() throws {
        let json = """
        {
          "schemaVersion": 1,
          "items": [
            {
              "id": "context7",
              "kind": "mcp",
              "name": "Context7",
              "publisher": "Upstash",
              "category": "development",
              "platforms": ["ios", "macos"],
              "description": {"en": "Docs", "zh-Hans": "文档"},
              "requirements": {"en": "Optional API key"},
              "sourceUrl": "https://example.test/source",
              "documentationUrl": "https://example.test/docs",
              "mcp": {
                "name": "context7",
                "config": {
                  "type": "http",
                  "url": "https://example.test/mcp",
                  "headers": {"Authorization": "Bearer ${CONTEXT7_API_KEY}"},
                  "futureField": {"enabled": true, "values": [1, "two"]}
                }
              }
            },
            {
              "id": "desktop-only",
              "kind": "mcp",
              "name": "Desktop only",
              "publisher": "Example",
              "category": "browser",
              "platforms": ["macos"],
              "description": {"en": "Desktop"},
              "sourceUrl": "https://example.test/source",
              "documentationUrl": "https://example.test/docs",
              "mcp": {"name": "desktop-only", "config": {"command": "node"}}
            },
            {
              "id": "claude-plugins",
              "kind": "plugin",
              "name": "Claude Plugins",
              "publisher": "Anthropic",
              "category": "productivity",
              "platforms": ["ios"],
              "description": {"en": "Official directory"},
              "sourceUrl": "https://example.test/source",
              "documentationUrl": "https://example.test/docs",
              "marketplace": {"source": "anthropics/plugins", "name": "official", "plugin": "one"}
            }
          ]
        }
        """

        let directory = try XCTUnwrap(PublicExtensionDirectory.parse(Data(json.utf8)))
        XCTAssertEqual(directory.items.map(\.id), ["context7", "claude-plugins"])

        let mcp = try XCTUnwrap(directory.items.first)
        XCTAssertEqual(mcp.mcpName, "context7")
        XCTAssertEqual(mcp.localizedDescription(locale: Locale(identifier: "zh-Hans")), "文档")
        let config = try XCTUnwrap(mcp.mcpConfig)
        XCTAssertEqual(config["url"] as? String, "https://example.test/mcp")
        XCTAssertEqual(config["headers"] as? [String: String], ["Authorization": "Bearer ${CONTEXT7_API_KEY}"])
        let futureField = try XCTUnwrap(config["futureField"] as? [String: Any])
        XCTAssertEqual(futureField["enabled"] as? Bool, true)
        XCTAssertEqual((futureField["values"] as? [Any])?.count, 2)
        let reloadedConfig = try XCTUnwrap(DesktopSettingsRepository.object(DesktopSettingsRepository.json(config)))
        XCTAssertEqual(DesktopSettingsRepository.json(reloadedConfig), DesktopSettingsRepository.json(config))

        let plugin = try XCTUnwrap(directory.items.last)
        XCTAssertNil(plugin.mcpConfig)
        XCTAssertEqual(plugin.marketplace?.source, "anthropics/plugins")
        XCTAssertEqual(plugin.marketplace?.plugin, "one")
    }

    func testRejectsUnsupportedSchemaAndSkipsMalformedEntries() throws {
        XCTAssertNil(PublicExtensionDirectory.parse(Data(#"{"schemaVersion":2,"items":[]}"#.utf8)))

        let json = #"{"schemaVersion":1,"items":[{"id":"broken"}]}"#
        let directory = try XCTUnwrap(PublicExtensionDirectory.parse(Data(json.utf8)))
        XCTAssertTrue(directory.items.isEmpty)
    }

    func testMCPConfigurationMatchRequiresSameNameAndExactJSON() {
        let template: [String: Any] = ["type": "http", "url": "https://example.test/mcp"]
        XCTAssertEqual(
            publicMCPTemplateMatch(name: "context7", template: template, configuredServers: [:]),
            .notConfigured
        )
        XCTAssertEqual(
            publicMCPTemplateMatch(name: "context7", template: template, configuredServers: ["context7": template]),
            .sameConfiguration
        )
        XCTAssertEqual(
            publicMCPTemplateMatch(
                name: "context7",
                template: template,
                configuredServers: ["context7": ["type": "http", "url": "https://different.test/mcp"]]
            ),
            .nameConflict
        )
    }

    func testMarketplaceMatchUsesSourceIdentityAndReportsSameNameConflict() {
        let candidate = PublicExtensionMarketplace(source: "owner/repo", name: "official", plugin: nil)
        let sameGitHubRepository = PluginMarketplace(
            name: "official",
            sourceJSON: #"{"source":"github","repo":"owner/repo"}"#,
            installLocation: nil,
            lastUpdated: nil
        )
        XCTAssertEqual(publicMarketplaceRegistration(candidate, registered: [sameGitHubRepository]), .registered(name: "official"))

        let sameRepository = PluginMarketplace(
            name: "actual-name",
            sourceJSON: #"{"source":"git","url":"https://github.com/Owner/Repo.git"}"#,
            installLocation: nil,
            lastUpdated: nil
        )
        XCTAssertEqual(publicMarketplaceRegistration(candidate, registered: [sameRepository]), .registered(name: "actual-name"))

        let sameNameDifferentSource = PluginMarketplace(
            name: "official",
            sourceJSON: #"{"source":"github","repo":"different/repo"}"#,
            installLocation: nil,
            lastUpdated: nil
        )
        XCTAssertEqual(
            publicMarketplaceRegistration(candidate, registered: [sameNameDifferentSource]),
            .nameConflict(existingName: "official")
        )

        let sameNameDirectoryPath = PluginMarketplace(
            name: "official",
            sourceJSON: #"{"source":"directory","path":"owner/repo"}"#,
            installLocation: nil,
            lastUpdated: nil
        )
        XCTAssertEqual(
            publicMarketplaceRegistration(candidate, registered: [sameNameDirectoryPath]),
            .nameConflict(existingName: "official")
        )

        let sameNameGitHubRef = PluginMarketplace(
            name: "official",
            sourceJSON: #"{"source":"github","repo":"owner/repo","ref":"next"}"#,
            installLocation: nil,
            lastUpdated: nil
        )
        XCTAssertEqual(
            publicMarketplaceRegistration(candidate, registered: [sameNameGitHubRef]),
            .nameConflict(existingName: "official")
        )

        let sameNameSparseGit = PluginMarketplace(
            name: "official",
            sourceJSON: #"{"source":"git","url":"https://github.com/owner/repo.git","sparsePaths":["plugins"]}"#,
            installLocation: nil,
            lastUpdated: nil
        )
        XCTAssertEqual(
            publicMarketplaceRegistration(candidate, registered: [sameNameSparseGit]),
            .nameConflict(existingName: "official")
        )

        let sameNameHostedURL = PluginMarketplace(
            name: "official",
            sourceJSON: #"{"source":"url","url":"https://github.com/owner/repo"}"#,
            installLocation: nil,
            lastUpdated: nil
        )
        XCTAssertEqual(
            publicMarketplaceRegistration(candidate, registered: [sameNameHostedURL]),
            .nameConflict(existingName: "official")
        )
        XCTAssertEqual(publicMarketplaceRegistration(candidate, registered: []), .notRegistered)
    }
}
