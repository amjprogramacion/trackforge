#!/usr/bin/env python3
"""Build a unified Tauri updater manifest from a GitHub release.

The release matrix uploads one updater manifest per platform. This script runs
after every matrix job and replaces those manifests with one latest.json that
contains all signed artifacts.

Required environment variables: GITHUB_TOKEN, TAG, REPO
"""

import json
import os
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone


TOKEN = os.environ["GITHUB_TOKEN"]
TAG = os.environ["TAG"]
REPO = os.environ["REPO"]
VERSION = TAG[1:] if TAG.startswith("v") else TAG
API = f"https://api.github.com/repos/{REPO}"


def api(url, method="GET", data=None, headers=None, raw=False):
    request_headers = {
        "Authorization": f"token {TOKEN}",
        "User-Agent": "trackforge-ci",
    }
    if headers:
        request_headers.update(headers)

    request = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers=request_headers,
    )
    with urllib.request.urlopen(request) as response:
        body = response.read()
    return body if raw else json.loads(body or b"{}")


def main():
    release = api(f"{API}/releases/tags/{TAG}")
    release_id = release["id"]
    assets = {asset["name"]: asset for asset in release.get("assets", [])}

    def find(suffix):
        for name, asset in assets.items():
            if name.endswith(suffix):
                return name, asset
        return None, None

    def asset_text(asset_id):
        return api(
            f"{API}/releases/assets/{asset_id}",
            headers={"Accept": "application/octet-stream"},
            raw=True,
        ).decode().strip()

    def download_url(name):
        return f"https://github.com/{REPO}/releases/download/{TAG}/{name}"

    platforms = {}

    mac_signature_name, mac_signature = find(".app.tar.gz.sig")
    if mac_signature:
        bundle_name = mac_signature_name[: -len(".sig")]
        platforms["darwin-aarch64"] = {
            "signature": asset_text(mac_signature["id"]),
            "url": download_url(bundle_name),
        }
        print(f"darwin-aarch64 -> {bundle_name}")
    else:
        print("WARNING: no .app.tar.gz.sig asset found; darwin-aarch64 omitted")

    windows_signature_name, windows_signature = find("-setup.exe.sig")
    if windows_signature:
        executable_name = windows_signature_name[: -len(".sig")]
        entry = {
            "signature": asset_text(windows_signature["id"]),
            "url": download_url(executable_name),
        }
        platforms["windows-x86_64"] = entry
        platforms["windows-x86_64-nsis"] = entry
        print(f"windows-x86_64 -> {executable_name}")
    else:
        print("WARNING: no -setup.exe.sig asset found; windows omitted")

    if not platforms:
        print(
            "ERROR: no signed updater artifacts found in the release.",
            file=sys.stderr,
        )
        sys.exit(1)

    manifest = {
        "version": VERSION,
        "notes": (release.get("body") or "").strip(),
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.000Z"),
        "platforms": platforms,
    }
    payload = json.dumps(manifest, indent=2).encode()

    existing_manifest = assets.get("latest.json")
    if existing_manifest:
        api(
            f"{API}/releases/assets/{existing_manifest['id']}",
            method="DELETE",
        )
        print("Deleted existing latest.json")

    api(
        (
            f"https://uploads.github.com/repos/{REPO}/releases/"
            f"{release_id}/assets?name=latest.json"
        ),
        method="POST",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    print(f"Uploaded latest.json with platforms: {', '.join(platforms)}")


if __name__ == "__main__":
    try:
        main()
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")
        print(f"HTTP {error.code} for {error.url}: {body}", file=sys.stderr)
        sys.exit(1)
