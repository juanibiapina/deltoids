import pathlib
helpers = pathlib.Path(__file__).with_name('background-cpu-probe.py').read_text().split("print(json.dumps(dict(root=str(root), binary=BIN)), flush=True)")[0]
exec(compile(helpers, str(pathlib.Path(__file__).with_name('background-cpu-probe.py')), 'exec'))
other = root / 'other-project'
other.mkdir()
def write_trace(cwd, trace_id=None):
    command = [BIN, 'write'] + ([trace_id] if trace_id else [])
    result = subprocess.run(command, input=json.dumps(dict(reason='CPU reproduction', path='file.txt', content=str(time.monotonic())+'\n')), text=True, capture_output=True, cwd=cwd, env=env, check=True)
    return json.loads(result.stdout)['traceId']
local_id = write_trace(repo)
other_id = write_trace(other)
trace_root = root / 'data' / 'edit' / 'traces'
line = (trace_root / other_id / 'entries.jsonl').read_text().splitlines()[0]
for number in range(500):
    folder = trace_root / f'fixture-{number:04}'
    folder.mkdir()
    (folder / 'entries.jsonl').write_text((line+'\n')*100)
print(json.dumps(dict(root=str(root), historical_entries=50000)), flush=True)
p, fd = launch()
drain(fd, 2)
os.write(fd, b']')
drain(fd, 2)
measure('Traces idle with 50000 unrelated history entries', p, fd)
measure('other project ordinary file writes', p, fd, lambda: (other / 'file.txt').write_text(str(time.monotonic())+'\n'))
measure('other project traced writes', p, fd, lambda: write_trace(other, other_id))
stop(p, fd)
