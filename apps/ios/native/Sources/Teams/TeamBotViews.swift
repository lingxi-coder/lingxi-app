import SwiftUI

struct TeamBotCardView: View {
    let bot: TeamBotCard
    let owner: String
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(bot.name, systemImage: "person.crop.square.fill").font(.headline)
            if !bot.description.isEmpty { Text(bot.description).font(.subheadline) }
            Text("Owner: \(owner)").font(.caption)
            if bot.enabled {
                Text(bot.online ? "Recently online · availability is not guaranteed" : "Offline · requests may wait for the owner's Desktop").font(.caption).foregroundStyle(.secondary)
                Text(bot.permitted ? "You can use this Bot" : "You do not have permission to use this Bot").font(.caption)
                Text(bot.use == "all" ? "Available to all team members" : "Available to selected team members").font(.caption).foregroundStyle(.secondary)
            } else {
                Text("Disabled · Rebind this Bot on LingXi Desktop to enable new requests.").font(.caption).foregroundStyle(.secondary)
                Text("Binding revision: \(bot.revision)").font(.caption).monospacedDigit()
            }
        }
    }
}

struct TeamBotQuestionView: View {
    let question: TeamBotQuestion
    let busy: Bool
    let submit: ([String: String]) -> Void
    @State private var answers: [String: String] = [:]
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("This question is for you, the person who started this request.").font(.caption)
            ForEach(question.questions, id: \.self) { prompt in
                Text(prompt).font(.subheadline)
                TextField("Your answer", text: Binding(get: { answers[prompt] ?? "" }, set: { answers[prompt] = $0 }), axis: .vertical)
                    .lineLimit(2...5).disabled(busy)
            }
            Button("Send answers") { submit(answers) }
                .disabled(busy || !TeamBotIntent.validAnswers(answers, for: question))
        }.accessibilityIdentifier("teams.bot.question.\(question.id)")
    }
}

struct TeamBotRecoveryView: View {
    let busy: Bool
    let submit: (String) -> Void
    @State private var text = ""
    var body: some View {
        DisclosureGroup("Settle expired Bot request") {
            Text("This records a final reply without running the Bot or its tools again. Check the owner's Desktop before settling an interrupted request.").font(.caption).foregroundStyle(.secondary)
            TextField("Final reply", text: $text, axis: .vertical).lineLimit(2...5).disabled(busy)
            Button("Record final reply") { submit(text) }
                .disabled(busy || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || text.utf8.count > 8192)
        }
    }
}
