#!/usr/bin/env python3
"""Run the Teams and Chat gateway against the pinned standalone Data binary."""
import argparse
import contextlib
import json
import os
import secrets
from pathlib import Path
import subprocess
import sys
import tempfile

REVISION = "4905f6ac3bbc997932d5024d7df52c0b5883f481"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--data-worktree", required=True, type=Path)
    parser.add_argument("--playwright", type=Path)
    parser.add_argument("--chrome", type=Path)
    parser.add_argument("--private-preview", action="store_true")
    parser.add_argument("--postgres-bin", type=Path,
                        help="PostgreSQL bin directory; with --private-preview, also run database sources end to end")
    parser.add_argument("--staging-web-port", type=int, help="Serve isolated staging and keep it running")
    args = parser.parse_args()
    if args.chrome and not args.playwright:
        parser.error("--chrome requires --playwright")
    if args.postgres_bin and not args.private_preview:
        parser.error("--postgres-bin requires --private-preview")
    data = args.data_worktree.resolve()
    assert subprocess.check_output(["git", "-C", data, "rev-parse", "HEAD"], text=True).strip() == REVISION
    assert not subprocess.check_output(["git", "-C", data, "status", "--porcelain"], text=True)
    sys.path.insert(0, str(data / "tests"))
    from acceptance import Fixture, PROJECT, FIXTURE_DB, PASSWORD, port, stop, wait_port  # pylint: disable=import-outside-toplevel

    root = Path(__file__).resolve().parents[1]
    with contextlib.ExitStack() as stack:
        folder = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="lx-data-app-")))
        old_umask = os.umask(0o077)
        stack.callback(os.umask, old_umask)
        fixture = Fixture(data / "target/debug/lingxi-data", folder, stack)
        fixture.setup()
        dataset = "team-orders"
        # Use a distinct product Team ID matching the Team messages contract.
        team = "data-team"
        for uid, actor in fixture.actors.items():
            actor["teams"][team] = 1
        from acceptance import put  # pylint: disable=import-outside-toplevel
        put(fixture.actors_path, fixture.actors)
        schema = {"type": "object", "additionalProperties": False, "properties": {"title": {"type": "string"}}}
        fixture.call("owner", "create", "create-orders", dataset=dataset, team=team, schema=schema)
        owner = fixture.identity("owner")
        reader = fixture.identity("reader")
        fixture.call("owner", "share", "share-orders", dataset=dataset, recipient=reader, access="edit")
        reference = fixture.call("owner", "put", "seed-orders", dataset=dataset, record="order-1",
                                 schemaVersion=1, expectedRevision=0, value={"title": "Prepare shipment"})
        private = None
        if args.private_preview:
            source, bot = "private-orders", "sharedbot_01"
            database_secret = folder / "preview.dsn"
            database_secret.write_text(f"mysql://preview:{PASSWORD}@127.0.0.1:{fixture.mysql}/{FIXTURE_DB}", encoding="utf8")
            key, config, ice = folder / "preview.key", folder / "preview-source.json", folder / "preview-ice.json"
            fixture.invoke(["test-key", "--output", key])
            put(ice, {"udpBind": "127.0.0.1:0"})
            put(config, {"source": source, "owner": owner, "key": {"localTestFile": str(key)}, "ice": {"udpBind": "127.0.0.1:0"},
                         "presets": {"main": {"keyField": "id", "fields": ["id", "title", "private_note"],
                         "input": {"kind": "mysql", "dsn": {"localTestFile": str(database_secret)}, "view": "tickets_view", "exact": [], "isolatedLoopback": True}}}})
            public_key = subprocess.check_output([fixture.cli, "host-key", "--config", config], text=True).strip()
            private = {"source": source, "bot": bot, "config": str(config), "ice": str(ice), "ca": str(fixture.ca),
                       "cli": str(fixture.cli), "folder": str(folder), "publicKey": public_key}
            if args.postgres_bin:
                # The Data acceptance fixture: a real PostgreSQL view behind a SELECT-only role.
                postgres = fixture.postgres_fixture(args.postgres_bin.resolve())
                postgres_config = folder / "postgres-source.json"
                put(postgres_config, {"source": "postgres-orders", "owner": owner, "key": {"localTestFile": str(key)},
                                      "ice": {"udpBind": "127.0.0.1:0"}, "presets": {"main": {
                                          "keyField": "id", "fields": ["id", "title", "exact_id", "exact_value", "private_note"],
                                          "input": postgres["input"]}}})
                private["postgres"] = str(postgres_config)
        product_port = port()
        inputs = folder / "product-inputs.json"
        put(inputs, {"endpoint": f"https://127.0.0.1:{product_port}", "actorFile": str(fixture.actors_path),
                     "team": team, "reference": reference, "private": private,
                     "owner": [owner, str(fixture.users["owner"][1])], "reader": [reader, str(fixture.users["reader"][1])]})
        env = {**os.environ, "GCLOUD_PROJECT": PROJECT, "FIRESTORE_EMULATOR_HOST": f"127.0.0.1:{fixture.firestore}",
               "FIREBASE_AUTH_EMULATOR_HOST": f"127.0.0.1:{fixture.auth}", "NODE_EXTRA_CA_CERTS": str(fixture.ca),
               "LINGXI_DATA_FIXTURE": str(inputs)}
        # Start the production adapter with an empty actor snapshot. All test
        # requests from this point use Firebase identities and product grants.
        adapter_env = {**env, "LINGXI_DATA_INTERNAL_ORIGIN": f"https://127.0.0.1:{fixture.api}",
                       "LINGXI_DATA_PRODUCT_PORT": str(product_port), "LINGXI_DATA_ACTOR_FILE": str(fixture.actors_path),
                       "LINGXI_DATA_PRODUCT_TLS_CERT": str(fixture.cert), "LINGXI_DATA_PRODUCT_TLS_KEY": str(fixture.tls_key)}
        if args.staging_web_port and args.private_preview:
            adapter_env.update({"LINGXI_DATA_PREVIEW_KEY": secrets.token_urlsafe(32), "LINGXI_DATA_FIREBASE_API_KEY": "demo",
                                "LINGXI_DATA_RULES": "1"})
        adapter, log = fixture.spawn("product-data", ["node", str(root / "scripts/serve-lingxi-data.mjs")], adapter_env)
        wait_port(product_port, adapter, log)
        if args.staging_web_port:
            staging, staging_log = fixture.spawn("staging", ["node", str(root / "scripts/serve-lingxi-data-staging.mjs")],
                                                 {**env, "LINGXI_DATA_STAGING_PORT": str(args.staging_web_port)})
            wait_port(args.staging_web_port, staging, staging_log)
            print(f"Web staging: http://127.0.0.1:{args.staging_web_port}/teams", flush=True)
            print(f"Local staging endpoint log: {staging_log}", flush=True)
            try:
                return staging.wait()
            except KeyboardInterrupt:
                return 0
        subprocess.run(["node", "--test", "test/lingxi-data.emulator.test.mjs"], cwd=root, env=env, check=True)
        if args.playwright:
            env["LINGXI_TEST_PLAYWRIGHT_MODULE"] = str(args.playwright.resolve())
            if args.chrome:
                env["LINGXI_TEST_CHROME"] = str(args.chrome.resolve())
            subprocess.run(["node", "--import", "../../packages/bridge-client/node_modules/tsx/dist/loader.mjs",
                            "--test", "--test-name-pattern", "HTTPS Agent Chat recovery",
                            "test/local-app-remote-browser.test.mjs"], cwd=root.parent / "electron", env=env, check=True)
            subprocess.run(["node", "--test", "test/lingxi-data-browser.test.mjs"],
                           cwd=root.parent / "electron", env=env, check=True)
        if args.private_preview:
            stop(adapter)
            adapter_env.update({"LINGXI_DATA_PREVIEW_KEY": secrets.token_urlsafe(32), "LINGXI_DATA_FIREBASE_API_KEY": "demo",
                                "LINGXI_DATA_RULES": "1"})
            subprocess.run(["node", "--test", "test/lingxi-data-private-preview.emulator.test.mjs"], cwd=root, env=adapter_env, check=True)
            if args.postgres_bin:
                subprocess.run(["node", "--test", "test/lingxi-data-source-kinds.emulator.test.mjs"], cwd=root, env=adapter_env,
                               check=True)



if __name__ == "__main__":
    sys.exit(main())
