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

    /**
     * A second review round caught what a single-call test cannot: every one
     * of the tests above calls `commitCustomChipText` ONCE with `value = null`,
     * which always looks correct even if the function accumulates rather than
     * replaces. `onValueChange(..., true)` commits on every keystroke and the
     * committed value is fed straight back in as `value` on the next
     * recomposition — a naive `values + text` (no filter, since `DomainList`
     * declares no `options` the way `MultipleChoice` does) left every partial
     * keystroke permanently in the list: typing "api.example.com" produced
     * `["a","ap","api",…,"api.example.com"]`, silently polluting the
     * permissions-adjacent allowed-domains list with unvalidated fragments
     * (no format check runs on this path). This test drives the SAME
     * multi-keystroke feedback loop the real Composable does — threading
     * `previousCustomText` exactly as `DesignerFieldChips` threads its
     * remembered `customText` — and asserts the final list holds exactly the
     * typed text, once.
     */
    @Test
    fun `typing progressively into the other box on a domain list field replaces the slot instead of accumulating fragments`() {
        val field = designField(allowsCustom = true).copy(kind = LocalAppFieldKind.DomainList)
        var value: LocalAppDesignValue? = null
        var previous = ""
        for (keystroke in listOf("a", "ap", "api", "api.example.com")) {
            value = commitCustomChipText(field, keystroke, value, previous)
            previous = keystroke
        }
        assertEquals(LocalAppDesignValue.StringList(listOf("api.example.com")), value)
    }

    /**
     * A THIRD review round caught what the first fix's readback still got
     * wrong: seeding the box to a GUESS (the list's last entry) meant the
     * guess was fed back as `previousCustomText` on keystroke #1 — and
     * whenever the list is non-empty at mount, that guess is GUARANTEED to
     * equal a real pre-existing entry (a saved draft, or an LLM-authored
     * `default_value`, questionnaire.rs:92), so `commitCustomChipText`'s
     * identity-removal deleted it with certainty, not as an edge case. A
     * review's own probe reproduced this on a `DomainList` seeded with
     * `["a.com","b.com"]`: the seed became `"b.com"`, and committing a
     * single keystroke silently dropped it. `customTextFor` now always
     * returns `""` for these three kinds instead of guessing — this is that
     * honest readback, asserted directly.
     */
    @Test
    fun `the other box on a string list field always starts empty because the custom entry cannot be identified`() {
        val field = designField(allowsCustom = true).copy(kind = LocalAppFieldKind.DomainList)
        assertEquals("", customTextFor(field, LocalAppDesignValue.StringList(listOf("a.com", "b.com"))))
    }

    /**
     * The permanent regression test for the review's own probe: seeding the
     * box from a NON-EMPTY pre-existing list (exactly what `customTextFor`
     * now returns `""` for, and what `DesignerFieldChips` threads as the
     * FIRST `previousCustomText`) must not delete anything already there —
     * the first keystroke may only APPEND. Every pre-existing entry
     * (`a.com`, `b.com` — a `DomainList`'s allowed-hosts, permissions-
     * adjacent) must survive the user's very first keystroke into the box.
     */
    @Test
    fun `the first keystroke into the other box never deletes a pre-existing list entry`() {
        val field = designField(allowsCustom = true).copy(kind = LocalAppFieldKind.DomainList)
        val preExisting = LocalAppDesignValue.StringList(listOf("a.com", "b.com"))
        // Mirrors `DesignerFieldChips`'s own `remember(field.id) {
        // mutableStateOf(customTextFor(field, value)) }` cold-start seed.
        val coldStartSeed = customTextFor(field, preExisting)
        val afterFirstKeystroke = commitCustomChipText(field, "c.com", preExisting, previousCustomText = coldStartSeed)
        assertEquals(
            "a.com and b.com must both survive the very first keystroke",
            LocalAppDesignValue.StringList(listOf("a.com", "b.com", "c.com")),
            afterFirstKeystroke,
        )
    }

    /**
     * A pre-existing list entry (e.g. one added through `StringListEditor`'s
     * own separate add flow, in between two of the custom box's keystrokes)
     * must survive — only the custom box's OWN previously-committed text
     * (`previousCustomText`, threaded explicitly, not re-derived from
     * `value`'s list position) is replaced. An earlier version of this fix
     * used a "the custom entry is always the list's LAST element" position
     * heuristic instead, which a review caught silently dropping exactly
     * this kind of separately-added entry (it becomes "last" and gets
     * mistaken for the stale fragment).
     */
    @Test
    fun `typing into the other box on a domain list field does not disturb a separately added entry`() {
        val field = designField(allowsCustom = true).copy(kind = LocalAppFieldKind.DomainList)
        var value: LocalAppDesignValue = commitCustomChipText(field, "a", value = null, previousCustomText = "")
        // Simulate StringListEditor's own add flow appending a real entry
        // after the custom box's first keystroke.
        value = LocalAppDesignValue.StringList((value as LocalAppDesignValue.StringList).values + "cdn.example.com")
        value = commitCustomChipText(field, "ap", value, previousCustomText = "a")
        assertEquals(LocalAppDesignValue.StringList(listOf("cdn.example.com", "ap")), value)
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
