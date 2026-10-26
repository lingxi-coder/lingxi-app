import json
from pathlib import Path
import tempfile
import unittest

from verify_capture import CaptureError, validate


TRACE = "a" * 32
INSTANCE = "e7c60bcb-845a-4214-943d-3399a7a4f5c1"


def span(name, number, parent=None, start=10, end=90):
    item = {"name": name, "traceId": TRACE, "spanId": f"{number:016x}",
            "startTimeUnixNano": str(start), "endTimeUnixNano": str(end)}
    if parent:
        item["parentSpanId"] = f"{parent:016x}"
    return item


def batch(resource_key, scope_key, item_key, items):
    return {resource_key: [{"resource": {"attributes": [
        {"key": "service.name", "value": {"stringValue": "lingxi-test"}},
        {"key": "service.instance.id", "value": {"stringValue": INSTANCE}}]},
        scope_key: [{item_key: items}]}]}


class VerifyCaptureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.spans = [span("lingxi.headless.request", 1, start=1, end=100),
                      span("lingxi.orchestrator.turn.streaming", 2, 1, start=5, end=95),
                      span("lingxi.llm.request", 3, 2, start=10, end=30),
                      span("lingxi.tool.execute", 4, 2, start=40, end=60)]
        self.logs = [{"traceId": TRACE, "spanId": f"{3:016x}",
                      "attributes": [{"key": "status", "value": {"stringValue": "success"}}]}]
        self.metrics = [{"name": "lingxi.request.count", "sum": {"dataPoints": [
            {"asInt": "1", "timeUnixNano": "100", "attributes": []}]}}]

    def write(self):
        for filename, value in (
            ("traces", batch("resourceSpans", "scopeSpans", "spans", self.spans)),
            ("metrics", batch("resourceMetrics", "scopeMetrics", "metrics", self.metrics)),
            ("logs", batch("resourceLogs", "scopeLogs", "logRecords", self.logs)),
        ):
            (self.directory / f"{filename}.jsonl").write_text(json.dumps(value) + "\n")

    def verify(self, **kwargs):
        return validate(self.directory, "lingxi.headless.request", "lingxi.orchestrator.turn",
                        "lingxi.llm.request", "lingxi.tool.execute", **kwargs)

    def test_complete_request_requires_all_signals_and_correlated_logs(self):
        self.write()
        self.assertEqual(self.verify()["trace_id"], TRACE)
        self.logs[0]["traceId"] = "b" * 32
        self.write()
        with self.assertRaisesRegex(CaptureError, "correlated"):
            self.verify()

    def test_spans_with_unrelated_ids_do_not_prove_full_chain(self):
        self.spans[3]["traceId"] = "b" * 32
        self.write()
        with self.assertRaisesRegex(CaptureError, "complete"):
            self.verify()

    def test_missing_parent_or_root_is_rejected(self):
        self.spans[0]["parentSpanId"] = f"{99:016x}"
        self.write()
        with self.assertRaisesRegex(CaptureError, "complete"):
            self.verify()

    def test_async_guard_cleanup_can_end_after_request_and_turn(self):
        self.spans[1]["endTimeUnixNano"] = "102"
        self.spans[3]["endTimeUnixNano"] = "103"
        self.write()
        self.assertEqual(self.verify()["trace_id"], TRACE)

    def test_async_admission_may_end_before_its_turn(self):
        self.spans.append(span("lingxi.bridge.admission", 5, 1, start=2, end=3))
        self.spans[1]["parentSpanId"] = f"{5:016x}"
        self.write()
        self.assertEqual(self.verify()["trace_id"], TRACE)

    def test_model_cannot_start_before_its_turn(self):
        self.spans[2]["startTimeUnixNano"] = "4"
        self.write()
        with self.assertRaisesRegex(CaptureError, "start during"):
            self.verify()

    def test_tool_cannot_start_after_its_turn(self):
        self.spans[3]["startTimeUnixNano"] = "96"
        self.spans[3]["endTimeUnixNano"] = "110"
        self.write()
        with self.assertRaisesRegex(CaptureError, "start during"):
            self.verify()

    def test_parent_cycle_is_rejected(self):
        self.spans[1]["parentSpanId"] = self.spans[3]["spanId"]
        self.write()
        with self.assertRaisesRegex(CaptureError, "Cycle"):
            self.verify()

    def test_malformed_parent_cannot_masquerade_as_root(self):
        self.spans[0]["parentSpanId"] = "invalid-parent"
        self.write()
        with self.assertRaisesRegex(CaptureError, "Invalid parent"):
            self.verify()

    def test_no_zero_duration_model_placeholder(self):
        self.spans[2]["endTimeUnixNano"] = self.spans[2]["startTimeUnixNano"]
        self.write()
        with self.assertRaises(CaptureError):
            self.verify()

    def test_private_markers_and_high_cardinality_fields_are_rejected(self):
        self.logs[0]["body"] = {"stringValue": "synthetic-secret-123"}
        self.write()
        with self.assertRaisesRegex(CaptureError, "marker"):
            self.verify(forbidden=["synthetic-secret-123"])
        self.logs[0].pop("body")
        self.metrics[0]["sum"]["dataPoints"][0]["attributes"] = [
            {"key": "request_id", "value": {"stringValue": "example"}}]
        self.write()
        with self.assertRaisesRegex(CaptureError, "Forbidden attribute"):
            self.verify()

    def test_empty_metric_declaration_does_not_count_as_receipt(self):
        self.metrics[0]["sum"]["dataPoints"] = []
        self.write()
        with self.assertRaisesRegex(CaptureError, "received"):
            self.verify()

    def test_ephemeral_process_resource_is_allowed_but_not_datapoint_identity(self):
        self.write()
        self.assertTrue(self.verify()["ok"])
        self.metrics[0]["sum"]["dataPoints"][0]["attributes"] = [
            {"key": "service.instance.id", "value": {"stringValue": INSTANCE}}]
        self.write()
        with self.assertRaisesRegex(CaptureError, "remain a resource"):
            self.verify()

    def test_process_resource_must_have_generated_uuid_shape(self):
        self.write()
        path = self.directory / "metrics.jsonl"
        path.write_text(path.read_text().replace(INSTANCE, "private-machine-name"))
        with self.assertRaisesRegex(CaptureError, "Invalid ephemeral"):
            self.verify()

    def test_disabled_run_rejects_data_even_from_other_service(self):
        self.assertEqual(self.verify(expect_empty=True)["mode"], "disabled")
        self.write()
        with self.assertRaisesRegex(CaptureError, "disabled run"):
            self.verify(expect_empty=True, service="another-service")

    def test_malformed_capture_is_rejected(self):
        (self.directory / "traces.jsonl").write_text("not-json\n")
        with self.assertRaisesRegex(CaptureError, "Invalid JSON"):
            self.verify()


if __name__ == "__main__":
    unittest.main()
