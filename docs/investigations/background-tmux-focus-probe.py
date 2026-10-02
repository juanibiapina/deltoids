import os, sys, pty, termios, tty, struct, fcntl, subprocess, pathlib, tempfile, time, select, shlex, json
if len(sys.argv) > 1 and sys.argv[1] == '--child':
    tty.setraw(0)
    os.write(1, b'\x1b[?1004h')
    with open(sys.argv[2], 'ab', buffering=0) as log:
        while True:
            data = os.read(0, 1024)
            if not data:
                break
            log.write(data)
    sys.exit(0)
root = pathlib.Path(tempfile.mkdtemp(prefix='deltoids-focus-'))
socket = str(root / 'tmux.sock')
env = dict(os.environ, TERM='xterm-256color')
env.pop('TMUX', None)
def tmux(*args, check=True):
    return subprocess.run(['tmux', '-S', socket, '-f', '/dev/null', *args], env=env, capture_output=True, text=True, check=check)
client = None
master = None
def drain(seconds):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        ready, _, _ = select.select([master], [], [], 0.05)
        if ready:
            try:
                os.read(master, 65536)
            except OSError:
                break
try:
    child = lambda name: shlex.join([sys.executable, __file__, '--child', str(root / name)])
    tmux('new-session', '-d', '-s', 'probe', '-n', 'one', child('one'))
    tmux('set-option', '-g', 'focus-events', 'on')
    tmux('new-window', '-d', '-t', 'probe', '-n', 'two', child('two'))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
    def setup():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    client = subprocess.Popen(['tmux', '-S', socket, 'attach-session', '-t', 'probe'], stdin=slave, stdout=slave, stderr=slave, env=env, preexec_fn=setup)
    os.close(slave)
    drain(0.6)
    tmux('select-window', '-t', 'probe:two')
    drain(0.4)
    os.write(master, b'\x1b[O')
    drain(0.4)
    os.write(master, b'\x1b[I')
    drain(0.4)
    tmux('select-window', '-t', 'probe:one')
    drain(0.4)
    one = (root / 'one').read_bytes()
    two = (root / 'two').read_bytes()
    assert b'\x1b[O' in one and b'\x1b[I' in one
    assert b'\x1b[O' in two and b'\x1b[I' in two
    print(json.dumps(dict(root=str(root), first_window=one.decode(), second_window=two.decode(), tmux_version=tmux('-V').stdout.strip())))
finally:
    tmux('kill-server', check=False)
    if client:
        client.wait(timeout=3)
    if master is not None:
        os.close(master)
