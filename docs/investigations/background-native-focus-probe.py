import json
import os
import pathlib
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import termios
import time
import tty


def tmux(*args):
    return subprocess.check_output(['tmux', *args], text=True).strip()


def app_state(bundle=None):
    activation = ''
    if bundle:
        activation = f'''const apps = $.NSRunningApplication.runningApplicationsWithBundleIdentifier({json.dumps(bundle)});
if (Number(apps.count) === 0) throw new Error("Application is not running");
const accepted = apps.objectAtIndex(0).activateWithOptions(2);
$.NSRunLoop.currentRunLoop.runUntilDate($.NSDate.dateWithTimeIntervalSinceNow(0.3));
'''
    source = 'ObjC.import("AppKit");\n' + activation + '''const front = $.NSWorkspace.sharedWorkspace.frontmostApplication;
JSON.stringify({bundle: ObjC.unwrap(front.bundleIdentifier), name: ObjC.unwrap(front.localizedName)});'''
    return json.loads(subprocess.check_output(['osascript', '-l', 'JavaScript', '-e', source], text=True))


def wait_for(predicate, message, seconds=5):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(message)


if len(sys.argv) > 1 and sys.argv[1] == '--child':
    log_path = pathlib.Path(sys.argv[2])
    original = termios.tcgetattr(0)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    try:
        tty.setraw(0)
        os.write(1, b'\x1b[?1004h')
        with log_path.open('ab', buffering=0) as log:
            log_path.with_suffix('.ready').touch()
            pending = b''
            while True:
                data = os.read(0, 1024)
                if not data or b'q' in data:
                    break
                pending += data
                log.write(b''.join(re.findall(rb'\x1b\[[IO]', pending)))
                pending = pending[-2:]
    finally:
        os.write(1, b'\x1b[?1004l')
        termios.tcsetattr(0, termios.TCSADRAIN, original)
    sys.exit(0)


root = pathlib.Path(tempfile.mkdtemp(prefix='deltoids-native-focus-'))
log = root / 'focus.bin'
clients = tmux('list-clients', '-F', '#{client_name}\t#{session_name}\t#{client_termname}').splitlines()
client, original_session, terminal = next(line.split('\t') for line in clients if 'ghostty' in line.lower())
original_app = app_state()
probe_session = f'deltoids-focus-{os.getpid()}'
created = False
try:
    assert tmux('show-options', '-gv', 'focus-events') == 'on'
    command = shlex.join([sys.executable, str(pathlib.Path(__file__).resolve()), '--child', str(log)])
    first_window = tmux('new-session', '-d', '-P', '-F', '#{window_id}', '-s', probe_session, command)
    created = True
    tmux('switch-client', '-c', client, '-t', probe_session)
    assert app_state('com.mitchellh.ghostty')['bundle'] == 'com.mitchellh.ghostty'
    wait_for(lambda: log.with_suffix('.ready').exists(), 'focus recorder did not start')
    # Switching windows is the positive control for tmux's pane forwarding.
    second = tmux('new-window', '-d', '-P', '-F', '#{window_id}', '-t', probe_session, 'cat')
    before = log.stat().st_size
    tmux('select-window', '-t', second)
    wait_for(lambda: b'\x1b[O' in log.read_bytes()[before:], 'window switch did not forward focus loss')
    tmux('select-window', '-t', first_window)
    wait_for(lambda: b'\x1b[I' in log.read_bytes()[before:], 'window return did not forward focus gain')
    before = log.stat().st_size
    away = app_state('com.apple.finder')
    assert away['bundle'] == 'com.apple.finder', away
    wait_for(lambda: b'\x1b[O' in log.read_bytes()[before:], 'native app switch did not forward focus loss')
    returned = app_state('com.mitchellh.ghostty')
    assert returned['bundle'] == 'com.mitchellh.ghostty', returned
    wait_for(lambda: b'\x1b[I' in log.read_bytes()[before:], 'native app return did not forward focus gain')
    print(json.dumps(dict(root=str(root), terminal=terminal, tmux_version=tmux('-V'), original_app=original_app, away=away, returned=returned, native_events=log.read_bytes()[before:].decode(), result='native application focus and tmux window focus forwarded')))
finally:
    if created:
        tmux('switch-client', '-c', client, '-t', original_session)
        tmux('kill-session', '-t', probe_session)
    app_state(original_app['bundle'])
