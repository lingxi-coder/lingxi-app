#!/usr/bin/env python3
"""Collect bounded Linux MCP shutdown snapshots without inspecting process data.

Run as a shell child. The parent creates a completion marker after cargo exits;
the monitor then writes a report for the shell to print only on test failure.
No command line, environment, file-descriptor targets, or process output is read.
"""

import os
import re
import signal
import sys
import time
from pathlib import Path


INTERVAL = 0.2
WORKER_EXIT_BUDGET = 0.6
SNAPSHOT_INTERVAL = 1.0
MAX_PROCESSES = 64
MAX_THREADS = 64
MAX_SNAPSHOTS = 6
TEST_BINARY = re.compile(r"mcp_serve-[0-9a-f]+\Z")
THREAD_NAMES = {
    "lingxi-mcp-runt": "shutdown",
    "lingxi-cli-runt": "cli",
    "tokio-runtime-w": "tokio",
    "lingxi-cli": "main",
}
WAIT_CHANNELS = {
    "futex_wait_queue": "futex",
    "futex_wait": "futex",
    "futex_wait_queue_me": "futex",
    "ep_poll": "epoll",
    "do_epoll_wait": "epoll",
    "do_poll": "poll",
    "do_sys_poll": "poll",
    "poll_schedule_timeout": "poll",
    "hrtimer_nanosleep": "timer",
    "schedule_hrtimeout_range_clock": "timer",
    "pipe_read": "pipe-read",
    "pipe_write": "pipe-write",
    "do_wait": "wait",
    "do_wait_intr": "wait",
    "wait_for_completion": "completion",
    "wait_woken": "wait",
    "0": "running-or-restricted",
}


def read_small(path, limit=512):
    try:
        with path.open("rb") as stream:
            value = stream.read(limit + 1)
        if len(value) > limit:
            return None
        return value.decode("ascii").strip()
    except (OSError, UnicodeError):
        return None


def process_details(proc_root, pid):
    """Return kernel start time and PPID, neither derived from process input."""
    stat = read_small(proc_root / str(pid) / "stat", 4096)
    if stat is None or ")" not in stat:
        return None
    fields = stat.rsplit(")", 1)[1].split()
    try:
        return int(fields[19]), int(fields[1])  # stat fields 22 and 4
    except (IndexError, ValueError):
        return None


def process_identity(proc_root, pid):
    details = process_details(proc_root, pid)
    return details[0] if details is not None else None


def executable_name(proc_root, pid):
    try:
        return os.path.basename(os.readlink(proc_root / str(pid) / "exe"))
    except OSError:
        return None


def task_ids(proc_root, pid):
    try:
        task_root = proc_root / str(pid) / "task"
        return sorted(int(entry.name) for entry in task_root.iterdir() if entry.name.isdigit())
    except OSError:
        return []


def children_by_task(proc_root, pid):
    children = []
    for tid in task_ids(proc_root, pid):
        data = read_small(proc_root / str(pid) / "task" / str(tid) / "children", 8192)
        if data is None:
            continue
        children.extend((tid, int(child)) for child in data.split() if child.isdigit())
    return children


def discover(proc_root, shell_pid, monitor_pid):
    """Find this shell's cargo, its MCP test binary and direct CLI children."""
    found = []
    for _, cargo in children_by_task(proc_root, shell_pid):
        cargo_details = process_details(proc_root, cargo)
        if (cargo == monitor_pid or cargo_details is None or cargo_details[1] != shell_pid
                or executable_name(proc_root, cargo) != "cargo"):
            continue
        for _, test in children_by_task(proc_root, cargo):
            test_details = process_details(proc_root, test)
            exe = executable_name(proc_root, test)
            if test_details is None or test_details[1] != cargo or not exe or not TEST_BINARY.fullmatch(exe):
                continue
            for test_tid, cli in children_by_task(proc_root, test):
                details = process_details(proc_root, cli)
                if (details is None or details[1] != test
                        or executable_name(proc_root, cli) != "lingxi-cli"):
                    continue
                if (process_details(proc_root, test) != test_details
                        or process_details(proc_root, cargo) != cargo_details):
                    break
                found.append((test_tid, cli, details[0]))
                if len(found) >= MAX_PROCESSES:
                    return found
    return found


def snapshot(proc_root, pid, identity):
    if process_identity(proc_root, pid) != identity:
        return None
    result = []
    tids = task_ids(proc_root, pid)
    if len(tids) > MAX_THREADS:
        # The shutdown thread is usually among the newest TIDs. Retain both
        # ends of the list while keeping the report and sampling cost bounded.
        tids = tids[: MAX_THREADS // 2] + tids[-(MAX_THREADS // 2) :]
    for tid in tids:
        root = proc_root / str(pid) / "task" / str(tid)
        name = read_small(root / "comm")
        channel = read_small(root / "wchan")
        result.append(
            (tid, THREAD_NAMES.get(name, "other"), WAIT_CHANNELS.get(channel, "other" if channel else "unavailable"))
        )
    if process_identity(proc_root, pid) != identity:
        return None
    return result


class Monitor:
    def __init__(self, proc_root, shell_pid, monitor_pid):
        self.proc_root = proc_root
        self.shell_pid = shell_pid
        self.monitor_pid = monitor_pid
        self.shell_identity = process_identity(proc_root, shell_pid)
        self.records = {}
        self.unavailable = self.shell_identity is None

    def sample(self, now):
        if self.shell_identity is None or process_identity(self.proc_root, self.shell_pid) != self.shell_identity:
            self.unavailable = True
            return
        for test_tid, pid, identity in discover(self.proc_root, self.shell_pid, self.monitor_pid):
            key = (pid, identity)
            threads = snapshot(self.proc_root, pid, identity)
            if threads is None or not any(name == "shutdown" for _, name, _ in threads):
                continue
            if key not in self.records:
                if len(self.records) >= MAX_PROCESSES:
                    continue
                self.records[key] = {"test_tid": test_tid, "start": now, "samples": []}
            record = self.records[key]
            elapsed = now - record["start"]
            samples = record["samples"]
            if not samples or elapsed - samples[-1][0] >= SNAPSHOT_INTERVAL:
                samples.append((elapsed, threads))
                if len(samples) > MAX_SNAPSHOTS:
                    del samples[0]

    def report(self):
        lines = [
            "MCP runtime shutdown diagnostics (thread names and wait channels classified; elapsed since first observation):",
            "Sampling is bounded and may miss threads or short-lived processes.",
        ]
        if not self.records:
            lines.append("No shutdown thread observed; /proc may be unavailable or the process may have exited before sampling.")
        if self.unavailable:
            lines.append("The test shell identity was unavailable; observations may be incomplete.")
        for (pid, _), record in self.records.items():
            for elapsed, threads in record["samples"]:
                lines.append(f"test_tid={record['test_tid']} cli_pid={pid} elapsed_ms={int(elapsed * 1000)}")
                for tid, name, wait in threads:
                    lines.append(f"  tid={tid} role={name} wait={wait}")
        return "\n".join(lines) + "\n"


def run_worker(parent, guard_pid, directory):
    report_path = directory / "report"
    done_path = directory / "done"
    monitor = Monitor(Path("/proc"), parent, guard_pid)
    try:
        (directory / "ready").write_text(str(os.getpid()), encoding="ascii")
    except OSError:
        pass
    try:
        while os.getppid() == guard_pid:
            monitor.sample(time.monotonic())
            if done_path.is_file():
                break
            time.sleep(INTERVAL)
        if os.getppid() == guard_pid:
            report_path.write_text(monitor.report(), encoding="ascii")
    except (OSError, ValueError):
        # Diagnostics must never affect the cargo result, and exception text
        # can contain local paths. The shell reports an unavailable snapshot.
        return 1
    return 0


def supervise(parent, directory):
    """Reap the owned sampler; do not wait indefinitely for it after cargo."""
    guard_pid = os.getpid()
    try:
        worker = os.fork()
    except OSError:
        return 1
    if worker == 0:
        try:
            code = run_worker(parent, guard_pid, directory)
        except BaseException:
            code = 1
        os._exit(code)

    deadline = None
    killed = False
    while True:
        try:
            exited, status = os.waitpid(worker, os.WNOHANG)
        except ChildProcessError:
            return 1
        if exited:
            return 0 if os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0 else 1
        now = time.monotonic()
        if deadline is None:
            if os.getppid() != parent:
                deadline = now
            elif (directory / "done").is_file():
                deadline = now + WORKER_EXIT_BUDGET
        if deadline is not None and now >= deadline:
            if killed:
                # Even SIGKILL cannot complete an uninterruptible kernel wait.
                # Do not turn diagnostic failure into an unbounded CI wait.
                return 1
            try:
                os.kill(worker, signal.SIGKILL)
            except ProcessLookupError:
                pass
            killed = True
            deadline = now + WORKER_EXIT_BUDGET
        time.sleep(0.02)


def main():
    if len(sys.argv) != 3:
        return 2
    try:
        parent = int(sys.argv[1])
    except ValueError:
        return 2
    if os.getppid() != parent:
        return 2
    return supervise(parent, Path(sys.argv[2]))


if __name__ == "__main__":
    raise SystemExit(main())
