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
            initSessionId: dto.initSessionId,
            // Carried, never assumed. Hardcoding `true` here compiles and
            // passes every fixture that describes a formed app, and shows the
            // engine's placeholder name on every shell card in the library.
            scaffolded: dto.scaffolded
        )
    }

    static func workflow(_ dto: AppWorkflowStateDto) -> LocalAppWorkflow {
        switch dto {
        case .draft: .draft
        case .publishedUnverified: .publishedUnverified
        case .publishedVerified: .publishedVerified
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
        case .dependencyChange: .dependencyChange
        case .camera: .camera
        case .photoLibrary: .photoLibrary
        case .microphone: .microphone
        case .location: .location
        case .notifications: .notifications
        case .clipboard: .clipboard
        case .share: .share
        case .textToSpeech: .textToSpeech
        case .files: .files
        case .filesRead: .filesRead
        case .filesWrite: .filesWrite
        case .deviceStatus: .deviceStatus
        case .haptics: .haptics
        case .deepLink: .deepLink
        case .calendar: .calendar
        case .contacts: .contacts
        case .media: .media
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
        case .captureView: String(localized: "local_apps_ui_action_capture_view")
        case .pointer: String(localized: "local_apps_ui_action_pointer")
        case .key: String(localized: "local_apps_ui_action_key")
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

    static func runtimeProfileFamily(_ dto: AppRuntimeProfileDto) -> LocalAppRuntimeProfileFamily {
        switch dto {
        case .reactDom: .reactDom
        case .canvas2d: .canvas2d
        case .three3d: .three3d
        case .phaser2d: .phaser2d
        case .babylon3d: .babylon3d
        }
    }

    static func runtimeProfileFamilyDto(_ family: LocalAppRuntimeProfileFamily) -> AppRuntimeProfileDto {
        switch family {
        case .reactDom: .reactDom
        case .canvas2d: .canvas2d
        case .three3d: .three3d
        case .phaser2d: .phaser2d
        case .babylon3d: .babylon3d
        }
    }

    static func runtimeProfileStatus(_ dto: AppRuntimeProfileStatusDto) -> LocalAppRuntimeProfileStatus {
        switch dto {
        case .verified: .verified
        case .dependenciesDirty: .dependenciesDirty
        case .coreDependencyDrift: .coreDependencyDrift
        case .rebuildRequired: .rebuildRequired
        case .migrationAvailable: .migrationAvailable
        case .runtimeBundleMissing: .runtimeBundleMissing
        case .runtimeContractCorrupt: .runtimeContractCorrupt
        }
    }

    static func dependencyChangeConfirmation(
        _ dto: AppDependencyChangeConfirmationRequestDto
    ) -> LocalAppDependencyChangeConfirmationPrompt {
        LocalAppDependencyChangeConfirmationPrompt(
            id: dto.requestId,
            appID: dto.appId,
            reason: dto.reason,
            changes: dto.changes.map { change in
                LocalAppDependencyChange(
                    kind: dependencyChangeKind(change.kind),
                    package: change.package,
                    version: change.version,
                    cacheStatus: change.cacheStatus,
                    downloadStatus: change.downloadStatus
                )
            },
            licenseRisk: dto.licenseRisk,
            sbomRisk: dto.sbomRisk,
            lifecycleScriptsBlocked: dto.lifecycleScriptsBlocked,
            nativeAddonsBlocked: dto.nativeAddonsBlocked,
            rollbackPolicy: dto.rollbackPolicy
        )
    }

    private static func dependencyChangeKind(
        _ kind: AppDependencyChangeKindDto
    ) -> LocalAppDependencyChangeKind {
        switch kind {
        case .add: .add
        case .update: .update
        case .remove: .remove
        }
    }

    static func runtimeProfileSurface(_ dto: AppSurfaceDto) -> LocalAppRuntimeProfileSurface {
        switch dto {
        case .dom:
            .dom
        case .canvas:
            .canvas
        }
    }

    static func verificationStatus(_ dto: LocalAppVerificationStatusDto) -> LocalAppVerificationStatus {
        switch dto {
        case .pending:
            .pending
        case .passed:
            .passed
        case .failed:
            .failed
        case .unverified:
            .unverified
        case .unavailable:
            .unavailable
        }
    }

    static func verificationSummary(_ dto: LocalAppVerificationSummaryDto) -> LocalAppVerificationSummary {
        LocalAppVerificationSummary(
            status: verificationStatus(dto.status),
            summary: dto.summary,
            code: dto.code
        )
    }

    static func gateStatus(_ dto: LocalAppGateStatusDto) -> LocalAppGateStatus {
        LocalAppGateStatus(
            gateID: dto.gateId,
            label: dto.label,
            status: verificationStatus(dto.status),
            available: dto.available,
            detail: dto.detail
        )
    }

    static func builtinPluginInventory(_ dto: LocalAppPluginInventoryDto) -> LocalAppBuiltinPluginInventory {
        LocalAppBuiltinPluginInventory(
            pluginID: dto.pluginId,
            displayName: dto.displayName,
            source: dto.source,
            version: dto.version,
            bundleDigest: dto.bundleSha256,
            manifestDefaultEnabled: dto.manifestDefaultEnabled,
            skillCount: Int(dto.counts.skills),
            agentCount: Int(dto.counts.agents),
            workflowCount: Int(dto.counts.workflows),
            templateCount: Int(dto.counts.templates),
            validationError: dto.validationError
        )
    }

    static func templateSummary(_ dto: LocalAppTemplateSummaryDto) -> LocalAppTemplateSummary {
        LocalAppTemplateSummary(
            templateID: dto.templateId,
            surface: runtimeProfileSurface(dto.surface),
            summary: dto.summary
        )
    }

    static func rejectedCandidate(_ dto: LocalAppRejectedCandidateDto) -> LocalAppRejectedCandidate {
        LocalAppRejectedCandidate(
            templateID: dto.templateId,
            reason: dto.reason
        )
    }

    static func receiptStatus(_ dto: LocalAppReceiptStatusDto) -> LocalAppReceiptStatus {
        LocalAppReceiptStatus(
            receiptID: dto.receiptId,
            appID: dto.appId,
            workflowRunID: dto.workflowRunId,
            approvalContractSHA256: dto.approvalContractSha256,
            candidateDigest: dto.candidateDigest,
            issuedAt: Date(timeIntervalSince1970: TimeInterval(dto.issuedAtMs) / 1_000),
            expiresAt: Date(timeIntervalSince1970: TimeInterval(dto.expiresAtMs) / 1_000),
            consumed: dto.consumed,
            superseded: dto.superseded
        )
    }

    static func mcpToolSurface(_ dto: LocalAppMcpToolSurfaceDto) -> LocalAppMcpToolSurface {
        LocalAppMcpToolSurface(
            name: dto.name,
            title: dto.title,
            description: dto.description,
            inputSchemaSummary: dto.inputSchemaJson,
            outputSchemaSummary: dto.outputSchemaJson,
            annotationsSummary: dto.annotationsJson,
            executionSummary: dto.executionJson,
            visibleMetaSummary: dto.visibleMetaJson,
            semanticFlowSummary: dto.semanticFlowJson,
            ceilingSummary: dto.permissionCeiling
        )
    }

    static func createConfirmation(_ dto: LocalAppCreateConfirmationRequestDto) -> LocalAppCreateConfirmationPrompt {
        LocalAppCreateConfirmationPrompt(
            requestID: dto.requestId,
            appID: dto.appId,
            name: dto.name,
            brief: dto.brief,
            selectedTemplate: templateSummary(dto.selectedTemplate),
            runtimeProfile: LocalAppRuntimeProfileOption(
                family: runtimeProfileFamily(dto.runtimeProfile.family),
                revision: dto.runtimeProfile.revision,
                contractSHA256: dto.runtimeProfile.contractSha256,
                surface: runtimeProfileSurface(dto.runtimeProfile.surface),
                corePackages: dto.runtimeProfile.corePackages.map {
                    LocalAppRuntimeProfilePackage(name: $0.name, version: $0.version)
                },
                cacheStatus: dto.runtimeProfile.cacheStatus,
                downloadStatus: dto.runtimeProfile.downloadStatus,
                available: dto.runtimeProfile.available,
                reason: dto.runtimeProfile.reason
            ),
            reason: dto.reason,
            rejected: dto.rejected.map(rejectedCandidate),
            initialTools: dto.initialTools.map(mcpToolSurface),
            requiredGates: dto.requiredGates.map(gateStatus),
            receipt: dto.receipt.map(receiptStatus)
        )
    }

    static func mcpToolField(_ dto: LocalAppMcpToolFieldDto) -> LocalAppMcpToolField {
        switch dto {
        case .name:
            .name
        case .title:
            .title
        case .description:
            .description
        case .inputSchema:
            .inputSchema
        case .outputSchema:
            .outputSchema
        case .annotations:
            .annotations
        case .execution:
            .execution
        case .visibleMeta:
            .visibleMeta
        case .semanticFlow:
            .semanticFlow
        case .permissionCeiling:
            .permissionCeiling
        }
    }

    static func mcpToolChangeKind(_ dto: LocalAppMcpToolChangeKindDto) -> LocalAppMcpToolChangeKind {
        switch dto {
        case .added:
            .added
        case .removed:
            .removed
        case .changed:
            .changed
        }
    }

    static func mcpToolDiff(_ dto: LocalAppMcpToolDiffDto) -> LocalAppMcpToolDiff {
        LocalAppMcpToolDiff(
            kind: mcpToolChangeKind(dto.kind),
            name: dto.name,
            before: dto.before.map(mcpToolSurface),
            after: dto.after.map(mcpToolSurface),
            changedFields: dto.changedFields.map(mcpToolField)
        )
    }

    static func mcpProposalApproval(
        _ dto: LocalAppMcpProposalApprovalRequestDto
    ) -> LocalAppMcpProposalApprovalPrompt {
        LocalAppMcpProposalApprovalPrompt(
            requestID: dto.requestId,
            appID: dto.appId,
            workflowRunID: dto.workflowRunId,
            summary: dto.summary,
            proposalSHA256: dto.proposalSha256,
            approvalContractSHA256: dto.approvalContractSha256,
            toolSurfaceSHA256: dto.toolSurfaceSha256,
            toolDiffs: dto.toolDiffs.map(mcpToolDiff),
            requiredFlowChanges: dto.requiredFlowChanges,
            excludedCapabilities: dto.excludedCapabilities,
            pendingGates: dto.pendingGates.map(gateStatus),
            receipt: dto.receipt.map(receiptStatus)
        )
    }

    static func managedMcpInventory(_ dto: ManagedLocalAppMcpServerDto) -> LocalAppManagedMcpInventory {
        LocalAppManagedMcpInventory(
            serverName: dto.serverName,
            appID: dto.appId,
            appName: dto.appName,
            buildID: dto.buildId,
            catalogDigest: dto.catalogSha256,
            toolSurfaceDigest: dto.toolSurfaceSha256,
            authoringRevision: dto.authoringRevision,
            enabled: dto.enabled,
            status: managedMcpStatus(dto.status),
            settingsRevision: dto.settingsRevision,
            pinnedToCurrentConversation: dto.pinnedToCurrentConversation,
            publicationState: workflow(dto.publicationState),
            mcpVerification: verificationSummary(dto.mcpVerification),
            uiVerification: verificationSummary(dto.uiVerification),
            enabledTools: Set(dto.enabledTools),
            widget: dto.widget.map {
                LocalAppManagedMcpWidget(
                    title: nil,
                    resourceURI: $0.resourceUri,
                    mimeType: $0.mimeType
                )
            },
            tools: dto.tools.map(mcpToolSurface)
        )
    }

    private static func managedMcpStatus(
        _ status: ManagedLocalAppMcpStatusDto
    ) -> LocalAppManagedMcpStatus {
        switch status {
        case .disabled: .disabled
        case .needsSetup: .needsSetup
        case .authoring: .authoring
        case .enabled: .enabled
        case .needsRevalidation: .needsRevalidation
        case .error: .error
        }
    }
}
#endif
