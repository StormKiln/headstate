#!/usr/bin/env python3
"""CI placement changes must not lose platform checks or required contexts."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('contexts', ROOT / 'scripts/check-required-contexts.py')
contexts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contexts)

class Throughput(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = (ROOT / '.github/workflows/ci.yml').read_text()
        cls.jobs = {k:'\n'.join(v) for k,v in contexts.jobs_of(cls.text).items()}

    def test_all_required_contexts_remain(self):
        self.assertTrue(set(contexts.REQUIRED_CONTEXTS) <= contexts.reported_contexts(self.text))

    def test_lint_aggregate_always_requires_both_halves(self):
        job = self.jobs['lint']
        for command in ['if: always()', 'needs: [lint-static, lint-native]',
                        'NEEDS_JSON: ${{ toJSON(needs) }}', 'python3 scripts/ci-maintenance.py --gate']:
            self.assertIn(command, job)
        self.assertNotIn('continue-on-error', job)

    def test_four_mac_jobs_keep_native_validation(self):
        mac = {k for k,v in self.jobs.items() if 'runs-on: macos-' in v}
        self.assertEqual(mac, {'lint-native','test-rust','build','mobile-ios'})
        for command in ['cargo fmt --check', 'cargo clippy --all-targets -- -D warnings']:
            self.assertIn(command, self.jobs['lint-native'])
        self.assertIn('python3 scripts/test-rust-ci.py race', self.jobs['test-rust'])
        self.assertIn('range(1, 4)', (ROOT / 'scripts/test-rust-ci.py').read_text())
        for command in ['yarn tauri build --bundles app', 'yarn tauri bundle --bundles dmg --no-sign',
                        'cargo check --target x86_64-apple-darwin --all-targets']:
            self.assertIn(command,self.jobs['build'])
        for command in ['make check-native-ios','make check-mobile-ios','make mobile-frontend']:
            self.assertIn(command,self.jobs['mobile-ios'])

    def test_portable_checks_run_in_full_on_linux(self):
        for name in ['lint-static','test-frontend','supply-chain']:
            self.assertIn('runs-on: ubuntu-latest',self.jobs[name])
        for command in ['yarn tsc -b --force','yarn eslint .','yarn knip','make lint-workflows',
                        'make check-wire-contract','python3 scripts/check-mobile-gate.py',
                        'python3 scripts/check-required-contexts.py']:
            self.assertIn(command,self.jobs['lint-static'])
        for command in ['bash scripts/test-frontend-ci.sh','make check-contrast',
                        'yarn playwright install --with-deps chromium']:
            self.assertIn(command,self.jobs['test-frontend'])
        for command in ['make deny','cargo deny check','yarn npm audit']:
            self.assertIn(command,self.jobs['supply-chain'])

    def test_setup_defaults_preserve_existing_consumers(self):
        setup = (ROOT / '.github/actions/setup/action.yml').read_text()
        self.assertIn("  node:\n    description: Install pinned Node and immutable Yarn dependencies\n    default: 'true'",setup)
        self.assertIn("  rust:\n    description: Install pinned Rust and restore its cache\n    default: 'true'",setup)
        self.assertIn("rust: 'false'",self.jobs['test-frontend'])
        self.assertIn("node: 'false'",self.jobs['lint-native'])
        self.assertIn("save-if: ${{ github.ref == 'refs/heads/main' }}",setup)

    def test_new_cache_classes_have_bounds(self):
        spec = importlib.util.spec_from_file_location('cache_budget', ROOT / 'scripts/check-cache-budget.py')
        cache = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cache)
        self.assertEqual(cache.CLASS_BUDGET_GIB['v0-rust-lint-native-Darwin-arm64'], 0.5)
        self.assertEqual(cache.CLASS_BUDGET_GIB['v0-rust-supply-chain-Linux-x64'], 0.2)

    def test_cleanup_never_executes_untrusted_workflow_source(self):
        workflow=(ROOT / '.github/workflows/ci-maintenance.yml').read_text()
        self.assertIn('ref: ${{ github.event.repository.default_branch }}',workflow)
        self.assertNotIn('pull_request_target:',workflow)
        self.assertNotIn('head_sha',workflow)
        self.assertNotIn('download-artifact',workflow)
        self.assertIn('persist-credentials: false',workflow)
        self.assertIn('actions: write',workflow)
        self.assertIn('python3 scripts/ci-maintenance.test.py',workflow)
        self.assertIn('python3 scripts/ci-maintenance.py --apply',workflow)

if __name__ == '__main__': unittest.main()
