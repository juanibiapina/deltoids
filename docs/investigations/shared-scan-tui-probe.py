import json
import os
from pathlib import Path
import select
import time

helpers_path = Path(__file__).with_name('background-cpu-probe.py')
helpers = helpers_path.read_text().split('print(json.dumps(dict(root=str(root), binary=BIN)), flush=True)')[0]
exec(compile(helpers, str(helpers_path), 'exec'))
if os.environ.get('DELTOIDS_FOCUS_REPO'):
    repo = Path(os.environ['DELTOIDS_FOCUS_REPO']).resolve()


def capture_until(marker, timeout=5):
    output = bytearray()
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        ready, _, _ = select.select([fd], [], [], 0.05)
        if ready:
            output.extend(os.read(fd, 65536))
            if marker.encode() in output:
                return bytes(output)
    raise AssertionError(f'No {marker!r} in TUI output: {bytes(output)!r}')


def measure(name, tick=None, quiet=False):
    output = bytearray()
    writes = 0
    before = cpu(p.pid)
    start = time.monotonic()
    last_tick = start - 0.3
    while time.monotonic() - start < 8:
        now = time.monotonic()
        if tick and now - last_tick >= 0.3:
            tick()
            writes += 1
            last_tick = now
        ready, _, _ = select.select([fd], [], [], 0.02)
        if ready:
            output.extend(os.read(fd, 65536))
    duration = time.monotonic() - start
    consumed = cpu(p.pid) - before
    if quiet:
        assert not output, f'{name} emitted {len(output)} bytes'
    result = dict(case=name, seconds=round(duration, 2), cpu_seconds=round(consumed, 2),
                  cpu_percent=round(100 * consumed / duration, 2), writes=writes,
                  frames=output.count(b'\x1b[?25l'), terminal_bytes=len(output))
    assert quiet or not tick or result['frames'] > 0, 'no delivered frames'
    print(json.dumps(result), flush=True)


(repo / 'main.txt').write_text('BENCH_START\n')
print(json.dumps(dict(binary=BIN, repo=str(repo), evidence_root=str(root))), flush=True)
p, fd = launch()
try:
    startup = capture_until('BENCH_START')
    assert b'\x1b[?1004h' in startup
    drain(fd, 1)
    measure('focused idle', quiet=True)
    measure('focused writes every 300ms', lambda: (repo / 'main.txt').write_text(f'{time.monotonic():.9f}\n'))
    drain(fd, 1)
    (repo / 'main.txt').write_text('FINAL_CONTENT_READY\n')
    start = time.monotonic()
    capture_until('FINAL_CONTENT_READY')
    print(json.dumps(dict(case='final content', milliseconds=round(1000 * (time.monotonic() - start), 1))), flush=True)
    drain(fd, 0.5)
    os.write(fd, b'\x1b[O')
    drain(fd, 0.5)
    measure('unfocused writes every 300ms', lambda: (repo / 'main.txt').write_text(f'{time.monotonic():.9f}\n'), quiet=True)
    (repo / 'main.txt').write_text('ZZZZZZZZZZZZZZZZZZZZ\n')
    assert drain(fd, 0.5) == 0
    start = time.monotonic()
    os.write(fd, b'\x1b[I')
    capture_until('ZZZZZZZZZZZZZZZZZZZZ')
    print(json.dumps(dict(case='focus catch-up', milliseconds=round(1000 * (time.monotonic() - start), 1))), flush=True)
    print(json.dumps(dict(result='idle, emitted frames, latest content, hidden deferral, and focus catch-up verified')), flush=True)
finally:
    stop(p, fd)
