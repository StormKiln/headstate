#!/usr/bin/env python3
"""#1779: publish one immutable, verified byte set; duplicates only verify.

Publication stays a draft until all uploaded bytes and updater signatures have
been checked. A failed upload response is reconciled by downloading what arrived;
no branch ever deletes or overwrites an asset.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from release_delivery import (GitHub, DeliveryError, check_identity, digest, parse_marker,
                              safe_name, verify_completed, verify_payload, with_marker)


def check_partial(api, release, expected, only=None):
    assets = api.assets(release['id'])
    names = [asset['name'] for asset in assets]
    if len(names) != len(set(names)) or not set(names) <= set(expected):
        raise DeliveryError('partial release contains duplicate or undeclared assets')
    with tempfile.TemporaryDirectory() as work:
        for asset in assets:
            if only is not None and asset['name'] not in only:
                continue
            if digest(api.download(asset, work)) != expected[asset['name']]:
                raise DeliveryError('partial release contains conflicting bytes; refusing overwrite')
    return set(names)


def check_response_identity(release, tag, sha, marker, release_id, *, draft):
    # #1785: the observed draft lost its tag during the body update. Do not
    # let a changed API identity send uploads to a different (or missing) tag.
    if (not isinstance(release, dict) or type(release.get('id')) is not int
            or release['id'] < 1 or (release_id is not None and release['id'] != release_id)
            or release.get('tag_name') != tag or release.get('target_commitish') != sha
            or release.get('draft') is not draft or parse_marker(release.get('body')) != marker):
        raise DeliveryError('release response changed the owned delivery identity; reconcile the draft before retrying')


def publish(api, tag, sha, run_id, run_attempt, kind, directory, notes,
            prerelease=False, build_number=None, updater_config=None, admit=lambda: None,
            generate_notes=False):
    release = api.release(tag)
    if release and not release.get('draft'):
        verify_completed(api, release, tag, sha, kind, updater_config)
        return 'noop'
    if kind == 'mobile' and not release:
        raise DeliveryError('mobile publication requires its existing owned reservation; refusing to recreate it after store uploads')
    prior = None
    if release:
        prior = parse_marker(release.get('body'))
        check_identity(prior, tag, sha, kind)
        if (prior['run_id'], prior['run_attempt']) != (run_id, run_attempt):
            raise DeliveryError('another run owns this unfinished delivery; refusing takeover')
        if kind == 'mobile' and prior['build_number'] != build_number:
            raise DeliveryError('mobile reservation build number changed')
        if kind == 'desktop':
            notes = release['body']  # Preserve generated changelog/user notes on a partial retry.
    directory = Path(directory)
    files = sorted(directory.iterdir())
    if not files or any(not path.is_file() or path.is_symlink() or not safe_name(path.name) for path in files):
        raise DeliveryError('release asset directory must contain only regular named files')
    hashes = {path.name: digest(path) for path in files}
    marker = dict(version=1, kind=kind, tag=tag, sha=sha, run_id=run_id,
                  run_attempt=run_attempt, state='uploading', assets=hashes)
    if kind == 'mobile': marker['build_number'] = build_number
    verify_payload(directory, marker, api.repo, updater_config)
    if prior and prior['assets'] and prior['assets'] != hashes:
        raise DeliveryError('rebuilt assets conflict with the reserved delivery byte set')
    if generate_notes and not release:
        generated = api.request(f'{api.base}/releases/generate-notes', method='POST',
                                data={'tag_name': tag, 'target_commitish': sha})
        notes = notes.rstrip() + '\n\n' + generated['body']
    body = with_marker(notes, marker)
    release_id = release['id'] if release else None
    if release:
        # Verify existing bytes before making even a provenance update.
        existing = check_partial(api, release, hashes)
        release = api.request(f'{api.base}/releases/{release["id"]}', method='PATCH', data={
            'tag_name': tag, 'target_commitish': sha, 'body': body, 'draft': True,
        })
        check_response_identity(release, tag, sha, marker, release_id, draft=True)
    else:
        try:
            release = api.request(f'{api.base}/releases', method='POST', data={
                'tag_name': tag, 'target_commitish': sha, 'draft': True,
                'prerelease': prerelease, 'name': f'Headstate {tag}', 'body': body,
            })
        except (DeliveryError, subprocess.TimeoutExpired):
            # Creation may have committed before the response disappeared.
            # Read the reservation; never issue a competing second create.
            release = api.release(tag)
            if not release:
                raise DeliveryError('draft creation could not be verified')
            if not release.get('draft'):
                verify_completed(api, release, tag, sha, kind, updater_config)
                return 'noop'
            recovered = parse_marker(release.get('body'))
            if recovered != marker:
                raise DeliveryError('another delivery owns the draft created concurrently')
        check_response_identity(release, tag, sha, marker, release_id, draft=True)
        existing = check_partial(api, release, hashes)
    release_id = release['id']
    for path in files:
        if path.name in existing: continue
        failure = 'upload returned success but the asset is absent from the draft'
        for attempt in range(3):
            try:
                api.upload(tag, path)
            except subprocess.TimeoutExpired:
                failure = 'upload timed out; its response may have been lost'
            except DeliveryError as error:
                failure = str(error)  # Transport reports only sanitized status/category.
            else:
                failure = 'upload returned success but the asset is absent from the draft'
            existing = check_partial(api, release, hashes, only={path.name})
            if path.name in existing: break
            if attempt < 2: time.sleep(5 * (attempt + 1))
        else:
            raise DeliveryError(f'could not verify upload after three attempts: {path.name}; '
                                f'draft {release_id}: {failure}. Reconcile this draft before retrying.')
    if existing != set(hashes):
        raise DeliveryError('not every expected asset arrived')
    # Fresh downloaded bytes must satisfy the same signature/manifest contract.
    with tempfile.TemporaryDirectory() as work:
        assets = api.assets(release['id'])
        if len(assets) != len(hashes) or {asset['name'] for asset in assets} != set(hashes):
            raise DeliveryError('final remote asset set changed before publication')
        for asset in assets:
            if digest(api.download(asset, work)) != hashes[asset['name']]:
                raise DeliveryError('final downloaded bytes changed before publication')
        verify_payload(work, marker, api.repo, updater_config)
    admit()
    marker['state'] = 'complete'
    try:
        release = api.request(f'{api.base}/releases/{release["id"]}', method='PATCH',
                              data={'tag_name': tag, 'target_commitish': sha,
                                    'body': with_marker(notes, marker), 'draft': False})
    except (DeliveryError, subprocess.TimeoutExpired):
        release = api.release(tag)
        if not release: raise DeliveryError('publication response was lost and completion cannot be verified')
    check_response_identity(release, tag, sha, marker, release_id, draft=False)
    verify_completed(api, release, tag, sha, kind, updater_config)
    return 'published'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ['repo', 'tag', 'sha', 'kind']: parser.add_argument('--' + flag, required=True)
    parser.add_argument('--run-id', type=int, default=int(os.environ.get('GITHUB_RUN_ID', '0')))
    parser.add_argument('--run-attempt', type=int, default=int(os.environ.get('GITHUB_RUN_ATTEMPT', '1')))
    parser.add_argument('--assets-dir')
    parser.add_argument('--notes-file')
    parser.add_argument('--build-number', type=int)
    parser.add_argument('--updater-config')
    parser.add_argument('--prerelease', action='store_true')
    parser.add_argument('--generate-notes', action='store_true')
    parser.add_argument('--verify-only', action='store_true')
    args = parser.parse_args()
    if args.kind not in {'desktop', 'mobile'}: parser.error('kind must be desktop or mobile')
    api = GitHub(args.repo)
    if args.verify_only:
        release = api.release(args.tag)
        if not release: raise DeliveryError('no completed release exists')
        print(json.dumps(verify_completed(api, release, args.tag, args.sha, args.kind, args.updater_config), sort_keys=True))
        return
    if not args.assets_dir or not args.notes_file or args.run_id < 1:
        parser.error('publication needs assets-dir, notes-file and positive run-id')
    admission = lambda: None
    if args.kind == 'desktop':
        spec = importlib.util.spec_from_file_location('release_ci', Path(__file__).with_name('wait-release-ci.py'))
        gate = importlib.util.module_from_spec(spec); spec.loader.exec_module(gate)
        admission = lambda: gate.wait(api, args.sha, args.tag, 600)
        admission()  # Check again before any GitHub release side effect.
    result = publish(api, args.tag, args.sha, args.run_id, args.run_attempt, args.kind,
                     args.assets_dir, Path(args.notes_file).read_text(), args.prerelease,
                     args.build_number, args.updater_config, admission, args.generate_notes)
    print('Verified existing delivery; no assets or store submissions changed.' if result == 'noop'
          else 'Published the verified, immutable delivery.')

if __name__ == '__main__':
    try: main()
    except (DeliveryError, ValueError, OSError, subprocess.SubprocessError) as error: sys.exit(str(error))
