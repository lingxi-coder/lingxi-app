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


class IntentionalIdenticalTranslationTests(unittest.TestCase):
    def test_exact_english_ui_terms_are_excluded(self):
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
