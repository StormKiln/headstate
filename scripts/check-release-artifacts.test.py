#!/usr/bin/env python3
"""Release publishers must not publish setup diagnostics (#1621).

The shared setup action now retains diagnostic artifacts in release runs too.
Downloading every artifact merged their log/JSON files into the public assets.
These regressions read the real publishers and exercise their download selection
against mixed bundle/diagnostic names. The simple prefix globs used here have
the same semantics in fnmatch and the pinned action's glob matcher; this is not
a replacement implementation of arbitrary Actions expressions or glob syntax.
"""
import fnmatch
import pathlib
import re
import runpy
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
PARSER = runpy.run_path(str(ROOT / 'scripts/check-workflow-shells.py'))


def selected(text, artifacts):
    jobs = dict(PARSER['jobs_of'](text))
    if 'publish' not in jobs:
        raise ValueError('missing publish job')
    downloads = [step for step in PARSER['steps_of'](jobs['publish'])
                 if any(re.match(r'^(?:      - |        )uses: actions/download-artifact@', line)
                        for line in step)]
    if len(downloads) != 1:
        raise ValueError('expected one publish artifact download')
    inputs = {}
    inside = False
    for line in downloads[0]:
        if re.match(r'^        with:\s*$', line):
            inside = True
        elif inside:
            if line.strip() and not line.startswith('          '):
                break
            match = re.match(r'^          ([\w-]+):\s*(.+)$', line)
            if match:
                inputs[match[1]] = match[2].strip().strip('\"\'')
    if 'artifact-ids' in inputs:
        raise ValueError('artifact IDs need an explicit selection regression')
    # The pinned action documents that name takes precedence over pattern.
    if 'name' in inputs:
        return {name for name in artifacts if name == inputs['name']}
    pattern = inputs.get('pattern', '*')
    return {name for name in artifacts if fnmatch.fnmatchcase(name, pattern)}


def workflow(inputs=''):
    return ('jobs:\n  publish:\n    steps:\n'
            '      - uses: actions/download-artifact@pinned\n'
            '        with:\n          path: release-assets\n' + inputs)


class ReleaseArtifacts(unittest.TestCase):
    def test_desktop_publisher_selects_only_platform_bundles(self):
        bundles = {'bundle-macos-latest', 'bundle-ubuntu-latest', 'bundle-windows-latest'}
        diagnostics = {f'setup-diagnostics-build-{os}' for os in ['macOS', 'Linux', 'Windows']}
        text = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertEqual(selected(text, bundles | diagnostics | {'unrelated-artifact'}), bundles)

    def test_mobile_publisher_selects_only_mobile_bundles(self):
        bundles = {'mobile-ios', 'mobile-android'}
        diagnostics = {'setup-diagnostics-ios-macOS', 'setup-diagnostics-android-Linux'}
        text = (ROOT / '.github/workflows/mobile-release.yml').read_text()
        self.assertEqual(selected(text, bundles | diagnostics | {'unrelated-artifact'}), bundles)

    def test_unfiltered_download_reproduces_diagnostic_contamination(self):
        artifacts = {'bundle-macos-latest', 'setup-diagnostics-build-macOS'}
        self.assertEqual(selected(workflow(), artifacts), artifacts)

    def test_prefix_filter_accepts_bundles_but_not_diagnostics(self):
        artifacts = {'bundle-macos-latest', 'setup-diagnostics-build-macOS'}
        text = workflow('          pattern: "bundle-*" # selected bundles\n')
        self.assertEqual(selected(text.replace('\n', '\r\n'), artifacts), {'bundle-macos-latest'})

    def test_comments_and_other_step_inputs_cannot_supply_the_filter(self):
        artifacts = {'bundle-macos-latest', 'setup-diagnostics-build-macOS'}
        text = workflow('          # pattern: bundle-*\n'
                        '      - uses: unrelated/action@pinned\n'
                        '        with:\n          pattern: bundle-*\n')
        self.assertEqual(selected(text, artifacts), artifacts)

    def test_name_overrides_pattern(self):
        artifacts = {'bundle-macos-latest', 'setup-diagnostics-build-macOS'}
        text = workflow('          name: setup-diagnostics-build-macOS\n'
                        '          pattern: bundle-*\n')
        self.assertEqual(selected(text, artifacts), {'setup-diagnostics-build-macOS'})

    def test_run_body_prose_is_not_an_artifact_download(self):
        text = workflow('          pattern: bundle-*\n'
                        '      - run: |\n'
                        '          cat <<\'YAML\'\n'
                        '          uses: actions/download-artifact@documented\n'
                        '          YAML\n')
        self.assertEqual(selected(text, {'bundle-macos-latest'}), {'bundle-macos-latest'})

    def test_missing_or_ambiguous_download_cannot_pass_vacuously(self):
        for text in ['jobs:\n  build:\n    steps: []\n',
                     workflow() + '      - uses: actions/download-artifact@other\n']:
            with self.subTest(text=text), self.assertRaises(ValueError):
                selected(text, {'bundle-macos-latest'})


if __name__ == '__main__':
    unittest.main()
