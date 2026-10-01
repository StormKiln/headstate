#!/usr/bin/env python3
"""Moving install policy behind diagnostics must not change its failure semantics."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('install-dependencies.sh')


class InstallPolicy(unittest.TestCase):
    def test_retry_and_dependency_failure(self):
        for mode, expected_count, expected_exit in (('dependency', 1, 1), ('recover', 3, 0), ('network', 5, 1)):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                yarn = root / 'yarn'
                yarn.write_text('''#!/bin/bash
n=0
[[ -f "$COUNT_FILE" ]] && n=$(cat "$COUNT_FILE")
n=$((n+1))
printf '%s' "$n" > "$COUNT_FILE"
[[ "$*" == "install --immutable" ]] || exit 99
if [[ "$INSTALL_MODE" == dependency ]]; then echo YN0028; exit 1; fi
if [[ "$INSTALL_MODE" == recover && "$n" -eq 3 ]]; then exit 0; fi
echo 'synthetic transport failure'
exit 7
''')
                yarn.chmod(0o755)
                sleep = root / 'sleep'
                sleep.write_text('#!/bin/bash\nexit 0\n')
                sleep.chmod(0o755)
                result = subprocess.run(['bash', str(SCRIPT)], env={**os.environ, 'PATH': f'{tmp}:{os.environ["PATH"]}',
                                        'COUNT_FILE': str(root / 'count'), 'INSTALL_MODE': mode},
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, expected_exit, result.stderr)
                self.assertEqual(int((root / 'count').read_text()), expected_count)


if __name__ == '__main__':
    unittest.main()
