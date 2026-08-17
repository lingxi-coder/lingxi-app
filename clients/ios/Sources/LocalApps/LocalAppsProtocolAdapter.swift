import Foundation

#if canImport(engine_mobileFFI)
enum LocalAppsProtocolAdapter {
    static func collection(_ dto: AppDataCollectionDto) -> LocalAppDataCollection {
        LocalAppDataCollection(
            id: dto.id,
            label: dto.label,
            fields: dto.fields.map(dataField),
            enabledByDefault: dto.enabledByDefault
        )
    }

    static func dataField(_ dto: AppDataFieldDto) -> LocalAppDataField {
        LocalAppDataField(
            id: dto.id,
            label: dto.label,
            fieldType: dataFieldType(dto.fieldType),
            required: dto.required,
            options: dto.options
        )
    }

    static func dataFieldType(_ dto: AppDataFieldTypeDto) -> LocalAppDataFieldType {
        switch dto {
        case .text: .text
        case .longText: .longText
        case .integer: .integer
        case .decimal: .decimal
        case .boolean: .boolean
        case .dateTime: .dateTime
        case .enum: .enumeration
        case .imageRef: .imageReference
        }
    }

    static func app(_ dto: AppRecordDto) -> LocalAppSummary {
        LocalAppSummary(
            id: dto.id,
            name: dto.name,
            brief: dto.brief,
            gitEnabled: dto.gitEnabled,
            updatedAt: Date(timeIntervalSince1970: TimeInterval(dto.updatedAtMs) / 1_000),
            workflow: workflow(dto.workflowState),
            workspaceRelativePath: dto.workspaceRel,
            initSessionId: dto.initSessionId
        )
    }

    static func workflow(_ dto: AppWorkflowStateDto) -> LocalAppWorkflow {
        switch dto {
        case .draft: .draft
        case .ready: .ready
        }
    }

    /// One catalog row (`AppSessionRowDto`) lowered to the UI model. The wire
    /// marks the pinned init session via `kind`; clients list it first.
    static func sessionRow(_ dto: AppSessionRowDto) -> LocalAppSessionRow {
        LocalAppSessionRow(
            uuid: dto.uuid,
            title: dto.title,
            relativeTime: RelativeTime.format(dto.modifiedRfc3339),
            messageCount: Int(dto.messageCount),
            isInit: dto.kind == .`init`
        )
    }

    static func runtime(
        _ dto: AppRuntimeStateDto,
        details: AppRuntimeDetailsDto? = nil,
        lastError: String?,
        knownURL: URL?
    ) -> LocalAppRuntimeStatus {
        let loopbackURL = details?.loopbackUrl.flatMap(URL.init(string:)) ?? knownURL
        if let reason = details?.suspensionReason {
            return .suspended(suspensionReason(reason))
        }
        return switch dto {
        case .stopped: .stopped
        case .starting: .starting
        case .running: .running(loopbackURL)
        case .stopping: .stopping
        case .failed: .failed(details?.lastError ?? lastError ?? String(localized: "local_apps_process_exited"))
        }
    }

    static func suspensionReason(_ reason: AppRuntimeSuspensionReasonDto) -> String {
        switch reason {
        case .backgrounded: String(localized: "local_apps_suspension_background")
        case .memoryWarning: String(localized: "local_apps_suspension_memory")
        case .runtimeQuota: String(localized: "local_apps_suspension_quota")
        case .processExited: String(localized: "local_apps_suspension_exited")
        }
    }

    static func capabilityKind(_ kind: AppCapabilityKindDto) -> LocalAppPermissionPrompt.Kind {
        switch kind {
        case .dataMutation: .dataMutation
        case .uiControl: .uiControl
        case .networkDomain: .networkDomain
        case .restoreCheckpoint: .restoreCheckpoint
        case .camera: .camera
        case .photoLibrary: .photoLibrary
        case .microphone: .microphone
        case .location: .location
        case .notifications: .notifications
        case .llm: .llm
        case .agentNotify: .agentNotify
        case .backgroundSchedule: .backgroundSchedule
        }
    }

    static func uiActionLabel(_ action: AppUiActionKindDto) -> String {
        switch action {
        case .inspect: String(localized: "local_apps_ui_action_inspect")
        case .click: String(localized: "local_apps_ui_action_click")
        case .fill: String(localized: "local_apps_ui_action_fill")
        case .select: String(localized: "local_apps_ui_action_select")
        case .toggle: String(localized: "local_apps_ui_action_toggle")
        case .scroll: String(localized: "local_apps_ui_action_scroll")
        case .navigate: String(localized: "local_apps_ui_action_navigate")
        case .back: String(localized: "local_apps_ui_action_back")
        case .reload: String(localized: "local_apps_ui_action_reload")
        }
    }

    static func authorizationDecision(_ value: LocalAppCapabilityDecision) -> AppAuthorizationDecisionDto {
        switch value {
        case .once: .allowOnce
        case .session: .allowSession
        case .always: .allowAlways
        case .deny: .deny
        }
    }

    static func checkpoint(_ dto: AppCheckpointDto) -> LocalAppCheckpoint {
        LocalAppCheckpoint(
            id: dto.id,
            label: dto.label,
            kind: checkpointKind(dto.kind),
            createdAt: Date(timeIntervalSince1970: TimeInterval(dto.createdAtMs) / 1_000)
        )
    }

    static func checkpointKind(_ kind: AppCheckpointKindDto) -> String {
        switch kind {
        case .scaffoldCreated: "scaffold"
        case .generationValidated: "validated"
        case .previewApproved: "preview"
        case .userApproved: "user"
        case .preRestore: "pre_restore"
        }
    }
}
#endif
