#!/usr/bin/env python3
"""Reserve mobile store delivery before upload; uncertainty never permits resubmission.

Signed workflow runs serialize globally. Draft reservations are durable consumed
build numbers, including runs that fail before the store replies. Recovery from
an incomplete reservation requires human reconciliation, never an automatic retry.
"""
import argparse
import json
import os
from pathlib import Path
import re
import sys

BUILD = re.compile(r'-build(\d+)\.(?:ipa|aab)$')


def marker_of(release):
    from release_delivery import parse_marker
    return parse_marker(release.get('body') or '')


def mobile_inventory(releases):
    """Identify consumed builds by provenance, even if GitHub renamed a draft.

    A body-only PATCH orphaned build46 after store acceptance (#1785). Its
    logical tag still owns that submission; a new version may advance the floor,
    but neither a renamed draft nor conflicting claims permit resubmission.
    """
    inventory = []
    claimed = set()
    for item in releases:
        marker = marker_of(item)
        api_tag = str(item.get('tag_name', ''))
        if marker and marker['kind'] == 'mobile':
            logical_tag = marker['tag']
            if (not re.fullmatch(r'mobile-v\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?', logical_tag)
                    or (api_tag.startswith('mobile-v') and api_tag != logical_tag)):
                raise ValueError('A mobile release has conflicting reservation tag identity.')
        elif api_tag.startswith('mobile-v'):
            if marker:
                raise ValueError('A mobile release has invalid delivery provenance.')
            logical_tag = api_tag
        else:
            continue
        if logical_tag in claimed:
            raise ValueError('Multiple releases claim the same mobile provenance tag.')
        claimed.add(logical_tag)
        inventory.append((item, marker, logical_tag))
    return inventory


def reserve(*, releases, tag, sha, run_id, run_number, mark, verify, create, run_attempt=1):
    inventory = mobile_inventory(releases)
    existing = [(item, marker) for item, marker, logical_tag in inventory if logical_tag == tag]
    if existing:
        item, marker = existing[0]
        if (item.get('tag_name') != tag or item.get('draft') or not marker or marker.get('state') != 'complete'
                or marker.get('kind') != 'mobile' or marker.get('tag') != tag
                or marker.get('sha') != sha):
            raise ValueError('Prior mobile delivery is incomplete or ambiguous; reconcile it before another store submission.')
        verify(item)
        return 'noop', marker['build_number']

    highest = mark
    for item, marker, _ in inventory:
        if marker:
            build = marker.get('build_number')
            if type(build) is not int or build <= 0:
                raise ValueError('A mobile release has an invalid build reservation.')
            highest = max(highest, build)
        elif item.get('draft'):
            raise ValueError('An older mobile draft has no trustworthy build reservation; reconcile it first.')
        for asset in item.get('assets', []):
            match = BUILD.search(asset.get('name', ''))
            if match:
                highest = max(highest, int(match[1]))
    build = max(run_number, highest + 1)
    if build > 2100000000:
        raise ValueError('Mobile build number exceeds the store limit.')
    marker = dict(version=1, kind='mobile', tag=tag, sha=sha, run_id=int(run_id),
                  run_attempt=int(run_attempt), state='reserved', assets={}, build_number=build)
    create(marker)  # Exactly once. An uncertain API result must remain a failure.
    return 'new', build


def check_owner(release, *, tag, sha, run_id, run_attempt, build):
    marker = marker_of(release)
    if (not release.get('draft') or not marker
            or any(marker.get(key) != value for key, value in {
                'kind': 'mobile', 'tag': tag, 'sha': sha, 'run_id': int(run_id),
                'run_attempt': int(run_attempt), 'build_number': int(build),
                'state': 'reserved', 'assets': {},
            }.items())):
        raise ValueError('This attempt does not own the reserved delivery; store resubmission is forbidden.')


def main():
    from release_delivery import GitHub, verify_completed, with_marker
    parser = argparse.ArgumentParser(description=__doc__)
    for field in ('repo', 'tag', 'sha', 'run-id', 'run-attempt'):
        parser.add_argument('--' + field, required=True)
    parser.add_argument('--run-number', type=int)
    parser.add_argument('--build-number', type=int)
    parser.add_argument('--mark-file', default='.github/mobile-build-high-water-mark')
    parser.add_argument('--check-owner', action='store_true')
    parser.add_argument('--reservation-json')
    args = parser.parse_args()
    if not re.fullmatch(r'mobile-v\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?', args.tag):
        raise ValueError('A signed mobile delivery requires a mobile-v version tag.')
    if not re.fullmatch(r'[0-9a-f]{40}', args.sha):
        raise ValueError('Delivery source must be a full commit SHA.')
    if args.check_owner:
        # Dependency outputs are emitted only after the write-capable reserve
        # job creates the durable claim. Build jobs need no draft read/write
        # permission. A rerun's incremented attempt cannot reuse this receipt.
        if not args.reservation_json:
            raise ValueError('Mobile delivery reservation receipt is missing.')
        marker = json.loads(args.reservation_json)
        item = {'draft': True, 'body': with_marker('', marker)}
        check_owner(item, tag=args.tag, sha=args.sha, run_id=args.run_id,
                    run_attempt=args.run_attempt, build=args.build_number)
        return
    api = GitHub(args.repo)
    mark_text = ''.join(line.strip() for line in Path(args.mark_file).read_text().splitlines()
                        if not line.startswith('#'))
    if not mark_text.isdigit() or not args.run_number or args.run_number <= 0:
        raise ValueError('Invalid committed high-water mark or workflow run number.')
    releases = list(api.pages(f'repos/{args.repo}/releases'))
    # Explicit asset pagination avoids truncating the high-water observation.
    for item, _, _ in mobile_inventory(releases):
        item['assets'] = list(api.assets(item['id']))
    reserved_marker = None
    def create(marker):
        nonlocal reserved_marker
        notes = ('Mobile delivery reserved before store upload. An incomplete delivery '
                 'must be reconciled before retrying.\n')
        created = api.request(f'repos/{args.repo}/releases', method='POST', data={
            'tag_name': args.tag, 'target_commitish': args.sha, 'draft': True,
            'prerelease': True, 'name': f'Headstate Companion {args.tag.removeprefix("mobile-v")}',
            'body': with_marker(notes, marker),
        })
        if (not isinstance(created, dict)
                or type(created.get('id')) is not int or created['id'] <= 0
                or created.get('tag_name') != args.tag
                or created.get('target_commitish') != args.sha
                or created.get('draft') is not True or marker_of(created) != marker):
            raise ValueError('Created mobile reservation identity is uncertain; reconcile it before store submission.')
        reserved_marker = marker
    disposition, build = reserve(
        releases=releases, tag=args.tag, sha=args.sha, run_id=args.run_id,
        run_attempt=args.run_attempt, run_number=args.run_number, mark=int(mark_text),
        verify=lambda item: verify_completed(api, item, args.tag, args.sha, 'mobile'),
        create=create,
    )
    print(f'Mobile delivery: {disposition}; build {build}')
    if output := os.environ.get('GITHUB_OUTPUT'):
        with open(output, 'a') as handle:
            handle.write(f'delivery={disposition}\nbuild_number={build}\n')
            if reserved_marker is not None:
                handle.write('reservation=' + json.dumps(reserved_marker, separators=(',', ':')) + '\n')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, RuntimeError) as error:
        sys.exit(str(error))
