import SwiftUI

/// The composer's model picker.
///
/// A sheet rather than a popover anchored to the chip. The popover was sized by
/// the composer's own height proposal and had to measure its content to grow at
/// all; adding a search field made that worse, because focusing it raises the
/// keyboard over exactly the space the popover occupies. A sheet is laid out by
/// the system, so the keyboard and the list negotiate their own space and the
/// list can use the full screen — which the "recent" section needs.
///
/// Rows carry the provider-QUALIFIED reference. Picking one submits that
/// reference verbatim, so two providers exposing the same wire id stay distinct.
struct ModelPickerSheet: View {
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared

    /// The engine's curated references (`ClientEvent::ModelList.models`).
    let availableModels: [String]
    let detailsByReference: [String: ModelRuntimeDetails]
    /// The reference the engine reports as active, for the checkmark.
    let activeModelId: String
    /// References the user picked before, most-recent-first.
    let recentModels: [String]
    let onSelect: (String) -> Void
    let onDismiss: () -> Void

    @State private var query = ""
    @State private var selectedDetails: ModelRuntimeDetails?

    var body: some View {
        NavigationStack {
            List {
                if let emptyStateKey = Self.emptyStateKey(
                    query: query,
                    visibleModels: visibleModels,
                    matches: matches
                ) {
                    Text(LocalizedStringKey(emptyStateKey))
                        .font(.system(size: 13))
                        .foregroundStyle(t.text3)
                        .listRowBackground(Color.clear)
                } else {
                    if !recentMatches.isEmpty {
                        Section("composer_recent_models") {
                            ForEach(recentMatches, id: \.self) { row($0, slot: "recent") }
                        }
                    }
                    ForEach(sections) { section in
                        Section(section.name) {
                            ForEach(section.models.map(\.reference), id: \.self) {
                                row($0, slot: "provider")
                            }
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollDismissesKeyboard(.immediately)
            // `.always`, not the default: an automatically-placed search field
            // hides under the title until the list is dragged down, and a picker
            // whose whole point is "find your model" should not make the user
            // discover the search box by scrolling up.
            .searchable(
                text: $query,
                placement: .navigationBarDrawer(displayMode: .always),
                prompt: "composer_search_models_placeholder")
            .navigationTitle("composer_select_model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_close", action: onDismiss)
                }
            }
            .accessibilityIdentifier("composer.model.menu")
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .fullScreenCover(item: $selectedDetails) { details in
            ModelDetailsSheet(
                details: details,
                accent: ModelDisplay.color(for: details.reference)
            )
        }
    }

    /// The references surviving the search box, in engine order.
    private var visibleModels: [String] {
        repository.visibleModelReferences(availableModels)
    }

    /// The references surviving the search box, in engine order.
    private var matches: [String] {
        ModelDisplay.filter(
            visibleModels,
            matching: query,
            detailsByReference: detailsByReference
        )
    }

    static func emptyStateKey(
        query: String,
        visibleModels: [String],
        matches: [String]
    ) -> String? {
        guard matches.isEmpty else { return nil }
        return query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && visibleModels.isEmpty
            ? "composer_no_visible_models"
            : "composer_no_matching_models"
    }

    /// Provider sections built from the surviving references — filter first,
    /// then group, so a provider whose models all filter out disappears with
    /// them (same order of operations as the Android picker).
    private var sections: [ModelProviderSection] {
        ModelDisplay.sections(for: matches, detailsByReference: detailsByReference)
    }

    /// Recently-picked references that also survive the search, in recency
    /// order. These deliberately ALSO appear under their own provider below:
    /// hiding them there would make provider sections shuffle as the recents
    /// change, which is more disorienting than one repeated row.
    private var recentMatches: [String] {
        let surviving = Set(matches)
        return recentModels.filter { surviving.contains($0) }
    }

    /// One selectable row.
    ///
    /// `slot` distinguishes the recent copy of a model from the copy under its
    /// own provider. Both are real, tappable rows, so they cannot share an
    /// accessibility identifier — a UI-test query for a bare reference would
    /// match two elements and fail with "Multiple matching elements found".
    @ViewBuilder
    private func row(_ reference: String, slot: String) -> some View {
        let item = ModelDisplay.item(for: reference, detailsByReference: detailsByReference)
        HStack(spacing: 10) {
            Button {
                onSelect(reference)
            } label: {
                HStack(spacing: 10) {
                    Circle().fill(item.color).frame(width: 8, height: 8)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(item.name)
                            .font(.scaledSystem(15, weight: .medium, relativeTo: .body))
                            .foregroundStyle(t.text)
                        Text(item.details?.description ?? item.modelId)
                            .font(.scaledSystem(12, relativeTo: .caption))
                            .foregroundStyle(t.text3)
                            .lineLimit(1)
                        if let summary = item.details?.summaryItems, !summary.isEmpty {
                            Text(summary.joined(separator: " · "))
                                .font(.scaledSystem(11, relativeTo: .caption))
                                .foregroundStyle(t.text3)
                                .lineLimit(2)
                        }
                    }
                    Spacer(minLength: 8)
                    if reference == activeModelId {
                        LXIcon(name: .check, size: 13, color: t.accent, stroke: 2.5)
                    }
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("composer.model.\(slot).row.\(reference)")

            if item.details != nil {
                Button {
                    selectedDetails = item.details
                } label: {
                    Image(systemName: "info.circle")
                        .font(.scaledSystem(15, weight: .medium, relativeTo: .body))
                        .foregroundStyle(t.text3)
                        .frame(width: 44, height: 44)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("composer.model.\(slot).info.\(reference)")
            }
        }
    }
}
