import json
import os
import pathlib
import select
import subprocess
import time

helpers_path = pathlib.Path(__file__).with_name('background-cpu-probe.py')
helpers = helpers_path.read_text().split('print(json.dumps(dict(root=str(root), binary=BIN)), flush=True)')[0]
exec(compile(helpers, str(helpers_path), 'exec'))

expect_deferred = os.environ.get('DELTOIDS_FOCUS_EXPECT_DEFER', '1') != '0'
if os.environ.get('DELTOIDS_FOCUS_REPO'):
    repo = pathlib.Path(os.environ['DELTOIDS_FOCUS_REPO']).resolve()


def record(reason, trace_id=None):
    command = [BIN, 'write'] + ([trace_id] if trace_id else [])
    result = subprocess.run(command, input=json.dumps(dict(reason=reason, path='trace.txt', content=reason + '\n')), text=True, capture_output=True, cwd=repo, env=env, check=True)
    return json.loads(result.stdout)['traceId']


def capture_until(fd, marker, seconds=5):
    end = time.monotonic() + seconds
    output = bytearray()
    while time.monotonic() < end:
        ready, _, _ = select.select([fd], [], [], min(0.05, max(0, end - time.monotonic())))
        if ready:
            output.extend(os.read(fd, 65536))
            if marker.encode() in output:
                return bytes(output)
    raise AssertionError(f'TUI did not emit {marker!r}: {bytes(output)!r}')


def quiet(seconds=0.6):
    drain(fd, 0.3)
    output = drain(fd, seconds)
    if expect_deferred:
        assert output == 0, f'idle or unfocused TUI emitted {output} bytes'


def measured(name, tick=None, should_be_quiet=False):
    before = cpu(p.pid)
    start = time.monotonic()
    output = drain(fd, 8, tick)
    elapsed = time.monotonic() - start
    consumed = cpu(p.pid) - before
    if should_be_quiet and expect_deferred:
        assert output == 0, f'{name}: emitted {output} terminal bytes'
    print(json.dumps(dict(case=name, seconds=round(elapsed, 2), cpu_seconds=round(consumed, 2), cpu_percent=round(100 * consumed / elapsed, 2), terminal_bytes=output)), flush=True)


config = root / 'config' / 'deltoids'
config.mkdir(parents=True)
(config / 'config.toml').write_text('''[[commands]]
key = "E"
command = "printf 'CHILD_FOCUS_PROBE'; printf 'CHILD_UPDATED_VIEW\\\\n' > {{filename}}"
subprocess = true
''')
trace_id = record('TRACE_INITIAL_HEADER')
(repo / 'main.txt').write_text('STARTING_DIFF_MARKER\n')
print(json.dumps(dict(root=str(root), repo=str(repo), binary=BIN, expect_deferred=expect_deferred)), flush=True)
p, fd = launch()
try:
    startup = capture_until(fd, 'STARTING_DIFF_MARKER')
    if expect_deferred:
        assert b'\x1b[?1004h' in startup, 'focus reporting was not enabled'
    quiet()
    measured('focused Files idle', should_be_quiet=True)
    tracked_tick = lambda: (repo / 'main.txt').write_text(str(time.monotonic()) + '\n')
    measured('focused tracked writes every 300ms', tracked_tick)
    os.write(fd, b'\x1b[O')
    drain(fd, 0.6)
    measured('unfocused tracked writes every 300ms', tracked_tick, should_be_quiet=True)
    (repo / 'main.txt').write_text('RESUMED_FILE_VALUE\n')
    quiet()
    started = time.monotonic()
    os.write(fd, b'\x1b[I')
    if expect_deferred:
        capture_until(fd, 'RESUMED_FILE_VALUE')
    else:
        drain(fd, 0.6)
    print(json.dumps(dict(case='Files focus return', milliseconds=round(1000 * (time.monotonic() - started), 1) if expect_deferred else None)), flush=True)
    quiet()
    os.write(fd, b'E')
    returned = capture_until(fd, 'CHILD_UPDAT')
    if expect_deferred:
        child = returned.index(b'CHILD_FOCUS_PROBE')
        assert b'\x1b[?1004l' in returned[:child], 'focus reporting remained enabled for the child'
        assert b'\x1b[?1004h' in returned[child:], 'focus reporting was not restored'
    quiet()
    os.write(fd, b']')
    capture_until(fd, 'TRACE_INITIAL_HEADER')
    quiet()
    os.write(fd, b'\x1b[O')
    drain(fd, 0.6)
    measured('unfocused local traced writes every 300ms', lambda: record(str(time.monotonic()), trace_id), should_be_quiet=True)
    # Histories record timestamps to whole seconds; make this trace newest.
    quiet(seconds=1.1)
    record('TRACE_FOCUS_RETURN_READY')
    quiet()
    started = time.monotonic()
    os.write(fd, b'\x1b[I')
    if expect_deferred:
        capture_until(fd, 'FOCUS_RETURN')
    else:
        drain(fd, 0.6)
    print(json.dumps(dict(case='Traces focus return', milliseconds=round(1000 * (time.monotonic() - started), 1) if expect_deferred else None)), flush=True)
    quiet()
    os.write(fd, b'q')
    shutdown = bytearray()
    end = time.monotonic() + 3
    while time.monotonic() < end:
        ready, _, _ = select.select([fd], [], [], 0.05)
        if ready:
            try:
                shutdown.extend(os.read(fd, 65536))
            except OSError:
                break
    p.wait(timeout=2)
    if expect_deferred:
        assert b'\x1b[?1004l' in shutdown, 'focus reporting was not disabled at exit'
    result = 'focus deferral, catch-up, idle output, foreground restoration, and exit verified' if expect_deferred else 'baseline measurements complete'
    print(json.dumps(dict(result=result)), flush=True)
finally:
    if p.poll() is None:
        stop(p, fd)
    else:
        os.close(fd)
