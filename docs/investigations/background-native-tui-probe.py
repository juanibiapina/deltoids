import json
import os
import pathlib
import shlex
import subprocess
import sys
import time

if len(sys.argv) > 1 and sys.argv[1] == '--launch':
    gate = pathlib.Path(sys.argv[2])
    deadline = time.monotonic() + 10
    while not gate.exists():
        assert time.monotonic() < deadline, 'launch gate was not opened'
        time.sleep(0.05)
    os.execv(sys.argv[3], [sys.argv[3]])

native_path = pathlib.Path(__file__).with_name('background-native-focus-probe.py')
exec(compile(native_path.read_text().split("if len(sys.argv) > 1 and sys.argv[1] == '--child':")[0], str(native_path), 'exec'))
helpers_path = pathlib.Path(__file__).with_name('background-cpu-probe.py')
exec(compile(helpers_path.read_text().split('print(json.dumps(dict(root=str(root), binary=BIN)), flush=True)')[0], str(helpers_path), 'exec'))
if os.environ.get('DELTOIDS_FOCUS_REPO'):
    repo = pathlib.Path(os.environ['DELTOIDS_FOCUS_REPO']).resolve()

clients = tmux('list-clients', '-F', '#{client_name}\t#{session_name}\t#{client_termname}').splitlines()
client, original_session, terminal = next(line.split('\t') for line in clients if 'ghostty' in line.lower())
original_app = app_state()
probe_session = f'deltoids-native-tui-{os.getpid()}'
log = root / 'terminal.bin'
log.touch()
gate = root / 'launch'
created = False
sampler = None


def screen():
    return tmux('capture-pane', '-p', '-t', pane)


def edits(seconds):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        (repo / 'main.txt').write_text(str(time.monotonic()) + '\n')
        time.sleep(min(0.3, max(0, end - time.monotonic())))


def measured(name):
    before = cpu(pid)
    output_before = log.stat().st_size
    start = time.monotonic()
    edits(8)
    elapsed = time.monotonic() - start
    consumed = cpu(pid) - before
    output = log.stat().st_size - output_before
    result = dict(case=name, seconds=round(elapsed, 2), cpu_seconds=round(consumed, 2), cpu_percent=round(100 * consumed / elapsed, 2), terminal_bytes=output)
    print(json.dumps(result), flush=True)
    return result


try:
    (repo / 'main.txt').write_text('INITIAL_NATIVE_DIFF\n')
    launch = shlex.join(['env', 'XDG_DATA_HOME=' + env['XDG_DATA_HOME'], 'XDG_CONFIG_HOME=' + env['XDG_CONFIG_HOME'], sys.executable, str(pathlib.Path(__file__).resolve()), '--launch', str(gate), BIN])
    pane = tmux('new-session', '-d', '-P', '-F', '#{pane_id}', '-c', str(repo), '-s', probe_session, launch)
    created = True
    tmux('pipe-pane', '-o', '-t', pane, 'cat >> ' + shlex.quote(str(log)))
    tmux('switch-client', '-c', client, '-t', probe_session)
    assert app_state('com.mitchellh.ghostty')['bundle'] == 'com.mitchellh.ghostty'
    gate.touch()
    wait_for(lambda: 'INITIAL_NATIVE_DIFF' in screen(), 'native TUI did not load', seconds=10)
    pid = int(tmux('display-message', '-p', '-t', pane, '#{pane_pid}'))
    time.sleep(0.6)
    print(json.dumps(dict(root=str(root), repo=str(repo), pid=pid, terminal=terminal, original_app=original_app)), flush=True)
    measured('native Ghostty focused tracked writes')
    sample_path = root / 'active-edit.sample.txt'
    sampler = subprocess.Popen(['sample', str(pid), '5', '-file', str(sample_path)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    edits(6)
    _, sample_error = sampler.communicate(timeout=5)
    assert sampler.returncode == 0, sample_error.decode()
    away = app_state('com.apple.finder')
    assert away['bundle'] == 'com.apple.finder', away
    time.sleep(0.6)
    hidden = measured('native Finder foreground tracked writes')
    assert hidden['terminal_bytes'] == 0, hidden
    (repo / 'main.txt').write_text('NATIVE_RETURN_READY\n')
    time.sleep(0.6)
    assert 'NATIVE_RETURN_READY' not in screen(), 'hidden TUI refreshed before native focus returned'
    started = time.monotonic()
    returned = app_state('com.mitchellh.ghostty')
    assert returned['bundle'] == 'com.mitchellh.ghostty', returned
    wait_for(lambda: 'NATIVE_RETURN_READY' in screen(), 'native focus return did not refresh the actual TUI')
    print(json.dumps(dict(case='native focus return', activation_request_to_frame_ms=round(1000 * (time.monotonic() - started), 1), sample=str(sample_path), result='actual Ghostty/tmux TUI defers background edits and catches up after native application activation')), flush=True)
finally:
    if sampler and sampler.poll() is None:
        sampler.terminate()
        sampler.wait(timeout=5)
    if created:
        tmux('switch-client', '-c', client, '-t', original_session)
        tmux('kill-session', '-t', probe_session)
    app_state(original_app['bundle'])
