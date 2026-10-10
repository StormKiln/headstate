#!/usr/bin/env python3
"""Store delivery reservations consume build numbers before any upload (#1779)."""
import importlib.util
import json
from pathlib import Path
import unittest
import sys
sys.path.insert(0, str(Path(__file__).parent))

SCRIPT = Path(__file__).with_name('reserve-mobile-release.py')

class Reservations(unittest.TestCase):
    def setUp(self):
        self.assertTrue(SCRIPT.exists(), 'mobile delivery reservation is missing')
        spec = importlib.util.spec_from_file_location('reservation', SCRIPT)
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)
        self.created = []
        self.checked = []

    def release(self, tag='mobile-v10.0.1', sha='a' * 40, build=200, draft=True, state='reserved'):
        marker = dict(version=1, kind='mobile', tag=tag, sha=sha, run_id=100, run_attempt=1, build_number=build,
                      state=state, assets={})
        return dict(tag_name=tag, draft=draft, body='<!-- headstate-delivery-v1:' + json.dumps(marker) + ' -->', assets=[])

    def reserve(self, releases, **changes):
        args = dict(tag='mobile-v10.0.2', sha='b' * 40, run_id='101', run_number=20,
                    mark=100, releases=releases, verify=lambda item: self.checked.append(item),
                    create=lambda body: self.created.append(body))
        args.update(changes)
        return self.module.reserve(**args)

    def orphan(self):
        # Observed release408407263 after body-only PATCH lost its REST tag;
        # Apple had already accepted build46, while GitHub had zero assets.
        item = self.release(tag='mobile-v10.0.2',
                            sha='89ad2f30b4bb8c62f84b34c0660567a53b80708c',
                            build=46, state='uploading')
        item.update(id=408407263, tag_name='untagged-82ea65cf29f7d316dbe2')
        return item

    def test_observed_orphan_cannot_resubmit_same_provenance_tag(self):
        with self.assertRaises(ValueError):
            self.reserve([self.orphan()], tag='mobile-v10.0.2', mark=45,
                         sha='89ad2f30b4bb8c62f84b34c0660567a53b80708c')
        self.assertEqual(self.created, [])
        self.assertEqual(self.checked, [])

    def test_observed_orphan_consumes_build_for_distinct_recovery_tag(self):
        self.assertEqual(self.reserve([self.orphan()], tag='mobile-v10.0.3', mark=45),
                         ('new', 47))
        self.assertEqual(self.created[0]['build_number'], 47)

    def test_orphan_cannot_hide_behind_complete_duplicate(self):
        complete = self.release(tag='mobile-v10.0.2', draft=False, state='complete')
        with self.assertRaises(ValueError):
            self.reserve([complete, self.orphan()], tag='mobile-v10.0.2', sha='a'*40)
        self.assertEqual(self.checked, [])
        self.assertEqual(self.created, [])

    def test_conflicting_mobile_identity_or_malformed_marker_fails_closed(self):
        from release_delivery import DeliveryError
        conflicting = self.orphan()
        conflicting['tag_name'] = 'mobile-v10.0.1'
        malformed = self.orphan()
        malformed['body'] = '<!-- headstate-delivery-v1:broken -->'
        for item in [conflicting, malformed]:
            with self.subTest(item=item), self.assertRaises((ValueError, DeliveryError)):
                self.reserve([item], tag='mobile-v10.0.3', mark=45)
        self.assertEqual(self.created, [])

    def test_renamed_complete_release_is_not_a_verified_noop(self):
        item = self.release(tag='mobile-v10.0.2', draft=False, state='complete')
        item['tag_name'] = 'untagged-renamed'
        with self.assertRaises(ValueError):
            self.reserve([item], tag='mobile-v10.0.2', sha='a'*40)
        self.assertEqual(self.checked, [])
        self.assertEqual(self.created, [])

    def test_nonmobile_drafts_do_not_block_reservation(self):
        unrelated = dict(tag_name='v10.0.2', draft=True, body='', assets=[])
        self.assertEqual(self.reserve([unrelated], mark=45), ('new', 46))

    def test_cli_paginates_orphan_assets_and_preserves_their_higher_floor(self):
        from unittest.mock import patch
        import contextlib
        import io
        import os
        import tempfile
        import release_delivery
        with tempfile.TemporaryDirectory() as directory:
            mark = Path(directory) / 'mark'
            mark.write_text('45')
            argv = [str(SCRIPT), '--repo', 'owner/repo', '--tag', 'mobile-v10.0.3',
                    '--sha', 'b'*40, '--run-id', '102', '--run-attempt', '1',
                    '--run-number', '20', '--mark-file', str(mark)]
            with patch.object(sys, 'argv', argv), patch.object(release_delivery, 'GitHub') as api, \
                    patch.dict(os.environ, {'GITHUB_OUTPUT': ''}), contextlib.redirect_stdout(io.StringIO()):
                client = api.return_value
                client.pages.return_value = [self.orphan()]
                client.request.side_effect = lambda _path, **kw: dict(id=123, **kw['data'])
                client.assets.return_value = [dict(name='Headstate-Companion-10.0.2-build61.ipa')]
                self.module.main()
                client.assets.assert_called_once_with(408407263)
                client.request.assert_called_once()
                request = client.request.call_args
                self.assertEqual(request.kwargs['method'], 'POST')
                marker = release_delivery.parse_marker(request.kwargs['data']['body'])
                self.assertEqual(marker['build_number'], 62)

    def test_creation_response_must_confirm_identity_before_upload_authority(self):
        from unittest.mock import patch
        import contextlib
        import io
        import os
        import tempfile
        import release_delivery
        for field, value in [('id', None), ('id', True), ('id', 0),
                             ('tag_name', 'untagged-renamed'), ('target_commitish', 'c'*40),
                             ('draft', False), ('body', ''), ('body', self.release()['body'])]:
            with self.subTest(field=field, value=value), tempfile.TemporaryDirectory() as directory:
                mark = Path(directory) / 'mark'
                mark.write_text('46')
                output = Path(directory) / 'output'
                argv = [str(SCRIPT), '--repo', 'owner/repo', '--tag', 'mobile-v10.0.3',
                        '--sha', 'b'*40, '--run-id', '102', '--run-attempt', '1',
                        '--run-number', '20', '--mark-file', str(mark)]
                with patch.object(sys, 'argv', argv), patch.object(release_delivery, 'GitHub') as api, \
                        patch.dict(os.environ, {'GITHUB_OUTPUT': str(output)}), contextlib.redirect_stdout(io.StringIO()):
                    client = api.return_value
                    client.pages.return_value = []
                    def response(_path, **kw):
                        result = dict(id=123, **kw['data'])
                        result[field] = value
                        return result
                    client.request.side_effect = response
                    with self.assertRaises(ValueError):
                        self.module.main()
                    client.request.assert_called_once()
                    self.assertFalse(output.exists(), 'uncertain creation must emit no store authority')

    def test_reversed_queue_order_allocates_above_every_reserved_build(self):
        result = self.reserve([self.release(build=301)])
        self.assertEqual(result, ('new', 302))
        self.assertEqual(self.created[0]['build_number'], 302)
        self.assertEqual(self.created[0]['sha'], 'b' * 40)

    def test_published_legacy_artifacts_and_committed_floor_are_respected(self):
        legacy = dict(tag_name='mobile-v9.0.0', draft=False, body='',
                      assets=[dict(name='Headstate-Companion-9.0.0-build250.ipa')])
        self.assertEqual(self.reserve([legacy]), ('new', 251))

    def test_complete_duplicate_verifies_bytes_without_creating_or_uploading(self):
        release = self.release(draft=False, state='complete')
        self.assertEqual(self.reserve([release], tag=release['tag_name'], sha='a'*40), ('noop', 200))
        self.assertEqual(self.checked, [release])
        self.assertEqual(self.created, [])

    def test_bad_completed_artifacts_fail_closed(self):
        def invalid(_):
            raise ValueError('checksum mismatch')
        with self.assertRaisesRegex(ValueError, 'checksum'):
            self.reserve([self.release(draft=False, state='complete')],
                         tag='mobile-v10.0.1', sha='a'*40, verify=invalid)
        self.assertEqual(self.created, [])

    def test_partial_or_ambiguous_store_upload_cannot_be_resubmitted_even_same_run(self):
        for assets in [[], [dict(name='partial.ipa')]]:
            release = self.release()
            release['assets'] = assets
            with self.subTest(assets=assets), self.assertRaises(ValueError):
                self.reserve([release], tag='mobile-v10.0.1', sha='a'*40, run_id='100')
        self.assertEqual(self.created, [])

    def test_changed_source_or_missing_provenance_cannot_claim_success(self):
        for release in [self.release(draft=False, state='complete'),
                        dict(tag_name='mobile-v10.0.1', draft=False, body='', assets=[])]:
            with self.subTest(release=release), self.assertRaises(ValueError):
                self.reserve([release], tag='mobile-v10.0.1')

    def test_unknown_partial_reservation_prevents_reusing_an_unknown_build(self):
        with self.assertRaises(ValueError):
            self.reserve([dict(tag_name='mobile-v9.0.0', draft=True, body='', assets=[])])

    def test_rerun_or_changed_job_identity_cannot_reuse_reservation_outputs(self):
        release = self.release()
        args = dict(tag='mobile-v10.0.1', sha='a'*40, run_id='100', run_attempt=1, build=200)
        self.module.check_owner(release, **args)
        for changes in [dict(run_attempt=2), dict(run_id='101'), dict(sha='b'*40),
                        dict(build=201), dict(tag='mobile-v10.0.2')]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.module.check_owner(release, **{**args, **changes})

    def test_read_only_builder_validates_receipt_without_github_access(self):
        from unittest.mock import patch
        import release_delivery
        receipt = release_delivery.parse_marker(self.release()['body'])
        argv = [str(SCRIPT), '--check-owner', '--repo', 'owner/repo',
                '--tag', receipt['tag'], '--sha', receipt['sha'], '--run-id', '100',
                '--run-attempt', '1', '--build-number', '200',
                '--reservation-json', json.dumps(receipt)]
        with patch.object(sys, 'argv', argv), patch.object(release_delivery, 'GitHub') as api:
            self.module.main()
            api.assert_not_called()
        argv[argv.index('--run-attempt') + 1] = '2'
        with patch.object(sys, 'argv', argv), patch.object(release_delivery, 'GitHub') as api:
            with self.assertRaises(ValueError):
                self.module.main()
            api.assert_not_called()

    def test_failed_or_ambiguous_creation_is_not_retried(self):
        def failed(body):
            self.created.append(body)
            raise RuntimeError('response lost after draft creation')
        with self.assertRaises(RuntimeError):
            self.reserve([], create=failed)
        self.assertEqual(len(self.created), 1)

class Workflow(unittest.TestCase):
    def test_noop_jobs_do_not_build_or_submit_to_stores(self):
        import runpy
        root = Path(__file__).resolve().parents[1]
        parser = runpy.run_path(str(root / 'scripts/check-workflow-shells.py'))
        text = (root / '.github/workflows/mobile-release.yml').read_text()
        jobs = dict(parser['jobs_of'](text))
        self.assertIn("'mobile-signed-delivery'", text)
        self.assertIn('  queue: max', text)
        self.assertIn('  cancel-in-progress: false', text)
        self.assertNotIn('--clobber', text)
        for job in ['ios-release', 'android-release']:
            body = '\n'.join(jobs[job])
            self.assertIn('needs: reserve', body)
            self.assertIn('BUILD_NUMBER: ${{ needs.reserve.outputs.build_number }}', body)
            steps = list(parser['steps_of'](jobs[job]))
            for step in steps:
                head = step[0]
                if 'actions/checkout@' in head:
                    continue
                guard = next((line for line in step if line.startswith('        if: ')), '')
                self.assertIn('needs.reserve.outputs.delivery', guard, head)
            store = next(step for step in steps if 'name: Upload to ' in step[0])
            self.assertIn("needs.reserve.outputs.delivery != 'noop'", '\n'.join(store))
            self.assertIn("env.DRY_RUN != 'true'", '\n'.join(store))
            self.assertIn('--check-owner', body)
            self.assertIn('--reservation-json "$MOBILE_RESERVATION"', body)
            self.assertIn('MOBILE_RESERVATION: ${{ needs.reserve.outputs.reservation }}', body)
            self.assertIn('--run-attempt "$GITHUB_RUN_ATTEMPT"', body)
        publish = '\n'.join(jobs['publish'])
        self.assertIn("needs.reserve.result == 'success'", publish)
        self.assertIn("needs.ios-release.result == 'success'", publish)
        self.assertIn('--verify-only', publish)
        self.assertIn('scripts/publish-release.py --kind mobile', publish)

if __name__ == '__main__':
    unittest.main()
