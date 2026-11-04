"""Exercise trigger decisions against realistic, independently captured gate output."""
import unittest
from unittest.mock import patch

import test_gate_triggers as trigger


class TriggerOutputTests(unittest.TestCase):
    gate = "check-client-protocol.sh"
    captured = (
        "CLIENT-PROTOCOL OK: generated TypeScript matches the resolved runtime\n"
        "...\n----------------------------------------------------------------------\n"
        "Ran 3 tests in 0.005s\n\nOK\n\nrunning 7 tests\n.......\n"
        "test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; "
        "0 filtered out; finished in 0.01s\n"
    )

    def decisions(self, *, invoked=None, standalone=None, invoked_rc=0, standalone_rc=0, gate=None):
        gate = gate or self.gate
        invoked = self.captured if invoked is None else invoked
        standalone = self.captured if standalone is None else standalone
        check_all = (
            f"=== RUNNING: {gate} ===\n{invoked}"
            f"=== RESULT: {gate} exit={invoked_rc} ===\n"
        )
        with patch.object(trigger, "discover_gates", return_value={gate}), \
             patch.object(trigger, "run_check_all", return_value=(invoked_rc, check_all)), \
             patch.object(trigger, "run_standalone", return_value=(standalone_rc, standalone)):
            return trigger.every_checked_in_gate_has_an_execution_trigger()

    def test_elapsed_times_alone_do_not_change_the_decision(self):
        later = self.captured.replace("0.005s", "0.023s").replace("0.01s", "1.25s")
        self.assertEqual(self.decisions(standalone=later), [])

    def test_python_test_count_changes_are_rejected(self):
        changed = self.captured.replace("Ran 3 tests", "Ran 4 tests")
        self.assertTrue(self.decisions(standalone=changed))

    def test_rust_test_count_and_result_changes_are_rejected(self):
        for changed in (
            self.captured.replace("7 passed", "8 passed"),
            self.captured.replace("test result: ok.", "test result: FAILED."),
        ):
            with self.subTest(changed=changed):
                self.assertTrue(self.decisions(standalone=changed))

    def test_exit_status_disagreement_is_rejected(self):
        self.assertTrue(self.decisions(standalone_rc=1))

    def test_empty_output_is_rejected_even_when_both_runs_match(self):
        self.assertTrue(self.decisions(invoked="", standalone=""))
        self.assertTrue(self.decisions(invoked="", standalone=self.captured))

    def test_non_timing_output_changes_are_rejected(self):
        changed = self.captured.replace("generated TypeScript matches", "stub claims")
        self.assertTrue(self.decisions(standalone=changed))

    def test_other_gates_keep_exact_output_comparison(self):
        changed = self.captured.replace("0.005s", "0.023s")
        self.assertTrue(self.decisions(gate="check-i18n-pairing.sh", standalone=changed))


if __name__ == "__main__":
    unittest.main()
