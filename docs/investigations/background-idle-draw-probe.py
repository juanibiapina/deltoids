import json
import os
import pathlib
import time

helpers_path = pathlib.Path(__file__).with_name('background-cpu-probe.py')
helpers = helpers_path.read_text().split('print(json.dumps(dict(root=str(root), binary=BIN)), flush=True)')[0]
exec(compile(helpers, str(helpers_path), 'exec'))
if os.environ.get('DELTOIDS_FOCUS_REPO'):
    repo = pathlib.Path(os.environ['DELTOIDS_FOCUS_REPO']).resolve()

p, fd = launch()
try:
    drain(fd, 3)
    before = cpu(p.pid)
    started = time.monotonic()
    output = drain(fd, 60)
    elapsed = time.monotonic() - started
    consumed = cpu(p.pid) - before
    assert output == 0, f'settled idle emitted {output} bytes'
    print(json.dumps(dict(repo=str(repo), seconds=round(elapsed, 2), cpu_seconds=round(consumed, 2), cpu_percent=round(100 * consumed / elapsed, 2), terminal_bytes=output)), flush=True)
finally:
    stop(p, fd)
