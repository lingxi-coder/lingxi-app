"""Regression checks for telemetry smoke fixture execution evidence."""
import unittest

from run_agent_smoke import has_successful_tool_result


MARKER = "TELEMETRY_TOOL_OK"
TOOL_ID = "telemetry_tool_1"


def result_request(content, *, tool_id=TOOL_ID, role="user", **fields):
    return {"messages": [{"role": role, "content": [{
        "type": "tool_result", "tool_use_id": tool_id, "content": content, **fields,
    }]}]}


class SuccessfulToolResultTests(unittest.TestCase):
    def test_assistant_command_and_permission_denial_are_not_execution(self):
        request = {"messages": [
            {"role": "assistant", "content": [{
                "type": "tool_use", "id": TOOL_ID, "name": "Bash",
                "input": {"command": f"printf {MARKER}"},
            }]},
            {"role": "user", "content": [{
                "type": "tool_result", "tool_use_id": TOOL_ID, "is_error": True,
                "content": f"Permission denied: printf {MARKER}",
            }]},
        ]}
        self.assertFalse(has_successful_tool_result(request))

    def test_real_stdout_accepts_false_or_absent_error_flag(self):
        for fields in ({}, {"is_error": False}):
            with self.subTest(fields=fields):
                self.assertTrue(has_successful_tool_result(result_request(MARKER, **fields)))
                self.assertTrue(has_successful_tool_result(result_request(
                    [{"type": "text", "text": MARKER + "\n"}], **fields)))

    def test_read_line_numbers_and_trailing_system_reminders(self):
        for output in (
            f"1\t{MARKER}\n",
            f"     1→{MARKER}\n",
            f"1\t{MARKER}\n\n<system-reminder>Read-only fixture.</system-reminder>",
            f"{MARKER}\n\n<system-reminder>Tool output note.</system-reminder>",
        ):
            with self.subTest(output=output):
                self.assertTrue(has_successful_tool_result(result_request(output)))

    def test_marker_inside_reminder_or_denial_does_not_count(self):
        for output in (
            f"Permission denied: Read({MARKER})",
            f"1\tPermission denied: {MARKER}",
            f"<system-reminder>\n1\t{MARKER}\n</system-reminder>",
            f"No output.\n<system-reminder>\n{MARKER}\n</system-reminder>",
        ):
            with self.subTest(output=output):
                self.assertFalse(has_successful_tool_result(result_request(output)))
        self.assertFalse(has_successful_tool_result(result_request(f"1\t{MARKER}", is_error=True)))

    def test_requires_matching_user_result_and_exact_output(self):
        for request in (
            result_request(MARKER, tool_id="another_tool"),
            result_request(MARKER, role="assistant"),
            result_request(MARKER, is_error=True),
            result_request(MARKER, is_error=None),
            result_request(MARKER, is_error="false"),
            result_request(f"printf {MARKER}"),
            result_request(f"Permission denied: {MARKER}"),
            result_request([{"type": "image", "text": MARKER}]),
            {"messages": [{"role": "user", "content": MARKER}]},
        ):
            with self.subTest(request=request):
                self.assertFalse(has_successful_tool_result(request))

    def test_malformed_content_is_not_execution(self):
        for messages in (None, "text", {}, [None], [{"role": "user", "content": [None]}]):
            with self.subTest(messages=messages):
                self.assertFalse(has_successful_tool_result({"messages": messages}))


if __name__ == "__main__":
    unittest.main()
