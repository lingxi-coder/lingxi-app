import SwiftUI

// MARK: - Reusable settings building blocks (Section / Row / RadioList)

/// Grouped inset card with optional uppercase label + footer note.
struct SettingsSection<Content: View>: View {
    @Environment(\.theme) private var t
    var label: String? = nil
    var footer: String? = nil
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let label {
                Text(label.uppercased())
                    .font(.system(size: 11, weight: .semibold)).tracking(0.6)
                    .foregroundColor(t.text4)
                    .padding(.horizontal, 4).padding(.bottom, 8)
            }
            VStack(spacing: 0) { content }
                .background(t.surface)
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
            if let footer {
                Text(footer).font(.system(size: 11)).foregroundColor(t.text4)
                    .lineSpacing(4).padding(.horizontal, 4).padding(.top, 8)
            }
        }
        .padding(.bottom, 22)
    }
}

/// A settings list row. Mirrors the prototype's `<Row>` exactly:
/// optional leading icon tile, title, sub, trailing value/accessory, chevron.
struct SettingsRow<Trailing: View>: View {
    @Environment(\.theme) private var t
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

    var body: some View {
        Button(action: { onTap?() }) { content }
            .buttonStyle(.plain)
            .disabled(onTap == nil)
    }

    private var content: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                if let icon {
                    let c = iconColor ?? t.accent
                    LXIcon(name: icon, size: 14, color: c, stroke: 1.8)
                        .frame(width: 28, height: 28)
                        .background(c.mix(with: t.surface, amount: 0.16))
                        .clipShape(RoundedRectangle(cornerRadius: 7))
                        .overlay(RoundedRectangle(cornerRadius: 7).stroke(c.tint(0.28), lineWidth: 0.5))
                }
                VStack(alignment: .leading, spacing: 2) {
                    if let labelView { labelView }
                    else {
                        Text(label).font(.system(size: 14, weight: .medium))
                            .foregroundColor(danger ? t.danger : t.text).lineLimit(1)
                    }
                    if let subView { subView }
                    else if let sub {
                        Text(sub).font(.system(size: 11.5)).foregroundColor(t.text4).lineLimit(1)
                    }
                }
                Spacer(minLength: 4)
                if let value {
                    Text(value).font(.system(size: 13)).foregroundColor(valueColor ?? t.text3).lineLimit(1)
                }
                trailing
                if chevron && onTap != nil {
                    LXIcon(name: .chevronR, size: 13, color: t.text4)
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 12)
            if !isLast { Rectangle().fill(t.border).frame(height: 0.5) }
        }
        .contentShape(Rectangle())
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
                                Text(o.label).font(.system(size: 14, weight: .medium)).foregroundColor(t.text)
                                if let sub = o.sub { Text(sub).font(.system(size: 11.5)).foregroundColor(t.text4) }
                            }
                            Spacer()
                            if value == o.value { LXIcon(name: .check, size: 16, color: t.accent, stroke: 2.5) }
                        }
                        .padding(.horizontal, 14).padding(.vertical, 12)
                        if i < options.count - 1 { Rectangle().fill(t.border).frame(height: 0.5) }
                    }
                }
                .buttonStyle(.plain)
            }
        }
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
    }
}

/// A dashed "+ add …" full-width button.
struct DashedAddButton: View {
    @Environment(\.theme) private var t
    let title: String
    var action: () -> Void = {}
    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                LXIcon(name: .plus, size: 15, color: t.text2, stroke: 2)
                Text(title).font(.system(size: 13.5, weight: .medium)).foregroundColor(t.text2)
            }
            .frame(maxWidth: .infinity).padding(13)
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, style: StrokeStyle(lineWidth: 1, dash: [4,3])))
        }
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
        .font(.system(size: 13.5, design: mono ? .monospaced : .default))
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
        Text(text).font(.system(size: 12, weight: .medium)).foregroundColor(t.text3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.bottom, 6)
    }
}

struct FieldHint: View {
    @Environment(\.theme) private var t
    let text: AttributedString
    init(_ s: String) { self.text = (try? AttributedString(markdown: s)) ?? AttributedString(s) }
    var body: some View {
        Text(text).font(.system(size: 10.5)).foregroundColor(t.text4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 4).padding(.bottom, 14)
    }
}
