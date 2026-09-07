#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import time


def stop(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def interrupted(signum, frame):
    raise KeyboardInterrupt


def main():
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--host', type=Path, required=True)
    parser.add_argument('--render-node', type=Path, required=True)
    parser.add_argument('--card', type=Path, required=True)
    parser.add_argument('--egl-vendor', type=Path, required=True)
    parser.add_argument('--gpui-device', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--steps', default='input,resize,scale,host-loss')
    parser.add_argument('--scenario', default='empty')
    parser.add_argument('--hold', type=int, default=1)
    args = parser.parse_args()
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    display = f'paneflow-tsync-{os.getpid()}'
    env = os.environ.copy()
    env.pop('DISPLAY', None)
    env.update(WAYLAND_DISPLAY=display, ZED_DEVICE_ID=args.gpui_device,
               PANEFLOW_BROWSER_RENDER_NODE=str(args.render_node),
               __EGL_VENDOR_LIBRARY_FILENAMES=str(args.egl_vendor))
    command = [
        'bwrap', '--bind', '/', '/', '--dev-bind', '/dev', '/dev',
        '--tmpfs', '/dev/dri',
        '--dev-bind', str(args.render_node), str(args.render_node),
        '--dev-bind', str(args.card), str(args.card),
        '--unsetenv', 'DISPLAY',
        'dbus-run-session', '--', 'mutter', '--headless',
        '--virtual-monitor', '1280x800', '--wayland', '--no-x11',
        '--wayland-display', display,
    ]
    run = None
    with (output / 'mutter.txt').open('w') as log:
        compositor = subprocess.Popen(command, env=env, stdout=log,
                                      stderr=subprocess.STDOUT, start_new_session=True)
        try:
            socket = Path(env['XDG_RUNTIME_DIR']) / display
            deadline = time.monotonic() + 20
            while not socket.exists():
                if compositor.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('isolated Wayland compositor did not start')
                time.sleep(0.1)
            prototype = [
                'bun', 'scripts/browser-qualification/prototype.mjs',
                '--binary', str(args.binary.resolve()), '--host', str(args.host.resolve()),
                '--display', 'wayland', '--scenario', args.scenario, '--steps', args.steps, '--hold', str(args.hold),
                '--output', str(output / 'prototype'),
            ]
            with (output / 'runner.stdout').open('w') as stdout, (output / 'runner.stderr').open('w') as stderr:
                run = subprocess.Popen(prototype, cwd=args.workspace, env=env,
                                       stdout=stdout, stderr=stderr, start_new_session=True)
                run.wait(timeout=320)
            native = json.loads((output / 'prototype/native.json').read_text())
            receipt = {
                'compositor_command': command, 'prototype_command': prototype,
                'returncode': run.returncode, 'status': native['status'],
                'verdict': native['verdict'], 'physical_presentation_measured': False,
            }
            (output / 'run.json').write_text(json.dumps(receipt, indent=2) + '\n')
            print(json.dumps(receipt, indent=2))
            return 0 if run.returncode == 0 and native['status'] == 'COMPLETED' else 1
        finally:
            if run is not None:
                stop(run)
            stop(compositor)


if __name__ == '__main__':
    raise SystemExit(main())
