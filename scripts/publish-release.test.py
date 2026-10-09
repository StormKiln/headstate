#!/usr/bin/env python3
"""#1779: immutable publication and verified duplicate delivery contracts."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import shutil
import unittest
from unittest.mock import patch
from types import SimpleNamespace
from release_delivery import GitHub, DeliveryError, parse_marker, with_marker, verify_completed

SPEC = importlib.util.spec_from_file_location('publisher', Path(__file__).with_name('publish-release.py'))
publisher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publisher)
SHA = 'a' * 40

class API:
    repo = 'octocat/hello-world'
    base = 'repos/octocat/hello-world'
    def __init__(self):
        self.value = None
        self.blobs = {}
        self.writes = []
        self.ambiguous = False
    def release(self, tag): return copy.deepcopy(self.value)
    def request(self, endpoint, method='GET', data=None):
        self.writes.append((method, copy.deepcopy(data)))
        if endpoint.endswith('/generate-notes'): return {'body': 'Generated changes'}
        if method == 'POST': self.value = dict(data, id=1)
        elif method == 'PATCH': self.value.update(data)
        return self.release('ignored')
    def assets(self, release_id):
        return [dict(name=name, id=name) for name in self.blobs]
    def download(self, asset, directory):
        path = Path(directory) / asset['name']; path.write_bytes(self.blobs[asset['name']]); return path
    def upload(self, tag, path):
        self.writes.append(('upload', path.name))
        if path.name in self.blobs: raise DeliveryError('asset already exists')
        self.blobs[path.name] = path.read_bytes()
        if self.ambiguous:
            self.ambiguous = False
            raise DeliveryError('response lost after successful upload')

class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        name = 'Headstate-Companion-1.2.3-build20.ipa'
        (self.directory / name).write_bytes(b'signed synthetic IPA')
        sha = hashlib.sha256(b'signed synthetic IPA').hexdigest()
        (self.directory / 'SHA256SUMS').write_text(f'{sha}  {name}\n')
        self.api = API()
        self.reserve(self.api)
    def reserve(self, api):
        evidence = dict(version=1, kind='mobile', tag='mobile-v1.2.3', sha=SHA,
                        run_id=10, run_attempt=1, state='reserved', assets={}, build_number=20)
        api.value = dict(id=1, tag_name='mobile-v1.2.3', draft=True, prerelease=True,
                         body=with_marker('Reserved mobile delivery', evidence))
    def publish(self, run_id=10, **kwargs):
        return publisher.publish(self.api, 'mobile-v1.2.3', SHA, run_id, 1, 'mobile',
                                 self.directory, 'Release notes', True, 20, **kwargs)
    def test_real_request_boundary_preserves_draft_identity_on_both_patches(self):
        # Reproduce the observed #1785 boundary: an omitted draft tag becomes
        # untagged. Exercise actual subprocess JSON serialization, not API.update.
        api = self.api
        requests = []
        def process(args, **kwargs):
            self.assertEqual(args[:2], ['gh', 'api'])
            self.assertEqual(args[2], f'{api.base}/releases/1')
            self.assertEqual(args[3:], ['--method', 'PATCH', '--input', '-'])
            data = json.loads(kwargs['input'])
            requests.append(data)
            api.value.update(data)
            api.value['tag_name'] = data.get('tag_name', 'untagged-observed')
            return SimpleNamespace(returncode=0, stdout=json.dumps(api.value), stderr='')
        api.request = lambda *args, **kwargs: GitHub.request(api, *args, **kwargs)
        upload = api.upload
        def upload_by_tag(tag, path):
            self.assertEqual(tag, api.value['tag_name'], 'upload cannot resolve a renamed draft')
            upload(tag, path)
        api.upload = upload_by_tag
        with patch('release_delivery.subprocess.run', side_effect=process):
            self.assertEqual(self.publish(), 'published')
        self.assertEqual(len(requests), 2)
        for data in requests:
            self.assertEqual(data['tag_name'], 'mobile-v1.2.3')
            self.assertEqual(data['target_commitish'], SHA)

    def test_changed_update_identity_stops_before_any_upload(self):
        for field, value in [('tag_name', 'untagged-observed'), ('id', 999),
                             ('target_commitish', 'b' * 40), ('draft', False)]:
            with self.subTest(field=field):
                api = API(); self.reserve(api); self.api = api
                original = api.request
                def change(*args, **kwargs):
                    result = original(*args, **kwargs)
                    result[field] = value
                    return result
                api.request = change
                with self.assertRaises(DeliveryError): self.publish()
                self.assertEqual(api.blobs, {})

    def test_failed_upload_reports_safe_reason_and_owned_draft(self):
        def fail(tag, path):
            raise DeliveryError('release asset upload failed (release not found; exit 1)')
        self.api.upload = fail
        with patch.object(publisher.time, 'sleep'), self.assertRaises(DeliveryError) as caught:
            self.publish()
        self.assertIn('release not found', str(caught.exception))
        self.assertIn('draft 1', str(caught.exception))

    def test_missing_mobile_reservation_never_creates_after_store_upload(self):
        self.api.value = None
        with self.assertRaises(DeliveryError): self.publish()
        self.assertEqual(self.api.writes, [])

    def test_lost_promotion_response_is_verified_without_another_write(self):
        class LostPromotion(API):
            def request(self, endpoint, method='GET', data=None):
                result = super().request(endpoint, method, data)
                if method == 'PATCH' and data.get('draft') is False:
                    raise DeliveryError('promotion response lost after commit')
                return result
        self.api = LostPromotion(); self.reserve(self.api)
        self.assertEqual(self.publish(), 'published')
        self.assertEqual(sum(method == 'PATCH' and data.get('draft') is False
                             for method, data in self.api.writes if isinstance(data, dict)), 1)

    def test_final_download_hash_mismatch_keeps_delivery_draft(self):
        class ChangedDownload(API):
            def __init__(self):
                super().__init__(); self.ipa_reads = 0
            def download(self, asset, directory):
                path = super().download(asset, directory)
                if asset['name'].endswith('.ipa'):
                    self.ipa_reads += 1
                    if self.value['draft'] and self.ipa_reads >= 2:
                        path.write_bytes(b'corrupt final downloaded bytes')
                return path
        self.api = ChangedDownload(); self.reserve(self.api)
        with self.assertRaises(DeliveryError): self.publish()
        self.assertTrue(self.api.value['draft'])

    def test_first_delivery_uploads_to_draft_then_completes_coherent_set(self):
        self.assertEqual(self.publish(), 'published')
        self.assertFalse(self.api.value['draft'])
        marker = verify_completed(self.api, self.api.value, 'mobile-v1.2.3', SHA, 'mobile')
        self.assertEqual(marker['state'], 'complete')
        self.assertEqual(set(marker['assets']), set(self.api.blobs))
        self.assertEqual(parse_marker(self.api.writes[0][1]['body'])['state'], 'uploading')
    def test_duplicate_verified_delivery_is_read_only_even_if_rebuild_bytes_differ(self):
        self.publish(); self.api.writes.clear()
        next(self.directory.glob('*.ipa')).write_bytes(b'different independently signed output')
        self.assertEqual(self.publish(run_id=11), 'noop')
        self.assertEqual(self.api.writes, [])
    def test_ambiguous_successful_upload_is_reconciled_without_overwrite(self):
        self.api.ambiguous = True
        self.assertEqual(self.publish(), 'published')
        self.assertEqual(sum(item[0] == 'upload' for item in self.api.writes), 2)
    def test_tampered_completed_asset_never_becomes_a_noop(self):
        self.publish(); self.api.writes.clear()
        self.api.blobs[next(name for name in self.api.blobs if name.endswith('.ipa'))] = b'corrupt'
        with self.assertRaises(DeliveryError): self.publish(run_id=11)
        self.assertEqual(self.api.writes, [])
    def test_foreign_partial_delivery_is_never_taken_over(self):
        self.publish(); self.api.value['draft'] = True; self.api.writes.clear()
        with self.assertRaises(DeliveryError): self.publish(run_id=11)
        self.assertEqual(self.api.writes, [])
    def test_conflicting_partial_bytes_are_not_clobbered(self):
        self.publish(); self.api.value['draft'] = True; self.api.writes.clear()
        self.api.blobs['SHA256SUMS'] = b'conflicting bytes'
        with self.assertRaises(DeliveryError): self.publish()
        self.assertFalse(any(item[0] == 'upload' for item in self.api.writes))
    def test_missing_or_duplicate_provenance_cannot_pass(self):
        self.publish(); body = self.api.value['body']
        for invalid in ['Human notes only', body + body]:
            self.api.value['body'] = invalid
            with self.assertRaises(DeliveryError): self.publish(run_id=11)
    def test_failed_final_admission_leaves_a_draft(self):
        def refuse(): raise DeliveryError('new CI attempt failed')
        with self.assertRaises(DeliveryError): self.publish(admit=refuse)
        self.assertTrue(self.api.value['draft'])
    def test_human_release_notes_survive_marker_updates(self):
        self.publish()
        self.assertIn('Release notes', self.api.value['body'])
        self.assertEqual(parse_marker(self.api.value['body'])['sha'], SHA)
    def test_extra_remote_asset_is_not_accepted(self):
        self.publish(); self.api.blobs['unexpected.log'] = b'private diagnostics'
        with self.assertRaises(DeliveryError): self.publish(run_id=11)

SPEC_CRYPTO = importlib.util.spec_from_file_location('crypto_fixtures', Path(__file__).with_name('release_delivery.test.py'))
crypto_fixtures = importlib.util.module_from_spec(SPEC_CRYPTO)
SPEC_CRYPTO.loader.exec_module(crypto_fixtures)

@unittest.skipUnless(shutil.which('minisign'), 'install minisign for real desktop publication fixtures')
class DesktopPublicationTests(crypto_fixtures.SignatureTests):
    def test_lost_draft_create_response_is_reconciled_without_another_create(self):
        class LostCreate(API):
            def request(self, endpoint, method='GET', data=None):
                result = super().request(endpoint, method, data)
                if method == 'POST' and endpoint.endswith('/releases'):
                    raise DeliveryError('create response lost after commit')
                return result
        api = LostCreate()
        result = publisher.publish(api, 'v1.2.3', SHA, 10, 1, 'desktop', self.assets,
                                   'Desktop notes', updater_config=self.config)
        self.assertEqual(result, 'published')
        self.assertEqual(sum(method == 'POST' for method, _ in api.writes), 1)
        self.assertEqual(len(api.blobs), 9)

if __name__ == '__main__': unittest.main()
