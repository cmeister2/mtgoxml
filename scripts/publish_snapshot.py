#!/usr/bin/env python3
"""Publish complete, manifest-addressed MTGO snapshots as GitHub Releases."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen


ASSETS = (
    "card-data.json.gz",
    "snapshot.json",
    "MTGO.application",
    "MTGO.exe.manifest",
    "SHA256SUMS",
)


class SnapshotError(RuntimeError):
    """A snapshot is invalid or cannot be published safely."""


class GitHubError(SnapshotError):
    """A GitHub request failed with a known HTTP status."""

    def __init__(self, status, message):
        super().__init__(f"GitHub HTTP {status}: {message}")
        self.status = status


class GitHub:
    """Small REST client that streams uploads rather than buffering JSON gzip."""

    def __init__(self, repository, token):
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
            raise SnapshotError("GITHUB_REPOSITORY must be an owner/repository name")
        if not token:
            raise SnapshotError("GH_TOKEN is required")
        self.repository = repository
        self.token = token

    def request(self, method, path, payload=None):
        """Send JSON to the repository's REST endpoint and decode the response."""
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        return self._request(
            method, f"https://api.github.com/repos/{self.repository}{path}", data
        )

    def _request(self, method, url, data=None, extra_headers=None):
        """Keep credentials in headers and surface every transport failure."""
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
            "X-GitHub-Api-Version": "2026-03-10",
            "User-Agent": "mtgoxml-snapshot-publisher",
        }
        headers.update(extra_headers or {})
        request = Request(url, data=data, headers=headers, method=method)
        try:
            with urlopen(request, timeout=120) as response:
                body = response.read()
        except HTTPError as error:
            raise GitHubError(error.code, error.reason) from error
        except (URLError, OSError) as error:
            raise SnapshotError(f"GitHub request failed: {error}") from error
        if not body:
            return None
        try:
            return json.loads(body)
        except (ValueError, UnicodeDecodeError) as error:
            raise SnapshotError("GitHub returned invalid JSON") from error

    def upload(self, release_id, path):
        """Stream one local asset with a known length to the exact draft id."""
        url = (
            f"https://uploads.github.com/repos/{self.repository}"
            f"/releases/{release_id}/assets?name={quote(path.name, safe='')}"
        )
        with path.open("rb") as source:
            return self._request(
                "POST",
                url,
                source,
                {
                    "Content-Length": str(path.stat().st_size),
                    "Content-Type": "application/octet-stream",
                },
            )


def digest_file(path):
    """Hash large assets with bounded memory."""
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_snapshot(directory):
    """Validate the tag against the exact saved application manifest bytes."""
    try:
        snapshot = json.loads((directory / "snapshot.json").read_text("utf-8"))
    except (OSError, ValueError) as error:
        raise SnapshotError("Cannot read snapshot.json") from error
    if (
        not isinstance(snapshot, dict)
        or type(snapshot.get("schema_version")) is not int
        or snapshot["schema_version"] != 1
    ):
        raise SnapshotError("Unsupported snapshot schema_version")
    digest = snapshot.get("application_manifest_sha256", "")
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise SnapshotError("Invalid application_manifest_sha256")
    if snapshot.get("tag") != f"mtgo-data-v1-{digest}":
        raise SnapshotError("Snapshot tag does not match its schema and manifest")
    for field in ("deployment_url", "application_manifest_url", "application_version"):
        if not isinstance(snapshot.get(field), str) or not snapshot[field]:
            raise SnapshotError(f"Snapshot requires {field}")
    if digest_file(directory / "MTGO.exe.manifest") != digest:
        raise SnapshotError("Saved application manifest does not match snapshot.json")
    return snapshot


def find_release(github, tag):
    """Find published releases directly and include drafts when resuming work."""
    try:
        release = github.request("GET", f"/releases/tags/{quote(tag, safe='')}")
    except GitHubError as error:
        if error.status != 404:
            raise
    else:
        if release.get("tag_name") != tag:
            raise SnapshotError("GitHub returned a release with an unexpected tag")
        return release

    # evenflow: find-draft
    # A tag lookup can omit drafts. Listing also verifies repository access;
    # a missing/inaccessible repository must not be treated as a new snapshot.
    page = 1
    while True:
        releases = github.request("GET", f"/releases?per_page=100&page={page}")
        for release in releases:
            if release.get("tag_name") == tag:
                return release
        if len(releases) < 100:
            return None
        page += 1


def complete_assets(github, release, expected_sizes=None, expected_digests=None):
    """Reject partial uploads before publishing or skipping a prior release."""
    assets = github.request("GET", f"/releases/{release['id']}/assets?per_page=100")
    by_name = {}
    for asset in assets:
        name = asset.get("name")
        if name in by_name:
            raise SnapshotError(f"Release contains duplicate asset {name}")
        by_name[name] = asset
    for name in ASSETS:
        asset = by_name.get(name)
        if not asset or asset.get("state") != "uploaded" or asset.get("size", 0) <= 0:
            raise SnapshotError(f"Release asset {name} is missing or incomplete")
        if expected_sizes and asset["size"] != expected_sizes[name]:
            raise SnapshotError(f"Uploaded asset {name} has an unexpected size")
        if expected_digests and asset.get("digest") != f"sha256:{expected_digests[name]}":
            raise SnapshotError(f"Uploaded asset {name} has an unexpected SHA-256")
    return by_name


def needs_export(github, snapshot):
    """Skip complete published snapshots and resume any matching draft."""
    release = find_release(github, snapshot["tag"])
    if release is None or release["draft"]:
        return True
    complete_assets(github, release)
    return False


def publish(github, directory, target):
    """Stage all assets in a draft, verify them, then publish the complete release."""
    if not re.fullmatch(r"[0-9a-f]{40}", target):
        raise SnapshotError("--target must be a full Git commit SHA")
    snapshot = read_snapshot(directory)
    count = snapshot.get("object_count")
    if type(count) is not int or count <= 0:
        raise SnapshotError("Snapshot must contain a positive exported object_count")
    for name in ASSETS[:-1]:
        if not (directory / name).is_file() or (directory / name).stat().st_size <= 0:
            raise SnapshotError(f"Export asset {name} is missing or empty")
    checksums = "".join(
        f"{digest_file(directory / name)}  {name}\n" for name in ASSETS[:-1]
    )
    (directory / "SHA256SUMS").write_text(checksums, encoding="ascii")
    sizes = {name: (directory / name).stat().st_size for name in ASSETS}
    digests = {name: digest_file(directory / name) for name in ASSETS}
    release = find_release(github, snapshot["tag"])
    if release is not None and not release["draft"]:
        complete_assets(github, release)
        print(f"Already published {snapshot['tag']}")
        return False
    if release is None:
        release = github.request(
            "POST",
            "/releases",
            {
                "tag_name": snapshot["tag"],
                "target_commitish": target,
                "name": f"MTGO card data {snapshot['application_version']}",
                "body": (
                    f"Resolved MTGO catalog: {count:,} objects; JSON schema 1.\n\n"
                    f"Application manifest: {snapshot['application_manifest_url']}\n\n"
                    f"Manifest SHA-256: `{snapshot['application_manifest_sha256']}`\n\n"
                    "Includes compressed JSON, source manifests, metadata, and SHA256SUMS."
                ),
                "draft": True,
                "prerelease": False,
                "make_latest": "false",
            },
        )
    if release.get("tag_name") != snapshot["tag"] or not release.get("draft"):
        raise SnapshotError("GitHub did not return the matching draft release")
    release_id = release["id"]
    # evenflow: stage-assets
    # A failed upload leaves a draft. Retry replaces only that draft's assets,
    # and keeps its original target rather than moving an existing Git tag.
    for name in ASSETS:
        current = github.request("GET", f"/releases/{release_id}")
        if not current["draft"]:
            raise SnapshotError("Release was published while uploads were in progress")
        assets = github.request("GET", f"/releases/{release_id}/assets?per_page=100")
        for asset in assets:
            if asset.get("name") == name:
                github.request("DELETE", f"/releases/assets/{asset['id']}")
        github.upload(release_id, directory / name)
    complete_assets(github, release, sizes, digests)
    # evenflow: publish-complete
    published = github.request(
        "PATCH", f"/releases/{release_id}", {"draft": False, "make_latest": "false"}
    )
    if published.get("draft") is not False or published.get("tag_name") != snapshot["tag"]:
        raise SnapshotError("GitHub did not confirm publication of the matching release")
    print(f"Published {snapshot['tag']}")
    return True


def main():
    """Expose the two workflow phases without shell interpolation of metadata."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("check", "publish"))
    parser.add_argument("directory", type=Path)
    parser.add_argument("--target")
    arguments = parser.parse_args()
    github = GitHub(os.environ.get("GITHUB_REPOSITORY", ""), os.environ.get("GH_TOKEN", ""))
    if arguments.command == "check":
        needed = needs_export(github, read_snapshot(arguments.directory))
        line = f"needs_export={str(needed).lower()}\n"
        if output := os.environ.get("GITHUB_OUTPUT"):
            with open(output, "a", encoding="utf-8") as destination:
                destination.write(line)
        print(line, end="")
    else:
        if not arguments.target:
            parser.error("publish requires --target")
        publish(github, arguments.directory, arguments.target)


if __name__ == "__main__":
    try:
        main()
    except (SnapshotError, OSError) as error:
        print(f"Snapshot release failed: {error}", file=sys.stderr)
        sys.exit(1)
