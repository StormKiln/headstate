#!/usr/bin/env python3
"""Real minisign verification and adversarial provenance/API fixtures (#1779)."""
import base64
import copy
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace
from release_delivery import (ApiError, DeliveryError, GitHub, digest, parse_marker,
                              verify_payload, with_marker)

SHA = 'a' * 40

def marker():
    return dict(version=1, kind='desktop', tag='v1.2.3', sha=SHA, run_id=1,
                run_attempt=1, state='complete', assets={})

class ProvenanceTests(unittest.TestCase):
    def test_upload_failure_diagnostics_keep_status_but_redact_process_output(self):
        for stderr, expected in [
            ('release not found\nsecret-token', 'release not found'),
            ('HTTP 403 forbidden https://host/?token=secret-token', 'HTTP 403'),
            ('private local path secret-token', 'exit 1'),
        ]:
            with self.subTest(stderr=stderr):
                result = SimpleNamespace(returncode=1, stderr=stderr, stdout='secret-token')
                with patch('release_delivery.subprocess.run', return_value=result), self.assertRaises(DeliveryError) as caught:
                    GitHub('octocat/hello-world').upload('mobile-v1.2.3', Path('artifact.ipa'))
                self.assertIn(expected, str(caught.exception))
                self.assertNotIn('secret-token', str(caught.exception))
                self.assertNotIn('https://', str(caught.exception))

    def test_marker_update_preserves_human_notes(self):
        body = with_marker('Human release notes\n\nChangelog.', marker())
        changed = marker(); changed['run_id'] = 2
        self.assertIn('Human release notes\n\nChangelog.', with_marker(body, changed))
        self.assertEqual(parse_marker(with_marker(body, changed)), changed)
    def test_missing_marker_is_absent_but_malformed_or_duplicate_is_failure(self):
        self.assertIsNone(parse_marker('Legacy release'))
        body = with_marker('Notes', marker())
        for malformed in [body + body, '<!-- headstate-delivery-v1:broken -->', body.replace('"version":1', '"version":1,"version":1')]:
            with self.subTest(body=malformed), self.assertRaises(DeliveryError): parse_marker(malformed)
    def test_unsafe_asset_names_and_hashes_are_refused(self):
        for name, checksum in [('../secret', 'a'*64), ('name', 'invalid'), ('x\\y', 'a'*64)]:
            bad = marker(); bad['assets'] = {name: checksum}
            with self.assertRaises(DeliveryError): with_marker('Notes', bad)
    def test_unpublished_draft_is_found_after_tag_endpoint_404(self):
        class DraftAPI(GitHub):
            def request(self, *args, **kwargs): raise ApiError('Not found', 404)
            def pages(self, endpoint, key=None): return [dict(id=2, tag_name='v1.2.3', draft=True)]
        self.assertTrue(DraftAPI('octocat/hello-world').release('v1.2.3')['draft'])
    def test_auth_failure_is_never_mistaken_for_missing_release(self):
        class DeniedAPI(GitHub):
            def request(self, *args, **kwargs): raise ApiError('Denied', 403)
        with self.assertRaises(ApiError): DeniedAPI('octocat/hello-world').release('v1.2.3')
    def test_multiple_drafts_with_same_tag_fail_closed(self):
        class DraftAPI(GitHub):
            def request(self, *args, **kwargs): raise ApiError('Not found', 404)
            def pages(self, endpoint, key=None): return [dict(id=i, tag_name='v1.2.3', draft=True) for i in [1,2]]
        with self.assertRaises(DeliveryError): DraftAPI('octocat/hello-world').release('v1.2.3')
    def test_pagination_does_not_drop_duplicate_runs_on_later_pages(self):
        class PagesAPI(GitHub):
            def request(self, endpoint, **kwargs):
                return {'workflow_runs': list(range(100)) if endpoint.endswith('page=1') else [100]}
        self.assertEqual(len(PagesAPI('octocat/hello-world').pages('runs', key='workflow_runs')), 101)

@unittest.skipUnless(shutil.which('minisign'), 'install minisign for real signature fixtures; release CI requires it')
class SignatureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name); self.assets = root / 'assets'; self.assets.mkdir()
        self.config = root / 'config.json'; self.secret = root / 'test.key'; public = root / 'test.pub'
        subprocess.run(['minisign', '-G', '-W', '-s', str(self.secret), '-p', str(public)], check=True, capture_output=True)
        self.config.write_text(json.dumps({'plugins': {'updater': {'pubkey': base64.b64encode(public.read_bytes()).decode()}}}))
        names = {'darwin-aarch64': 'Headstate-v1.2.3.app.tar.gz', 'darwin-x86_64': 'Headstate-v1.2.3.app.tar.gz',
                 'windows-x86_64': 'Headstate_1.2.3_x64-setup.exe', 'linux-x86_64': 'Headstate_1.2.3_amd64.AppImage'}
        signatures = {}
        for name in set(names.values()):
            bundle = self.assets / name; bundle.write_bytes(b'synthetic bundle: ' + name.encode())
            raw = root / 'signature.minisig'
            subprocess.run(['minisign', '-S', '-s', str(self.secret), '-m', str(bundle), '-x', str(raw)], check=True, capture_output=True)
            signatures[name] = base64.b64encode(raw.read_bytes()).decode()
            (self.assets / (name + '.sig')).write_text(signatures[name])
        for name in ['Headstate_1.2.3_universal.dmg', 'Headstate_1.2.3_amd64.deb']:
            (self.assets / name).write_bytes(b'synthetic installer')
        self.manifest = dict(version='1.2.3', platforms={platform: dict(signature=signatures[name], url=f'https://github.com/octocat/hello-world/releases/download/v1.2.3/{name}') for platform, name in names.items()})
        self.write_manifest()
    def write_manifest(self): (self.assets / 'latest.json').write_text(json.dumps(self.manifest))
    def verify(self):
        evidence = marker(); evidence['assets'] = {p.name: digest(p) for p in self.assets.iterdir()}
        verify_payload(self.assets, evidence, 'octocat/hello-world', self.config)
    def test_real_signed_nine_asset_set_passes(self): self.verify()
    def test_rehashed_tampered_bundle_still_fails_cryptographic_verification(self):
        (self.assets / 'Headstate_1.2.3_x64-setup.exe').write_bytes(b'changed bytes with recomputed SHA256')
        with self.assertRaises(subprocess.CalledProcessError): self.verify()
    def test_cross_release_manifest_url_is_rejected(self):
        self.manifest['platforms']['linux-x86_64']['url'] = self.manifest['platforms']['linux-x86_64']['url'].replace('v1.2.3/', 'v1.2.2/')
        self.write_manifest()
        with self.assertRaises(DeliveryError): self.verify()
    def test_detached_signature_mismatch_is_rejected(self):
        (self.assets / 'Headstate_1.2.3_x64-setup.exe.sig').write_text('different')
        with self.assertRaises(DeliveryError): self.verify()
    def test_wrong_installer_version_cannot_hide_behind_suffix(self):
        (self.assets / 'Headstate_1.2.3_universal.dmg').rename(self.assets / 'Headstate_1.2.2_universal.dmg')
        with self.assertRaises(DeliveryError): self.verify()

if __name__ == '__main__': unittest.main()
