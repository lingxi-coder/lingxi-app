import Foundation
import SwiftUI

struct ConversationSlashCommand: Identifiable, Equatable, Hashable {
    let name: String
    let description: String
    let aliases: [String]
    let argumentHint: String?
    let menuDescription: String?
    let source: String
    let hidden: Bool

    var id: String { name }
    var canonicalTrigger: String { "/\(name)" }

    func matchesExactToken(_ token: String) -> Bool {
        token == name || aliases.contains(token)
    }

    func matchedAlias(for token: String) -> String? {
        aliases.first(where: { $0 == token })
    }
}

struct SlashCommandSuggestion: Identifiable, Equatable {
    enum MatchTier: Int, Comparable {
        case exactName = 0
        case exactAlias = 1
        case namePrefix = 2
        case aliasPrefix = 3
        case fuzzyName = 4
        case fuzzyDescription = 5

        static func < (lhs: MatchTier, rhs: MatchTier) -> Bool {
            lhs.rawValue < rhs.rawValue
        }
    }

    let command: ConversationSlashCommand
    let tier: MatchTier
    let matchedAlias: String?

    var id: String { command.id }
}

enum SlashCommandMatcher {
    static func exactCommand(
        in text: String,
        catalog: [ConversationSlashCommand]
    ) -> ConversationSlashCommand? {
        guard let token = commandToken(in: text) else { return nil }
        return catalog.first(where: { $0.matchesExactToken(token) })
    }

    static func argumentHint(
        for text: String,
        catalog: [ConversationSlashCommand]
    ) -> String? {
        guard hasArgumentBoundary(in: text),
              let token = commandToken(in: text),
              let command = catalog.first(where: { $0.matchesExactToken(token) })
        else { return nil }
        let hint = command.argumentHint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return hint.isEmpty ? nil : hint
    }

    static func suggestions(
        for text: String,
        catalog: [ConversationSlashCommand]
    ) -> [SlashCommandSuggestion] {
        guard let token = commandToken(in: text), !hasArgumentBoundary(in: text) else {
            return []
        }

        if token.isEmpty {
            return catalog
                .filter { !$0.hidden }
                .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
                .map {
                    SlashCommandSuggestion(command: $0, tier: .namePrefix, matchedAlias: nil)
                }
        }

        return catalog.compactMap { command in
            let exactAlias = command.matchedAlias(for: token)
            let tier: SlashCommandSuggestion.MatchTier?

            if command.name == token {
                tier = .exactName
            } else if exactAlias != nil {
                tier = .exactAlias
            } else if command.name.hasPrefix(token) {
                tier = .namePrefix
            } else if let alias = command.aliases.first(where: { $0.hasPrefix(token) }) {
                return makeSuggestion(command: command, tier: .aliasPrefix, matchedAlias: alias)
            } else if command.name.localizedStandardContains(token) {
                tier = .fuzzyName
            } else if searchableText(for: command).localizedStandardContains(token) {
                tier = .fuzzyDescription
            } else {
                tier = nil
            }

            guard let tier else { return nil }
            if command.hidden, tier != .exactName, tier != .exactAlias {
                return nil
            }
            return SlashCommandSuggestion(command: command, tier: tier, matchedAlias: exactAlias)
        }
        .sorted { lhs, rhs in
            if lhs.tier != rhs.tier { return lhs.tier < rhs.tier }
            return lhs.command.name.localizedStandardCompare(rhs.command.name) == .orderedAscending
        }
    }

    static func shouldAcceptSuggestion(
        for text: String,
        catalog: [ConversationSlashCommand]
    ) -> Bool {
        guard text.first == "/" else { return false }
        guard !hasArgumentBoundary(in: text) else { return false }
        return exactCommand(in: text, catalog: catalog) == nil
    }

    static func commandToken(in text: String) -> String? {
        guard text.first == "/" else { return nil }
        let commandSlice = text.dropFirst().prefix { !$0.isWhitespace && !$0.isNewline }
        return String(commandSlice)
    }

    static func hasArgumentBoundary(in text: String) -> Bool {
        guard text.first == "/" else { return false }
        return text.dropFirst().contains(where: { $0.isWhitespace || $0.isNewline })
    }

    private static func searchableText(for command: ConversationSlashCommand) -> String {
        [command.description, command.menuDescription, command.aliases.joined(separator: " ")]
            .compactMap { $0 }
            .joined(separator: " ")
    }

    private static func makeSuggestion(
        command: ConversationSlashCommand,
        tier: SlashCommandSuggestion.MatchTier,
        matchedAlias: String?
    ) -> SlashCommandSuggestion {
        SlashCommandSuggestion(
            command: command,
            tier: tier,
            matchedAlias: matchedAlias
        )
    }
}

struct SlashCommandSuggestionPanel: View {
    @Environment(\.theme) private var t
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    let suggestions: [SlashCommandSuggestion]
    let isLoading: Bool
    let selectedID: String?
    let onSelect: (ConversationSlashCommand) -> Void

    var body: some View {
        Group {
            if isLoading {
                HStack(spacing: 8) {
                    ProgressView()
                        .tint(t.accent)
                    Text(String(localized: "skills_loading_state"))
                        .font(.subheadline)
                        .foregroundStyle(t.text3)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
            } else if !suggestions.isEmpty {
                ScrollView {
                    LazyVStack(spacing: 2) {
                        ForEach(suggestions) { suggestion in
                            suggestionButton(suggestion)
                        }
                    }
                    .padding(6)
                }
                // A ScrollView has no useful intrinsic height. With only a
                // max-height constraint, the chat/composer VStack can compress
                // it to almost zero as soon as the keyboard appears. Give the
                // palette an explicit, content-aware viewport so several rows
                // remain visible while longer catalogs still scroll.
                .frame(height: suggestionListHeight)
                .layoutPriority(1)
                .scrollIndicators(.visible)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("composer.slash-command-list")
            }
        }
        .padding(.horizontal, 10)
        .padding(.top, 8)
        .padding(.bottom, 4)
        .background(t.windowBg)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("slash_commands_title")
    }

    private var suggestionListHeight: CGFloat {
        let rowHeight: CGFloat = dynamicTypeSize.isAccessibilitySize ? 112 : 76
        let minimumHeight: CGFloat = dynamicTypeSize.isAccessibilitySize ? 280 : 240
        let maximumHeight: CGFloat = dynamicTypeSize.isAccessibilitySize ? 420 : 360
        let contentHeight = CGFloat(suggestions.count) * rowHeight + 12
        return min(max(contentHeight, minimumHeight), maximumHeight)
    }

    private func suggestionButton(_ suggestion: SlashCommandSuggestion) -> some View {
        let command = suggestion.command
        let isSelected = selectedID == command.id
        let description = command.menuDescription ?? command.description

        return Button {
            onSelect(command)
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        Text(command.canonicalTrigger)
                            .font(.body.monospaced().weight(.semibold))
                            .foregroundStyle(isSelected ? .white : t.text)
                        if let matchedAlias = suggestion.matchedAlias, matchedAlias != command.name {
                            Text("/\(matchedAlias)")
                                .font(.caption.monospaced())
                                .foregroundStyle(isSelected ? .white.opacity(0.82) : t.accent)
                        }
                    }
                    if !description.isEmpty {
                        Text(description)
                            .font(.subheadline)
                            .foregroundStyle(isSelected ? .white.opacity(0.88) : t.text3)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                    }
                }
                Spacer(minLength: 0)
                if let hint = command.argumentHint,
                   !hint.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    Text(hint)
                        .font(.caption.monospaced())
                        .foregroundStyle(isSelected ? .white : t.text)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(isSelected ? t.accent : t.surface)
            .clipShape(.rect(cornerRadius: 12))
            .overlay {
                RoundedRectangle(cornerRadius: 12)
                    .stroke(isSelected ? t.accent : t.borderStrong, lineWidth: 0.5)
            }
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(commandAccessibilityLabel(command))
        .accessibilityHint("slash_command_select_hint")
        .accessibilityIdentifier("composer.slash-command.\(command.name)")
    }

    private func commandAccessibilityLabel(_ command: ConversationSlashCommand) -> String {
        let description = command.menuDescription ?? command.description
        let aliases = command.aliases.isEmpty
            ? ""
            : String(localized: "slash_command_aliases") + ": " + command.aliases.joined(separator: ", ")
        return [command.canonicalTrigger, description, aliases]
            .filter { !$0.isEmpty }
            .joined(separator: ", ")
    }
}

struct ConversationCommandOutput: Identifiable, Equatable {
    let id: String
    let command: String
    let text: String
    let isError: Bool
}

struct SlashCommandOutputCard: View {
    @Environment(\.theme) private var t

    let output: ConversationCommandOutput

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                LXIcon(
                    name: output.isError ? .warning : .workflow,
                    size: 12,
                    color: output.isError ? t.danger : t.accent,
                    stroke: 1.8
                )
                Text(output.command)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(t.text)
                    .textSelection(.enabled)
                Spacer(minLength: 0)
            }

            if !output.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                Text(output.text)
                    .font(.system(size: 12.5))
                    .foregroundStyle(output.isError ? t.danger : t.text2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background((output.isError ? t.danger.tint(0.08) : t.surface.opacity(0.72)))
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay {
            RoundedRectangle(cornerRadius: 14)
                .stroke((output.isError ? t.danger : t.borderStrong).opacity(0.45), lineWidth: 0.5)
        }
        .padding(.bottom, 14)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("slash_command_output_title")
        .accessibilityIdentifier("conversation.slash-command-output")
    }
}
