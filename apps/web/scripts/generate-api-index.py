#!/usr/bin/env python3
"""Build a source declaration catalog from the four SDK repositories' Git HEAD.

This is a source index, not rustdoc or a resolved export graph. It lists public
declarations in the selected first-party contract/facade sources, with comments
and source signatures. It does not expand macros, resolve aliases/re-exports,
evaluate platform/feature cfg, or include generated native bindings. Committed
blobs are used so every source link identifies exactly the indexed declaration.

Run from any directory: python3 apps/web/scripts/generate-api-index.py
Use --workspace /path/to/lingxi when the sibling checkout directory differs.
"""

from __future__ import annotations

import argparse
import bisect
from dataclasses import dataclass
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import quote


@dataclass
class Scope:
    start: int
    end: int
    kind: str
    owner: str = ""
    public: bool = False
    test: bool = False


def git(repo: Path, *args: str) -> str:
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True)


def source_url(repo: Path) -> str:
    remote = git(repo, "remote", "get-url", "origin").strip()
    match = re.search(r"(?:github\.com[^/:]*[:/])([^/]+/[^/]+?)(?:\.git)?$", remote)
    if not match:
        raise ValueError(f"Expected a GitHub origin for {repo.name}")
    return "https://github.com/" + match.group(1)


def masked(source: str) -> str:
    """Hide comments/literals without moving source positions or newlines."""
    result = list(source)
    pattern = re.compile(
        r"/\*[\s\S]*?\*/|//[^\n]*|r(?P<hashes>\#*)\"[\s\S]*?\"(?P=hashes)"
        r"|\"(?:\\[\s\S]|[^\"\\])*\"|`(?:\\[\s\S]|[^`\\])*`"
        r"|'(?:\\.|[^'\\\n])'"
    )
    for match in pattern.finditer(source):
        for pos in range(match.start(), match.end()):
            if result[pos] != "\n":
                result[pos] = " "
    return "".join(result)


def brace_pairs(code: str) -> dict[int, int]:
    stack: list[int] = []
    result: dict[int, int] = {}
    for pos, char in enumerate(code):
        if char == "{":
            stack.append(pos)
        elif char == "}" and stack:
            result[stack.pop()] = pos
    return result


def preceding(source: str, pos: int) -> str:
    """Comments and attributes immediately preceding a declaration."""
    prefix = source[:pos].rstrip()
    chunks: list[str] = []
    # Walk backwards so an earlier attribute cannot consume unrelated code.
    while prefix:
        line_start = prefix.rfind("\n") + 1
        line = prefix[line_start:]
        if line.lstrip().startswith("///"):
            chunks.insert(0, line)
            prefix = prefix[:line_start].rstrip()
            continue
        if prefix.endswith("]"):
            start = prefix.rfind("#[")
            if start >= 0:
                attr = prefix[start:]
                cleaned = masked(attr)
                depth = 0
                closing = -1
                for index, char in enumerate(cleaned[1:], 1):
                    depth += (char == "[") - (char == "]")
                    if depth == 0:
                        closing = index
                        break
                if closing == len(cleaned) - 1:
                    chunks.insert(0, attr)
                    prefix = prefix[:start].rstrip()
                    continue
        break
    if chunks:
        return "\n".join(chunks)
    # Only the nearest, immediately preceding block can describe this item.
    # Searching from an earlier opening comment could span multiple blocks and
    # attribute the module overview to every declaration in the file.
    if prefix.endswith("*/"):
        start = prefix.rfind("/**")
        if start >= 0:
            block = prefix[start:]
            if block.find("*/") == len(block) - 2:
                return block
    return ""


def summary(source: str, pos: int) -> str:
    docs = preceding(source, pos)
    if docs.startswith("/**"):
        lines = [re.sub(r"^\s*\* ?", "", line).strip() for line in docs[3:-2].splitlines()]
    else:
        lines = [re.sub(r"^\s*/// ?", "", line).strip() for line in docs.splitlines() if re.match(r"\s*///", line)]
    paragraph: list[str] = []
    for line in lines:
        if not line and paragraph:
            break
        if line:
            paragraph.append(line)
    return " ".join(paragraph)[:800]


def test_attribute(source: str, pos: int) -> bool:
    attrs = preceding(source, pos)
    return bool(re.search(r"#\[(?:test\b|(?:\w+::)?test\b)|#\[cfg\s*\(\s*(?:test\b|(?:all|any)\s*\([^\]]*\btest\b)|test-support", attrs))


def signature_end(code: str, start: int, kind: str, language: str) -> int:
    """Keep balanced argument/tuple signatures; stop before implementation."""
    parens = brackets = braces = 0
    alias = kind == "type" and language in ("typescript", "rust")
    for pos in range(start, len(code)):
        char = code[pos]
        if char == "(":
            parens += 1
        elif char == ")":
            parens -= 1
        elif char == "[":
            brackets += 1
        elif char == "]":
            brackets -= 1
        elif char == "{" and parens == 0 and brackets == 0:
            if not alias:
                return pos
            braces += 1
        elif char == "}" and alias:
            braces -= 1
        elif char == ";" and parens == brackets == braces == 0:
            return pos + 1
        elif char == "=" and language == "kotlin" and parens == brackets == 0:
            return pos
        elif char == "\n" and language in ("swift", "kotlin") and parens == brackets == 0:
            return pos
    return len(code)


def rust_declarations(source: str, known_public_types: set[str] | None = None) -> list[tuple[str, str, str, str, int]]:
    code = masked(source)
    pairs = brace_pairs(code)
    heads = re.compile(
        r"^[ \t]*(?P<public>pub(?!\s*\()\s+)?"
        r"(?:(?:async|unsafe|default|const(?=\s+fn\b))\s+|extern\s+(?:\"[^\"]*\"\s+)?)*"
        r"(?P<kind>struct|enum|trait|fn|type|const|static|mod)\s+(?P<name>[A-Za-z_]\w*)",
        re.M,
    )
    scopes: list[Scope] = []
    declarations = list(heads.finditer(code))
    public_types = {m.group("name") for m in declarations if m.group("public") and m.group("kind") in ("struct", "enum", "trait")}
    public_types.update(known_public_types or set())
    for match in declarations:
        end = signature_end(code, match.start(), match.group("kind"), "rust")
        brace = end if end < len(code) and code[end] == "{" else -1
        if brace in pairs and match.group("kind") in ("struct", "enum", "trait", "fn", "mod"):
            scopes.append(Scope(brace, pairs[brace], match.group("kind"), match.group("name"), bool(match.group("public")), test_attribute(source, match.start()) or match.group("name") in ("tests", "test_support")))
    for match in re.finditer(r"^[ \t]*impl\b", code, re.M):
        end = signature_end(code, match.start(), "impl", "rust")
        if end not in pairs:
            continue
        header = code[match.end():end].strip()
        if header.startswith("<"):
            depth = 0
            for pos, char in enumerate(header):
                depth += (char == "<") - (char == ">")
                if depth == 0:
                    header = header[pos + 1:].strip()
                    break
        target = header.split(" where ")[0]
        if " for " in target:
            target = target.rsplit(" for ", 1)[1]
        owner_match = re.match(r"(?:[\w]+::)*([A-Za-z_]\w*)", target)
        owner = owner_match.group(1) if owner_match else ""
        scopes.append(Scope(end, pairs[end], "impl", owner, owner in public_types, test_attribute(source, match.start())))

    output: list[tuple[str, str, str, str, int]] = []
    for match in declarations:
        containing = sorted((s for s in scopes if s.start < match.start() < s.end), key=lambda s: s.start)
        if any(s.test or s.kind == "fn" for s in containing) or test_attribute(source, match.start()):
            continue
        scope = containing[-1] if containing else None
        implicit_trait = scope and scope.kind == "trait" and scope.public
        if not match.group("public") and not implicit_trait:
            continue
        if scope and scope.kind in ("impl", "trait") and not scope.public:
            # Public methods on private implementation helpers aren't SDK declarations.
            continue
        kind, name = match.group("kind"), match.group("name")
        if kind in ("fn", "type", "const") and scope and scope.kind in ("impl", "trait"):
            name = scope.owner + "::" + name
            if kind == "fn":
                kind = "method"
        end = signature_end(code, match.start(), match.group("kind"), "rust")
        output.append((name, kind, source[match.start():end].strip(), summary(source, match.start()), match.start()))
    return output


def native_declarations(source: str, language: str) -> list[tuple[str, str, str, str, int]]:
    code = masked(source)
    pairs = brace_pairs(code)
    if language == "typescript":
        heads = re.compile(r"^[ \t]*export\s+(?:(?:declare|abstract|async)\s+)*(?P<kind>interface|class|type|enum|function|const|let)\s+(?P<name>[A-Za-z_$]\w*)", re.M)
    elif language == "swift":
        heads = re.compile(r"^[ \t]*public\s+(?:(?:final|static|mutating|convenience)\s+)*(?P<kind>class|struct|enum|protocol|func|init|let|var)\b(?:\s+(?P<name>[A-Za-z_]\w*))?", re.M)
    else:
        heads = re.compile(r"^[ \t]*(?P<private>private\s+|internal\s+|protected\s+)?(?:public\s+)?(?:(?:data|suspend|override|open|const|sealed|inline|abstract)\s+)*(?P<kind>class|object|interface|fun)\s+(?:<[^>]+>\s+)?(?P<name>[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*)", re.M)
    matches = list(heads.finditer(code))
    scopes: list[Scope] = []
    for match in matches:
        kind = match.group("kind")
        if kind not in ("class", "interface", "struct", "enum", "object", "protocol", "function", "fun", "func", "init"):
            continue
        end = signature_end(code, match.start(), kind, language)
        private = language == "kotlin" and bool(match.groupdict().get("private"))
        if end in pairs:
            scopes.append(Scope(end, pairs[end], kind, match.group("name") or "init", not private))
        elif kind in ("class", "interface", "struct", "enum", "object", "protocol"):
            # Kotlin data classes can end after their constructor without braces.
            scopes.append(Scope(match.start(), end, kind, match.group("name") or "init", not private))
    result = []
    for match in matches:
        if match.groupdict().get("private"):
            continue
        inside = [scope for scope in scopes if scope.start < match.start() < scope.end]
        if any(not scope.public or scope.kind in ("function", "fun", "func", "init") for scope in inside):
            continue
        kind, name = match.group("kind"), match.group("name") or "init"
        if inside:
            name = inside[-1].owner + "." + name
        if kind in ("func", "fun"):
            kind = "method" if inside else "function"
        end = signature_end(code, match.start(), match.group("kind"), language)
        result.append((name, kind, source[match.start():end].strip(), summary(source, match.start()), match.start()))

    if language == "typescript":
        methods = re.compile(r"^[ \t]*(?!(?:private|protected)\b)(?:(?:public|override|async|static|readonly)\s+)*(?:(?P<accessor>get|set)\s+)?(?P<name>[A-Za-z_$]\w*)(?:<[^;{}]*>)?\s*\(", re.M)
        for match in methods.finditer(code):
            inside = [scope for scope in scopes if scope.start < match.start() < scope.end and scope.kind == "class"]
            if not inside:
                continue
            owner = inside[-1]
            # A class member starts directly in the class, never in a method body.
            depth = sum(1 for start, end in pairs.items() if owner.start < start < match.start() < end < owner.end)
            if depth:
                continue
            end = signature_end(code, match.start(), "method", language)
            result.append((owner.owner + "." + match.group("name"), "accessor" if match.group("accessor") else "method", source[match.start():end].strip(), summary(source, match.start()), match.start()))
    return result


def selected(sdk: str, file: str) -> bool:
    if re.search(r"(^|/)(?:tests?|fixtures?|vendor|snapshots|examples)(/|\.)|(?:_tests?|test_support)\.(?:rs|ts)$", file):
        return False
    if sdk == "llm":
        return file.startswith("src/") and file.endswith(".rs")
    if sdk == "harness":
        # Explicit first-party contract and facade coverage; platform assembly
        # internals, built-in tool implementations and presentation are omitted.
        return file.endswith(".rs") and (
            file in ("crates/runtime/src/lib.rs", "crates/runtime/src/api.rs")
            or file.startswith("crates/runtime/src/models/")
            or file.startswith("crates/client/src/protocol/")
            or file in ("crates/client/src/lib.rs", "crates/client/src/adapter/mod.rs", "crates/client/src/adapter/listener.rs", "crates/client/src/adapter/sink.rs", "crates/client/src/adapter/controls.rs", "crates/client/src/adapter/turn.rs")
            or file.startswith("crates/tool-api/src/")
            or file.startswith("crates/skill-api/src/")
            or file.startswith("crates/plugin/src/")
        )
    if sdk == "mobile":
        return (file.startswith("crates/mobile-linux-api/src/") and file.endswith(".rs")) or file in (
            "ios/SDK/MobileLinuxRuntime.swift",
            "android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt",
            "android/installer/src/main/kotlin/io/lingxi/mobilelinux/RootfsInstaller.kt",
        )
    return file.startswith("packages/bridge-client/src/") and file.endswith(".ts") and Path(file).stem in ("index", "protocol", "toolview", "lockfile", "validation", "version", "client")


def build(workspace: Path) -> dict:
    repositories = []
    entries = []
    timestamps = []
    for sdk, dirname in (("harness", "harness-runtime"), ("llm", "llm-client"), ("mobile", "mobile-linux-runtime"), ("bridge", "lingxi-app")):
        repo = workspace / dirname
        revision = git(repo, "rev-parse", "HEAD").strip()
        if not re.fullmatch(r"[0-9a-f]{40}", revision):
            raise ValueError(f"Invalid Git revision for {repo}")
        base = source_url(repo)
        repositories.append({"sdk": sdk, "revision": revision, "sourceUrl": base})
        timestamps.append(datetime.fromisoformat(git(repo, "show", "-s", "--format=%cI", revision).strip()))
        files = sorted(git(repo, "ls-tree", "-r", "--name-only", revision).splitlines())
        sources = {file: git(repo, "show", f"{revision}:{file}") for file in files if selected(sdk, file)}
        # Inherent methods are often implemented in another facade file.
        public_types = {match.group(1) for file, source in sources.items() if file.endswith(".rs") for match in re.finditer(r"\bpub\s+(?:struct|enum|trait)\s+([A-Za-z_]\w*)", masked(source))}
        for file, source in sources.items():
            language = {".rs": "rust", ".ts": "typescript", ".swift": "swift", ".kt": "kotlin"}[Path(file).suffix]
            declarations = rust_declarations(source, public_types) if language == "rust" else native_declarations(source, language)
            newlines = [pos for pos, char in enumerate(source) if char == "\n"]
            for name, kind, signature, docs, pos in declarations:
                line = bisect.bisect_left(newlines, pos) + 1
                entries.append({"sdk": sdk, "name": name, "kind": kind, "signature": signature, "summary": docs, "file": file, "line": line, "url": f"{base}/blob/{revision}/{quote(file, safe='/')}#L{line}"})
    entries.sort(key=lambda item: (item["sdk"], item["name"].casefold(), item["file"], item["line"]))
    unique = {(item["sdk"], item["file"], item["line"], item["name"]): item for item in entries}
    return {"generatedAt": max(timestamps).astimezone(timezone.utc).isoformat().replace("+00:00", "Z"), "repositories": repositories, "entries": list(unique.values())}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=Path(__file__).resolve().parents[4])
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / "public" / "api-index.json")
    parser.add_argument("--check", action="store_true", help="Fail if the saved catalog differs from committed source snapshots.")
    args = parser.parse_args()
    content = json.dumps(build(args.workspace.resolve()), ensure_ascii=False, indent=2) + "\n"
    if args.check:
        if not args.output.is_file() or args.output.read_text() != content:
            raise SystemExit("API catalog is stale; run generate-api-index.py")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(content)
    data = json.loads(content)
    counts = {repo["sdk"]: sum(item["sdk"] == repo["sdk"] for item in data["entries"]) for repo in data["repositories"]}
    print(json.dumps({"output": str(args.output), "declarations": counts}, ensure_ascii=False))


if __name__ == "__main__":
    main()
