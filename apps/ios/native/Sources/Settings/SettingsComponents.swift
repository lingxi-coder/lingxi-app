import SwiftUI

// MARK: - Reusable settings building blocks (Section / Row / RadioList)

/// Grouped inset card with optional label + footer note.
struct SettingsSection<Content: View>: View {
    @Environment(\.theme) private var t
    var label: String? = nil
    var footer: String? = nil
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let label {
                Text(label)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 16).padding(.bottom, 8)
            }
            VStack(spacing: 0) { content }
                .background(Color(uiColor: .secondarySystemGroupedBackground))
                .clipShape(.rect(cornerRadius: 16))
            if let footer {
                Text(footer).font(.footnote).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true).padding(.horizontal, 16).padding(.top, 8)
            }
        }
        .padding(.bottom, 22)
    }
}

/// A settings list row with separate semantics for interactive and static content:
/// optional leading icon tile, title, sub, trailing value/accessory, chevron.
struct SettingsRow<Trailing: View>: View {
    @Environment(\.theme) private var t
    @Environment(\.dynamicTypeSize) private var typeSize
    var icon: LXIconName? = nil
    var iconColor: Color? = nil
    var label: String = ""
    var labelView: AnyView? = nil
    var sub: String? = nil
    var subView: AnyView? = nil
    var value: String? = nil
    var valueColor: Color? = nil
    var chevron: Bool = true
    var danger: Bool = false
    var isLast: Bool = false
    var onTap: (() -> Void)? = nil
    var trailing: Trailing

    init(icon: LXIconName? = nil, iconColor: Color? = nil, label: String = "",
         labelView: AnyView? = nil, sub: String? = nil, subView: AnyView? = nil,
         value: String? = nil, valueColor: Color? = nil, chevron: Bool = true,
         danger: Bool = false, isLast: Bool = false, onTap: (() -> Void)? = nil,
         @ViewBuilder trailing: () -> Trailing) {
        self.icon = icon; self.iconColor = iconColor; self.label = label
        self.labelView = labelView; self.sub = sub; self.subView = subView
        self.value = value; self.valueColor = valueColor; self.chevron = chevron
        self.danger = danger; self.isLast = isLast; self.onTap = onTap
        self.trailing = trailing()
    }

    @ViewBuilder
    var body: some View {
        if let onTap {
            Button(action: onTap) { content }
                .buttonStyle(SettingsRowButtonStyle(pressedFill: t.surfaceHover))
        } else {
            content
        }
    }

    private var content: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                if let icon {
                    let c = iconColor ?? t.accent
                    Image(systemName: icon.settingsSystemImage)
                        .font(.body.weight(.medium)).foregroundStyle(.white)
                        .frame(width: 29, height: 29)
                        .background(c, in: RoundedRectangle(cornerRadius: 7))
                        .accessibilityHidden(true)
                }
                let layout = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: 8))
                    : AnyLayout(HStackLayout(spacing: 8))
                layout {
                    VStack(alignment: .leading, spacing: 4) {
                        if let labelView { labelView }
                        else {
                            Text(label).font(.body)
                                .foregroundStyle(danger ? Color.red : Color.primary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        if let subView { subView }
                        else if let sub {
                            Text(sub).font(.footnote).foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    HStack(spacing: 8) {
                        if let value {
                            Text(value).font(.body).foregroundStyle(valueColor ?? Color.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        trailing
                            .environment(\.settingsControlLabel, label.isEmpty ? (sub ?? "") : label)
                    }
                }
                if chevron && onTap != nil {
                    Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary).accessibilityHidden(true)
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            .frame(minHeight: 48)
            if !isLast { Divider().padding(.leading, icon == nil ? 16 : 57) }
        }
        .contentShape(Rectangle())
    }
}

private struct SettingsRowButtonStyle: ButtonStyle {
    let pressedFill: Color

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(configuration.isPressed ? pressedFill : .clear)
    }
}

extension SettingsRow where Trailing == EmptyView {
    init(icon: LXIconName? = nil, iconColor: Color? = nil, label: String = "",
         labelView: AnyView? = nil, sub: String? = nil, subView: AnyView? = nil,
         value: String? = nil, valueColor: Color? = nil, chevron: Bool = true,
         danger: Bool = false, isLast: Bool = false, onTap: (() -> Void)? = nil) {
        self.icon = icon; self.iconColor = iconColor; self.label = label
        self.labelView = labelView; self.sub = sub; self.subView = subView
        self.value = value; self.valueColor = valueColor; self.chevron = chevron
        self.danger = danger; self.isLast = isLast; self.onTap = onTap
        self.trailing = EmptyView()
    }
}

/// Single-select radio list (check on the right of the selected row).
struct RadioOption: Identifiable { let value: String; let label: String; var sub: String? = nil
    var id: String { value } }

struct RadioList: View {
    @Environment(\.theme) private var t
    let options: [RadioOption]
    @Binding var value: String

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(options.enumerated()), id: \.element.id) { i, o in
                Button { value = o.value } label: {
                    VStack(spacing: 0) {
                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(o.label).font(.body).foregroundStyle(.primary)
                                if let sub = o.sub { Text(sub).font(.footnote).foregroundStyle(.secondary) }
                            }
                            Spacer()
                            if value == o.value { Image(systemName: "checkmark").font(.body.weight(.semibold)).foregroundStyle(t.accent) }
                        }
                        .padding(.horizontal, 14).padding(.vertical, 12)
                        if i < options.count - 1 { Divider().padding(.leading, 16) }
                    }
                }
                .buttonStyle(SettingsRowButtonStyle(pressedFill: t.surfaceHover))
                .accessibilityAddTraits(value == o.value ? [.isSelected] : [])
            }
        }
        .background(Color(uiColor: .secondarySystemGroupedBackground))
        .clipShape(RoundedRectangle(cornerRadius: 12))

    }
}

/// A consistent native add action used by collection editors.
struct DashedAddButton: View {
    @Environment(\.theme) private var t
    let title: String
    var action: () -> Void = {}
    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: "plus.circle.fill")
                Text(title)
            }
            .font(.body)
            .foregroundStyle(t.accent)
            .frame(maxWidth: .infinity, minHeight: 44)
            .padding(.vertical, 4)
            .background(t.surface, in: .rect(cornerRadius: 12))
        }
        .buttonStyle(.plain)
    }
}

/// Standard text-field style used across edit pages (monospaced).
struct SettingsField: View {
    @Environment(\.theme) private var t
    @Binding var text: String
    var placeholder: String = ""
    var mono: Bool = true
    var secure: Bool = false
    var trailingPadding: CGFloat = 12

    var body: some View {
        Group {
            if secure {
                SecureField(placeholder, text: $text)
            } else {
                TextField(placeholder, text: $text)
            }
        }
        .font(.system(.body, design: mono ? .monospaced : .default))
        .foregroundColor(t.text)
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .padding(.vertical, 11).padding(.leading, 12).padding(.trailing, trailingPadding)
        .background(Color(uiColor: .secondarySystemGroupedBackground))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
    }
}

struct FieldLabel: View {
    @Environment(\.theme) private var t
    let text: String
    var body: some View {
        Text(text).font(.subheadline).foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.bottom, 6)
    }
}

struct FieldHint: View {
    @Environment(\.theme) private var t
    let text: AttributedString
    init(_ s: String) { self.text = (try? AttributedString(markdown: s)) ?? AttributedString(s) }
    var body: some View {
        Text(text).font(.footnote).foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 4).padding(.bottom, 14)
    }
}

private extension LXIconName {
    var settingsSystemImage: String {
        switch self {
        case .pin: "person.crop.circle.fill"
        case .sun: "sun.max.fill"
        case .mic: "mic.fill"
        case .globe: "globe"
        case .search: "magnifyingglass"
        case .wrench: "wrench.and.screwdriver.fill"
        case .link: "link"
        case .dream: "moon.stars.fill"
        case .message: "bubble.left.fill"
        case .pencil: "pencil"
        case .check: "checkmark.shield.fill"
        case .clock: "clock.fill"
        case .folder: "folder.fill"
        case .plug: "puzzlepiece.extension.fill"
        case .sparkle: "sparkles"
        case .cog: "gearshape.fill"
        case .skill: "square.stack.3d.up.fill"
        case .workflow: "arrow.triangle.branch"
        case .listChecks: "checklist"
        case .book: "book.fill"
        default: "slider.horizontal.3"
        }
    }
}
