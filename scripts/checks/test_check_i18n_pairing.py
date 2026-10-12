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


if __name__ == "__main__":
    unittest.main()
