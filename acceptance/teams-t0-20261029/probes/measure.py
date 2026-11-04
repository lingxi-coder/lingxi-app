"""Measure the two locally built native SDK probes without modifying them."""
import hashlib
import json
import zipfile
from pathlib import Path

root = Path(__file__).resolve().parents[3]


def file_metadata(path):
    return {
        "path": str(path),
        "bytes": path.stat().st_size,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }


android = {"firebaseBoM": "35.0.2", "firestoreVersion": "27.0.1", "build": "Debug, no shrinking"}
for flavor in ("baseline", "firestore"):
    apk = next((root / "acceptance/teams-t0-20261029/probes/android/build/outputs/apk" / flavor / "debug").glob("*.apk"))
    metadata = file_metadata(apk)
    with zipfile.ZipFile(apk) as archive:
        metadata["uncompressedBytes"] = sum(entry.file_size for entry in archive.infolist())
        metadata["dexBytes"] = sum(entry.file_size for entry in archive.infolist() if entry.filename.endswith(".dex"))
    android[flavor] = metadata
android["apkDeltaBytes"] = android["firestore"]["bytes"] - android["baseline"]["bytes"]
android["uncompressedDeltaBytes"] = android["firestore"]["uncompressedBytes"] - android["baseline"]["uncompressedBytes"]

ios = {"firebaseSDK": "12.19.2", "build": "Release, iphoneos arm64, unsigned"}
for target in ("Baseline", "WithFirestore"):
    app = root / "output/teams-t0-ios/DerivedData/Build/Products/Release-iphoneos" / f"{target}.app"
    ios[target] = {
        "path": str(app),
        "bundleBytes": sum(path.stat().st_size for path in app.rglob("*") if path.is_file()),
        "executable": file_metadata(app / target),
    }
ios["bundleDeltaBytes"] = ios["WithFirestore"]["bundleBytes"] - ios["Baseline"]["bundleBytes"]
ios["executableDeltaBytes"] = ios["WithFirestore"]["executable"]["bytes"] - ios["Baseline"]["executable"]["bytes"]

result = {"scope": "Minimal dependency probes; not full signed product or device acceptance", "android": android, "ios": ios}
output = root / "acceptance/teams-t0-20261029/sizes.json"
output.write_text(json.dumps(result, indent=2) + "\n")
print(output)
