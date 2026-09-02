import XCTest
@testable import LingxiCode

// Shared construction helpers for the LocalApps test suite. Centralized here
// so the local-apps test files do not each hand-roll their own doubles for
// the same DTOs and models.

@MainActor func makeStore() -> LocalAppsStore { LocalAppsStore() }

func dataField(id: String) -> LocalAppDataField {
    LocalAppDataField(id: id, label: id, fieldType: .text, required: false, options: [])
}

#if canImport(engine_mobileFFI)
    /// A wire record for the general-purpose tests: a FORMED app.
    ///
    /// `scaffolded` defaults to `true` because that is what every caller of
    /// this fixture means — an app with a real name and a real brief. The
    /// draft-shell tests do NOT use this default: they build their records
    /// through `LocalAppsStoreTests.shellRecord`, which states `scaffolded`
    /// explicitly, so no shell assertion can pass by inheriting a default.
    func appRecord(
        id: String,
        name: String,
        brief: String = "简介",
        workflowState: AppWorkflowStateDto = .publishedUnverified,
        initSessionId: String? = nil,
        scaffolded: Bool = true
    ) -> AppRecordDto {
        AppRecordDto(
            id: id,
            name: name,
            brief: brief,
            gitEnabled: true,
            createdAtMs: 1,
            updatedAtMs: 2,
            workflowState: workflowState,
            conversationId: nil,
            initSessionId: initSessionId,
            workspaceRel: "apps/\(id)/workspace",
            scaffolded: scaffolded
        )
    }

    func appSessionRow(
        uuid: String,
        title: String = "会话",
        modified: String = "2026-08-09T10:00:00Z",
        messageCount: UInt32 = 3,
        kind: AppSessionKindDto = .conversation,
        mode: SessionModeDto = .code
    ) -> AppSessionRowDto {
        AppSessionRowDto(
            uuid: uuid,
            mode: mode,
            title: title,
            modifiedRfc3339: modified,
            messageCount: messageCount,
            kind: kind
        )
    }
#endif
