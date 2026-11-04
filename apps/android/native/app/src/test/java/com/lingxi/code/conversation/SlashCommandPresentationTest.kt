package com.lingxi.code.conversation

class SlashCommandPresentationTest {
    @org.junit.Test fun commandPlacementAndSuggestionsRespectRuntimeMetadata() {
        org.junit.Assert.assertFalse(slashCommandEchoesRequest("/usage"))
        org.junit.Assert.assertFalse(slashCommandEchoesRequest("/plan open"))
        org.junit.Assert.assertTrue(slashCommandEchoesRequest("/plan Fix the UI"))
        org.junit.Assert.assertTrue(slashCommandEchoesRequest("/fusion compare"))
        org.junit.Assert.assertEquals(SlashMessagePlacement.Temporary, ConversationSlashCommand("recap").messagePlacement())
        org.junit.Assert.assertEquals(SlashMessagePlacement.Conditional, ConversationSlashCommand("status", source = "plugin").messagePlacement())
        val catalog = listOf(ConversationSlashCommand("help"), ConversationSlashCommand("secret", hidden = true))
        org.junit.Assert.assertEquals(listOf("help"), slashCommandSuggestions("/", catalog).map { it.name })
        org.junit.Assert.assertEquals(listOf("secret"), slashCommandSuggestions("/secret", catalog).map { it.name })
        org.junit.Assert.assertTrue(slashCommandSuggestions("/help argument", catalog).isEmpty())
    }
}
