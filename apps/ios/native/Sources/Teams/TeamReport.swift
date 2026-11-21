import Foundation
import SwiftUI

struct TeamReportLink: Identifiable, Hashable {
    let dataset: String
    let release: String
    var id: String { dataset + "/" + release }

    init?(_ text: String) {
        let parts = text.split(separator: "/", omittingEmptySubsequences: false)
        guard parts.count == 3, parts[0] == "lingxi-data:release",
              parts[1].wholeMatch(of: #/[a-zA-Z0-9_-]{1,64}/#) != nil,
              parts[2].wholeMatch(of: #/[a-f0-9]{64}/#) != nil else { return nil }
        dataset = String(parts[1])
        release = String(parts[2])
    }
}

struct TeamPublishedReport: Decodable {
    struct Spec: Decodable { let title: String }
    struct Point: Decodable { let group: String; let value: String }
    struct Cell: Decodable, Hashable { let partition: String; let group: String; let value: String }
    let dataset: String
    let release: String
    let spec: Spec
    let canReview: Bool
    let points: [Point]
    let cells: [Cell]
}

struct TeamReportSheet: View {
    let link: TeamReportLink
    let conversation: String
    let model: TeamMessagingModel
    @Environment(\.dismiss) private var dismiss
    @State private var latest = false
    @State private var report: TeamPublishedReport?
    @State private var status: LocalizedStringKey?
    @State private var sending = false

    var body: some View {
        NavigationStack {
            List {
                Picker("Report version", selection: $latest) {
                    Text("Published version").tag(false)
                    Text("Latest version").tag(true)
                }.pickerStyle(.segmented).disabled(sending)
                if let report {
                    let maxValue = report.points.compactMap { Double($0.value).map(abs) }.max() ?? 0
                    Section(report.spec.title) {
                        ForEach(report.points, id: \.group) { point in
                            VStack(alignment: .leading) {
                                LabeledContent(point.group, value: point.value)
                                ProgressView(value: maxValue > 0 ? abs(Double(point.value) ?? 0) / maxValue : 0)
                                    .accessibilityHidden(true)
                            }
                            .accessibilityElement(children: .combine)
                            .accessibilityIdentifier("teams.report.point." + point.group)
                        }
                    }
                    if report.canReview {
                        Section("Review a partition") {
                            ForEach(report.cells, id: \.self) { cell in
                                Button(cell.partition + " · " + cell.group) {
                                    sending = true
                                    Task { @MainActor in
                                        defer { sending = false }
                                        do {
                                            try await model.openReportReview(conversation: conversation, report: report, cell: cell)
                                            status = "Review case open"
                                        } catch { self.report = nil; status = "Report unavailable" }
                                    }
                                }.disabled(sending)
                            }
                        }
                    }
                }
                if let status { Text(status) }
            }
            .navigationTitle("Published report")
            .toolbar { Button("Done") { dismiss() } }
            .task(id: latest) {
                report = nil; status = nil
                do {
                    let value = try await model.readReport(conversation: conversation, link: link, latest: latest)
                    try Task.checkCancellation()
                    report = value
                } catch is CancellationError { }
                catch { if !Task.isCancelled { status = "Report unavailable" } }
            }
        }
    }
}
