"""Qualification tests: unavailable/occluded/native proxies never become acceptance."""
import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location('native_bench', pathlib.Path(__file__).with_name('transcript-native-bench.py'))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)

class Qualification(unittest.TestCase):
    def test_requires_every_phase_and_verified_distinct_processes(self):
        good = {'status': 'measured-proxies', 'samples': [
            {'phase': phase, 'visible': True, 'hostPID': 10, 'webContentPID': 11,
             'hostResidentBytes': 100, 'hostPhysicalFootprintBytes': 90,
             'webContentResidentBytes': 200, 'webContentPhysicalFootprintBytes': 180}
            for phase in ['baseline', 'open', 'append', 'idle']]}
        self.assertTrue(bench.qualified(good))
        for field, value in [('visible', False), ('webContentPID', 10), ('webContentResidentBytes', None)]:
            broken = {**good, 'samples': [dict(s) for s in good['samples']]}
            broken['samples'][1][field] = value
            self.assertFalse(bench.qualified(broken))
        changed = {**good, 'samples': [dict(s) for s in good['samples']]}
        changed['samples'][1]['webContentPID'] = 12
        self.assertFalse(bench.qualified(changed))
        self.assertFalse(bench.qualified({**good, 'samples': good['samples'][:-1]}))
        self.assertFalse(bench.qualified({'status': 'unavailable', 'samples': []}))
        self.assertFalse(bench.qualified({**good, 'status': 'passed'}))

if __name__ == '__main__':
    unittest.main()
