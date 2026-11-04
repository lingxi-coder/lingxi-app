#!/usr/bin/env python3
"""Summarize the relevant part of fresh cargo metadata JSON from stdin."""
import json
import sys
from collections import deque

d = json.load(sys.stdin)
packages = {p["id"]: p for p in d["packages"]}
nodes = {p["id"]: p for p in d["resolve"]["nodes"]}
targets = [p for p in packages.values() if p["name"] == "opaque-debug" and p["version"] == "0.3.1"]
assert len(targets) == 1
target = targets[0]["id"]
reverse = {}
for node in nodes.values():
    for dep in node["deps"]:
        reverse.setdefault(dep["pkg"], []).append((node["id"], dep["dep_kinds"]))

roots = set(d["workspace_members"])
consumers = []
for parent, dep_kinds in sorted(reverse.get(target, [])):
    queue = deque([(parent, [(target, dep_kinds), (parent, None)])])
    seen = {target, parent}
    path = None
    while queue:
        current, chain = queue.popleft()
        if current in roots:
            path = [{"package": packages[p]["name"] + "@" + packages[p]["version"], "edge_from_parent": edge} for p, edge in reversed(chain)]
            break
        for up, kinds in reverse.get(current, []):
            if up not in seen:
                seen.add(up)
                queue.append((up, chain[:-1] + [(current, kinds), (up, None)]))
    consumers.append({"package": packages[parent]["name"] + "@" + packages[parent]["version"], "direct_dependency_kinds": dep_kinds, "example_workspace_path": path})

print(json.dumps({"package_count": len(packages), "target": target, "target_enabled_features": nodes[target]["features"], "direct_consumers": consumers}, indent=2))
