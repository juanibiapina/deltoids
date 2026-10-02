#!/usr/bin/env python3
"""Run with uv run --with pyte docs/investigations/files-navigation-stress.py."""

import argparse
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time

source = pathlib.Path(__file__).with_name('files-navigation-probe.py')
spec = importlib.util.spec_from_file_location('navigation_probe', source)
probe = importlib.util.module_from_spec(spec)
sys.dont_write_bytecode = True
spec.loader.exec_module(probe)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=pathlib.Path, default=pathlib.Path('target/release/deltoids'))
    parser.add_argument('--language', choices=['rs', 'ts'], default='rs')
    parser.add_argument('--loops', type=int, default=100)
    args = parser.parse_args()
    binary = args.binary.resolve()
    suffix = args.language
    with tempfile.TemporaryDirectory(prefix='deltoids-navigation-stress-') as temp:
        root = pathlib.Path(temp)
        repo = root / 'repo'
        repo.mkdir()
        (repo / 'z').mkdir()
        config = root / 'config' / 'deltoids'
        config.mkdir(parents=True)
        (config / 'config.toml').write_text(
            '[theme]\nmode = "dark"\nsyntax_theme = "TokyoNight"\n'
            '[[commands]]\nkey = "E"\ncommand = "printf FOREGROUND_OK"\nsubprocess = true\n'
        )
        env = dict(os.environ, XDG_CONFIG_HOME=str(root / 'config'),
                   XDG_DATA_HOME=str(root / 'data'), TERM='xterm-256color', RV_NO_ICONS='1')
        paths = [f'{letter}.{suffix}' for letter in 'abc'] + [f'z/f{index:03}.{suffix}' for index in range(100)]
        subprocess.run(['git', 'init', '-q', str(repo)], check=True)
        for path in paths:
            (repo / path).write_text('// initial\n')
        subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True)
        subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Probe',
                        '-c', 'user.email=probe@example.com', 'commit', '-qm', 'fixture'], check=True)

        def content(marker, lines):
            return f'// {marker}\n' + ''.join(
                (f'fn item_{line}() {{ let value = "some syntax"; }}\n' if suffix == 'rs'
                 else f'function item_{line}() {{ const value = "some syntax"; }}\n')
                for line in range(lines)
            ) + f'// END_{marker}\n'

        for letter, lines in [('a', 300), ('b', 10_000), ('c', 300)]:
            (repo / f'{letter}.{suffix}').write_text(content(f'READY_{letter.upper()}', lines))
        for index in range(100):
            (repo / f'z/f{index:03}.{suffix}').write_text(content(f'READY_Z{index:03}', 300))
        tui = probe.Terminal(binary, repo, env, suffix)

        def move(keys, text):
            started = time.monotonic()
            os.write(tui.fd, keys)
            return (tui.wait(text, timeout=30) - started) * 1000

        try:
            tui.wait('READY_A', timeout=30)
            headers, exits = [], []
            for _ in range(args.loops):
                move(b'g', 'READY_A')
                headers.append(move(b'j', f'b.{suffix}'))
                exits.append(move(b'j', 'READY_C'))
            # Mouse selection of the third root file: SGR down/up at column 12, row 4.
            move(b'g', 'READY_A')
            mouse_ms = move(b'\x1b[<0;12;4M\x1b[<0;12;4m', 'READY_C')
            tui.close()
            tui = probe.Terminal(binary, repo, env, suffix)
            tui.wait('READY_A', timeout=30)
            large_started = time.monotonic()
            large_ms = move(b'j', 'READY_B')
            tui.drain(0.05)
            large_complete_ms = (tui.wait_settled() - large_started) * 1000
            move(b'j', 'READY_C')
            directory_ms = move(b'j', 'READY_Z000')
            # Jump to the directory's last file while the rest are pending.
            started = time.monotonic()
            os.write(tui.fd, b'2G')
            deadline = started + 30
            while not any('END_READY_Z099' in row[30:] for row in tui.screen.display):
                assert time.monotonic() < deadline, 'directory viewport did not become ready'
                tui.read(0.002)
                os.write(tui.fd, b'G')
            directory_end_ms = (tui.last_read_at - started) * 1000
            directory_exit_ms = move(b'1g', 'READY_A')
            # Resize while the huge file is selected to force a fresh render.
            move(b'j', 'READY_B')
            os.write(tui.fd, b'<')
            tui.drain(0.02)
            os.write(tui.fd, b'\x1b[O')
            tui.drain(0.3)
            blurred, blurred_cpu = tui.quiet_window(0.3)
            assert blurred == 0, f'focus-lost redraw: {blurred} bytes'
            os.write(tui.fd, b'\x1b[I')
            tui.wait('READY_B', timeout=30)
            tui.wait_settled()
            tui.drain(1)
            idle, idle_cpu = tui.quiet_window(0.3)
            assert idle == 0, f'idle redraw: {idle} bytes'
            move(b'g', 'READY_A')
            tui.drain(0.02)
            os.write(tui.fd, b'E')
            deadline = time.monotonic() + 10
            tail = b''
            while b'FOREGROUND_OK' not in tail:
                assert time.monotonic() < deadline, 'foreground command did not run'
                if tui.read(0.002):
                    tail = tail[-32:] + tui.last_data
            tui.drain(0.1)
            assert any('READY_A' in row[30:] for row in tui.screen.display), 'foreground return did not restore Files'
            # Quit with another render epoch outstanding.
            os.write(tui.fd, b'>')
            tui.drain(0.01)
            quit_ms = tui.close()
            tui = None
            report = {'language': suffix, 'transitions': args.loops,
                      'large_file_content_ms': round(large_ms, 2),
                      'large_file_complete_ms': round(large_complete_ms, 2), 'mouse_ms': round(mouse_ms, 2),
                      'directory_first_content_ms': round(directory_ms, 2),
                      'directory_last_viewport_ms': round(directory_end_ms, 2),
                      'directory_exit_ms': round(directory_exit_ms, 2),
                      'idle_bytes': idle, 'focus_lost_bytes': blurred, 'pending_quit_ms': quit_ms,
                      'foreground_restored': True,
                      'quiet_cpu_seconds': {'idle': idle_cpu, 'focus_lost': blurred_cpu}}
            for label, values in [('large_selection_header_ms', headers), ('large_selection_exit_ms', exits)]:
                report[label] = {f'p{int(p * 100)}': probe.percentile(values, p) for p in [0.5, 0.95, 0.99]}
            print(json.dumps(report, indent=2))
            assert report['large_selection_header_ms']['p99'] <= 33, report
            assert report['large_selection_exit_ms']['p99'] <= 33, report
            assert large_ms <= 50, report
            assert mouse_ms <= 33, report
            assert directory_exit_ms <= 33, report
            assert quit_ms <= 1000, report
        finally:
            if tui is not None:
                tui.close()


if __name__ == '__main__':
    main()
