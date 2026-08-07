package com.lingxi.code.localapps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppsScreenTest {
    /** The literal rule from lingxi-code/local-apps/src/manifest.rs validate_identifier. */
    private val identifier = Regex("^[a-z][a-z0-9_]{0,63}$")

    // local-apps#questionnaire, Task 19 — the Android designer now renders the
    // LLM-authored questionnaire (steps come from `state.questionnaires`, not
    // a static template) and the two chip affordances (`allowsCustom`'s
    // `Other…` box, `allowsDefer`'s 「由你决定」 chip) it had no UI treatment
    // for before this task, plus the four intermediate/failure states.

    @Test
    fun `steps come from the questionnaire not a template`() {
        val state = LocalAppsUiState(
            questionnaires = mapOf("a" to listOf(oneStep())),
            distributionMode = LocalAppRuntimeMode.StaticExport,
        )
        assertEquals(1, designerSteps(state, appId = "a").size)
    }

    @Test
    fun `steps are ordered by their declared order, not insertion order`() {
        val state = LocalAppsUiState(
            questionnaires = mapOf("a" to listOf(oneStep(order = 2u, id = "second"), oneStep(order = 1u, id = "first"))),
            distributionMode = LocalAppRuntimeMode.StaticExport,
        )
        assertEquals(listOf("first", "second"), designerSteps(state, appId = "a").map { it.id })
    }

    @Test
    fun `a field that allows defer offers the defer chip`() {
        assertTrue(chipsFor(designField(allowsDefer = true)).contains(DesignerChip.Defer))
    }

    @Test
    fun `a field with no options and no defer offers no chips at all`() {
        assertTrue(chipsFor(designField()).isEmpty())
    }

    @Test
    fun `the defer chip always comes after the field's declared options`() {
        val field = designField(allowsDefer = true, options = listOf(LocalAppFieldOption("a", "A")))
        assertEquals(listOf(DesignerChip.Option("a"), DesignerChip.Defer), chipsFor(field))
    }

    @Test
    fun `a field that allows custom offers the other input`() {
        assertTrue(showsCustomInput(designField(allowsCustom = true)))
    }

    @Test
    fun `a field that does not allow custom offers no other input`() {
        assertFalse(showsCustomInput(designField(allowsCustom = false)))
    }

    @Test
    fun `a single choice field's chip row never repeats its own picker's options`() {
        // LocalAppDynamicField already renders every SingleChoice option as
        // its own FilterChip row; chipsFor must not render the same option
        // set a second time underneath it — only the genuinely new defer
        // chip belongs here for choice kinds.
        val field = designField(allowsDefer = true, options = listOf(LocalAppFieldOption("a", "A")))
            .copy(kind = LocalAppFieldKind.SingleChoice)
        assertEquals(listOf(DesignerChip.Defer), chipsFor(field))
    }

    @Test
    fun `a multiple choice field's chip row never repeats its own picker's options`() {
        val field = designField(allowsDefer = true, options = listOf(LocalAppFieldOption("a", "A")))
            .copy(kind = LocalAppFieldKind.MultipleChoice)
        assertEquals(listOf(DesignerChip.Defer), chipsFor(field))
    }

    /**
     * The critical defect a review caught: `commitCustomChipText` used to
     * send `LocalAppDesignValue.Text` for every field kind except
     * `MultipleChoice`. For a `SingleChoice` field, that value "saves" fine
     * (`apply_patch` inserts any shape blindly) but fails LATER at
     * `begin_planning` -> `validate_answers` (questionnaire.rs):
     * `SingleChoice.accepts(ShortText)` is false, so a `Text` answer for a
     * `SingleChoice` field rejects with "answered with a value of the wrong
     * kind" — a raw engine error at the exact 生成方案 tap this task exists
     * to unblock. `allowsCustom` is legal on any field kind
     * (`validate_field`, questionnaire.rs, ties it to nothing), so this is a
     * real, reachable combination, not a hypothetical one.
     */
    @Test
    fun `typing into the other box on a single choice field commits a choice value not text`() {
        val field = designField(allowsCustom = true, options = listOf(LocalAppFieldOption("a", "A")))
            .copy(kind = LocalAppFieldKind.SingleChoice)
        val committed = commitCustomChipText(field, "自定义答案", value = null)
        assertEquals(LocalAppDesignValue.Choice("自定义答案"), committed)
    }

    @Test
    fun `typing into the other box on a multiple choice field keeps the checked options and appends text`() {
        val field = designField(allowsCustom = true, options = listOf(LocalAppFieldOption("a", "A"), LocalAppFieldOption("b", "B")))
            .copy(kind = LocalAppFieldKind.MultipleChoice)
        val committed = commitCustomChipText(field, "自定义答案", value = LocalAppDesignValue.Choices(listOf("a")))
        assertEquals(LocalAppDesignValue.Choices(listOf("a", "自定义答案")), committed)
    }

    @Test
    fun `typing into the other box on a domain list field commits a string list value not text`() {
        val field = designField(allowsCustom = true).copy(kind = LocalAppFieldKind.DomainList)
        val committed = commitCustomChipText(field, "api.example.com", value = null)
        assertEquals(LocalAppDesignValue.StringList(listOf("api.example.com")), committed)
    }

    @Test
    fun `choosing defer stores the deferred value rather than clearing the field`() {
        var recorded: LocalAppDesignValue? = null
        selectChip(designField(allowsDefer = true), DesignerChip.Defer) { recorded = it }
        assertEquals(
            "defer is an answer, not an absence",
            LocalAppDesignValue.Deferred,
            recorded,
        )
    }

    @Test
    fun `choosing an option on a multiple choice field toggles it into the selection`() {
        val field = designField(options = listOf(LocalAppFieldOption("a", "A"), LocalAppFieldOption("b", "B")))
            .copy(kind = LocalAppFieldKind.MultipleChoice)
        var recorded: LocalAppDesignValue? = null
        selectChip(field, DesignerChip.Option("a"), currentValue = LocalAppDesignValue.Choices(listOf("b"))) {
            recorded = it
        }
        assertEquals(LocalAppDesignValue.Choices(listOf("b", "a")), recorded)
    }

    @Test
    fun `the designer is read only while the model is working`() {
        assertFalse(isDesignerEditable(LocalAppWorkflow.AuthoringQuestionnaire))
        assertFalse(isDesignerEditable(LocalAppWorkflow.Planning))
        assertFalse(isDesignerEditable(LocalAppWorkflow.QuestionnaireFailed))
        assertFalse(isDesignerEditable(LocalAppWorkflow.PlanFailed))
        assertTrue(isDesignerEditable(LocalAppWorkflow.CollectingSpec))
    }

    @Test
    fun generatedFieldIdMatchesTheEngineIdentifierContract() {
        val first = generateFieldId(emptyList())
        assertTrue("minted id $first is rejected by the manifest", identifier.matches(first))

        val second = generateFieldId(listOf(field(first)))
        assertTrue("minted id $second is rejected by the manifest", identifier.matches(second))
        assertEquals("field_2", second)

        // The natural index is already taken, so the loop must step past it.
        assertEquals("field_3", generateFieldId(listOf(field("field_2"))))
    }

    @Test
    fun `domain editor rejects everything the manifest contract rejects`() {
        // Both enable the Add button under Kotlin's Unicode-aware
        // isLetterOrDigit(), and both are refused by manifest::validate_domain
        // at ingest, so the designer could offer a value it can never persist.
        assertFalse("uppercase is not ASCII-lowercase", isValidDomain("API.Example.com"))
        assertFalse("non-ASCII labels are rejected by the engine", isValidDomain("北京.cn"))

        assertTrue(isValidDomain("api.example.com"))
        assertTrue(isValidDomain("cdn-1.example.com"))
        assertFalse(isValidDomain("https://api.example.com"))
        assertFalse(isValidDomain("-api.example.com"))
        assertFalse(isValidDomain("api..example.com"))
    }

    private fun field(id: String) = LocalAppDataField(id, "新字段", LocalAppDataFieldType.Text, false)

    private fun oneStep(id: String = "basics", order: UInt = 1u) = LocalAppDesignStep(
        id = id,
        order = order,
        title = "基础",
        description = null,
        fields = listOf(designField()),
    )

    private fun designField(
        id: String = "purpose",
        allowsCustom: Boolean = false,
        allowsDefer: Boolean = false,
        options: List<LocalAppFieldOption> = emptyList(),
    ) = LocalAppDesignField(
        id = id,
        label = "用途",
        description = null,
        kind = LocalAppFieldKind.ShortText,
        required = true,
        allowsCustom = allowsCustom,
        allowsDefer = allowsDefer,
        defaultValue = null,
        options = options,
    )
}
