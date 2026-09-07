#!/usr/bin/env python3
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--build-output', type=Path, required=True)
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    original = Path(__file__).resolve().parents[4]
    workspace = args.workspace.resolve()
    if workspace == original:
        raise ValueError('experimental runtime requires an isolated workspace')
    manifest_path = workspace / 'native/browser/manifest.toml'
    manifest = tomllib.loads(manifest_path.read_text())
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    name = f"cef_binary_{manifest['cef_version']}_linux64_tsync_experimental"
    archive = output / f'{name}.tar.bz2'
    binaries = {
        'chrome_sandbox': 'chrome-sandbox',
        'libcef.so': 'libcef.so',
        'libEGL.so': 'libEGL.so',
        'libGLESv2.so': 'libGLESv2.so',
        'libvk_swiftshader.so': 'libvk_swiftshader.so',
        'libvulkan.so.1': 'libvulkan.so.1',
        'v8_context_snapshot.bin': 'v8_context_snapshot.bin',
        'vk_swiftshader_icd.json': 'vk_swiftshader_icd.json',
    }
    with tempfile.TemporaryDirectory(prefix='bundle-', dir=output) as scratch:
        root = Path(scratch) / name
        (root / 'Release').mkdir(parents=True)
        (root / 'Resources/locales').mkdir(parents=True)
        for source, destination in binaries.items():
            shutil.copy2(args.build_output / source, root / 'Release' / destination)
        for resource in ['chrome_100_percent.pak', 'chrome_200_percent.pak', 'resources.pak', 'icudtl.dat']:
            shutil.copy2(args.build_output / resource, root / 'Resources' / resource)
        locales = sorted((args.build_output / 'locales').glob('*.pak'))
        if not locales:
            raise ValueError('CEF locale resources are absent')
        for locale in locales:
            shutil.copy2(locale, root / 'Resources/locales' / locale.name)
        files = sorted(path for path in root.rglob('*') if path.is_file())
        hashes = {path.relative_to(root).as_posix(): digest(path) for path in files}
        total = sum(path.stat().st_size for path in files)
        maximum = max(path.stat().st_size for path in files)
        with tarfile.open(archive, 'w:bz2', compresslevel=3) as bundle:
            bundle.add(root, arcname=name)
    lines = [f'{key} = {json.dumps(value)}' for key, value in manifest.items() if key != 'targets']
    target = {
        'unpacked_size': total,
        'max_file_size': maximum,
        'availability': 'development',
        'native_qualification': 'tsync_source_experiment_not_qualified',
        'archive': archive.name,
        'url': archive.as_uri(),
        'sha256': digest(archive),
        'size': archive.stat().st_size,
    }
    lines += ['', '[targets."x86_64-unknown-linux-gnu"]']
    lines += [f'{key} = {json.dumps(value)}' for key, value in target.items()]
    lines += ['', '[targets."x86_64-unknown-linux-gnu".files]']
    lines += [f'{json.dumps(key)} = {json.dumps(value)}' for key, value in hashes.items()]
    candidate = output / 'manifest.toml'
    candidate.write_text('\n'.join(lines) + '\n')
    subprocess.run([
        'python3', str(workspace / 'scripts/fetch-browser.py'),
        '--target', 'x86_64-unknown-linux-gnu', '--archive', str(archive),
        '--manifest', str(candidate),
        '--destination', str(workspace / 'native/browser/prebuilt'),
    ], check=True)
    shutil.copy2(candidate, manifest_path)
    receipt = {
        'workspace': str(workspace), 'build_output': str(args.build_output.resolve()),
        'manifest_sha256': digest(candidate), 'archive_sha256': target['sha256'],
        'runtime': str(workspace / 'native/browser/prebuilt/x86_64-unknown-linux-gnu' / target['sha256']),
        'full_cef_qualification': False,
    }
    (output / 'stage.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    main()
