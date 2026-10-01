import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
spec=importlib.util.spec_from_file_location('native',Path(__file__).with_name('check-native.py'))
native=importlib.util.module_from_spec(spec);spec.loader.exec_module(native)
class NativeTests(unittest.TestCase):
    def tree(self, root):
        (root/'src-mobile/plugins/p/android/src/main/java').mkdir(parents=True)
        (root/'src-mobile/Cargo.toml').write_text('[workspace]\nmembers=["plugins/p"]\n')
        (root/'src-mobile/plugins/p/build.rs').write_text('fn main() { b.android_path("android"); }')
        (root/'src-mobile/plugins/p/android/build.gradle.kts').write_text('plugins {}')
        (root/'src-mobile/plugins/p/android/src/main/java/P.kt').write_text('class P')
    def test_inventory_requires_every_declared_module_and_source(self):
        with tempfile.TemporaryDirectory(prefix='native path ') as d:
            root=Path(d);self.tree(root)
            self.assertEqual([p.name for p in native.inventory(root,'android')],['p'])
            (root/'src-mobile/plugins/p/android/build.gradle.kts').unlink()
            with self.assertRaises(ValueError): native.inventory(root,'android')
            with self.assertRaises(ValueError): native.inventory(root,'ios')
    def test_no_empty_or_omitted_compiler_task(self):
        native.check_android_output('> Task :p:compileDebugKotlin\n',['p'])
        for text in ['', '> Task :p:compileDebugKotlin NO-SOURCE', '> Task :other:compileDebugKotlin']:
            with self.assertRaises(ValueError): native.check_android_output(text,['p'])
    def test_compiler_failure_propagates(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(subprocess.CalledProcessError): native.run(['python3','-c','raise SystemExit(17)'],Path(d))
    def test_metadata_must_resolve_one_real_native_api(self):
        for packages in [[], [{'name':'tauri','manifest_path':'/missing/Cargo.toml','version':'2'}], [{'name':'tauri'}]*2]:
            with self.assertRaises(ValueError): native.tauri_source(Path('/unused'),lambda *a:json.dumps({'packages':packages}))
    def test_ci_wiring_and_missing_step_or_condition(self):
        config=(native.ROOT/'.github/workflows/ci.yml').read_text()
        native.check_ci(config)
        for platform in ['android','ios']:
            bad=config.replace(f'run: make check-native-{platform}',f'run: echo skipped-{platform}')
            with self.assertRaises(ValueError): native.check_ci(bad)
            bad=config.replace(f"if: needs.mobile-changes.outputs.run == 'true'\n        run: make check-native-{platform}",f"if: false\n        run: make check-native-{platform}")
            with self.assertRaises(ValueError): native.check_ci(bad)
    def test_swift_second_plugin_failure_missing_scheme_and_lock_drift(self):
        with tempfile.TemporaryDirectory(prefix='swift path ') as d:
            root=Path(d);(root/'scripts/native/ios').mkdir(parents=True)
            (root/'scripts/native/ios/Package.resolved').write_text('{"version":1}')
            tauri=root/'tauri';(tauri/'mobile/ios-api').mkdir(parents=True)
            modules=[]
            for name in ['one','two']:
                module=root/name;(module/'ios/Sources').mkdir(parents=True)
                (module/'ios/Package.swift').write_text('package')
                modules.append(module)
            for mode in ['second','scheme','lock','ok']:
                work=root/mode;seen=[]
                def runner(args,cwd):
                    if '-list' in args:
                        return json.dumps({'workspace':{'schemes':[] if mode=='scheme' else ['tauri-plugin-'+cwd.parent.name]}})
                    if 'build' in args:
                        seen.append(cwd.parent.name)
                        if mode=='second' and cwd.parent.name=='two': raise subprocess.CalledProcessError(3,args)
                        if mode=='lock': (cwd/'Package.resolved').write_text('{"version":2}')
                    return ''
                if mode=='ok': native.compile_ios(root,work,modules,tauri,runner)
                else:
                    with self.assertRaises((ValueError,subprocess.CalledProcessError)): native.compile_ios(root,work,modules,tauri,runner)
                if mode in ['second','ok']: self.assertEqual(seen,['one','two'])
    def test_host_provider_policies_must_both_survive(self):
        with tempfile.TemporaryDirectory() as d:
            path=Path(d)/'AndroidManifest.xml'
            source=(native.ROOT/'scripts/native/android/export-host/src/main/AndroidManifest.xml').read_text().replace('${applicationId}','com.headstate.nativecheck')
            with self.assertRaises(ValueError):
                path.write_text(source);native.check_providers(path)
            plugin=(native.ROOT/'src-mobile/plugins/headstate-export/android/src/main/AndroidManifest.xml').read_text().replace('${applicationId}','com.headstate.nativecheck')
            provider=plugin[plugin.index('<provider'):plugin.index('</provider>')+len('</provider>')]
            source=source.replace('</application>',provider+'</application>')
            path.write_text(source);native.check_providers(path)
            path.write_text(source.replace('@xml/headstate_export_paths','@xml/file_paths'))
            with self.assertRaises(ValueError): native.check_providers(path)
if __name__=='__main__': unittest.main()
