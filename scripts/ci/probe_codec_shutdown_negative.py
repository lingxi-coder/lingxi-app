#!/usr/bin/env python3
"""Run controlled regressions against the Windows codec shutdown tests."""
from pathlib import Path
import hashlib
import os
import signal
import subprocess
import sys
import tempfile

source = Path('packages/remote-control-client/src/isolated_codec.rs')
before = source.read_bytes()
original = before.decode('utf8')
newline = '\r\n' if b'\r\n' in before else '\n'

cases = [
    {
        'label': 'premature Q before the admitted audio request',
        'old': '''                    tokio::try_join!(close_audio, close_video)?;
                    stage!("parent closed both data write halves; sending Q");
                    control_write.write_all(b"Q").await?;''',
        'new': '''                    control_write.write_all(b"Q").await?;
                    tokio::try_join!(close_audio, close_video)?;
                    stage!("parent closed both data write halves; sending Q");''',
        'target': ['--test', 'codec_worker'],
        'test': 'shutdown_drains_admitted_request_before_data_fin_and_control_q',
        'expected': 'Q was delivered before the admitted request completed',
    },
    {
        'label': 'partial frame treated as clean EOF',
        'old': '    stream.read_exact(&mut lens[1..])?;',
        'new': '''    if let Err(error) = stream.read_exact(&mut lens[1..]) {
        if error.kind() == io::ErrorKind::UnexpectedEof { return Ok(None); }
        return Err(error);
    }''',
        'target': ['--lib'],
        'test': 'isolated_codec::tests::codec_frame_eof_is_clean_only_between_complete_frames',
        'expected': 'unwrap_err()',
    },
    {
        'label': 'real worker sends D after a partial frame',
        'old': 'if requested_stop && audio_ok && video_ok && !fatal.load(Ordering::Acquire)',
        'new': 'if requested_stop && video_ok && !fatal.load(Ordering::Acquire)',
        'count': 2,
        'target': ['--test', 'codec_worker'],
        'test': 'real_worker_partial_frame_never_sends_clean_receipt',
        'expected': 'partial frame must not receive graceful D',
    },
    {
        'label': 'stuck request exceeds shutdown bound',
        'old': 'const STOP_TIMEOUT: Duration = Duration::from_secs(5);',
        'new': 'const STOP_TIMEOUT: Duration = Duration::from_secs(20);',
        'target': ['--test', 'codec_worker'],
        'test': 'stuck_admitted_request_makes_bounded_shutdown_unverified',
        'expected': 'shutdown must be bounded even while the lane lock is held',
    },
    {
        'label': 'stop timeout incorrectly accepted without child exit',
        'old': '''                stage!("parent stop timeout; requesting kill");
                let _ = self.inner.force.try_send(());
                return Err(failure("codec worker stop unverified"));''',
        'new': '''                stage!("parent stop timeout; requesting kill");
                let _ = self.inner.force.try_send(());
                return Ok(());''',
        'target': ['--test', 'codec_worker'],
        'test': 'worker_receipt_without_actual_exit_is_unverified',
        'expected': 'a FIN or D receipt without child exit must remain unverified',
    },
]

def terminate_exact_child(process):
    kill_error = None
    try:
        process.kill()
    except OSError as error:
        kill_error = str(error)
    try:
        status = process.wait(timeout=2)
        detail = f'exact child reaped with status {status}'
    except (subprocess.TimeoutExpired, OSError) as error:
        detail = f'exact child termination unconfirmed: {error}'
    return f'{detail}; kill error={kill_error!r}; descendant status unverified'


def run_bounded(command, deadline):
    # A regular file avoids a pipe held open by grandchildren after the
    # supervised process times out. Never wait indefinitely after kill.
    with tempfile.TemporaryFile(mode='w+b') as output:
        process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT)
        try:
            status = process.wait(timeout=deadline)
        except subprocess.TimeoutExpired as error:
            detail = terminate_exact_child(process)
            raise RuntimeError(f'subprocess exceeded deadline; {detail}') from error
        except BaseException:
            print(terminate_exact_child(process), flush=True)
            raise
        output.seek(0)
        return status, output.read().decode('utf8', errors='replace')


def interrupt(_signum, _frame):
    raise SystemExit('negative-control script interrupted')


signal.signal(signal.SIGTERM, interrupt)
selftest = os.environ.get('CODEC_SHUTDOWN_NEGATIVE_SELFTEST')
if selftest not in (None, 'exception', 'timeout', 'unconfirmed'):
    raise RuntimeError(f'unknown negative-control self-test: {selftest}')
selected = os.environ.get('CODEC_SHUTDOWN_NEGATIVE_ONLY')
if selected and selected not in {case['label'] for case in cases}:
    raise RuntimeError(f'unknown negative control: {selected}')

try:
    for case in cases:
        if selected and case['label'] != selected:
            continue
        count = case.get('count', 1)
        old = case['old'].replace('\n', newline)
        new = case['new'].replace('\n', newline)
        if original.count(old) != count:
            raise RuntimeError(f"unexpected negative target count: {case['label']}")
        source.write_bytes(original.replace(old, new, count).encode('utf8'))
        if selftest == 'exception':
            raise RuntimeError('synthetic failure after mutation')
        if selftest == 'timeout':
            run_bounded([sys.executable, '-c', 'import time; time.sleep(60)'], 0.25)
            raise RuntimeError('timeout fixture incorrectly returned')
        if selftest == 'unconfirmed':
            class NoExitReceipt:
                def kill(self):
                    raise OSError('synthetic kill failure')

                def wait(self, timeout):
                    assert timeout == 2
                    raise subprocess.TimeoutExpired('synthetic child', timeout)

            raise RuntimeError(terminate_exact_child(NoExitReceipt()))
        command = [
            'cargo', 'test', '--locked', '-p', 'remote-control-client', '--features',
            'native-media,codec-trace', *case['target'], case['test'], '--', '--exact',
            '--test-threads=1', '--nocapture', '--color', 'never',
        ]
        status, output = run_bounded(command, 600)
        print(output, flush=True)
        if (status == 0 or '1 failed;' not in output
                or case['expected'] not in output):
            raise RuntimeError(f"negative control did not fail as expected: {case['label']}")
        print(f"Negative control detected: {case['label']}", flush=True)
finally:
    source.write_bytes(before)
    restored = source.read_bytes()
    if restored != before:
        raise RuntimeError('failed to restore codec source after negative controls')
    print(f"Restored codec source SHA-256: {hashlib.sha256(restored).hexdigest()}", flush=True)
