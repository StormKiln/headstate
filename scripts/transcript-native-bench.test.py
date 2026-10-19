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

class NumericAndShapeQualification(unittest.TestCase):
    def good(self):
        return {'status': 'measured-proxies', 'samples': [
            dict(phase=phase, visible=True, hostPID=10, webContentPID=11,
                 **{field: 100 for field in bench.MEMORY})
            for phase in bench.PHASES]}

    def test_memory_requires_positive_uint64_integer_bytes(self):
        for field in bench.MEMORY:
            for value in [True, False, float('nan'), float('inf'), -float('inf'),
                          1.0, 1.5, 0, -1, 2**64, '100', None, [], {}]:
                with self.subTest(field=field, value=value):
                    result = self.good(); result['samples'][1][field] = value
                    self.assertFalse(bench.qualified(result))
        result = self.good()
        for sample in result['samples']:
            for field in bench.MEMORY: sample[field] = 2**64 - 1
        self.assertTrue(bench.qualified(result))
        for sample in result['samples']:
            for field in bench.MEMORY: sample[field] = 1
        self.assertTrue(bench.qualified(result))

    def test_pid_requires_positive_signed_native_integer(self):
        for field in ['hostPID', 'webContentPID']:
            for value in [True, False, 1.0, float('nan'), float('inf'), 0, -1,
                          2**31, '100', None, [], {}]:
                with self.subTest(field=field, value=value):
                    result = self.good()
                    for sample in result['samples']: sample[field] = value
                    self.assertFalse(bench.qualified(result))
        result = self.good()
        for sample in result['samples']:
            sample['hostPID'] = 1; sample['webContentPID'] = 2**31 - 1
        self.assertTrue(bench.qualified(result))

    def test_malformed_shapes_return_false_without_exception(self):
        for result in [None, [], True, 'private', 1, {}, {'status': 'measured-proxies'},
                       *[dict(status='measured-proxies', samples=v)
                         for v in [None, True, 1, {}, 'private', [None], [1], [{}]]]]:
            with self.subTest(result=result): self.assertFalse(bench.qualified(result))
        for replacement in [None, [], True, 'private', 1]:
            result = self.good(); result['samples'][1] = replacement
            self.assertFalse(bench.qualified(result))
        for key in ['phase', 'visible', 'hostPID', 'webContentPID', *bench.MEMORY]:
            result = self.good(); del result['samples'][1][key]
            self.assertFalse(bench.qualified(result))

    def test_phase_order_and_identity_remain_required(self):
        result = self.good(); result['samples'].reverse()
        self.assertFalse(bench.qualified(result))
        result = self.good(); result['samples'][1] = dict(result['samples'][0])
        self.assertFalse(bench.qualified(result))
        result = self.good(); result['samples'].append(dict(result['samples'][-1]))
        self.assertFalse(bench.qualified(result))
        for field,value in [('visible', 1), ('hostPID', 12), ('webContentPID', 10)]:
            result = self.good(); result['samples'][1][field] = value
            self.assertFalse(bench.qualified(result))

if __name__ == '__main__':
    unittest.main()
