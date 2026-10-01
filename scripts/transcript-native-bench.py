#!/usr/bin/env python3
"""Bounded own-process WKWebView proxies. Never certifies native budgets."""
import argparse
import functools
import http.server
import json
import pathlib
import platform
import subprocess
import sys
import threading
import urllib.parse

PHASES = ['baseline', 'open', 'append', 'idle']
MEMORY = ['hostResidentBytes', 'hostPhysicalFootprintBytes',
          'webContentResidentBytes', 'webContentPhysicalFootprintBytes']


def qualified(result):
    samples = result.get('samples', [])
    if result.get('status') != 'measured-proxies' or [s.get('phase') for s in samples] != PHASES:
        return False
    identities = set()
    for sample in samples:
        host, renderer = sample.get('hostPID'), sample.get('webContentPID')
        if not isinstance(host, int) or not isinstance(renderer, int) or min(host, renderer) <= 0 or host == renderer:
            return False
        identities.add((host, renderer))
        if sample.get('visible') is not True or any(not isinstance(sample.get(k), (int, float)) or sample[k] <= 0 for k in MEMORY):
            return False
    return len(identities) == 1


class SyntheticServer(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, fixtures, **kwargs):
        self.fixtures = fixtures
        super().__init__(*args, **kwargs)

    def translate_path(self, path):
        path = urllib.parse.unquote(urllib.parse.urlsplit(path).path)
        root = self.fixtures if path.startswith('/fixtures/') else pathlib.Path(self.directory)
        relative = path.removeprefix('/fixtures/') if path.startswith('/fixtures/') else path.lstrip('/')
        candidate = (root / relative).resolve()
        if not candidate.is_relative_to(root.resolve()):
            return str(root / '__refused__')
        return str(candidate)

    def log_message(self, *_args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('fixtures', type=pathlib.Path)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--fixture', choices=['messages-1k', 'messages-10k', 'tool-heavy-70mb', 'huge-result-5mb'], default='messages-1k')
    args = parser.parse_args()
    if platform.system() != 'Darwin':
        parser.error('requires macOS; no native measurement taken')
    dist = pathlib.Path('dist-harness').resolve()
    payload = args.fixtures / f'{args.fixture}.window-end.json'
    if not payload.is_file() or not (dist / 'harness/transcript.html').is_file():
        parser.error('run bench-transcript-browser first with persistent BENCH_TRANSCRIPT_OUT')
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    result_path = out / 'result.json'
    if result_path.exists():
        parser.error('choose a new output directory; existing measurements are preserved')
    source = pathlib.Path(__file__).resolve().parent / 'transcript-native'
    with (out / 'build.log').open('w') as log:
        subprocess.run(['xcrun', 'clang', '-c', str(source / 'usage.c'), '-o', str(out / 'usage.o')], stdout=log, stderr=subprocess.STDOUT, check=True, timeout=120)
        subprocess.run(['xcrun', 'swiftc', str(source / 'Runner.swift'), str(out / 'usage.o'), '-o', str(out / 'transcript-native')], stdout=log, stderr=subprocess.STDOUT, check=True, timeout=120)
    handler = functools.partial(SyntheticServer, directory=str(dist), fixtures=args.fixtures.resolve())
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f'http://127.0.0.1:{server.server_port}/harness/transcript.html?fixture={args.fixture}.window-end&mode=follow'
    try:
        with (out / 'runner.log').open('w') as log:
            try:
                process = subprocess.run([str(out / 'transcript-native'), url, str(result_path)], stdout=log, stderr=subprocess.STDOUT, timeout=25)
                code = process.returncode
            except subprocess.TimeoutExpired:
                # subprocess kills only the owned host, never global WebKit apps.
                code = 3
        result = json.loads(result_path.read_text()) if result_path.exists() else {
            'status': 'unavailable', 'reason': 'Owned runner timed out or failed before producing a result', 'samples': []}
        complete = code == 0 and qualified(result)
        if not complete and result.get('status') != 'unavailable':
            result.update(status='unavailable', reason='Incomplete, invalid or unstable own-process samples; no qualified native measurement')
        result.update({'fixture': args.fixture, 'system': platform.platform(), 'viewport': [1280, 800],
                       'nativeBudgetAcceptance': 'unmeasured'})
        result_path.write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result, indent=2))
        return 0 if complete else 3
    finally:
        server.shutdown()
        server.server_close()


if __name__ == '__main__':
    sys.exit(main())
