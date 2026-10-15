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
                    .foregroundColor(t.text4)
                    .padding(.horizontal, 16).padding(.bottom, 8)
            }
            VStack(spacing: 0) { content }
                .background(t.surface)
                .clipShape(.rect(cornerRadius: 16))
            if let footer {
                Text(footer).font(.footnote).foregroundColor(t.text4)
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
                    LXIcon(name: icon, size: 14, color: c, stroke: 1.8)
                        .frame(width: 32, height: 32)
                        .background(c.mix(with: t.surface, amount: 0.16))
                        .clipShape(.rect(cornerRadius: 9))
                }
                let layout = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: 8))
                    : AnyLayout(HStackLayout(spacing: 8))
                layout {
                    VStack(alignment: .leading, spacing: 4) {
                        if let labelView { labelView }
                        else {
                            Text(label).font(.body)
                                .foregroundStyle(danger ? t.danger : t.text)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        if let subView { subView }
                        else if let sub {
                            Text(sub).font(.footnote).foregroundStyle(t.text2)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    HStack(spacing: 8) {
                        if let value {
                            Text(value).font(.body).foregroundStyle(valueColor ?? t.text2)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        trailing
                            .environment(\.settingsControlLabel, label.isEmpty ? (sub ?? "") : label)
                    }
                }
                if chevron && onTap != nil {
                    LXIcon(name: .chevronR, size: 13, color: t.text4)
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            .frame(minHeight: 48)
            if !isLast { Divider().padding(.leading, icon == nil ? 16 : 60) }
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
                                Text(o.label).font(.body).foregroundColor(t.text)
                                if let sub = o.sub { Text(sub).font(.footnote).foregroundColor(t.text4) }
                            }
                            Spacer()
                            if value == o.value { LXIcon(name: .check, size: 16, color: t.accent, stroke: 2.5) }
                        }
                        .padding(.horizontal, 14).padding(.vertical, 12)
                        if i < options.count - 1 { Divider().padding(.leading, 16) }
                    }
                }
                .buttonStyle(SettingsRowButtonStyle(pressedFill: t.surfaceHover))
                .accessibilityAddTraits(value == o.value ? [.isSelected] : [])
            }
        }
        .background(t.surface)
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
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
    }
}

struct FieldLabel: View {
    @Environment(\.theme) private var t
    let text: String
    var body: some View {
        Text(text).font(.subheadline).foregroundColor(t.text3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.bottom, 6)
    }
}

struct FieldHint: View {
    @Environment(\.theme) private var t
    let text: AttributedString
    init(_ s: String) { self.text = (try? AttributedString(markdown: s)) ?? AttributedString(s) }
    var body: some View {
        Text(text).font(.footnote).foregroundColor(t.text4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 4).padding(.bottom, 14)
    }
}
