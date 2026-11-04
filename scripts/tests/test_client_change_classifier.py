import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest


CLASSIFIER = Path(__file__).resolve().parents[1] / "ci" / "classify_client_changes.py"
WORKFLOW = Path(__file__).resolve().parents[2] / ".github" / "workflows" / "clients.yml"


class ClientChangeClassifierTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "CI test")
        self.git("config", "user.email", "ci-test@example.invalid")
        self.initial = self.commit("initial", allow_empty=True)

    def git(self, *args, cwd=None):
        return subprocess.run(
            ["git", *args], cwd=cwd or self.repo, check=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        ).stdout.decode().strip()

    def write(self, name, content="test\n"):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def commit(self, message, allow_empty=False):
        self.git("add", "--", ".")
        args = ["commit", "-q", "-m", message]
        if allow_empty:
            args.append("--allow-empty")
        self.git(*args)
        return self.git("rev-parse", "HEAD")

    def classify(self, before, after, *, event=None, env=None, cwd=None, raw=None):
        if event is None:
            event = {"ref": "refs/heads/main", "before": before, "after": after}
        event_path = self.root / "event.json"
        event_path.write_text(raw if raw is not None else json.dumps(event), encoding="utf-8")
        run_env = os.environ.copy()
        run_env.update({
            "GITHUB_EVENT_NAME": "push",
            "GITHUB_REF": "refs/heads/main",
            "GITHUB_SHA": after,
            "GITHUB_EVENT_PATH": str(event_path),
        })
        if env:
            run_env.update(env)
        return subprocess.run(
            [sys.executable, str(CLASSIFIER)], cwd=cwd or self.repo, env=run_env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        ).returncode == 0

    def test_audit_add_modify_and_delete(self):
        self.write("supply-chain/audits.toml")
        self.write("supply-chain/reviews/first.md")
        before = self.commit("add audit records")
        self.assertTrue(self.classify(self.initial, before))
        self.write("supply-chain/audits.toml", "updated\n")
        (self.repo / "supply-chain/reviews/first.md").unlink()
        self.write("supply-chain/reviews/nested/review.md")
        after = self.commit("update audit records")
        self.assertTrue(self.classify(before, after))

    def test_nul_safe_paths_and_similar_prefixes(self):
        self.write("supply-chain/reviews/a\nwith\tspaces-雪.md")
        audit = self.commit("unusual review name")
        self.assertTrue(self.classify(self.initial, audit))
        self.write("supply-chain/reviews-extra/not-a-review.md")
        code = self.commit("similar prefix")
        self.assertFalse(self.classify(audit, code))

    def test_rename_checks_both_old_and_new_paths(self):
        self.write("supply-chain/reviews/old.md")
        self.write("apps/client/move-in.md")
        base = self.commit("seed")
        self.git("mv", "supply-chain/reviews/old.md", "supply-chain/reviews/new.md")
        within = self.commit("review rename")
        self.assertTrue(self.classify(base, within))
        self.git("mv", "supply-chain/reviews/new.md", "apps/client/move-out.md")
        moved_out = self.commit("rename out")
        self.assertFalse(self.classify(within, moved_out))
        self.git("mv", "apps/client/move-in.md", "supply-chain/reviews/move-in.md")
        moved_in = self.commit("rename in")
        self.assertFalse(self.classify(moved_out, moved_in))

    def test_build_inputs_and_deletions_require_clients(self):
        paths = [
            "docs/mobile-linux/mobile-linux-native-pins.json",
            "authorization/grants.json",
            "docs/guide.md",
            "supply-chain/audits.toml.extra",
        ]
        for path in paths:
            with self.subTest(path=path):
                before = self.git("rev-parse", "HEAD")
                self.write(path)
                after = self.commit("add build input")
                self.assertFalse(self.classify(before, after))
                (self.repo / path).unlink()
                deleted = self.commit("delete build input")
                self.assertFalse(self.classify(after, deleted))

    def test_more_than_3000_mixed_paths_never_truncates(self):
        for index in range(3001):
            self.write(f"supply-chain/reviews/{index:04d}.md")
        audit = self.commit("large audit-only push")
        self.assertTrue(self.classify(self.initial, audit))
        self.write("z-client/build-input.txt")
        after = self.commit("large mixed push")
        self.assertFalse(self.classify(self.initial, after))

    def test_push_range_includes_earlier_commits(self):
        self.write("apps/client/source.txt")
        self.commit("client code")
        self.write("supply-chain/reviews/review.md")
        after = self.commit("later audit record")
        self.assertFalse(self.classify(self.initial, after))

    def test_unknown_events_and_invalid_payloads_run_clients(self):
        self.write("supply-chain/audits.toml")
        after = self.commit("review")
        before = self.initial
        self.assertTrue(self.classify(before, after))
        for changes in (
            {"GITHUB_EVENT_NAME": "pull_request"},
            {"GITHUB_EVENT_NAME": "workflow_dispatch"},
            {"GITHUB_REF": "refs/heads/other"},
            {"GITHUB_SHA": before},
            {"GITHUB_EVENT_PATH": str(self.root / "missing.json")},
            {"PATH": ""},
        ):
            with self.subTest(env=changes):
                self.assertFalse(self.classify(before, after, env=changes))
        for event in (
            {}, [], {"ref": "refs/heads/other", "before": before, "after": after},
            {"ref": "refs/heads/main", "before": "0" * 40, "after": after},
            {"ref": "refs/heads/main", "before": "f" * 40, "after": after},
            {"ref": "refs/heads/main", "before": before[:10], "after": after},
            {"ref": "refs/heads/main", "before": before, "after": "0" * 40},
        ):
            with self.subTest(event=event):
                self.assertFalse(self.classify(before, after, event=event))
        self.assertFalse(self.classify(before, after, raw="{not json"))
        self.assertFalse(self.classify(after, after))

    def test_nonancestor_empty_diff_and_shallow_history_run_clients(self):
        self.write("supply-chain/reviews/a.md")
        left = self.commit("left branch")
        self.git("checkout", "-q", "-b", "side", self.initial)
        self.write("supply-chain/reviews/b.md")
        right = self.commit("right branch")
        self.assertFalse(self.classify(left, right))
        empty = self.commit("empty", allow_empty=True)
        self.assertFalse(self.classify(right, empty))

        shallow = self.root / "shallow"
        self.git("clone", "-q", "--depth=1", self.repo.as_uri(), str(shallow))
        self.assertEqual(self.git("rev-parse", "--is-shallow-repository", cwd=shallow), "true")
        self.assertFalse(self.classify(right, empty, cwd=shallow))

    def test_workflow_checkout_failure_runs_all_clients(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        start = workflow.index("      - name: Classify client changes\n")
        start = workflow.index("        run: |\n", start) + len("        run: |\n")
        lines = []
        for line in workflow[start:].splitlines():
            if line and not line.startswith("          "):
                break
            lines.append(line)
        command = textwrap.dedent("\n".join(lines))
        for outcome in ("failure", "skipped", "success"):
            with self.subTest(outcome=outcome):
                output = self.root / "output"
                output.write_text("", encoding="utf-8")
                env = os.environ.copy()
                env.update({"CHECKOUT_OUTCOME": outcome, "GITHUB_OUTPUT": str(output)})
                # The temporary checkout has no classifier. A failed checkout,
                # or a successful checkout missing its script, must fall back.
                subprocess.run(["bash", "-e", "-o", "pipefail", "-c", command], cwd=self.repo,
                               env=env, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.assertEqual(output.read_text(encoding="utf-8"), "audit_only=false\n")


if __name__ == "__main__":
    unittest.main()
