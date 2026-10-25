import SwiftUI

// MARK: - First-run welcome
//
// A single welcome page. Entering the app only updates `setupDone`.

struct SetupWizardView: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var t

    var body: some View {
        ZStack {
            t.windowBg.ignoresSafeArea()

            VStack(spacing: 0) {
                GeometryReader { viewport in
                    ScrollView {
                        bodyContent
                            .padding(.horizontal, 30)
                            .padding(.vertical, 16)
                            .frame(
                                maxWidth: .infinity,
                                minHeight: viewport.size.height,
                                alignment: .center
                            )
                    }
                    .accessibilityIdentifier("onboarding.content")
                }
                footer
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
    }

    // MARK: footer — primary CTA
    private var footer: some View {
        Button(action: finish) {
            Label(String(localized: "onboarding_cta_finish"), systemImage: "arrow.forward")
                .font(.body.weight(.semibold))
                .frame(maxWidth: .infinity, minHeight: 32)
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.large)
        .tint(t.accent)
        .accessibilityIdentifier("onboarding.primaryAction")
        .padding(.horizontal, 30)
        .padding(.top, 14)
        .padding(.bottom, 16)
    }

    private var bodyContent: some View {
        VStack(spacing: 0) {
            badge(.sparkles)
            wizH(String(localized: "onboarding_welcome_title"))
            wizSub(String(localized: "onboarding_welcome_subtitle"))
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("onboarding.welcome")
    }

    // MARK: small building blocks
    private func badge(_ icon: LXIconName) -> some View {
        LXIcon(name: icon, size: 30, color: .white, stroke: 1.9)
            .frame(width: 66, height: 66)
            .background(t.accent)
            .clipShape(RoundedRectangle(cornerRadius: 20))
            .padding(.bottom, 22)
    }
    private func wizH(_ text: String) -> some View {
        Text(text).font(.title2.weight(.bold))
            .foregroundStyle(t.text)
            .multilineTextAlignment(.center).padding(.bottom, 10)
    }
    private func wizSub(_ text: String) -> some View {
        Text(text).font(.subheadline).lineSpacing(4)
            .foregroundStyle(t.text2)
            .multilineTextAlignment(.center).frame(maxWidth: 290).padding(.bottom, 26)
    }

    private func finish() {
        app.setupDone = true
    }
}
