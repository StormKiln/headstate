#!/usr/bin/env python3
"""Compile actual mobile plugins: Rust checks previously missed broken native sources.
Stages full modules outside the checkout, resolves Tauri from the lock, and never
accepts an empty inventory. CI invokes compile mode, not discovery or dry-run.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import tomllib
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent

def inventory(root, platform):
    cargo = tomllib.loads((root / 'src-mobile/Cargo.toml').read_text())
    members = [root / 'src-mobile' / p for p in cargo['workspace']['members'] if p.startswith('plugins/')]
    result = []
    for member in members:
        build = re.sub(r'//[^\n]*', '', (member / 'build.rs').read_text())
        if f'.{platform}_path(' not in build:
            continue
        module = member / platform
        manifest = module / ('Package.swift' if platform == 'ios' else 'build.gradle.kts')
        extension = '*.swift' if platform == 'ios' else '*.kt'
        source_root = module / ('Sources' if platform == 'ios' else 'src/main')
        if not manifest.is_file() or not list(source_root.rglob(extension)):
            raise ValueError(f'{member.name}: missing manifest or native sources')
        result.append(member)
    discovered = {p.parent.parent.name for p in (root/'src-mobile/plugins').glob(f'*/{platform}/' + ('Package.swift' if platform == 'ios' else 'build.gradle.kts'))}
    if not result or {p.name for p in result} != discovered:
        raise ValueError(f'{platform}: empty or undeclared native module inventory')
    return result

def run(args, cwd):
    print('+', ' '.join(map(str,args)), flush=True)
    result = subprocess.run(args, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if 'metadata' not in args: print(result.stdout, end='', flush=True)
    result.check_returncode()
    return result.stdout

def tauri_source(root, runner=run):
    data = json.loads(runner(['cargo','metadata','--locked','--format-version','1','--manifest-path',str(root/'src-mobile/Cargo.toml')], root))
    matches = [p for p in data['packages'] if p['name'] == 'tauri']
    if len(matches) != 1: raise ValueError('resolved Tauri is absent or ambiguous')
    path = Path(matches[0]['manifest_path']).parent
    if not (path/'mobile/android').is_dir() or not (path/'mobile/ios-api').is_dir():
        raise ValueError('resolved Tauri native API missing')
    print('Tauri', matches[0]['version'], path)
    return path

def copy(source, target):
    shutil.copytree(source,target,ignore=shutil.ignore_patterns('build','.build','.gradle','.swiftpm','.tauri','target','DerivedData'))

def check_android_output(output, names):
    for name in names:
        task = f':{name}:compileDebugKotlin'
        if not re.search(re.escape(task)+r'(?:\s|$)', output) or re.search(re.escape(task)+r'\s+NO-SOURCE', output):
            raise ValueError(f'{name}: native compiler task absent or NO-SOURCE')

def check_providers(path):
    key = '{http://schemas.android.com/apk/res/android}'
    providers = {p.get(key+'authorities'): p for p in ET.parse(path).getroot().findall('./application/provider')}
    expected = {'com.headstate.nativecheck.fileprovider': ('androidx.core.content.FileProvider','@xml/file_paths'),
                'com.headstate.nativecheck.headstate.export': ('com.pktstorm.headstate.export.MarkdownExportProvider','@xml/headstate_export_paths')}
    for authority, (name, resource) in expected.items():
        provider = providers.get(authority)
        if provider is None or provider.get(key+'name') != name or provider.get(key+'exported') != 'false' or provider.get(key+'grantUriPermissions') != 'true':
            raise ValueError('native export host provider missing or unsafe')
        if not any(m.get(key+'resource') == resource for m in provider.findall('meta-data')):
            raise ValueError('native export host provider path policy replaced')

def compile_android(root, work, modules, tauri, runner=run, update=False):
    copy(root/'scripts/native/android', work/'android')
    base = work/'android'
    copy(tauri/'mobile/android', base/'tauri-android')
    names = ['tauri-android'] + [p.name for p in modules]
    projects = names + ['export-host']
    for module in modules: copy(module/'android', base/module.name)
    (base/'settings.gradle').write_text('rootProject.name = "headstate-native-check"\n' + ''.join(f"include ':{name}'\n" for name in projects))
    for name in projects + ['root']:
        lock = root/'scripts/native/android/locks'/f'{name}.lockfile'
        if not update and not lock.is_file(): raise ValueError(f'{name}: missing native dependency lock')
        if lock.is_file(): shutil.copyfile(lock, base/('buildscript-gradle.lockfile' if name == 'root' else f'{name}/gradle.lockfile'))
    runner([str(base/'gradlew'),'--version'],base)
    tasks = [f':{name}:{task}' for name in names[1:] for task in ['assembleDebug','testDebugUnitTest']] + [':export-host:assembleDebug']
    output = runner([str(base/'gradlew'), *tasks,'--no-daemon','--console=plain', *(['--write-locks'] if update else [])],base)
    check_android_output(output,names[1:])
    manifests = list((base/'export-host/build/intermediates/merged_manifests').rglob('AndroidManifest.xml'))
    if not manifests: raise ValueError('native export host merged manifest missing')
    for manifest in manifests: check_providers(manifest)
    if update:
        for name in projects + ['root']:
            source = base/('buildscript-gradle.lockfile' if name == 'root' else f'{name}/gradle.lockfile')
            if not source.is_file(): raise ValueError(f'{name}: lock was not generated')
            shutil.copyfile(source,root/'scripts/native/android/locks'/f'{name}.lockfile')

def compile_ios(root, work, modules, tauri, runner=run):
    runner(['xcodebuild','-version'],root)
    lock = root/'scripts/native/ios/Package.resolved'
    expected = json.loads(lock.read_text())
    for module in modules:
        base = work/module.name
        copy(module/'ios',base/'ios')
        copy(tauri/'mobile/ios-api',base/'.tauri/tauri-api')
        shutil.copyfile(lock,base/'ios/Package.resolved')
        scheme = 'tauri-plugin-'+module.name
        common = ['-clonedSourcePackagesDirPath',str(work/'packages'),'-onlyUsePackageVersionsFromResolvedFile']
        listing = runner(['xcodebuild','-list','-json',*common],base/'ios')
        info = json.loads(listing[listing.index('{'):])
        schemes = info.get('workspace',info.get('project',{})).get('schemes',[])
        if scheme not in schemes: raise ValueError(f'{module.name}: expected scheme missing')
        runner(['xcodebuild','-scheme',scheme,'-destination','generic/platform=iOS Simulator','-derivedDataPath',str(base/'derived'),*common,'CODE_SIGNING_ALLOWED=NO','build'],base/'ios')
        if json.loads((base/'ios/Package.resolved').read_text()) != expected:
            raise ValueError(f'{module.name}: Swift dependency lock changed')

def check_ci(text):
    spec = importlib.util.spec_from_file_location('mobile_gate', ROOT/'scripts/check-mobile-gate.py')
    gate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gate)
    jobs = gate.jobs_of(text)
    for platform in ['android','ios']:
        job = jobs.get(f'mobile-{platform}',[])
        if f'mobile-{platform}' not in gate.gated_jobs(jobs):
            raise ValueError('native gate lost mobile change dependency')
        steps = [step for step in gate.steps_of(job) if f'        run: make check-native-{platform}' in step]
        if len(steps) != 1 or "        if: needs.mobile-changes.outputs.run == 'true'" not in steps[0] or any('continue-on-error:' in line for line in steps[0]):
            raise ValueError(f'{platform}: required native compile gate missing or bypassed')


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('platform',choices=['ios','android'])
    parser.add_argument('--work-dir',type=Path)
    parser.add_argument('--update-locks',action='store_true',help='Maintainer-only explicit Android lock refresh; CI never uses this')
    args=parser.parse_args()
    modules=inventory(ROOT,args.platform)
    print('Native modules:', ', '.join(p.name for p in modules),flush=True)
    work=args.work_dir or Path(tempfile.mkdtemp(prefix=f'headstate-native-{args.platform}-'))
    work.mkdir(parents=True,exist_ok=True)
    start=time.monotonic()
    tauri=tauri_source(ROOT)
    if args.platform == 'android': compile_android(ROOT,work,modules,tauri,update=args.update_locks)
    else:
        if args.update_locks: raise ValueError('Swift lock updates require explicit reviewed Package.resolved')
        compile_ios(ROOT,work,modules,tauri)
    print(f'Native {args.platform} compilation passed in {time.monotonic()-start:.1f}s; artifacts: {work}')
if __name__ == '__main__': main()
