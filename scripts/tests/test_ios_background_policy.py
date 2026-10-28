"""Keep permitted iOS background tasks tied to their reviewed source owners."""
import importlib.util
from pathlib import Path
import plistlib
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "ios_background_policy", ROOT / "scripts/mobile-linux/check-ios-background-policy.py"
)
POLICY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(POLICY)

SOURCE_FILES = (
    "Cron/CronModels.swift",
    "Cron/CronSystemAdapters.swift",
    "App/AppNotificationDelegate.swift",
    "App/ConversationBackgroundActivity.swift",
    "Apps/LocalAppAgentBackground.swift",
    "Voice/VoiceAudioSessionCoordinator.swift",
    "App/RootView.swift",
)


class IOSBackgroundPolicyTests(unittest.TestCase):
    def test_reviewed_identifiers_match_current_task_owners(self):
        prefix = POLICY.ios_bundle_id()
        self.assertEqual(POLICY.reviewed_identifiers(), {
            prefix + ".cron.reconcile",
            prefix + ".conversation.continued.*",
            prefix + ".local-app.*",
            prefix + ".full.local-app.*",
        })
        self.assertNotIn(prefix + ".full.conversation.continued.*", POLICY.reviewed_identifiers())

    def test_checked_in_plist_and_source_contracts_pass(self):
        POLICY.validate_plist(ROOT / POLICY.INFO, ROOT)

    def test_unreviewed_local_agent_submission_strategy_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            for relative in SOURCE_FILES:
                destination = repo / POLICY.SOURCES / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / POLICY.SOURCES / relative, destination)

            agent_source = repo / POLICY.SOURCES / "Apps/LocalAppAgentBackground.swift"
            agent_source.write_text(agent_source.read_text().replace(
                "request.strategy = .fail", "request.strategy = .queue"
            ))
            with self.assertRaisesRegex(ValueError, "missing reviewed processing identifier"):
                POLICY.validate_processing_sources(repo)

    def test_local_agent_requires_the_sdk_task_request_cancellation_api(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            for relative in SOURCE_FILES:
                destination = repo / POLICY.SOURCES / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / POLICY.SOURCES / relative, destination)

            agent_source = repo / POLICY.SOURCES / "Apps/LocalAppAgentBackground.swift"
            agent_source.write_text(agent_source.read_text().replace(
                "cancel(taskRequestWithIdentifier:", "cancelTaskRequest(withIdentifier:"
            ))
            with self.assertRaisesRegex(ValueError, "missing reviewed processing identifier"):
                POLICY.validate_processing_sources(repo)

    def assert_local_agent_mutation_rejected(self, original, replacement):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            for relative in SOURCE_FILES:
                destination = repo / POLICY.SOURCES / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / POLICY.SOURCES / relative, destination)
            source = repo / POLICY.SOURCES / "Apps/LocalAppAgentBackground.swift"
            text = source.read_text()
            self.assertEqual(text.count(original), 1, "mutation must target one reviewed source anchor")
            source.write_text(text.replace(original, replacement))
            with self.assertRaisesRegex(ValueError, "missing reviewed Local App Agent"):
                POLICY.validate_processing_sources(repo)

    def test_stale_contexts_and_identifiers_cannot_expire_or_cancel_current_work(self):
        mutations = (
            ("requestIDsByIdentifier[task.identifier]", 'requestIDsByIdentifier["other"]'),
            ("              isCurrentContext(contextKey, identifier: task.identifier),\n", ""),
            ("await self?.expire(contextKey, identifier: identifier)",
             'await self?.expire("other", identifier: identifier)'),
            ("await self?.expire(contextKey, identifier: identifier)",
             'await self?.expire(contextKey, identifier: "other")'),
            ("guard isCurrentContext(contextKey, identifier: identifier), let context = contexts[contextKey] else { return }",
             "guard let context = contexts[contextKey] else { return }"),
            ("if let expectedIdentifier, contexts[contextKey]?.identifier != expectedIdentifier { return }", ""),
            ("actualIdentifier == expectedIdentifier && !finished", "!finished"),
        )
        for original, replacement in mutations:
            with self.subTest(anchor=original, replacement=replacement):
                self.assert_local_agent_mutation_rejected(original, replacement)

    def test_cleanup_requires_the_owned_reservation_and_matching_request(self):
        mutations = (
            ("guard let ownedReservationID else { return }", ""),
            ("try await clear(ownedReservationID)", 'try await clear("other")'),
            ('"reservationId": .string(reservationID)', '"reservationId": .string("other")'),
            ("LocalAppAgentWakeCleanup.request(wake: wake, reservationID: reservationID, reason: reason)",
             'LocalAppAgentWakeCleanup.request(wake: wake, reservationID: "other", reason: reason)'),
            ("LocalAppAgentWakeCleanup.perform(\n                ownedReservationID: context.reservationID",
             "LocalAppAgentWakeCleanup.perform(\n                ownedReservationID: nil"),
            ('fields["reason"] = .string("system_expiration")',
             'fields["reason"] = .string("explicit")'),
        )
        for original, replacement in mutations:
            with self.subTest(anchor=original, replacement=replacement):
                self.assert_local_agent_mutation_rejected(original, replacement)

    def test_result_delivery_window_and_foreground_retry_remain_available(self):
        mutations = (
            ("browserDelivery.finishBackgroundTaskAfterStartingResultDelivery(",
             "browserDelivery.finishWithoutResultDelivery("),
            ("iOS could not start this Agent task immediately. Return to the app and try again.",
             "iOS could not start this Agent task immediately."),
        )
        for original, replacement in mutations:
            with self.subTest(anchor=original):
                self.assert_local_agent_mutation_rejected(original, replacement)

    def test_unreviewed_background_identifier_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            info_path = repo / POLICY.INFO
            info_path.parent.mkdir(parents=True)
            with (ROOT / POLICY.INFO).open("rb") as stream:
                info = plistlib.load(stream)
            info["BGTaskSchedulerPermittedIdentifiers"].append(
                POLICY.ios_bundle_id() + ".full.conversation.continued.*"
            )
            with info_path.open("wb") as stream:
                plistlib.dump(info, stream)

            with self.assertRaisesRegex(ValueError, "unreviewed processing modes / permitted identifiers"):
                POLICY.validate_plist(info_path, repo)


if __name__ == "__main__":
    unittest.main()
