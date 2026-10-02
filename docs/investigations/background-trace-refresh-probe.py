import json
import os
import pathlib
import select
import shutil
import subprocess
import time

helpers_path = pathlib.Path(__file__).with_name('background-cpu-probe.py')
helpers = helpers_path.read_text().split('print(json.dumps(dict(root=str(root), binary=BIN)), flush=True)')[0]
exec(compile(helpers, str(helpers_path), 'exec'))


def record(cwd, marker, trace_id=None):
    command = [BIN, 'write'] + ([trace_id] if trace_id else [])
    result = subprocess.run(command, input=json.dumps(dict(reason=marker, path=marker + '.txt', content=marker + '\n')), text=True, capture_output=True, cwd=cwd, env=env, check=True)
    return json.loads(result.stdout)['traceId']


def capture_until(fd, marker, seconds=5):
    end = time.monotonic() + seconds
    output = bytearray()
    while time.monotonic() < end:
        ready, _, _ = select.select([fd], [], [], min(0.05, max(0, end - time.monotonic())))
        if ready:
            output.extend(os.read(fd, 65536))
            if marker.encode() in output:
                return
    raise AssertionError(f'TUI did not display {marker!r}: {bytes(output)!r}')


other = root / 'other'
other.mkdir()
record(repo, 'LOCAL_INITIAL_READY')
p, fd = launch()
try:
    capture_until(fd, 'Files')
    os.write(fd, b']')
    capture_until(fd, 'LOCAL_INITIAL_READY')
    record(other, 'OTHER_REFRESH_READY')
    drain(fd, 1)
    new_id = record(repo, 'LOCAL_NEW_TRACE_READY')
    capture_until(fd, 'NEW_TRACE_READY')
    record(repo, 'LOCAL_APPEND_READY', new_id)
    capture_until(fd, 'LOCAL_APPEND_READY.txt')
    shutil.rmtree(root / 'data' / 'edit' / 'traces' / new_id)
    capture_until(fd, 'INITIAL')
    print(json.dumps(dict(root=str(root), result='initial trace, new local trace, append, and deletion refresh without input')))
finally:
    stop(p, fd)
