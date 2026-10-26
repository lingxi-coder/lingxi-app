"""Optional integration test against the pinned native development Collector."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from urllib.request import Request

from run_agent_smoke import LOCAL_HTTP, port, wait_health
from verify_capture import flatten, records


@unittest.skipUnless(os.environ.get('OTELCOL_CONTRIB'), 'Set OTELCOL_CONTRIB to run the native Collector regression')
class CollectorSchemaTests(unittest.TestCase):
    def test_keeps_hook_phases_and_failure_classes_without_private_fields(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            grpc, http, health = port(), port(), port()
            config = (Path(__file__).resolve().parents[2] / 'deploy/telemetry/collector.yaml').read_text()
            config = config.replace(':4317', f':{grpc}').replace(':4318', f':{http}').replace(':13133', f':{health}')
            config_path = root / 'collector.yaml'
            config_path.write_text(config)
            env = {**os.environ, 'LINGXI_OTEL_BIND_ADDRESS': '127.0.0.1', 'LINGXI_OTEL_OUTPUT_DIR': str(root)}
            with (root / 'collector.log').open('w') as log:
                process = subprocess.Popen([os.environ['OTELCOL_CONTRIB'], '--config', str(config_path)], env=env, stdout=log, stderr=log)
                try:
                    wait_health(f'http://127.0.0.1:{health}/', process)
                    def attrs(**values):
                        return [{'key': key, 'value': {'stringValue': value}} for key, value in values.items()]
                    points = [{'asInt': '1', 'timeUnixNano': str(time.time_ns()),
                               'attributes': attrs(phase=phase, tool_name='Read', prompt='PRIVATE_CANARY')}
                              for phase in ('pre', 'post')]
                    logs = [{'timeUnixNano': str(time.time_ns()), 'body': {'stringValue': 'PRIVATE_CANARY'},
                             'attributes': attrs(error_kind=kind, prompt='PRIVATE_CANARY')}
                            for kind in ('network', 'rate_limited')]
                    payloads = {
                        'metrics': {'resourceMetrics': [{'scopeMetrics': [{'metrics': [{'name': 'hook.count', 'sum': {'aggregationTemporality': 2, 'isMonotonic': True, 'dataPoints': points}}]}]}]},
                        'logs': {'resourceLogs': [{'scopeLogs': [{'logRecords': logs}]}]},
                    }
                    for signal, payload in payloads.items():
                        request = Request(f'http://127.0.0.1:{http}/v1/{signal}', data=json.dumps(payload).encode(), headers={'Content-Type': 'application/json'})
                        with LOCAL_HTTP.open(request, timeout=5) as response:
                            self.assertEqual(response.status, 200)
                finally:
                    process.terminate()
                    process.wait(timeout=10)
            metrics = flatten(records(root, 'metrics'), 'resourceMetrics', 'scopeMetrics', 'metrics', None)
            exported_points = metrics[0]['sum']['dataPoints']
            exported_logs = flatten(records(root, 'logs'), 'resourceLogs', 'scopeLogs', 'logRecords', None)
            def values(items, key):
                return {attr['value']['stringValue'] for item in items for attr in item.get('attributes', []) if attr['key'] == key}
            self.assertEqual(values(exported_points, 'phase'), {'pre', 'post'})
            self.assertEqual(values(exported_logs, 'error_kind'), {'network', 'rate_limited'})
            self.assertNotIn('PRIVATE_CANARY', json.dumps([metrics, exported_logs]))


if __name__ == '__main__':
    unittest.main()
