import os, pty, fcntl, termios, struct, subprocess, tempfile, pathlib, time, select, json

BIN = os.environ.get('DELTOIDS_CPU_BINARY', str(pathlib.Path(__file__).resolve().parents[2] / 'target/release/deltoids'))
root = pathlib.Path(tempfile.mkdtemp(prefix='deltoids-cpu-'))
repo = root / 'repo'
repo.mkdir()
env = dict(os.environ, XDG_DATA_HOME=str(root / 'data'), XDG_CONFIG_HOME=str(root / 'config'), TERM='xterm-256color')
def git(*args):
    subprocess.run(['git', '-C', str(repo), *args], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
git('init')
(repo / '.gitignore').write_text('ignored/\n')
(repo / 'main.txt').write_text('initial\n')
git('add', '.')
git('-c', 'user.name=Probe', '-c', 'user.email=probe@example.com', 'commit', '-m', 'fixture')

def cpu(pid):
    value = subprocess.check_output(['ps', '-p', str(pid), '-o', 'time='], text=True).strip()
    minutes, seconds = value.split(':')
    return int(minutes) * 60 + float(seconds)

def launch():
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
    def setup():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    process = subprocess.Popen([BIN], stdin=slave, stdout=slave, stderr=slave, cwd=repo, env=env, preexec_fn=setup)
    os.close(slave)
    return process, master

def drain(master, duration, tick=None):
    end = time.monotonic() + duration
    count = 0
    last_tick = 0
    while time.monotonic() < end:
        now = time.monotonic()
        if tick and now - last_tick >= 0.3:
            tick()
            last_tick = now
        ready, _, _ = select.select([master], [], [], min(0.05, max(0, end-now)))
        if ready:
            try:
                count += len(os.read(master, 65536))
            except OSError:
                break
    return count

def measure(name, process, master, tick=None):
    before = cpu(process.pid)
    start = time.monotonic()
    output = drain(master, 8, tick)
    duration = time.monotonic() - start
    seconds = cpu(process.pid) - before
    print(json.dumps(dict(case=name, pid=process.pid, seconds=round(duration, 2), cpu_seconds=round(seconds, 2), cpu_percent=round(100*seconds/duration, 2), terminal_bytes=output)), flush=True)

def stop(process, master):
    os.write(master, b'q')
    drain(master, 0.5)
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.terminate()
        process.wait(timeout=2)
    os.close(master)

print(json.dumps(dict(root=str(root), binary=BIN)), flush=True)
p, fd = launch()
drain(fd, 2)
measure('small clean idle', p, fd)
stop(p, fd)
for directory in range(100):
    folder = repo / f'd{directory:03}'
    folder.mkdir()
    for number in range(300):
        (folder / f'f{number:03}.txt').write_text('unchanged\n')
git('add', '.')
git('-c', 'user.name=Probe', '-c', 'user.email=probe@example.com', 'commit', '-m', 'large fixture')
p, fd = launch()
drain(fd, 2)
measure('30000 tracked files clean idle', p, fd)
os.write(fd, b'\x1b[O')
drain(fd, 0.5)
measure('30000 tracked files focus lost', p, fd)
ignored = repo / 'ignored'
ignored.mkdir()
measure('ignored writes every 300ms', p, fd, lambda: (ignored / 'out.txt').write_text(str(time.monotonic())))
measure('tracked writes every 300ms', p, fd, lambda: (repo / 'main.txt').write_text(str(time.monotonic())+'\n'))
stop(p, fd)
