#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import subprocess
import sys
import time
import tomllib

from proc_observer import LIMITS, Observer, monotonic_ns


HERE = Path(__file__).resolve().parent


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as handle:
        for data in iter(lambda: handle.read(1024 * 1024), b''):
            result.update(data)
    return result.hexdigest()


def receipt(path, value):
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def events(path):
    with path.open('rb') as handle:
        data = handle.read(8 * 1024 * 1024 + 1)
    if len(data) > 8 * 1024 * 1024:
        raise RuntimeError('PROTOTYPE_LOG_LIMIT')
    lines = data.split(b'\n')
    return [json.loads(line) for line in lines[:-1] if line]


def cache_inventory(directory):
    values = []
    pending = [(directory, 0)]
    inspected = 0
    while pending:
        base, depth = pending.pop()
        if depth > 32:
            raise RuntimeError('CACHE_DEPTH_LIMIT')
        with os.scandir(base) as entries:
            for entry in entries:
                inspected += 1
                if inspected > 4096:
                    raise RuntimeError('CACHE_INVENTORY_LIMIT')
                if entry.is_symlink():
                    raise RuntimeError('CACHE_SYMLINK')
                path = Path(entry.path)
                if entry.is_dir(follow_symlinks=False):
                    pending.append((path, depth + 1))
                elif entry.is_file(follow_symlinks=False):
                    values.append({'path': str(path.relative_to(directory)), 'size': entry.stat().st_size})
                else:
                    raise RuntimeError('CACHE_NONREGULAR_FILE')
    return values


def prepare(source, output, token):
    workspace = output / 'workspace'
    workspace.mkdir()
    for relative in ['scripts/browser-qualification', 'bench/browser/fixtures']:
        destination = workspace / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source / relative, destination)
    shutil.copy2(source / 'scripts/fetch-browser.py', workspace / 'scripts/fetch-browser.py')
    browser = workspace / 'native/browser'
    browser.mkdir(parents=True)
    shutil.copy2(source / 'native/browser/manifest.toml', browser / 'manifest.toml')
    manifest = tomllib.loads((browser / 'manifest.toml').read_text())
    target = {'x86_64': 'x86_64', 'aarch64': 'aarch64'}.get(os.uname().machine)
    if target is None:
        raise RuntimeError('UNSUPPORTED_ARCHITECTURE')
    target += '-unknown-linux-gnu'
    candidate = manifest['targets'][target]
    runtime = Path('native/browser/prebuilt') / target / candidate['sha256']
    (workspace / runtime).parent.mkdir(parents=True)
    shutil.copytree(source / runtime, workspace / runtime, symlinks=False)
    fixture = workspace / 'bench/browser/fixtures/fixture.js'
    original = fixture.read_text()
    start = original.index('function webglFixture() {')
    end = original.index('function networkFixture() {', start)
    replacement = (HERE / 'shader-churn.js').read_text().replace('__PANEFLOW_GATE_TOKEN__', token)
    fixture.write_text(original[:start] + replacement + '\n' + original[end:])
    shutil.copy2(HERE / 'fixtures.mjs', workspace / 'scripts/browser-qualification/fixtures.mjs')
    return workspace, {'runtime_archive_sha256': candidate['sha256'],
                       'libcef_sha256': digest(workspace / runtime / 'Release/libcef.so'),
                       'fixture_sha256': digest(fixture),
                       'fixture_server_sha256': digest(workspace / 'scripts/browser-qualification/fixtures.mjs')}


def wait_host(log, process):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError('RUNNER_EXITED_BEFORE_BASELINE')
        if log.exists():
            hosts = [item for item in events(log) if item.get('event') == 'host_ready']
            if len(hosts) == 1:
                return hosts[0]['pid']
            if len(hosts) > 1:
                raise RuntimeError('HOST_RESTARTED_BEFORE_BASELINE')
        time.sleep(0.005)
    raise RuntimeError('HOST_READY_TIMEOUT')


def execute(args, output, workspace, control, cache, token, binding):
    command = [sys.executable, str(HERE / 'run-wayland.py'), '--workspace', str(workspace),
               '--binary', str(args.binary.resolve()), '--host', str(args.host.resolve()),
               '--render-node', args.render_node, '--card', args.card, '--egl-vendor', args.egl_vendor,
               '--gpui-device', args.gpui_device, '--output', str(output / 'run'),
               '--scenario', 'webgl', '--steps', '', '--hold', '60']
    env = os.environ.copy()
    env.update(XDG_CACHE_HOME=str(cache), MESA_SHADER_CACHE_DIR=str(cache), MESA_SHADER_CACHE_DISABLE='false',
               __GL_SHADER_DISK_CACHE='1', __GL_SHADER_DISK_CACHE_PATH=str(cache),
               PANEFLOW_SHADER_GATE_DIRECTORY=str(control), PANEFLOW_SHADER_GATE_TOKEN=token)
    for key in list(env):
        if key.startswith('PANEFLOW_M1_'):
            del env[key]
    result = {'schema_version': 1, 'status': 'INCOMPLETE', 'm1_qualification': 'NOT_EVALUATED',
              'physical_presentation_measured': False, 'binding': binding, 'command': command,
              'limits': LIMITS, 'cache_before': cache_inventory(cache), 'observation_seconds': 20,
              'cache_environment': {key: env[key] for key in ['XDG_CACHE_HOME', 'MESA_SHADER_CACHE_DIR', 'MESA_SHADER_CACHE_DISABLE', '__GL_SHADER_DISK_CACHE', '__GL_SHADER_DISK_CACHE_PATH']}}
    observer = Observer(output)
    process = None
    try:
        with (output / 'runner.stdout').open('x') as stdout, (output / 'runner.stderr').open('x') as stderr:
            process = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr)
            log = output / 'run/prototype/prototype.jsonl'
            host_pid = wait_host(log, process)
            ready_deadline = time.monotonic() + 10
            while True:
                if process.poll() is not None or time.monotonic() >= ready_deadline:
                    raise RuntimeError('GPU_SANDBOX_START_TIMEOUT')
                try:
                    chain = observer.discover(host_pid)
                except RuntimeError as error:
                    if str(error) != 'GPU_IDENTITY_AMBIGUOUS:[]':
                        raise
                    time.sleep(0.02)
                    continue
                main_thread = observer.thread(chain[0]['pid'], chain[0]['pid'], True)
                if main_thread['seccomp'] == 2 and main_thread['no_new_privs'] == 1 and main_thread['filter_count'] >= 1:
                    break
                time.sleep(0.02)
            baseline = None
            for _ in range(5):
                try:
                    baseline = observer.census(chain)
                    break
                except (FileNotFoundError, ProcessLookupError, RuntimeError) as error:
                    observer.error('baseline_attempt', error)
                    if isinstance(error, RuntimeError) and str(error) != 'THREAD_INVENTORY_CHANGED':
                        raise
                    time.sleep(0.05)
            if baseline is None:
                raise RuntimeError('BASELINE_INCOMPLETE')
            receipt(output / 'baseline.json', baseline)
            time.sleep(2 / observer.hz)
            release = {'token': token, 'baseline_complete': True, 'baseline_sha256': digest(output / 'baseline.json'),
                       'release_ns': str(monotonic_ns()), 'clock': 'CLOCK_MONOTONIC'}
            receipt(control / 'release.json', release)
            observer.emit({'event': 'gate_released', **release})
            result['growth'] = observer.watch(chain, baseline, 20, process)
            process.wait(timeout=90)
            observed = events(log)
            titles = [item.get('native', {}).get('title') for item in observed if item.get('event') == 'native']
            hosts = [item for item in observed if item.get('event') == 'host_ready']
            native = json.loads((output / 'run/prototype/native.json').read_text())
            verdict = native.get('verdict', {})
            native_gpu = [member for snapshot in native.get('snapshots', []) for member in snapshot.get('tree', [])
                          if member.get('role') == 'gpu-process' and member.get('pid') == chain[0]['pid']
                          and str(member.get('start_ticks')) == chain[0]['start_ticks']]
            result.update(returncode=process.returncode, shader_titles=titles, cache_after=cache_inventory(cache),
                          observed_release=json.loads((control / 'observed.json').read_text()), native_verdict=verdict)
            success = (process.returncode == 0 and len(hosts) == 1 and bool(native_gpu)
                       and 'PANEFLOW_SHADER_CHURN_PASSED:120' in titles and 'PANEFLOW_SHADER_CHURN_FAILED' not in titles
                       and result['growth']['status'] == 'OBSERVED_SECURE_NEW_DISK_WORKER'
                       and bool(result['cache_after']) and not result['cache_before']
                       and all(verdict.get(key) == 'MEASURED' for key in ['sandbox', 'page_presented', 'x11_connection_absent', 'observed_processes_exited']))
            result['status'] = 'PASS_BOUNDED_GROWTH' if success else 'NOT_PROVEN'
    except (Exception, KeyboardInterrupt) as error:
        result['error'] = f'{type(error).__name__}: {error}'
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                result['cleanup_error'] = 'Runner did not finish its scoped cleanup within 30 seconds'
                result['status'] = 'INCOMPLETE'
        observer.close()
        result['observer_errors'] = observer.errors
        result['read_bytes'] = observer.bytes_read
        receipt(output / 'cache-growth.json', result)
    print(json.dumps(result, indent=2))
    return 0 if result['status'] == 'PASS_BOUNDED_GROWTH' else 1


def interrupted(signum, frame):
    raise KeyboardInterrupt


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--source-workspace', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--host', type=Path, required=True)
    parser.add_argument('--render-node', required=True)
    parser.add_argument('--card', required=True)
    parser.add_argument('--egl-vendor', required=True)
    parser.add_argument('--gpui-device', required=True)
    parser.add_argument('--run', action='store_true')
    args = parser.parse_args()
    source, output = args.source_workspace.resolve(), args.output.resolve()
    if not args.run:
        print(json.dumps({'status': 'NOT_EXECUTED', 'source': str(source), 'output': str(output), 'requires': '--run'}))
        return 0
    if source == output or source in output.parents or output.exists():
        raise ValueError('Output must be a new directory outside the source workspace')
    signal.signal(signal.SIGTERM, interrupted)
    output.mkdir(parents=True, mode=0o700)
    control, cache = output / 'control', output / 'fresh-cache'
    control.mkdir(mode=0o700)
    cache.mkdir(mode=0o700)
    token = secrets.token_hex(16)
    workspace, binding = prepare(source, output, token)
    binding.update(binary_sha256=digest(args.binary.resolve()), host_sha256=digest(args.host.resolve()),
                   harness_sha256=digest(Path(__file__)), observer_sha256=digest(HERE / 'proc_observer.py'),
                   runner_sha256=digest(HERE / 'run-wayland.py'))
    receipt(output / 'binding.json', binding)
    return execute(args, output, workspace, control, cache, token, binding)


if __name__ == '__main__':
    raise SystemExit(main())
