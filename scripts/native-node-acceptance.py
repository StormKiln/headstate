#!/usr/bin/env python3
"""Opt-in synthetic native SIGSEGV evidence; never a spontaneous-crash fix claim."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import resource
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
FLAGS = ['--trace-exit', '--report-on-fatalerror', '--report-uncaught-exception',
         '--report-exclude-env', '--report-exclude-network']
SOURCE = r"""#include <node_api.h>
static napi_value syntheticNativeCrash(napi_env env, napi_callback_info info) {
  (void)env; (void)info; *(volatile int *)0 = 1; return 0;
}
static napi_value init(napi_env env, napi_value exports) {
  napi_value fn;
  napi_create_function(env, "syntheticNativeCrash", NAPI_AUTO_LENGTH, syntheticNativeCrash, 0, &fn);
  napi_set_named_property(env, exports, "crash", fn); return exports;
}
NAPI_MODULE(NODE_GYP_MODULE_NAME, init)
"""


def accepted(status, evidence, pid):
    return (status == -signal.SIGSEGV and evidence.get('status') == 'captured'
            and evidence.get('pid') == pid and evidence.get('exception', {}).get('signal') == 'SIGSEGV'
            and evidence.get('symbolizedFrames', 0) > 0)


def no_core():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def bounded_child(argv, cwd, timeout=10):
    started = time.time()
    child = subprocess.Popen(argv, cwd=cwd, env={}, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, preexec_fn=no_core,
                             start_new_session=True)
    timed_out = False
    try:
        status = child.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        os.killpg(child.pid, signal.SIGKILL)
        status = child.wait(timeout=5)
    return child.pid, status, started, time.time(), timed_out


def run(node, out):
    if platform.system() != 'Darwin':
        raise RuntimeError('native acceptance requires macOS IPS collection')
    out.mkdir(parents=True, exist_ok=False)
    node = Path(node).resolve(strict=True)
    version = subprocess.check_output([str(node), '--version'], env={}, text=True, timeout=5).strip()
    if version != 'v' + (ROOT / '.nvmrc').read_text().strip().removeprefix('v'):
        raise RuntimeError('fixture requires exact repository Node pin')
    source = out / 'synthetic_native.c'
    source.write_text(SOURCE)
    subprocess.run(['clang', '-O0', '-g', '-bundle', '-undefined', 'dynamic_lookup',
                    '-I' + str(node.parent.parent / 'include/node'), str(source),
                    '-o', str(out / 'synthetic_native.node')], check=True,
                   timeout=30, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    (out / 'main.cjs').write_text("require('./synthetic_native.node').crash();\n")
    (out / 'worker.cjs').write_text("new (require('node:worker_threads').Worker)(require('node:path').join(__dirname, 'main.cjs'));\n")
    spec = importlib.util.spec_from_file_location('collector', ROOT / 'scripts/collect-frontend-crash.py')
    collector = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(collector)
    result = {'schema': 1, 'scope': 'controlled native main/worker collection only; spontaneous cause unknown',
              'nodeVersion': version, 'nodeSha256': hashlib.sha256(node.read_bytes()).hexdigest(),
              'architecture': platform.machine(), 'osRelease': platform.release(),
              'flags': FLAGS, 'coreLimit': 0, 'attempts': [], 'pass': False}
    for role in ['main', 'worker']:
        pid, status, started, ended, timed_out = bounded_child(
            [str(node), *FLAGS, '--report-directory=' + str(out), str(out / (role + '.cjs'))], out)
        evidence = collector.collect([Path.home() / 'Library/Logs/DiagnosticReports',
                                      Path('/Library/Logs/DiagnosticReports')], pid, started, ended)
        result['attempts'].append({'role': role, 'pid': pid, 'exit': status,
                                   'timedOut': timed_out, 'evidence': evidence,
                                   'pass': not timed_out and accepted(status, evidence, pid)})
        (out / 'evidence.json').write_text(json.dumps(result, indent=2) + '\n')
    result['pass'] = all(attempt['pass'] for attempt in result['attempts'])
    (out / 'evidence.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'pass': result['pass'], 'attempts': [
        {'role': a['role'], 'exit': a['exit'], 'status': a['evidence']['status'],
         'symbolizedFrames': a['evidence'].get('symbolizedFrames', 0)} for a in result['attempts']]}))
    return result['pass']


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--node', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    raise SystemExit(0 if run(args.node, args.output) else 1)
