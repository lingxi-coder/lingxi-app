import Foundation
import XCTest

@testable import LingxiCode

/// The model picker's two pure pieces: the search predicate and the recents
/// store. Both are deliberately free of SwiftUI so they can be tested directly.
final class ModelPickerTests: XCTestCase {
    private let catalog = [
        "anthropic/claude-sonnet-5",
        "anthropic/claude-opus-4-8",
        "openai/gpt-5.5",
        "deepseek/deepseek-v4-flash",
        "openrouter/openrouter/auto",
    ]

    // MARK: filter

    func testBlankQueryKeepsEveryReferenceInEngineOrder() {
        XCTAssertEqual(ModelDisplay.filter(catalog, matching: ""), catalog)
        XCTAssertEqual(ModelDisplay.filter(catalog, matching: "   "), catalog)
    }

    func testFilterMatchesFriendlyNameWireIdAndProviderName() {
        // Friendly display name ("Claude Sonnet 5"), not present in the wire id.
        XCTAssertEqual(
            ModelDisplay.filter(catalog, matching: "sonnet"),
            ["anthropic/claude-sonnet-5"])
        // Bare wire id.
        XCTAssertEqual(
            ModelDisplay.filter(catalog, matching: "gpt-5.5"),
            ["openai/gpt-5.5"])
        // Provider display name — "DeepSeek" is the label for provider id
        // "deepseek", and matching it must pull in that provider's models.
        XCTAssertEqual(
            ModelDisplay.filter(catalog, matching: "DeepSeek"),
            ["deepseek/deepseek-v4-flash"])
    }

    func testFilterIsCaseInsensitiveAndPreservesOrder() {
        XCTAssertEqual(
            ModelDisplay.filter(catalog, matching: "CLAUDE"),
            ["anthropic/claude-sonnet-5", "anthropic/claude-opus-4-8"])
    }

    func testFilterReturnsEmptyWhenNothingMatches() {
        XCTAssertEqual(ModelDisplay.filter(catalog, matching: "llama"), [])
    }

    /// An OpenRouter wire id contains a slash of its own, so the qualified
    /// reference has two. Searching for the bare id must still find it.
    func testFilterFindsAnAggregatorModelWhoseWireIdContainsASlash() {
        XCTAssertEqual(
            ModelDisplay.filter(catalog, matching: "openrouter/auto"),
            ["openrouter/openrouter/auto"])
    }

    /// Filtering feeds `sections(for:)`, so a provider whose models all filter
    /// out must disappear along with them.
    func testFilteredReferencesStillGroupByProvider() {
        let sections = ModelDisplay.sections(
            for: ModelDisplay.filter(catalog, matching: "claude"))
        XCTAssertEqual(sections.map(\.providerId), ["anthropic"])
        XCTAssertEqual(sections.first?.models.count, 2)
    }

    // MARK: recents

    private func makeRecents() throws -> (ModelRecents, UserDefaults, String) {
        let suite = "model-recents-tests-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        return (ModelRecents(defaults: defaults), defaults, suite)
    }

    func testRecordMovesTheNewestPickToTheFront() throws {
        let (recents, defaults, suite) = try makeRecents()
        defer { defaults.removePersistentDomain(forName: suite) }

        recents.record("openai/gpt-5.5")
        recents.record("anthropic/claude-sonnet-5")

        XCTAssertEqual(
            recents.references(),
            ["anthropic/claude-sonnet-5", "openai/gpt-5.5"])
    }

    func testRecordingAnExistingReferenceMovesItInsteadOfDuplicating() throws {
        let (recents, defaults, suite) = try makeRecents()
        defer { defaults.removePersistentDomain(forName: suite) }

        recents.record("openai/gpt-5.5")
        recents.record("anthropic/claude-sonnet-5")
        recents.record("openai/gpt-5.5")

        XCTAssertEqual(
            recents.references(),
            ["openai/gpt-5.5", "anthropic/claude-sonnet-5"])
    }

    func testRecentsAreCappedAtTheLimitDroppingTheOldest() throws {
        let (recents, defaults, suite) = try makeRecents()
        defer { defaults.removePersistentDomain(forName: suite) }

        let picks = (0...ModelRecents.limit).map { "p\($0)/m\($0)" }
        picks.forEach(recents.record)

        XCTAssertEqual(recents.references().count, ModelRecents.limit)
        XCTAssertEqual(recents.references().first, picks.last)
        XCTAssertFalse(
            recents.references().contains(picks[0]),
            "the oldest pick must be forgotten")
    }

    func testBlankReferencesAreIgnored() throws {
        let (recents, defaults, suite) = try makeRecents()
        defer { defaults.removePersistentDomain(forName: suite) }

        recents.record("")
        recents.record("   ")

        XCTAssertEqual(recents.references(), [])
    }

    func testRecentsPersistAcrossInstances() throws {
        let (recents, defaults, suite) = try makeRecents()
        defer { defaults.removePersistentDomain(forName: suite) }

        recents.record("anthropic/claude-sonnet-5")

        XCTAssertEqual(
            ModelRecents(defaults: defaults).references(),
            ["anthropic/claude-sonnet-5"])
    }

    /// A remembered model whose provider was removed is no longer offered by the
    /// engine, so it must not be rendered as an unselectable row.
    func testResolvedDropsReferencesTheEngineNoLongerOffers() throws {
        let (recents, defaults, suite) = try makeRecents()
        defer { defaults.removePersistentDomain(forName: suite) }

        recents.record("deepseek/deepseek-v4-flash")
        recents.record("anthropic/claude-sonnet-5")

        XCTAssertEqual(
            recents.resolved(against: ["anthropic/claude-sonnet-5", "openai/gpt-5.5"]),
            ["anthropic/claude-sonnet-5"])
        XCTAssertEqual(recents.resolved(against: []), [])
    }

    /// `resolved` reports recency order, not the catalog's order.
    func testResolvedKeepsRecencyOrderNotCatalogOrder() throws {
        let (recents, defaults, suite) = try makeRecents()
        defer { defaults.removePersistentDomain(forName: suite) }

        recents.record("openai/gpt-5.5")
        recents.record("deepseek/deepseek-v4-flash")

        XCTAssertEqual(
            recents.resolved(against: catalog),
            ["deepseek/deepseek-v4-flash", "openai/gpt-5.5"])
    }
}
