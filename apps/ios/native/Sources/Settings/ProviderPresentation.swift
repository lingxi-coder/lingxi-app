import SwiftUI

struct ProviderBrandMark: View {
    let providerID: String
    let name: String
    var size: CGFloat = 32
    var tint: Color = .primary

    private var asset: String? {
        switch providerID {
        case "anthropic": "claude"
        case "deepseek": "deepseek"
        case "google", "gemini": "gemini"
        case "github-copilot": "githubcopilot"
        case "kimi", "kimi-code": "kimi"
        case "minimax", "qwen": providerID
        case "openai", "openai-chatgpt": "openai"
        case "openrouter": "openrouter"
        case "xai": "xai"
        case "zai", "glm-coding": "zai"
        default: nil
        }
    }

    var body: some View {
        Group {
            if let asset {
                Image("provider_brand_\(asset)")
                    .resizable()
                    .scaledToFit()
                    .frame(width: size * 0.65, height: size * 0.65)
                    .foregroundStyle(Color.primary)
            } else {
                Text(String(name.prefix(1)))
                    .font(.footnote.weight(.bold))
                    .foregroundStyle(tint)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

struct ProviderInfoButton: View {
    let message: String
    @State private var expanded = false

    var body: some View {
        Button { expanded = true } label: {
            Image(systemName: "info.circle").padding(6)
        }
        .buttonStyle(.borderless)
        .accessibilityLabel(Text("session_details_button"))
        .popover(isPresented: $expanded) {
            ScrollView {
                Text(message)
                    .font(.subheadline)
                    .padding()
                    .frame(maxWidth: 300, alignment: .leading)
            }
            .scrollBounceBehavior(.basedOnSize)
            .presentationCompactAdaptation(.popover)
        }
    }
}

extension View {
    func providerMessage(_ message: Binding<String?>, identifier: String = "") -> some View {
        alert("provider_accounts_title", isPresented: Binding(
            get: { message.wrappedValue != nil },
            set: { if !$0 { message.wrappedValue = nil } }
        ), presenting: message.wrappedValue) { _ in
            Button("common_ok", role: .cancel) {}
        } message: { text in
            Text(text).accessibilityIdentifier(identifier)
        }
    }
}
