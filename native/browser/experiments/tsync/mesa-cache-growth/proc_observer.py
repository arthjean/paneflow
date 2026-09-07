import json
import os
from pathlib import Path
import re
import time


LIMITS = {'processes': 8192, 'threads': 256, 'read_bytes': 128 * 1024 * 1024,
          'artifact_bytes': 16 * 1024 * 1024, 'samples': 1500, 'errors': 128}
DISK = re.compile(r'(?:^|:)disk\$(\d+)$')


def monotonic_ns():
    return time.clock_gettime_ns(time.CLOCK_MONOTONIC)


def boottime_ns():
    return time.clock_gettime_ns(time.CLOCK_BOOTTIME)


class Observer:
    def __init__(self, directory):
        self.directory = Path(directory)
        self.bytes_read = 0
        self.bytes_written = 0
        self.samples = 0
        self.errors = []
        self.hz = os.sysconf('SC_CLK_TCK')
        self.stream = (self.directory / 'thread-observations.jsonl').open('x')

    def close(self):
        self.stream.close()

    def read(self, path, cap=32768):
        with open(path, 'rb') as handle:
            data = handle.read(cap + 1)
        self.bytes_read += len(data)
        if len(data) > cap or self.bytes_read > LIMITS['read_bytes']:
            raise RuntimeError('PROC_READ_LIMIT')
        return data.decode()

    def emit(self, event):
        line = json.dumps(event, separators=(',', ':')) + '\n'
        self.bytes_written += len(line.encode())
        if self.bytes_written > LIMITS['artifact_bytes']:
            raise RuntimeError('ARTIFACT_LIMIT')
        self.stream.write(line)
        self.stream.flush()

    def error(self, operation, error):
        if len(self.errors) >= LIMITS['errors']:
            raise RuntimeError('ERROR_LIMIT')
        value = {'operation': operation, 'error': str(error), 'at_ns': str(monotonic_ns())}
        self.errors.append(value)
        self.emit({'event': 'observation_error', **value})

    def stat(self, pid, tid=None):
        path = f'/proc/{pid}' + (f'/task/{tid}' if tid is not None else '')
        text = self.read(path + '/stat', 4096)
        close = text.rfind(')')
        fields = text[close + 2:].split()
        if close < 0 or len(fields) < 20 or int(text.split('(', 1)[0]) != (tid or pid):
            raise RuntimeError('INVALID_STAT')
        return {'pid': pid, 'tid': tid, 'parent': int(fields[1]), 'start_ticks': fields[19]}

    def tasks(self, pid):
        values = []
        with os.scandir(f'/proc/{pid}/task') as entries:
            for entry in entries:
                if entry.name.isdecimal():
                    values.append(int(entry.name))
                    if len(values) > LIMITS['threads']:
                        raise RuntimeError('THREAD_LIMIT')
        return sorted(values)

    def thread(self, pid, tid, security=False):
        before = self.stat(pid, tid)
        prefix = f'/proc/{pid}/task/{tid}'
        result = {**before, 'name': self.read(prefix + '/comm', 128).strip()}
        if security:
            text = self.read(prefix + '/status')
            for name, key in [('Seccomp', 'seccomp'), ('NoNewPrivs', 'no_new_privs'), ('Seccomp_filters', 'filter_count')]:
                match = re.search(r'^' + name + r':\s+(\d+)\s*$', text, re.M)
                if not match:
                    raise RuntimeError('MISSING_SECURITY_FIELD:' + name)
                result[key] = int(match[1])
        if before != self.stat(pid, tid):
            raise RuntimeError('THREAD_IDENTITY_CHANGED')
        return result

    def discover(self, host_pid):
        identities = {}
        with os.scandir('/proc') as entries:
            for entry in entries:
                if not entry.name.isdecimal():
                    continue
                if len(identities) >= LIMITS['processes']:
                    raise RuntimeError('PROCESS_LIMIT')
                try:
                    item = self.stat(int(entry.name))
                    identities[item['pid']] = item
                except (FileNotFoundError, ProcessLookupError):
                    continue
        if host_pid not in identities:
            raise RuntimeError('HOST_ABSENT')
        descendants = {host_pid}
        for _ in range(len(identities)):
            added = {pid for pid, item in identities.items() if item['parent'] in descendants}
            if added <= descendants:
                break
            descendants |= added
        candidates = []
        for pid in sorted(descendants - {host_pid}):
            args = self.read(f'/proc/{pid}/cmdline', 65536).split('\0')
            names = [self.read(f'/proc/{pid}/task/{tid}/comm', 128).strip() for tid in self.tasks(pid)]
            if '--type=gpu-process' in args or 'VizCompositorTh' in names:
                if any(arg.split('=')[0] in ['--no-sandbox', '--disable-gpu-sandbox', '--disable-seccomp-filter-sandbox'] for arg in args):
                    raise RuntimeError('SANDBOX_DISABLED')
                candidates.append(pid)
        if len(candidates) != 1:
            raise RuntimeError('GPU_IDENTITY_AMBIGUOUS:' + str(candidates))
        chain = []
        pid = candidates[0]
        while True:
            if pid not in identities or any(item['pid'] == pid for item in chain):
                raise RuntimeError('ANCESTRY_INVALID')
            chain.append(identities[pid])
            if pid == host_pid:
                break
            pid = identities[pid]['parent']
        self.validate_chain(chain)
        return chain

    def validate_chain(self, chain):
        for identity in chain:
            if self.stat(identity['pid']) != identity:
                raise RuntimeError('PROCESS_IDENTITY_CHANGED')

    def census(self, chain):
        self.validate_chain(chain)
        pid = chain[0]['pid']
        started = monotonic_ns()
        tids = self.tasks(pid)
        threads = [self.thread(pid, tid, True) for tid in tids]
        if tids != self.tasks(pid):
            raise RuntimeError('THREAD_INVENTORY_CHANGED')
        for item in threads:
            if self.stat(pid, item['tid'])['start_ticks'] != item['start_ticks']:
                raise RuntimeError('THREAD_IDENTITY_CHANGED')
            if item['seccomp'] != 2 or item['no_new_privs'] != 1 or item['filter_count'] < 1:
                raise RuntimeError('THREAD_SECURITY_FAILED:' + str(item['tid']))
        self.validate_chain(chain)
        result = {'chain': chain, 'threads': threads, 'started_ns': str(started),
                  'ended_ns': str(monotonic_ns()), 'ended_boottime_ns': str(boottime_ns()),
                  'clock_ticks_per_second': self.hz, 'complete': bool(threads)}
        if not result['complete']:
            raise RuntimeError('EMPTY_THREAD_CENSUS')
        return result

    def watch(self, chain, baseline, duration, process):
        pid = chain[0]['pid']
        initial = {(t['tid'], t['start_ticks']) for t in baseline['threads']}
        initial_disk = {key for t in baseline['threads'] if DISK.search(t['name']) for key in [(t['tid'], t['start_ticks'])]}
        initial_indices = [int(DISK.search(t['name']).group(1)) for t in baseline['threads'] if DISK.search(t['name'])]
        candidates = {}
        confirmed = []
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError('RUNNER_EXITED_DURING_OBSERVATION')
            self.samples += 1
            if self.samples > LIMITS['samples']:
                raise RuntimeError('SAMPLE_LIMIT')
            self.validate_chain(chain)
            current = []
            for tid in self.tasks(pid):
                try:
                    current.append(self.thread(pid, tid))
                except (FileNotFoundError, ProcessLookupError) as error:
                    self.error('thread_disappeared', error)
            disks = [t for t in current if DISK.search(t['name'])]
            keys = {(t['tid'], t['start_ticks']) for t in disks}
            self.emit({'event': 'disk_inventory', 'at_ns': str(monotonic_ns()), 'gpu_pid': pid,
                       'thread_count': len(current), 'disk_threads': disks})
            for thread in disks:
                key = (thread['tid'], thread['start_ticks'])
                if key in initial:
                    continue
                born_after = int(thread['start_ticks']) * 1_000_000_000 > int(baseline['ended_boottime_ns']) * self.hz
                value = self.thread(pid, thread['tid'], True)
                if value['start_ticks'] != thread['start_ticks'] or value['name'] != thread['name']:
                    raise RuntimeError('DISK_WORKER_IDENTITY_CHANGED')
                secure = value['seccomp'] == 2 and value['no_new_privs'] == 1 and value['filter_count'] >= 1
                disk_index = int(DISK.search(thread['name']).group(1))
                new_index = bool(initial_indices) and disk_index > max(initial_indices)
                growth = bool(initial_disk) and initial_disk <= keys and len(keys) > len(initial_disk) and new_index
                if key not in candidates:
                    candidates[key] = {'first_seen_ns': str(monotonic_ns()), 'observations': 0}
                candidate = candidates[key]
                candidate.update(thread=value, born_after_baseline=born_after, secure=secure,
                                 initial_disk_workers_still_present=initial_disk <= keys, new_index_above_baseline=new_index,
                                 concurrent_disk_count_increased=len(keys) > len(initial_disk), last_seen_ns=str(monotonic_ns()))
                candidate['observations'] += 1
                self.emit({'event': 'new_disk_worker', **candidate})
                if not secure:
                    raise RuntimeError('NEW_DISK_WORKER_SECURITY_FAILED')
                if born_after and secure and growth and candidate['observations'] >= 2 and key not in confirmed:
                    confirmation = self.census(chain)
                    self.emit({'event': 'growth_confirmed', 'worker': list(key), 'census': confirmation})
                    confirmed.append(key)
            time.sleep(0.02)
        final = self.census(chain)
        return {'status': 'OBSERVED_SECURE_NEW_DISK_WORKER' if confirmed else 'NOT_OBSERVED',
                'confirmed': [list(key) for key in confirmed], 'candidates': list(candidates.values()),
                'final': final, 'poll_interval_ms': 20, 'samples': self.samples,
                'read_bytes': self.bytes_read, 'artifact_bytes': self.bytes_written,
                'errors': self.errors, 'claim_scope': 'new disk worker in the same GPU; util_queue instance identity unavailable'}
