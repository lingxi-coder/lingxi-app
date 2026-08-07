import XCTest
@testable import LingxiCode

// Shared construction helpers for the LocalApps test suite
// (local-apps#questionnaire, Tasks 13-16). Centralized here so the
// questionnaire/plan/designer/create-entry test files that follow do not
// each hand-roll their own doubles for the same DTOs and models.

@MainActor func makeStore() -> LocalAppsStore { LocalAppsStore() }

func designField(allowsCustom: Bool = false, allowsDefer: Bool = false) -> LocalAppDesignField {
    LocalAppDesignField(
        id: "features",
        label: "需要哪些功能",
        description: "",
        type: .multipleChoice,
        required: true,
        allowsCustom: allowsCustom,
        allowsDefer: allowsDefer,
        defaultValue: nil,
        options: [LocalAppDesignOption(value: "list", label: "笔记列表")]
    )
}

/// General-purpose design field builder used across the existing designer
/// tests (debounce, conflict, revision-race, …), where only `id`/`type`
/// vary and every field is required.
func designField(id: String, type: LocalAppFieldType) -> LocalAppDesignField {
    LocalAppDesignField(
        id: id,
        label: id,
        description: "",
        type: type,
        required: true,
        allowsCustom: false,
        allowsDefer: false,
        defaultValue: nil,
        options: []
    )
}

func dataField(id: String) -> LocalAppDataField {
    LocalAppDataField(id: id, label: id, fieldType: .text, required: false, options: [])
}

func notesPlan() -> LocalAppPlan {
    LocalAppPlan(
        collections: [LocalAppDataCollection(
            id: "notes",
            label: "Notes",
            fields: [LocalAppDataField(id: "title", label: "Title", fieldType: .text, required: true, options: [])],
            enabledByDefault: true
        )],
        capabilities: [],
        domains: [],
        summary: "记事本"
    )
}

/// `notesPlan()` with one external domain added — `domains` is a `var` on
/// `LocalAppPlan` specifically so tests (and eventually Task 15's plan
/// confirmation sheet) can derive a variant like this without rebuilding the
/// whole plan by hand.
func planWithDomain(_ domain: String) -> LocalAppPlan {
    var plan = notesPlan()
    plan.domains = [domain]
    return plan
}

#if canImport(engine_mobileFFI)
    func oneStepDTO() -> AppDesignStepDto {
        AppDesignStepDto(
            id: "basics",
            order: 0,
            title: "功能",
            description: nil,
            fields: [designFieldDTO(allowsCustom: true, allowsDefer: true)]
        )
    }

    func designFieldDTO(allowsCustom: Bool = false, allowsDefer: Bool = false) -> AppDesignFieldDto {
        AppDesignFieldDto(
            id: "features",
            label: "需要哪些功能",
            description: nil,
            fieldType: .multipleChoice,
            required: true,
            allowsCustom: allowsCustom,
            allowsDefer: allowsDefer,
            defaultValue: nil,
            options: [AppDesignFieldOptionDto(value: "list", label: "笔记列表")]
        )
    }

    func oneStep() -> LocalAppDesignStep {
        LocalAppsProtocolAdapter.designStep(oneStepDTO())
    }

    func onePlanDTO() -> AppPlanDto {
        AppPlanDto(
            collections: [AppDataCollectionDto(
                id: "notes",
                label: "Notes",
                fields: [AppDataFieldDto(id: "title", label: "Title", fieldType: .text, required: true, options: [])],
                enabledByDefault: true
            )],
            capabilities: [],
            domains: [],
            summary: "记事本"
        )
    }
#endif
