#!/usr/bin/env python3
"""Run with uv run --with pyte docs/investigations/files-navigation-probe.py."""

import argparse
import fcntl
import json
import multiprocessing
import os
import pathlib
import pty
import queue
import select
import struct
import subprocess
import tempfile
import termios
import time

import pyte


def percentile(values, fraction):
    ordered = sorted(values)
    return round(ordered[min(len(ordered) - 1, int(len(ordered) * fraction))], 2)


class Terminal:
    def __init__(self, binary, repo, env, suffix='rs'):
        self.suffix = suffix
        self.screen = pyte.Screen(120, 40)
        self.stream = pyte.ByteStream(self.screen)
        self.fd, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))

        self.process = subprocess.Popen(
            [str(binary), 'tui'], stdin=slave, stdout=slave, stderr=slave,
            cwd=repo, env=env, start_new_session=True,
        )
        os.close(slave)
        context = multiprocessing.get_context('fork')
        self.output = context.Queue()
        self.stopping = context.Event()
        self.reader = context.Process(target=self.receive, daemon=True)
        self.reader.start()

    def receive(self):
        self.output.cancel_join_thread()
        while not self.stopping.is_set():
            if not select.select([self.fd], [], [], 0.01)[0]:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                return
            if not data:
                return
            self.output.put((time.monotonic(), data))

    def read(self, timeout):
        try:
            self.last_read_at, data = self.output.get(timeout=timeout)
        except queue.Empty:
            return 0
        self.last_data = data
        self.stream.feed(data)
        return len(data)

    def drain(self, duration):
        deadline = time.monotonic() + duration
        count = 0
        while time.monotonic() < deadline:
            count += self.read(min(0.01, max(0, deadline - time.monotonic())))
        return count

    def wait(self, text, timeout=10):
        deadline = time.monotonic() + timeout
        while not any(text in row[30:] for row in self.screen.display):
            assert time.monotonic() < deadline, f'timed out waiting for {text}: {self.screen.display}'
            self.read(0.002)
        return self.last_read_at

    def move(self, keys, index):
        start = time.monotonic()
        os.write(self.fd, keys)
        header = self.wait(f'f{index:03}.{self.suffix}')
        content = self.wait(f'FILE_{index:03}_READY')
        return (header - start) * 1000, (content - start) * 1000

    def wait_settled(self, timeout=30):
        deadline = time.monotonic() + timeout
        while any('Rendering…' in row[30:] for row in self.screen.display):
            assert time.monotonic() < deadline, 'visible rendering did not finish'
            self.read(0.002)
        return self.last_read_at

    def cpu_seconds(self):
        value = subprocess.check_output(['ps', '-o', 'time=', '-p', str(self.process.pid)], text=True).strip()
        if '.' not in value:
            return None
        return sum(float(part) * 60 ** position for position, part in enumerate(reversed(value.split(':'))))

    def quiet_window(self, duration):
        before = self.cpu_seconds()
        count = self.drain(duration)
        after = self.cpu_seconds()
        delta = None if before is None or after is None else round(after - before, 2)
        return count, delta

    def rss_mib(self):
        kib = subprocess.check_output(['ps', '-o', 'rss=', '-p', str(self.process.pid)], text=True)
        return round(int(kib.strip()) / 1024, 2)

    def close(self):
        started = time.monotonic()
        os.write(self.fd, b'q')
        try:
            self.process.wait(timeout=2)
            assert self.process.returncode == 0, self.process.returncode
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            self.stopping.set()
            self.reader.join(timeout=1)
            assert not self.reader.is_alive(), 'terminal reader did not stop'
            self.output.close()
            os.close(self.fd)
        return round((time.monotonic() - started) * 1000, 2)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=pathlib.Path, default=pathlib.Path('target/release/deltoids'))
    parser.add_argument('--files', type=int, default=110)
    parser.add_argument('--lines', type=int, default=300)
    parser.add_argument('--language', choices=['rs', 'ts'], default='rs')
    parser.add_argument('--assert-fast', action='store_true')
    args = parser.parse_args()
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix='deltoids-navigation-') as temp:
        root = pathlib.Path(temp)
        repo = root / 'repo'
        repo.mkdir()
        config = root / 'config' / 'deltoids'
        config.mkdir(parents=True)
        (config / 'config.toml').write_text('[theme]\nmode = "dark"\nsyntax_theme = "TokyoNight"\n')
        env = dict(os.environ, XDG_CONFIG_HOME=str(root / 'config'),
                   XDG_DATA_HOME=str(root / 'data'), TERM='xterm-256color', RV_NO_ICONS='1')
        subprocess.run(['git', 'init', '-q', str(repo)], check=True)
        for index in range(args.files):
            (repo / f'f{index:03}.{args.language}').write_text('// original\n')
        subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True)
        subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Probe',
                        '-c', 'user.email=probe@example.com', 'commit', '-qm', 'fixture'], check=True)
        for index in range(args.files):
            text = f'// FILE_{index:03}_READY\n' + ''.join(
                (f'fn item_{line}() {{ let value = "some syntax"; }}\n' if args.language == 'rs'
                 else f'function item_{line}() {{ const value = "some syntax"; }}\n')
                for line in range(args.lines)
            )
            (repo / f'f{index:03}.{args.language}').write_text(text)
        tui = Terminal(binary, repo, env, args.language)
        try:
            tui.wait('FILE_000_READY', timeout=30)
            first = [tui.move(b'j', index) for index in range(1, args.files)]
            warm = [tui.move(b'k', index) for index in range(args.files - 2, -1, -1)]
            tui.drain(0.1)
            warm_rss = tui.rss_mib()
            # Jump beyond the preparation radius after a fresh process starts.
            tui.close()
            tui = Terminal(binary, repo, env, args.language)
            tui.wait('FILE_000_READY', timeout=30)
            cold_jump = tui.move(b'G', args.files - 1)
            tui.drain(1)
            cold_rss = tui.rss_mib()
            idle, idle_cpu = tui.quiet_window(0.3)
            assert idle == 0, f'idle redraw: {idle} bytes'
            os.write(tui.fd, b'\x1b[O')
            tui.drain(0.2)
            blurred, blurred_cpu = tui.quiet_window(0.3)
            assert blurred == 0, f'focus-lost redraw: {blurred} bytes'
            os.write(tui.fd, b'\x1b[I')
            tui.drain(0.3)
            quit_ms = tui.close()
            tui = None
            report = {'binary': str(binary), 'files': args.files, 'language': args.language, 'lines_per_file': args.lines,
                      'cold_jump_ms': {'header': round(cold_jump[0], 2), 'content': round(cold_jump[1], 2)},
                      'idle_bytes': idle, 'focus_lost_bytes': blurred, 'quit_ms': quit_ms,
                      'tui_rss_mib': {'after_all_visits': warm_rss, 'cold_jump': cold_rss},
                      'quiet_cpu_seconds': {'idle': idle_cpu, 'focus_lost': blurred_cpu}}
            for name, timings in [('first_traversal', first), ('reverse_traversal', warm)]:
                report[name] = {label: {f'p{int(p * 100)}': percentile([row[col] for row in timings], p)
                                      for p in [0.5, 0.95, 0.99]}
                                for col, label in enumerate(['header_ms', 'content_ms'])}
            print(json.dumps(report, indent=2))
            if args.assert_fast:
                assert report['first_traversal']['header_ms']['p95'] <= 16, report
                assert report['first_traversal']['header_ms']['p99'] <= 33, report
                assert report['first_traversal']['content_ms']['p95'] <= 50, report
                assert report['reverse_traversal']['content_ms']['p95'] <= 16, report
        finally:
            if tui is not None:
                tui.close()


if __name__ == '__main__':
    main()
