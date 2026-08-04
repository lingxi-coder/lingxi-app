#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
import pathlib
import urllib.parse


def package_name(lock_path: str) -> str:
    marker = "node_modules/"
    if marker not in lock_path:
        raise ValueError(f"not a node_modules package path: {lock_path}")
    return lock_path.rsplit(marker, 1)[1]


def spdx_id(lock_path: str) -> str:
    digest = hashlib.sha256(lock_path.encode("utf-8")).hexdigest()[:16]
    return f"SPDXRef-Npm-{digest}"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--lock", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    lock_path = pathlib.Path(args.lock)
    lock = json.loads(lock_path.read_text(encoding="utf-8"))
    package_entries = []
    for path, package in sorted(lock["packages"].items()):
        if not path:
            continue
        name = package_name(path)
        version = package["version"]
        integrity = package["integrity"]
        algorithm, encoded = integrity.split("-", 1)
        if algorithm != "sha512":
            raise SystemExit(f"unsupported npm integrity algorithm for {path}: {algorithm}")
        checksum = base64.b64decode(encoded, validate=True).hex()
        package_entries.append(
            {
                "name": name,
                "SPDXID": spdx_id(path),
                "versionInfo": version,
                "downloadLocation": package.get("resolved", "NOASSERTION"),
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": "NOASSERTION",
                "checksums": [{"algorithm": "SHA512", "checksumValue": checksum}],
                "sourceInfo": f"npm package-lock path: {path}",
                "externalRefs": [
                    {
                        "referenceCategory": "PACKAGE-MANAGER",
                        "referenceType": "purl",
                        "referenceLocator": (
                            f"pkg:npm/{urllib.parse.quote(name, safe='/')}@{urllib.parse.quote(version, safe='')}"
                        ),
                    }
                ],
            }
        )

    lock_digest = hashlib.sha256(lock_path.read_bytes()).hexdigest()
    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": "lingxi-local-app-runtime",
        "documentNamespace": f"https://lingxi.app/spdx/local-app-runtime/{lock_digest}",
        "creationInfo": {
            "created": "1970-01-01T00:00:00Z",
            "creators": ["Organization: LingXi", "Tool: generate-local-app-sbom.py"],
        },
        "packages": package_entries,
        "relationships": [
            {
                "spdxElementId": "SPDXRef-DOCUMENT",
                "relationshipType": "DESCRIBES",
                "relatedSpdxElement": package["SPDXID"],
            }
            for package in package_entries
        ],
    }
    output = pathlib.Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(document, indent=2, sort_keys=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
