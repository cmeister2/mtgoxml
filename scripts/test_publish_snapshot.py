"""Exercise release state transitions without credentials or GitHub mutations."""

import contextlib
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError, URLError

import publish_snapshot as publisher


TARGET = "a" * 40


class FakeGitHub:
    """Model GitHub's published-only tag lookup and resumable draft uploads."""

    def __init__(self):
        self.release = None
        self.assets = []
        self.calls = []
        self.uploads = 0
        self.fail_upload = None
        self.bad_digest = None
        self.next_asset_id = 1

    def request(self, method, path, payload=None):
        self.calls.append((method, path, payload))
        if method == "GET" and path.startswith("/releases/tags/"):
            if self.release and not self.release["draft"]:
                return dict(self.release)
            raise publisher.GitHubError(404, "Not Found")
        if method == "GET" and path.startswith("/releases?per_page=100&page="):
            return [dict(self.release)] if self.release else []
        if method == "POST" and path == "/releases":
            if self.release:
                raise AssertionError("Cannot create a second release for the same tag")
            self.release = {"id": 7, **payload}
            return dict(self.release)
        if method == "GET" and path == "/releases/7":
            return dict(self.release)
        if method == "GET" and path == "/releases/7/assets?per_page=100":
            return [dict(asset) for asset in self.assets]
        if method == "DELETE" and path.startswith("/releases/assets/"):
            self.assets = [asset for asset in self.assets if path != f"/releases/assets/{asset['id']}"]
            return None
        if method == "PATCH" and path == "/releases/7":
            if payload.get("draft") is False:
                if {asset["name"] for asset in self.assets} != set(publisher.ASSETS):
                    raise AssertionError("Published before all assets were uploaded")
            self.release.update(payload)
            return dict(self.release)
        raise AssertionError(f"Unexpected API call {method} {path}")

    def upload(self, release_id, path):
        if not self.release["draft"] or release_id != self.release["id"]:
            raise AssertionError("Uploads must target the matching draft")
        self.uploads += 1
        if self.uploads == self.fail_upload:
            raise publisher.SnapshotError("Injected upload failure")
        self.calls.append(("UPLOAD", path.name, None))
        self.assets.append(
            {
                "id": self.next_asset_id,
                "name": path.name,
                "size": path.stat().st_size,
                "state": "uploaded",
                "digest": "sha256:" + (
                    "0" * 64 if path.name == self.bad_digest else publisher.digest_file(path)
                ),
            }
        )
        self.next_asset_id += 1


class PublicationTests(unittest.TestCase):
    """Keep successful, interrupted and unchanged publication behavior distinct."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        manifest = b"exact application manifest\r\n"
        (self.directory / "MTGO.exe.manifest").write_bytes(manifest)
        (self.directory / "MTGO.application").write_bytes(b"deployment")
        (self.directory / "card-data.json.gz").write_bytes(b"compressed JSON fixture")
        digest = hashlib.sha256(manifest).hexdigest()
        self.snapshot = {
            "schema_version": 1,
            "deployment_url": "https://example.invalid/MTGO.application",
            "application_manifest_url": "https://example.invalid/v1/MTGO.exe.manifest",
            "application_version": "1.2.3.4",
            "application_manifest_sha256": digest,
            "tag": f"mtgo-data-v1-{digest}",
            "object_count": 42,
        }
        self.write_metadata()
        self.github = FakeGitHub()
        self.output = contextlib.redirect_stdout(io.StringIO())
        self.output.__enter__()
        self.addCleanup(self.output.__exit__, None, None, None)

    def write_metadata(self):
        (self.directory / "snapshot.json").write_text(json.dumps(self.snapshot), "utf-8")

    def publish(self, target=TARGET):
        return publisher.publish(self.github, self.directory, target)

    def test_new_snapshot_stages_verifies_then_publishes_without_changing_latest(self):
        self.assertTrue(publisher.needs_export(self.github, self.snapshot))
        self.assertTrue(self.publish())
        self.assertFalse(self.github.release["draft"])
        creates = [call for call in self.github.calls if call[:2] == ("POST", "/releases")]
        self.assertEqual(len(creates), 1)
        self.assertIs(creates[0][2]["draft"], True)
        self.assertEqual(creates[0][2]["target_commitish"], TARGET)
        self.assertEqual(creates[0][2]["make_latest"], "false")
        self.assertEqual(self.github.calls[-1], ("PATCH", "/releases/7", {"draft": False, "make_latest": "false"}))
        expected = "".join(
            f"{publisher.digest_file(self.directory / name)}  {name}\n"
            for name in publisher.ASSETS[:-1]
        )
        self.assertEqual((self.directory / "SHA256SUMS").read_text("ascii"), expected)

    def test_unchanged_published_snapshot_needs_no_export_or_mutation(self):
        self.publish()
        self.github.calls.clear()
        self.assertFalse(publisher.needs_export(self.github, self.snapshot))
        self.assertFalse(self.publish())
        self.assertTrue(all(call[0] == "GET" for call in self.github.calls))

    def test_interrupted_draft_is_found_and_replaced_before_publication(self):
        self.github.fail_upload = 3
        with self.assertRaisesRegex(publisher.SnapshotError, "Injected"):
            self.publish()
        self.assertTrue(self.github.release["draft"])
        self.assertEqual(len(self.github.assets), 2)
        self.assertTrue(publisher.needs_export(self.github, self.snapshot))
        self.github.fail_upload = None
        self.publish(target="b" * 40)
        self.assertFalse(self.github.release["draft"])
        self.assertEqual(self.github.release["target_commitish"], TARGET)
        self.assertEqual(len(self.github.assets), 5)
        self.assertEqual(sum(call[:2] == ("POST", "/releases") for call in self.github.calls), 1)

    def test_incomplete_published_release_fails_instead_of_replacing_it(self):
        self.publish()
        self.github.assets.pop()
        self.github.calls.clear()
        with self.assertRaisesRegex(publisher.SnapshotError, "missing or incomplete"):
            publisher.needs_export(self.github, self.snapshot)
        with self.assertRaisesRegex(publisher.SnapshotError, "missing or incomplete"):
            self.publish()
        self.assertTrue(all(call[0] == "GET" for call in self.github.calls))

    def test_unfinished_asset_is_not_considered_a_complete_snapshot(self):
        self.publish()
        self.github.assets[0]["state"] = "starter"
        with self.assertRaisesRegex(publisher.SnapshotError, "missing or incomplete"):
            publisher.needs_export(self.github, self.snapshot)

    def test_mismatched_uploaded_digest_leaves_draft_unpublished(self):
        self.github.bad_digest = "card-data.json.gz"
        with self.assertRaisesRegex(publisher.SnapshotError, "unexpected SHA-256"):
            self.publish()
        self.assertTrue(self.github.release["draft"])
        self.assertFalse(any(call[0] == "PATCH" for call in self.github.calls))

    def test_publication_response_must_confirm_matching_published_release(self):
        original_request = self.github.request

        def refuse_publication(method, path, payload=None):
            if method == "PATCH":
                return dict(self.github.release)
            return original_request(method, path, payload)

        self.github.request = refuse_publication
        with self.assertRaisesRegex(publisher.SnapshotError, "did not confirm publication"):
            self.publish()
        self.assertTrue(self.github.release["draft"])

    def test_published_during_upload_is_not_clobbered(self):
        original_upload = self.github.upload

        def publish_after_upload(release_id, path):
            original_upload(release_id, path)
            self.github.release["draft"] = False

        self.github.upload = publish_after_upload
        with self.assertRaisesRegex(publisher.SnapshotError, "published while uploads"):
            self.publish()
        self.assertEqual(len(self.github.assets), 1)
        self.assertFalse(any(call[0] == "DELETE" for call in self.github.calls))

    def test_missing_export_is_rejected_before_creating_release(self):
        self.snapshot.pop("object_count")
        self.write_metadata()
        with self.assertRaisesRegex(publisher.SnapshotError, "object_count"):
            self.publish()
        self.assertEqual(self.github.calls, [])

    def test_application_manifest_digest_and_tag_are_validated(self):
        (self.directory / "MTGO.exe.manifest").write_bytes(b"different snapshot")
        with self.assertRaisesRegex(publisher.SnapshotError, "does not match"):
            publisher.read_snapshot(self.directory)
        self.snapshot["tag"] = "mtgo-data-v1-" + "f" * 64
        self.write_metadata()
        with self.assertRaisesRegex(publisher.SnapshotError, "tag does not match"):
            publisher.read_snapshot(self.directory)

    def test_only_404_tag_lookup_can_mean_missing(self):
        for status in (401, 403, 429, 500, 503):
            with self.subTest(status=status):
                with patch.object(self.github, "request", side_effect=publisher.GitHubError(status, "Failure")):
                    with self.assertRaises(publisher.GitHubError) as raised:
                        publisher.needs_export(self.github, self.snapshot)
                    self.assertEqual(raised.exception.status, status)

    def test_inaccessible_repository_does_not_mean_new_snapshot(self):
        with patch.object(self.github, "request", side_effect=publisher.GitHubError(404, "Not Found")) as request:
            with self.assertRaises(publisher.GitHubError):
                publisher.needs_export(self.github, self.snapshot)
            self.assertEqual(request.call_count, 2)

    def test_draft_lookup_visits_later_release_pages(self):
        draft = {"tag_name": self.snapshot["tag"], "draft": True, "id": 7}
        with patch.object(self.github, "request", side_effect=[
            publisher.GitHubError(404, "Not Found"),
            [{"tag_name": f"other-{i}"} for i in range(100)],
            [draft],
        ]) as request:
            self.assertEqual(publisher.find_release(self.github, self.snapshot["tag"]), draft)
            self.assertEqual(request.call_args.args[1], "/releases?per_page=100&page=2")

    def test_upload_passes_file_stream_with_length_instead_of_buffered_bytes(self):
        github = publisher.GitHub("owner/repository", "test-token")
        path = self.directory / "card-data.json.gz"

        def inspect_request(method, url, data, headers):
            self.assertEqual(method, "POST")
            self.assertIn("/releases/7/assets?name=card-data.json.gz", url)
            self.assertTrue(hasattr(data, "read"))
            self.assertEqual(headers["Content-Length"], str(path.stat().st_size))
            self.assertEqual(data.read(), path.read_bytes())

        with patch.object(github, "_request", side_effect=inspect_request):
            github.upload(7, path)

    def test_http_client_preserves_status_and_rejects_transport_failure(self):
        github = publisher.GitHub("owner/repository", "test-token")
        with patch.object(publisher, "urlopen", side_effect=HTTPError(
            "https://api.github.com/example", 403, "Forbidden", {}, None
        )):
            with self.assertRaises(publisher.GitHubError) as raised:
                github.request("GET", "/releases")
            self.assertEqual(raised.exception.status, 403)
        with patch.object(publisher, "urlopen", side_effect=URLError("connection refused")):
            with self.assertRaisesRegex(publisher.SnapshotError, "request failed"):
                github.request("GET", "/releases")


if __name__ == "__main__":
    unittest.main()
