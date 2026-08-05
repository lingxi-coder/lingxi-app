package com.lingxi.code.theme

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Integrity checks for the in-app language catalog: the exact supported list
 * (code → label, follow-system first) and the [AppLanguage.canonical] mapping
 * from in-app codes to the BCP-47 tags fed to `Locale.forLanguageTag`.
 */
class AppLanguageTest {

    @Test
    fun supportedLanguages_followSystemFirst_thenSixOptionsInOrder() {
        assertEquals(
            listOf("", "zh-CN", "zh-TW", "en-US", "ja-JP", "ko-KR"),
            AppLanguage.SUPPORTED.map { it.first },
        )
        assertEquals(
            listOf("跟随系统", "简体中文", "繁體中文", "English", "日本語", "한국어"),
            AppLanguage.SUPPORTED.map { it.second },
        )
    }

    @Test
    fun canonical_mapsEverySupportedCodeToItsBcp47Tag() {
        assertEquals("", AppLanguage.canonical(""))
        assertEquals("zh-Hans", AppLanguage.canonical("zh-CN"))
        assertEquals("zh-Hant", AppLanguage.canonical("zh-TW"))
        assertEquals("en", AppLanguage.canonical("en-US"))
        assertEquals("ja", AppLanguage.canonical("ja-JP"))
        assertEquals("ko", AppLanguage.canonical("ko-KR"))
    }

    @Test
    fun canonical_unknownCodeFallsBackToSystem() {
        assertEquals("", AppLanguage.canonical("fr-FR"))
        assertEquals("", AppLanguage.canonical("nonsense"))
    }

    @Test
    fun label_resolvesEverySupportedCode() {
        AppLanguage.SUPPORTED.forEach { (code, label) ->
            assertEquals(label, AppLanguage.label(code))
        }
    }

    @Test
    fun label_unknownCodeFallsBackToFollowSystem() {
        assertEquals("跟随系统", AppLanguage.label("fr-FR"))
    }
}
