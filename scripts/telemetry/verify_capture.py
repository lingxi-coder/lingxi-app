#!/usr/bin/env python3
"""Validate received OTLP JSON; never print captured bodies or attribute values."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys


class CaptureError(ValueError):
    pass


FORBIDDEN_ATTRIBUTES = {
    "prompt", "user_prompt", "user.message", "response", "assistant_response",
    "tool_parameters", "tool_arguments", "tool_result", "tool_content", "path",
    "file.path", "cwd", "url", "http.url", "url.full", "authorization",
    "api_key", "token", "account_uuid", "user.id", "user.email", "host.name",
    "session_id", "session.id", "request_id", "request.id",
    "process.command_line", "exception.message", "exception.stacktrace",
}
MEASUREMENT_ATTRIBUTES = {
    "duration_ms", "durationMs", "elapsed_ms", "uptime_ms", "attempt",
    "attempt_number", "retry_count", "turn_index", "input_tokens", "output_tokens",
    "cache_read_tokens", "cache_creation_tokens", "inputTokens", "outputTokens",
    "cachedInputTokens", "uncachedInputTokens", "cost_usd", "cost_usd_micros",
    "costUSD", "first_token_ms", "prompt_length", "response_length", "tool_count",
}


def records(directory: Path, signal: str) -> list[dict]:
    result = []
    for path in sorted(directory.glob(f"{signal}*.jsonl")):
        for line_number, line in enumerate(path.read_text().splitlines(), 1):
            if not line.strip():
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError as error:
                raise CaptureError(f"Invalid JSON in {path.name}:{line_number}") from error
            if not isinstance(value, dict):
                raise CaptureError(f"Expected OTLP object in {path.name}:{line_number}")
            result.append(value)
    return result


def check_privacy(value, forbidden: list[str], metric: bool = False, path: tuple = ()):
    if isinstance(value, dict):
        key = value.get("key")
        if isinstance(key, str) and (key in FORBIDDEN_ATTRIBUTES or
                                    (metric and key in MEASUREMENT_ATTRIBUTES)):
            raise CaptureError("Forbidden attribute found in exported telemetry")
        if key == "service.instance.id":
            resource_attribute = path in {
                (signal, "resource", "attributes")
                for signal in ("resourceSpans", "resourceMetrics", "resourceLogs")
            }
            instance_id = value.get("value", {}).get("stringValue")
            if not resource_attribute:
                raise CaptureError("Process identity must remain a resource attribute")
            if not isinstance(instance_id, str) or not re.fullmatch(
                    r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}",
                    instance_id):
                raise CaptureError("Invalid ephemeral process identity")
        for name, item in value.items():
            check_privacy(item, forbidden, metric, (*path, name))
    elif isinstance(value, list):
        for item in value:
            check_privacy(item, forbidden, metric, path)
    elif isinstance(value, str) and any(marker in value for marker in forbidden):
        raise CaptureError("Private acceptance marker found in exported telemetry")


def flatten(batches: list[dict], resource_key: str, scope_key: str, item_key: str,
            service: str | None) -> list[dict]:
    result = []
    for batch in batches:
        for resource in batch.get(resource_key, []):
            attrs = resource.get("resource", {}).get("attributes", [])
            service_name = next((a.get("value", {}).get("stringValue") for a in attrs
                                 if a.get("key") == "service.name"), None)
            if service is not None and service_name != service:
                continue
            for scope in resource.get(scope_key, []):
                result.extend(scope.get(item_key, []))
    return result


def valid_id(value: str | None, size: int) -> bool:
    return bool(value and re.fullmatch(rf"[0-9a-fA-F]{{{size}}}", value)
                and int(value, 16) != 0)


def descendant(span: dict, ancestor: dict, index: dict) -> bool:
    current = span
    seen = set()
    while current.get("parentSpanId"):
        parent_id = current["parentSpanId"].lower()
        if parent_id in seen:
            raise CaptureError("Cycle in trace parent graph")
        seen.add(parent_id)
        parent = index.get((current["traceId"].lower(), parent_id))
        if parent is None:
            return False
        if parent is ancestor:
            return True
        current = parent
    return False


def require_started_in_turn(span: dict, turn: dict):
    # Parent links express causality, not strict wall-clock containment. A host
    # can receive turn_ended before Rust finishes dropping its operation guards.
    # Model/tool work must start during the turn, but its exported end can be
    # later than the turn or request root.
    started = int(span["startTimeUnixNano"])
    if not int(turn["startTimeUnixNano"]) <= started <= int(turn["endTimeUnixNano"]):
        raise CaptureError("Model/tool span did not start during its turn")


def validate(directory: Path, root_name: str, turn_prefix: str, model_name: str,
             tool_name: str, service: str | None = None, forbidden: list[str] | None = None,
             expect_empty: bool = False) -> dict:
    if not directory.is_dir():
        raise CaptureError("Capture directory does not exist")
    data = {signal: records(directory, signal) for signal in ("traces", "metrics", "logs")}
    for signal, batches in data.items():
        check_privacy(batches, forbidden or [], metric=signal == "metrics")
    spans = flatten(data["traces"], "resourceSpans", "scopeSpans", "spans", service)
    metrics = flatten(data["metrics"], "resourceMetrics", "scopeMetrics", "metrics", service)
    logs = flatten(data["logs"], "resourceLogs", "scopeLogs", "logRecords", service)
    metric_points = sum(len(metric.get(kind, {}).get("dataPoints", []))
                        for metric in metrics
                        for kind in ("sum", "gauge", "histogram", "exponentialHistogram", "summary"))
    counts = {"spans": len(spans), "metric_points": metric_points, "logs": len(logs)}
    if expect_empty:
        # Empty mode covers every service, even if --service was supplied.
        if any(flatten(data[s], rk, sk, ik, None) for s, rk, sk, ik in (
            ("traces", "resourceSpans", "scopeSpans", "spans"),
            ("metrics", "resourceMetrics", "scopeMetrics", "metrics"),
            ("logs", "resourceLogs", "scopeLogs", "logRecords"),
        )):
            raise CaptureError("Telemetry received during disabled run")
        return {"ok": True, "mode": "disabled", **counts}
    if not all(counts.values()):
        raise CaptureError("Expected received traces, metrics and logs")
    index = {}
    for span in spans:
        if not valid_id(span.get("traceId"), 32) or not valid_id(span.get("spanId"), 16):
            raise CaptureError("Invalid trace/span ID")
        parent_id = span.get("parentSpanId")
        if parent_id and parent_id != "0" * 16 and not valid_id(parent_id, 16):
            raise CaptureError("Invalid parent span ID")
        try:
            if int(span["endTimeUnixNano"]) <= int(span["startTimeUnixNano"]):
                raise CaptureError("Span has no positive completed duration")
        except (KeyError, ValueError) as error:
            raise CaptureError("Missing or invalid span timestamps") from error
        key = (span["traceId"].lower(), span["spanId"].lower())
        if key in index:
            raise CaptureError("Duplicate span ID in capture")
        index[key] = span
    roots = [s for s in spans if s.get("name") == root_name
             and not valid_id(s.get("parentSpanId"), 16)]
    for root in roots:
        request_spans = [s for s in spans if descendant(s, root, index)]
        turns = [s for s in request_spans if s.get("name", "").startswith(turn_prefix)]
        for turn in turns:
            children = [s for s in spans if descendant(s, turn, index)]
            names = {s.get("name") for s in children}
            if not {model_name, tool_name}.issubset(names):
                continue
            for child in children:
                if child.get("name") in (model_name, tool_name):
                    require_started_in_turn(child, turn)
            trace_id = root["traceId"].lower()
            request_span_ids = {s["spanId"].lower() for s in [root, turn, *children]}
            if not any(log.get("traceId", "").lower() == trace_id and
                       log.get("spanId", "").lower() in request_span_ids for log in logs):
                raise CaptureError("No log is correlated with the completed request trace")
            return {"ok": True, "mode": "request", **counts,
                    "trace_id": trace_id, "root": root_name, "turn": turn["name"],
                    "model": model_name, "tool": tool_name}
    raise CaptureError("No complete root -> turn -> model/tool trace found")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--root-span", default="lingxi.headless.request")
    parser.add_argument("--turn-prefix", default="lingxi.orchestrator.turn")
    parser.add_argument("--model-span", default="lingxi.llm.request")
    parser.add_argument("--tool-span", default="lingxi.tool.execute")
    parser.add_argument("--service")
    parser.add_argument("--forbid", action="append", default=[], help="Synthetic private test marker")
    parser.add_argument("--expect-empty", action="store_true")
    args = parser.parse_args()
    try:
        result = validate(args.directory, args.root_span, args.turn_prefix, args.model_span,
                          args.tool_span, args.service, args.forbid, args.expect_empty)
    except (CaptureError, OSError) as error:
        print(json.dumps({"ok": False, "error": str(error)}), file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
