#!/usr/bin/env python3
"""Focused checks for generated Android resources recognized by i18n pairing."""

import json
import tempfile
import unittest
from pathlib import Path

import check_i18n_pairing


class GoogleServicesGeneratedResourceTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.repo = Path(self.temp_dir.name)
        app = self.repo / "apps/android/native/app"
        app.mkdir(parents=True)
        self.gradle_path = app / "build.gradle.kts"
        self.config_path = app / "google-services.json"
        self.gradle_path.write_text(
            'plugins { id("com.google.gms.google-services") }\n'
            'defaultConfig {\n    applicationId = "com.example.app"\n}\n',
            encoding="utf-8",
        )
        self.config_path.write_text(
            json.dumps(
                {
                    "client": [
                        {
                            "client_info": {
                                "android_client_info": {
                                    "package_name": "com.example.app"
                                }
                            },
                            "oauth_client": [
                                {"client_type": 3, "client_id": "web-client"}
                            ],
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_matching_plugin_package_and_web_client_verify_generated_resource(self):
        self.assertEqual(
            check_i18n_pairing._verified_google_services_resource_names(self.repo),
            {"default_web_client_id"},
        )

    def test_missing_plugin_does_not_exempt_resource_reference(self):
        self.gradle_path.write_text(
            'defaultConfig {\n    applicationId = "com.example.app"\n}\n',
            encoding="utf-8",
        )
        self.assertEqual(
            check_i18n_pairing._verified_google_services_resource_names(self.repo), set()
        )

    def test_catalog_alias_requires_matching_plugin_id_and_application(self):
        self.gradle_path.write_text(
            'plugins { alias(libs.plugins.google.services) }\n'
            'defaultConfig {\n    applicationId = "com.example.app"\n}\n',
            encoding="utf-8",
        )
        catalog = self.repo / check_i18n_pairing.ANDROID_VERSION_CATALOG
        catalog.parent.mkdir(parents=True)
        catalog.write_text('[plugins]\ngoogle-services = { id = "com.google.gms.google-services", version = "1" }\n')
        self.assertEqual(
            check_i18n_pairing._verified_google_services_resource_names(self.repo),
            {"default_web_client_id"},
        )
        catalog.write_text('[plugins]\ngoogle-services = { id = "com.example.other", version = "1" }\n')
        self.assertEqual(check_i18n_pairing._verified_google_services_resource_names(self.repo), set())
        catalog.write_text('[plugins]\nother = { id = "com.google.gms.google-services", version = "1" }\n')
        self.assertEqual(check_i18n_pairing._verified_google_services_resource_names(self.repo), set())

    def test_mismatched_package_or_missing_web_client_does_not_exempt(self):
        config = json.loads(self.config_path.read_text(encoding="utf-8"))
        config["client"][0]["client_info"]["android_client_info"]["package_name"] = (
            "com.example.other"
        )
        self.config_path.write_text(json.dumps(config), encoding="utf-8")
        self.assertEqual(
            check_i18n_pairing._verified_google_services_resource_names(self.repo), set()
        )

        config["client"][0]["client_info"]["android_client_info"]["package_name"] = (
            "com.example.app"
        )
        config["client"][0]["oauth_client"] = [
            {"client_type": 1, "client_id": "android-client"}
        ]
        self.config_path.write_text(json.dumps(config), encoding="utf-8")
        self.assertEqual(
            check_i18n_pairing._verified_google_services_resource_names(self.repo), set()
        )


class AndroidDefaultResourceTests(unittest.TestCase):
    def test_discovers_hand_maintained_defaults_without_trusting_generated_or_other_locales(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            values = repo / check_i18n_pairing.ANDROID_RES_ROOT / "values"
            values.mkdir(parents=True)
            (values / "local_apps_v2.xml").write_text(
                '<resources><!-- <string name="commented">No</string> -->'
                '<string translatable="false" name="actual">Yes</string></resources>',
                encoding="utf-8",
            )
            (values / "strings.xml").write_text(
                '<resources><string name="stale_generated">Old</string></resources>',
                encoding="utf-8",
            )
            foreign = values.parent / "values-ja"
            foreign.mkdir()
            (foreign / "extra.xml").write_text(
                '<resources><string name="localized_only">No default</string></resources>',
                encoding="utf-8",
            )
            self.assertEqual(check_i18n_pairing._android_companion_names(repo), {"actual"})

    def test_all_manual_strings_require_nonempty_locales_and_matching_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            res = repo / check_i18n_pairing.ANDROID_RES_ROOT
            for name in check_i18n_pairing.ANDROID_LOCALE_DIR.values():
                (res / name).mkdir(parents=True)
            (res / "values/manual.xml").write_text(
                '<resources><string name="title">Title</string>'
                '<string name="args">%1$s: %2$.4f</string>'
                '<string name="unindexed">%s %d</string>'
                '<string name="literal" formatted="false">100% complete</string>'
                '<string name="private" translatable="false">internal</string></resources>',
                encoding="utf-8",
            )
            valid = ('<resources><string name="title">Translated</string>'
                     '<string name="args">%2$.4f / %1$s</string>'
                     '<string name="unindexed">%s %d</string>'
                     '<string name="literal" formatted="false">Complete 100%</string></resources>')
            for name in ("values-en", "values-ja", "values-ko", "values-zh-rTW"):
                (res / name / "manual.xml").write_text(valid, encoding="utf-8")
            self.assertEqual(check_i18n_pairing._android_companion_locale_issues(repo), ({}, {}, {}, {}))

            (res / "values-ko/manual.xml").write_text(
                '<resources><string name="title"> </string>'
                '<string name="args">%1$d</string>'
                '<string name="unindexed">%d %s</string></resources>', encoding="utf-8"
            )
            # A stale generated file must not conceal the missing manual value.
            (res / "values-ko/strings.xml").write_text(
                '<resources><string name="literal">stale</string></resources>', encoding="utf-8"
            )
            (res / "values-ja/extra.xml").write_text(
                '<resources><string name="orphan">Extra</string></resources>', encoding="utf-8"
            )
            gaps, empty, formats, extra = check_i18n_pairing._android_companion_locale_issues(repo)
            self.assertEqual(gaps, {"ko": ["literal"]})
            self.assertEqual(empty, {"ko": ["title"]})
            self.assertEqual(formats, {"ko": ["args", "unindexed"]})
            self.assertEqual(extra, {"ja": ["orphan"]})

    def test_manual_locale_issues_cannot_be_hidden_by_a_baseline(self):
        locales = check_i18n_pairing.LOCALES
        baseline = {
            "totalKeys": 1,
            "androidIllegalUnpaired": 0,
            "iosSourceOrphans": 0,
            "androidSourceOrphans": 0,
            "empty": {locale: 0 for locale in locales},
            "identicalToBase": {locale: 0 for locale in locales},
            "androidCompanionLocaleGapsTotal": 999,
        }
        current = dict(baseline, androidCompanionLocaleGapsTotal=1,
                       androidCompanionLocaleGaps={"ja": ["missing"]},
                       androidCompanionEmpty={"ko": ["blank"]},
                       androidCompanionFormatErrors={"en": ["format"]},
                       androidCompanionExtra={"zh-Hant": ["extra"]})
        problems = check_i18n_pairing.compare(baseline, current)
        self.assertEqual(len(problems), 4)
        self.assertTrue(any("missing" in problem for problem in problems))
        self.assertTrue(any("blank" in problem for problem in problems))
        self.assertTrue(any("format" in problem for problem in problems))
        self.assertTrue(any("extra" in problem for problem in problems))

    def test_duplicate_manual_strings_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            values = repo / check_i18n_pairing.ANDROID_RES_ROOT / "values"
            values.mkdir(parents=True)
            for name in ("first.xml", "second.xml"):
                (values / name).write_text('<resources><string name="same">value</string></resources>')
            with self.assertRaises(SystemExit):
                check_i18n_pairing._android_companion_names(repo)


class IntentionalIdenticalTranslationTests(unittest.TestCase):
    def test_exact_reviewed_ui_terms_are_excluded(self):
        for (locale, key), value in check_i18n_pairing.INTENTIONAL_IDENTICAL_TO_BASE.items():
            with self.subTest(key=key):
                self.assertTrue(
                    check_i18n_pairing._is_intentional_identical_to_base(
                        locale, key, value, value
                    )
                )

    def test_chinese_copy_wrong_locale_and_wrong_key_still_count(self):
        base = {
            "drawer_tab_chats": "Chat",
            "drawer_tab_chat": "Chat",
            "new_chinese_key": "对话",
        }
        english = {
            "drawer_tab_chats": "Chat",
            "drawer_tab_chat": "Chat",
            "new_chinese_key": "对话",
        }
        japanese = {"drawer_tab_chats": "Chat"}
        self.assertEqual(
            check_i18n_pairing._identical_to_base_count(
                "en", list(base), base, english
            ),
            2,
        )
        self.assertEqual(
            check_i18n_pairing._identical_to_base_count(
                "ja", ["drawer_tab_chats"], base, japanese
            ),
            1,
        )
        self.assertFalse(
            check_i18n_pairing._is_intentional_identical_to_base(
                "en", "drawer_tab_chats", "对话", "对话"
            )
        )


class SwiftUILocalizedStringReferenceTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.repo = Path(self.temp_dir.name)

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_project_resource_keys_only_from_swiftui_first_arguments(self):
        source_root = self.repo / check_i18n_pairing.IOS_SOURCE_ROOT
        source_root.mkdir(parents=True)
        (source_root / "Localized.swift").write_text(
            r"""
            Text("drawer_open_sidebar_a11y")
            Label("account_title", systemImage: "account_system_image")
            SecureField("account_password", text: $password)
            TextField("account_email", text: $email)
            Button("account_sign_in") {}
            Toggle("account_enabled", isOn: $enabled)
            Text(register ? "account_email_register" : "account_email_sign_in")
            Text("All")
            Button("Retry") {}
            Text(custom ? "Pause" : "Resume")
            Label("People", systemImage: "not_a_resource_key")
            Text("account_form_title" + suffix)
            Text("value \\(name)")
            String(localized: "A")
            """,
            encoding="utf-8",
        )
        self.assertEqual(
            set(check_i18n_pairing._ios_source_refs(self.repo)),
            {
                "drawer_open_sidebar_a11y",
                "account_title",
                "account_password",
                "account_email",
                "account_sign_in",
                "account_enabled",
                "account_email_register",
                "account_email_sign_in",
                # The pre-existing explicit localization scanner is unchanged.
                "A",
            },
        )
if __name__ == "__main__":
    unittest.main()
