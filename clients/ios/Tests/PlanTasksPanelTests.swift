// PlanTasksPanelTests.swift — the plan panel's locale source and row identity.
//
// Two regressions this pins down, both invisible to a screenshot in the
// developer's own language:
//
//  1. The overflow clause separator must come from the ENVIRONMENT locale.
//     `LocalizationManager` applies the in-app language by swizzling
//     `Bundle.localizedString` and by injecting `effectiveLocale()` into the
//     environment (`RootView`); it never moves `Locale.current`. A panel that
//     asks `Locale.current` therefore joins Chinese clauses with ", " for every
//     user whose DEVICE is English but who picked 简体中文 in the app.
//
//  2. Row identity. `PlanUpdated` replaces the list wholesale, so a purely
//     positional key turns a reorder into "every row's content changed"
//     instead of "the rows moved" — but `ConversationPlanTask.id`
//     (`taskId ?? subject`) alone is NOT unique either: TodoWrite V1 items
//     arrive with `taskId == nil` and nothing rejects two todos with the same
//     text, so identical subjects would hand `ForEach` a duplicate
//     identifier. The key is therefore the task id when there is one and an
//     index-qualified subject when there is not.
//
// Hermetic: no engine handle, no rendering host.

import SwiftUI
import XCTest

@testable import LingxiCode

@MainActor
final class PlanTasksPanelTests: XCTestCase {

    private func task(
        _ subject: String,
        _ state: ConversationPlanTaskState,
        id: String? = nil
    ) -> ConversationPlanTask {
        ConversationPlanTask(taskId: id, subject: subject, activeForm: nil, state: state)
    }

    // MARK: - Defect 1: the separator follows the in-app language

    /// The separator is a pure function of the locale it is HANDED. Reading the
    /// process locale instead fails one of these two halves on any host.
    func testClauseSeparatorIsDerivedFromTheSuppliedLocale() {
        for identifier in ["zh-Hans", "zh-Hant", "zh-CN", "ja"] {
            XCTAssertEqual(
                PlanTasksPanel.clauseSeparator(for: Locale(identifier: identifier)),
                "、",
                "\(identifier) enumerates with the ideographic comma"
            )
        }
        for identifier in ["en", "en-US", "ko", "de"] {
            XCTAssertEqual(
                PlanTasksPanel.clauseSeparator(for: Locale(identifier: identifier)),
                ", ",
                "\(identifier) enumerates with a Latin comma"
            )
        }
    }

    /// The panel must take that locale from the environment `RootView` injects.
    /// A `static var` reading `Locale.current` cannot see it — so assert the
    /// stored `@Environment(\.locale)` the fix depends on is actually declared.
    func testPanelReadsTheLocaleFromTheEnvironment() {
        let panel = PlanTasksPanel(tasks: [])
        let localeProperty = Mirror(reflecting: panel).children.first { $0.label == "_locale" }
        let declared = localeProperty.map { type(of: $0.value) }

        XCTAssertTrue(
            declared == Environment<Locale>.self,
            """
            PlanTasksPanel must hold @Environment(\\.locale); the in-app language \
            switch is delivered through the environment, never through \
            Locale.current. Found: \(String(describing: declared))
            """
        )
    }

    /// End to end for the overflow line: the same hidden remainder renders with
    /// `、` under 简体中文 and `, ` under English, on one and the same host.
    func testOverflowSummaryJoinsClausesWithTheLocaleSeparator() {
        let hidden = [
            task("write the migration", .inProgress),
            task("back-fill rows", .pending),
            task("ship it", .pending),
        ]

        let chinese = PlanTasksPanel.overflowSummary(
            hidden: hidden, locale: Locale(identifier: "zh-Hans"))
        let english = PlanTasksPanel.overflowSummary(
            hidden: hidden, locale: Locale(identifier: "en"))

        XCTAssertNotNil(chinese)
        XCTAssertNotNil(english)
        XCTAssertTrue(chinese?.contains("、") == true, "got: \(chinese ?? "nil")")
        XCTAssertFalse(chinese?.contains(", ") == true, "got: \(chinese ?? "nil")")
        XCTAssertTrue(english?.contains(", ") == true, "got: \(english ?? "nil")")
        XCTAssertFalse(english?.contains("、") == true, "got: \(english ?? "nil")")
    }

    /// `overflow_summary` returns nothing when nothing is hidden, in any locale.
    func testOverflowSummaryIsNilWithoutAHiddenRemainder() {
        XCTAssertNil(PlanTasksPanel.overflowSummary(hidden: [], locale: Locale(identifier: "zh-Hans")))
        XCTAssertNil(PlanTasksPanel.overflowSummary(hidden: [], locale: Locale(identifier: "en")))
    }

    // MARK: - Defect 2: rows are keyed by the model's identity

    /// The row list must be a `ForEach` keyed by a `String`, never by an `Int`
    /// offset — an offset key turns a reordered plan into "every row's content
    /// changed" instead of "the rows moved".
    func testRowsAreKeyedByAStringIdentityNotByPosition() {
        let panel = PlanTasksPanel(tasks: [
            task("alpha", .inProgress, id: "t1"),
            task("beta", .pending, id: "t2"),
        ])

        let rendered = String(describing: type(of: panel.taskRows))

        XCTAssertTrue(
            rendered.contains(", String,"),
            """
            Plan rows must be keyed by a String identity. A ForEach over \
            enumerated() keyed by \\.offset keys by Int, so a reordered plan \
            re-renders by position instead of moving rows. Found: \(rendered)
            """
        )
    }

    /// The identity the `ForEach` leans on: the stable V2 task id when there is
    /// one, so a reorder stays a MOVE.
    func testRowKeysUseTheEngineTaskIdWhenThereIsOne() {
        let before = [task("a", .completed, id: "t1"), task("b", .pending, id: "t2")]
        let after = [before[1], before[0]]

        XCTAssertEqual(PlanTasksPanel.rows(for: before).map(\.id), ["t1", "t2"])
        // Reordered: the same identities, moved — NOT two mutated rows.
        XCTAssertEqual(PlanTasksPanel.rows(for: after).map(\.id), ["t2", "t1"])
    }

    /// The C4 defect. `plan_tasks_from_todowrite_input` sets `id: None` for
    /// every TodoWrite V1 item and `validate_todos` never checks `content` for
    /// uniqueness, so two identical todos reach the panel with the SAME
    /// `ConversationPlanTask.id` (`taskId ?? subject`). Handing `ForEach` a
    /// duplicate identifier is undefined row identity plus a runtime
    /// diagnostic, so the row key must be index-qualified in that case —
    /// exactly Electron's ``key={task.id ?? `${i}:${task.subject}`}``.
    func testDuplicateSubjectsWithoutIdsStillProduceUniqueRowKeys() {
        let duplicated = [
            task("run the migration", .inProgress),
            task("run the migration", .pending),
            task("run the migration", .completed),
        ]

        // The model's own identity DOES collide — that is the hazard.
        XCTAssertEqual(Set(duplicated.map(\.id)).count, 1)

        let keys = PlanTasksPanel.rows(for: duplicated).map(\.id)
        XCTAssertEqual(keys, ["0:run the migration", "1:run the migration", "2:run the migration"])
        XCTAssertEqual(Set(keys).count, duplicated.count, "ForEach identifiers must be unique")
    }

    /// The rows must still carry the tasks through in order, unchanged.
    func testRowsPreserveTheTaskOrderAndPayload() {
        let tasks = [task("alpha", .inProgress, id: "t1"), task("beta", .pending)]
        let rows = PlanTasksPanel.rows(for: tasks)

        XCTAssertEqual(rows.map(\.task), tasks)
        XCTAssertEqual(rows.map(\.id), ["t1", "1:beta"])
    }

    /// The model identity itself is unchanged: task id first, subject second.
    func testTaskIdentityPrefersTheTaskIdAndFallsBackToTheSubject() {
        XCTAssertEqual(task("alpha", .pending, id: "t1").id, "t1")
        XCTAssertEqual(task("alpha", .pending).id, "alpha")
    }
}
