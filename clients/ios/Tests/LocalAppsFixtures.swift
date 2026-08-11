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
    func appRecord(
        id: String,
        name: String,
        brief: String = "简介",
        workflowState: AppWorkflowStateDto = .ready,
        initSessionId: String? = nil
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
            workspaceRel: "apps/\(id)/workspace"
        )
    }

    func appSessionRow(
        uuid: String,
        title: String = "会话",
        modified: String = "2026-08-09T10:00:00Z",
        messageCount: UInt32 = 3,
        kind: AppSessionKindDto = .conversation
    ) -> AppSessionRowDto {
        AppSessionRowDto(
            uuid: uuid,
            title: title,
            modifiedRfc3339: modified,
            messageCount: messageCount,
            kind: kind
        )
    }
#endif
