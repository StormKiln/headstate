#!/usr/bin/env python3
"""Shared immutable release evidence and GitHub transport (#1779).

Two tag-triggered publishers formerly used --clobber against the same release.
A complete delivery now identifies one coherent byte set; its duplicate verifies
that set and does no writes. Partial or conflicting deliveries fail closed.
"""
import base64
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.parse import quote, unquote, urlparse

PREFIX = '<!-- headstate-delivery-v1:'

class DeliveryError(RuntimeError):
    pass

class ApiError(DeliveryError):
    def __init__(self, message, status=None):
        super().__init__(message)
        self.status = status


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise DeliveryError(f'duplicate JSON key: {key}')
        result[key] = value
    return result


def safe_name(name):
    return (isinstance(name, str) and 0 < len(name) < 256
            and name not in {'.', '..'} and '/' not in name and '\\' not in name
            and not any(ord(char) < 32 for char in name))


def parse_marker(body):
    body = body or ''
    if isinstance(body, str) and PREFIX not in body:
        return None
    if not isinstance(body, str) or body.count(PREFIX) != 1:
        raise DeliveryError('release must contain exactly one delivery provenance marker')
    match = re.search(re.escape(PREFIX) + r'(.*?) -->', body, re.S)
    if not match:
        raise DeliveryError('malformed delivery provenance marker')
    try:
        marker = json.loads(match.group(1), object_pairs_hook=unique)
    except (ValueError, TypeError) as error:
        raise DeliveryError('invalid delivery provenance JSON') from error
    if (not isinstance(marker, dict) or marker.get('version') != 1
            or marker.get('kind') not in {'desktop', 'mobile'}
            or marker.get('state') not in {'reserved', 'uploading', 'complete'}
            or not isinstance(marker.get('tag'), str)
            or not re.fullmatch(r'[0-9a-f]{40}', str(marker.get('sha', '')))
            or type(marker.get('run_id')) is not int or marker['run_id'] < 1
            or type(marker.get('run_attempt')) is not int or marker['run_attempt'] < 1
            or not isinstance(marker.get('assets'), dict)):
        raise DeliveryError('invalid delivery provenance fields')
    if marker['kind'] == 'mobile' and (type(marker.get('build_number')) is not int or marker['build_number'] < 1):
        raise DeliveryError('mobile provenance needs a positive build number')
    if any(not safe_name(name) or not isinstance(digest, str)
           or not re.fullmatch(r'[0-9a-f]{64}', digest)
           for name, digest in marker['assets'].items()):
        raise DeliveryError('invalid asset name or SHA-256 in provenance')
    return marker


def with_marker(notes, marker):
    notes = notes or ''
    if PREFIX in notes:
        parse_marker(notes)
        notes = re.sub(re.escape(PREFIX) + r'.*? -->', '', notes, flags=re.S).rstrip()
    result = notes.rstrip() + '\n\n' + PREFIX + json.dumps(marker, sort_keys=True, separators=(',', ':')) + ' -->\n'
    parse_marker(result)
    return result


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


class GitHub:
    def __init__(self, repo):
        if not re.fullmatch(r'[\w.-]+/[\w.-]+', repo):
            raise DeliveryError('expected owner/repository')
        self.repo = repo
        self.base = f'repos/{repo}'

    def request(self, endpoint, method='GET', data=None):
        args = ['gh', 'api', endpoint, '--method', method]
        if data is not None:
            args += ['--input', '-']
        result = subprocess.run(args, input=json.dumps(data) if data is not None else None,
                                text=True, capture_output=True, timeout=60)
        if result.returncode:
            found = re.search(r'HTTP (\d{3})', result.stderr)
            raise ApiError(f'GitHub request failed: {method} {endpoint}: {result.stderr.strip()}',
                           int(found.group(1)) if found else None)
        return json.loads(result.stdout, object_pairs_hook=unique) if result.stdout.strip() else None

    def pages(self, endpoint, key=None):
        separator = '&' if '?' in endpoint else '?'
        output = []
        for page in range(1, 101):
            response = self.request(f'{endpoint}{separator}per_page=100&page={page}')
            items = response[key] if key else response
            if not isinstance(items, list):
                raise DeliveryError('invalid paginated GitHub response')
            output.extend(items)
            if len(items) < 100:
                return output
        raise DeliveryError('GitHub pagination exceeds the bounded 100-page limit')

    def release(self, tag):
        try:
            return self.request(f'{self.base}/releases/tags/{quote(tag, safe="")}')
        except ApiError as error:
            if error.status == 404:
                # The tag endpoint may omit unpublished drafts. They still own
                # the delivery, and must never be mistaken for an unused tag.
                matches = [release for release in self.pages(f'{self.base}/releases')
                           if release.get('tag_name') == tag]
                if len(matches) > 1:
                    raise DeliveryError('multiple releases or drafts claim this tag')
                return matches[0] if matches else None
            raise

    def assets(self, release_id):
        return self.pages(f'{self.base}/releases/{release_id}/assets')

    def download(self, asset, directory):
        if not safe_name(asset['name']):
            raise DeliveryError('unsafe remote asset name')
        target = Path(directory) / asset['name']
        with target.open('wb') as output:
            result = subprocess.run(['gh', 'api', f'{self.base}/releases/assets/{asset["id"]}',
                                     '-H', 'Accept: application/octet-stream'], stdout=output,
                                    stderr=subprocess.PIPE, timeout=300)
        if result.returncode:
            raise DeliveryError('could not download a release asset for verification')
        return target

    def upload(self, tag, asset):
        # No --clobber. An ambiguous response is reconciled by reading bytes.
        result = subprocess.run(['gh', 'release', 'upload', tag, str(asset), '--repo', self.repo],
                                capture_output=True, text=True, timeout=300)
        if result.returncode:
            # Do not echo stderr: it can contain credentials, URLs or local paths.
            status = re.search(r'\bHTTP ([1-5][0-9]{2})\b', result.stderr)
            category = (f'HTTP {status[1]}' if status else
                        'release not found' if 'release not found' in result.stderr.lower() else
                        'upload command failed')
            raise DeliveryError(f'release asset upload failed ({category}; exit {result.returncode})')


def check_identity(marker, tag, sha, kind):
    if not marker:
        raise DeliveryError('release has no delivery provenance')
    if (marker['tag'], marker['sha'], marker['kind']) != (tag, sha, kind):
        raise DeliveryError('existing release provenance belongs to another tag, commit or delivery kind')


def verify_payload(directory, marker, repo, updater_config=None):
    directory = Path(directory)
    names = set(marker['assets'])
    if marker['kind'] == 'mobile':
        ipas = [name for name in names if name.endswith('.ipa')]
        aabs = [name for name in names if name.endswith('.aab')]
        if len(ipas) != 1 or len(aabs) > 1 or names != {'SHA256SUMS', *ipas, *aabs}:
            raise DeliveryError('mobile delivery must contain one IPA, checksums and at most one AAB')
        checksums = {}
        for line in (directory / 'SHA256SUMS').read_text().splitlines():
            match = re.fullmatch(r'([0-9a-f]{64}) [ *](.+)', line)
            if not match or match[2] in checksums or not safe_name(match[2]):
                raise DeliveryError('invalid or duplicate mobile checksum entry')
            checksums[match[2]] = match[1]
        expected = {name: marker['assets'][name] for name in names - {'SHA256SUMS'}}
        if checksums != expected:
            raise DeliveryError('mobile checksums do not describe the complete artifact set')
        version = marker['tag'].removeprefix('mobile-v')
        prefix = f'Headstate-Companion-{version}-build{marker["build_number"]}'
        if any(name != prefix + Path(name).suffix for name in ipas + aabs):
            raise DeliveryError('mobile artifact version or build number disagrees with provenance')
        return
    if len(names) != 9 or 'latest.json' not in names or not updater_config:
        raise DeliveryError('desktop delivery requires nine assets and the updater public key')
    manifest = json.loads((directory / 'latest.json').read_text(), object_pairs_hook=unique)
    platforms = {'darwin-aarch64', 'darwin-x86_64', 'windows-x86_64', 'linux-x86_64'}
    if manifest.get('version') != marker['tag'].removeprefix('v') or set(manifest.get('platforms', {})) != platforms:
        raise DeliveryError('updater manifest has the wrong version or platform set')
    expected = {
        'darwin-aarch64': f'Headstate-{marker["tag"]}.app.tar.gz',
        'darwin-x86_64': f'Headstate-{marker["tag"]}.app.tar.gz',
    }
    for platform, suffix in [('windows-x86_64', '_x64-setup.exe'), ('linux-x86_64', '_amd64.AppImage')]:
        matches = [name for name in names if name.endswith(suffix)]
        if len(matches) != 1:
            raise DeliveryError('ambiguous updater bundle')
        if matches[0] != f'Headstate_{marker["tag"].removeprefix("v")}{suffix}':
            raise DeliveryError('updater bundle filename has the wrong version')
        expected[platform] = matches[0]
    bundles = set(expected.values())
    remainder = names - bundles - {name + '.sig' for name in bundles} - {'latest.json'}
    if len(remainder) != 2 or sum(name.endswith('.dmg') for name in remainder) != 1 or sum(name.endswith('.deb') for name in remainder) != 1:
        raise DeliveryError('desktop installer asset set is incomplete or contains extras')
    version = marker['tag'].removeprefix('v')
    if remainder != {f'Headstate_{version}_universal.dmg', f'Headstate_{version}_amd64.deb'}:
        raise DeliveryError('desktop installer filename has the wrong version')
    config = json.loads(Path(updater_config).read_text())
    with tempfile.TemporaryDirectory() as work:
        key = Path(work) / 'updater.pub'
        key.write_bytes(base64.b64decode(config['plugins']['updater']['pubkey'], validate=True))
        for platform, entry in manifest['platforms'].items():
            name = expected[platform]
            url = urlparse(entry['url'])
            if (url.scheme != 'https' or url.netloc != 'github.com' or url.query or url.fragment
                    or unquote(url.path) != f'/{repo}/releases/download/{marker["tag"]}/{name}'):
                raise DeliveryError('updater URL does not identify this release asset')
            signature = (directory / (name + '.sig')).read_text().strip()
            if signature != entry['signature'].strip():
                raise DeliveryError('manifest and detached updater signatures differ')
            sig = Path(work) / 'bundle.minisig'
            sig.write_bytes(base64.b64decode(signature, validate=True))
            subprocess.run(['minisign', '-V', '-q', '-m', str(directory / name), '-p', str(key), '-x', str(sig)],
                           check=True, timeout=60)


def verify_completed(api, release, tag, sha, kind, updater_config=None):
    marker = parse_marker(release.get('body'))
    check_identity(marker, tag, sha, kind)
    if release.get('draft') or release.get('tag_name') != tag or marker['state'] != 'complete':
        raise DeliveryError('existing delivery is not a verified completed release')
    assets = api.assets(release['id'])
    names = [asset['name'] for asset in assets]
    if len(set(names)) != len(names) or set(names) != set(marker['assets']):
        raise DeliveryError('remote release asset set differs from delivery provenance')
    with tempfile.TemporaryDirectory() as work:
        for asset in assets:
            path = api.download(asset, work)
            if digest(path) != marker['assets'][asset['name']]:
                raise DeliveryError('remote asset bytes differ from delivery provenance')
        verify_payload(work, marker, api.repo, updater_config)
    return marker
