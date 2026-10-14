@testable import LingxiCode
import XCTest

final class PluginCatalogTests: XCTestCase {
    private let json = """
    {
      "installed": [
        {"id": "local-app-builder@lingxi", "name": "local-app-builder", "display_name": "Local App Builder",
         "version": "2.1.0", "path": "/home/.lingxi/plugins/cache/lingxi/local-app-builder/2.1.0",
         "default_enabled": true, "description": "Build apps", "dependencies": [], "config_schema_json": "null"},
        {"id": "quiet@acme", "name": "quiet", "display_name": "", "version": "1.0.0", "path": "/p",
         "default_enabled": false, "description": "", "dependencies": ["base@acme"], "config_schema_json": "null"},
        {"id": "", "name": "broken"}
      ],
      "available": [
        {"id": "local-app-builder@lingxi", "name": "local-app-builder", "marketplace": "lingxi", "version": "",
         "description": "Build apps", "installed": true, "upgrade_available": false},
        {"id": "notes@acme", "name": "notes", "marketplace": "acme", "version": "0.3.0",
         "description": "Notes", "installed": false, "upgrade_available": false}
      ],
      "marketplaces": [
        {"name": "lingxi", "source_json": "{\\"source\\":\\"github\\",\\"repo\\":\\"lingxi-coder/local-app-builder\\"}",
         "install_location": "/home/.lingxi/plugins/marketplaces/lingxi", "last_updated": null},
        {"name": "acme", "source_json": "{\\"source\\":\\"directory\\",\\"path\\":\\"/srv/acme\\"}", "install_location": null}
      ],
      "enabled_json": "{\\"local-app-builder@lingxi\\": false}",
      "revisions": {"user": "abc", "project": "def"}
    }
    """

    func testParsesTheEngineRows() {
        let catalog = PluginCatalog.parse(json)
        XCTAssertEqual(catalog.installed.map(\.id), ["local-app-builder@lingxi", "quiet@acme"])
        // A blank display name falls back to the plugin's name.
        XCTAssertEqual(catalog.installed("quiet@acme")?.displayName, "quiet")
        XCTAssertEqual(catalog.installed("quiet@acme")?.dependencies, ["base@acme"])
        XCTAssertEqual(catalog.installed("local-app-builder@lingxi")?.marketplace, "lingxi")
        XCTAssertEqual(catalog.revisions, ["user": "abc", "project": "def"])
        XCTAssertEqual(catalog.installable.map(\.id), ["notes@acme"])
    }

    func testAnExplicitSettingWinsOverTheManifestDefault() throws {
        var catalog = PluginCatalog.parse(json)
        XCTAssertFalse(catalog.isEnabled(try XCTUnwrap(catalog.installed("local-app-builder@lingxi"))))
        XCTAssertFalse(catalog.isEnabled(try XCTUnwrap(catalog.installed("quiet@acme"))))
        catalog.enabled = ["quiet@acme": true]
        XCTAssertTrue(catalog.isEnabled(try XCTUnwrap(catalog.installed("quiet@acme"))))
    }

    func testMarketplacesSayWhereTheyComeFrom() throws {
        let catalog = PluginCatalog.parse(json)
        let lingxi = try XCTUnwrap(catalog.marketplace("lingxi"))
        XCTAssertEqual(lingxi.sourceKind, "github")
        XCTAssertEqual(lingxi.sourceLocation, "lingxi-coder/local-app-builder")
        XCTAssertEqual(catalog.marketplace("acme")?.sourceLocation, "/srv/acme")
        XCTAssertNil(catalog.marketplace("missing"))
    }

    func testMissingOrMalformedJSONIsAnEmptyCatalog() {
        XCTAssertEqual(PluginCatalog.parse(nil), PluginCatalog())
        XCTAssertEqual(PluginCatalog.parse("not json"), PluginCatalog())
    }

    func testTheLingxiMarketplaceIsAddedOnceAndNeverBroughtBack() {
        let empty = PluginCatalog()
        XCTAssertEqual(LingxiMarketplace.sourceToAdd(empty, seen: false, providerRegion: "international"), LingxiMarketplace.githubSource)
        // No mirror for mainland China yet: GitHub serves every region.
        XCTAssertEqual(LingxiMarketplace.sourceToAdd(empty, seen: false, providerRegion: "china_mainland"), LingxiMarketplace.githubSource)
        // Already registered, or registered once and removed by the user since.
        XCTAssertNil(LingxiMarketplace.sourceToAdd(PluginCatalog.parse(json), seen: false, providerRegion: nil))
        XCTAssertNil(LingxiMarketplace.sourceToAdd(empty, seen: true, providerRegion: nil))
    }
}

final class PluginDetailsTests: XCTestCase {
    private var root: URL!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appending(path: "plugin-details-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    private func write(_ path: String, _ text: String) throws {
        let file = root.appending(path: path)
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: file)
    }

    func testReadsTheStandardLayout() throws {
        try write(
            "plugin/.lingxi-plugin/plugin.json",
            #"{"name":"p","author":{"name":"LingXi"},"license":"MIT","repository":{"url":"https://example.test/p"},"commands":["./extra"],"agents":"../outside"}"#
        )
        try write("plugin/skills/create-app/SKILL.md", "---\nname: create-local-app\ndescription: >\n  Plan and build\n  a local app.\n---\nBody")
        try write("plugin/skills/notes.md", "not a skill")
        try write("plugin/commands/build.md", "---\ndescription: \"Build it\"\n---\n")
        try write("plugin/extra/nested/ship.md", "no frontmatter")
        try write("plugin/hooks/hooks.json", #"{"hooks":{"PreToolUse":[{"matcher":"Bash"}],"Stop":[]}}"#)
        try write("plugin/.mcp.json", #"{"mcpServers":{"files":{"command":"x"}}}"#)
        try write("outside/evil.md", "---\nname: evil\n---\n")

        let details = PluginDetailsReader.read(pluginDir: root.appending(path: "plugin"))

        XCTAssertEqual(details.vendor, "LingXi")
        XCTAssertEqual(details.license, "MIT")
        XCTAssertEqual(details.repository, "https://example.test/p")
        XCTAssertEqual(details.skills, [PluginComponent(name: "create-local-app", description: "Plan and build a local app.")])
        XCTAssertEqual(details.commands, [PluginComponent(name: "build", description: "Build it"), PluginComponent(name: "ship", description: "")])
        // A declared path that leaves the plugin is ignored.
        XCTAssertEqual(details.agents, [])
        XCTAssertEqual(details.hooks, ["PreToolUse", "Stop"])
        XCTAssertEqual(details.mcpServers, ["files"])
    }

    func testTheVendorFallsBackToTheMarketplaceOwner() throws {
        try write("plugin/.lingxi-plugin/plugin.json", #"{"name":"p"}"#)
        try write("market/.lingxi-plugin/marketplace.json", #"{"name":"lingxi","owner":{"name":"LingXi"}}"#)
        let plugin = root.appending(path: "plugin")
        XCTAssertEqual(PluginDetailsReader.vendor(pluginDir: plugin, marketplaceDir: root.appending(path: "market")), "LingXi")
        XCTAssertNil(PluginDetailsReader.vendor(pluginDir: plugin))
        // A string author counts too.
        try write("plugin/.lingxi-plugin/plugin.json", #"{"name":"p","author":"Acme"}"#)
        XCTAssertEqual(PluginDetailsReader.vendor(pluginDir: plugin, marketplaceDir: root.appending(path: "market")), "Acme")
    }

    func testFrontmatterHandlesQuotesAndBlockScalars() {
        let fields = frontmatter("---\nname: 'it''s'\ndescription: |\n  line one\n  line two\nother: plain\n---\nbody: no")
        XCTAssertEqual(fields, ["name": "it's", "description": "line one\nline two", "other": "plain"])
        XCTAssertEqual(frontmatter("no frontmatter"), [:])
    }
}
