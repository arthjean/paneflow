#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import signal
import subprocess


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run_x11(workspace, output, extra, env):
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--host', type=Path, required=True)
    parser.add_argument('--render-node', required=True)
    parser.add_argument('--egl-vendor', required=True)
    parser.add_argument('--gpui-device', required=True)
    parser.add_argument('--card')
    native = parser.parse_args(extra)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    output.mkdir(parents=True, exist_ok=False)
    env.pop('WAYLAND_DISPLAY', None)
    env.update(ZED_DEVICE_ID=native.gpui_device,
               PANEFLOW_BROWSER_RENDER_NODE=native.render_node,
               __EGL_VENDOR_LIBRARY_FILENAMES=native.egl_vendor)
    command = [
        'bun', 'scripts/browser-qualification/prototype.mjs',
        '--binary', str(native.binary.resolve()), '--host', str(native.host.resolve()),
        '--display', 'x11', '--scenario', 'webgl', '--steps', 'input',
        '--hold', '30', '--output', str(output / 'prototype'),
    ]
    with (output / 'runner.stdout').open('w') as stdout, (output / 'runner.stderr').open('w') as stderr:
        process = subprocess.Popen(command, cwd=workspace, env=env, stdout=stdout,
                                   stderr=stderr, start_new_session=True)
        try:
            process.wait(timeout=320)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
    return subprocess.CompletedProcess(command, process.returncode), command


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--display', choices=['wayland', 'x11'], default='wayland')
    args, extra = parser.parse_known_args()
    here = Path(__file__).resolve().parent
    workspace = args.workspace.resolve()
    if workspace == here.parents[3]:
        raise ValueError('shader experiment requires an isolated source copy')
    pipeline = workspace.parent / 'runtime-pipeline.json'
    if pipeline.exists() and json.loads(pipeline.read_text())['status'] != 'FINISHED_REQUIRES_REVIEW':
        raise ValueError('wait until the initial runtime pipeline has finished')
    fixture = workspace / 'bench/browser/fixtures/fixture.js'
    original = fixture.read_text()
    start = original.index('function webglFixture() {')
    end = original.index('function networkFixture() {', start)
    replacement = (here / 'shader-churn.js').read_text()
    modified = original[:start] + replacement + '\n' + original[end:]
    cache = args.output.resolve().with_name(args.output.name + '-cache')
    cache.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.update(XDG_CACHE_HOME=str(cache), MESA_SHADER_CACHE_DISABLE='false',
               MESA_SHADER_CACHE_DIR=str(cache), __GL_SHADER_DISK_CACHE='1',
               __GL_SHADER_DISK_CACHE_PATH=str(cache))
    fixture.write_text(modified)
    try:
        output = args.output.resolve()
        if args.display == 'x11':
            result, command = run_x11(workspace, output, extra, env)
        else:
            command = [
                'python3', str(here / 'run-wayland.py'), '--workspace', str(workspace),
                '--output', str(output), '--scenario', 'webgl',
                '--steps', 'input', '--hold', '30', *extra,
            ]
            result = subprocess.run(command, env=env)
        (output / 'fixture.js').write_text(modified)
        log = output / 'prototype/prototype.jsonl'
        events = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
        titles = [event['native'].get('title') for event in events if event.get('event') == 'native' and event.get('native', {}).get('native') == 'title']
        completed = 'PANEFLOW_SHADER_CHURN_PASSED:120' in titles and 'PANEFLOW_SHADER_CHURN_FAILED' not in titles
        native_path = output / 'prototype/native.json'
        native = json.loads(native_path.read_text()) if native_path.exists() else {}
        verdict = native.get('verdict', {})
        gpu_processes = [process for snapshot in native.get('snapshots', []) for process in snapshot.get('tree', []) if process.get('role') == 'gpu-process']
        gpu_threads = bool(gpu_processes) and all(process.get('thread_security_complete') and process.get('thread_security') and all(thread.get('seccomp') == 2 and thread.get('no_new_privs') == 1 and isinstance(thread.get('filter_count'), int) and thread['filter_count'] >= 1 for thread in process['thread_security']) for process in gpu_processes)
        sandbox = verdict.get('sandbox') == 'MEASURED' and gpu_threads
        presentation = verdict.get('page_presented') == 'MEASURED' and (args.display == 'x11' or verdict.get('x11_connection_absent') == 'MEASURED')
        shutdown = verdict.get('observed_processes_exited') == 'MEASURED'
        receipt = {
            'command': command, 'returncode': result.returncode,
            'verified_120_shader_programs': completed,
            'sandbox_and_gpu_threads_verified': sandbox,
            'display': args.display,
            'gpu_presentation_verified': presentation,
            'native_wayland_presentation_verified': args.display == 'wayland' and presentation,
            'x11_session': verdict.get('x11_session'),
            'observed_processes_exited': shutdown,
            'page_pixel_oracle_reads_per_program': 1,
            'native_shader_caches_requested': True,
            'fresh_cache_directory': str(cache),
            'original_fixture_sha256': digest(original.encode()),
            'experimental_fixture_sha256': digest(modified.encode()),
            'status': 'PASS' if result.returncode == 0 and completed and sandbox and presentation and shutdown else 'FAIL',
            'physical_presentation_measured': False,
        }
        (output / 'shader-churn.json').write_text(json.dumps(receipt, indent=2) + '\n')
        print(json.dumps(receipt, indent=2))
        return 0 if receipt['status'] == 'PASS' else 1
    finally:
        if fixture.read_text() == modified:
            fixture.write_text(original)
        else:
            raise RuntimeError('fixture changed during the experiment; refusing to overwrite it')


if __name__ == '__main__':
    raise SystemExit(main())
