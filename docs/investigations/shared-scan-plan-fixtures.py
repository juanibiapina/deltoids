#!/usr/bin/env python3
"""Compare the plan probe with current repository methods on real Git fixtures."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

probe = Path(sys.argv[1] if len(sys.argv) > 1 else '/tmp/deltoids-shared-scan-plan-probe').resolve()
root = Path(tempfile.mkdtemp(prefix='deltoids-shared-scan-review-'))
env = dict(os.environ, GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull,
           GIT_AUTHOR_NAME='Probe', GIT_AUTHOR_EMAIL='probe@example.test',
           GIT_COMMITTER_NAME='Probe', GIT_COMMITTER_EMAIL='probe@example.test')

def git(repo, *args, check=True):
    return subprocess.run(['git', '-C', str(repo), *args], env=env,
                          capture_output=True, check=check)

base = root / 'base'
base.mkdir()
git(base, 'init', '-q', '-b', 'main')
a = ''.join(f'a line {i:02d}\n' for i in range(30))
b = ''.join(f'b other {i:02d}\n' for i in range(30))
(base / 'a.txt').write_text(a)
(base / 'b.txt').write_text(b)
(base / '.gitignore').write_text('ignored/\n')
git(base, 'add', '.')
git(base, 'commit', '-qm', 'base')
results = []

def case(name, setup, *, unborn=False, mode=None):
    repo = root / name
    if unborn:
        repo.mkdir()
        git(repo, 'init', '-q', '-b', 'main')
    else:
        shutil.copytree(base, repo, symlinks=True)
    try:
        setup(repo)
    except OSError as error:
        if name != 'non-utf8-path' or error.errno != 92:
            raise
        record = {'case': name, 'passed': None, 'skipped': 'filesystem rejects invalid UTF-8 filenames (EILSEQ)'}
        results.append(record)
        print(json.dumps(record), flush=True)
        return
    git(repo, 'update-index', '--refresh', check=False)
    command = [str(probe), str(repo)]
    if mode:
        command += ['0', mode]
    result = subprocess.run(command, env=env, capture_output=True, text=True)
    record = {'case': name, 'passed': result.returncode == 0,
              'stdout': result.stdout.strip(), 'stderr': result.stderr.strip()}
    results.append(record)
    print(json.dumps(record), flush=True)

def write(repo, path, text):
    (repo / path).write_text(text)

def staged(repo):
    write(repo, 'a.txt', a + 'staged\n')
    git(repo, 'add', 'a.txt')

def staged_then_edited(repo):
    staged(repo)
    write(repo, 'a.txt', a + 'staged\nunstaged\n')

def cancelled(repo):
    staged(repo)
    write(repo, 'a.txt', a)

def rename(repo, staged=False, edited=False):
    (repo / 'a.txt').rename(repo / 'renamed.txt')
    if edited:
        write(repo, 'renamed.txt', a + 'edit\n')
    if staged:
        git(repo, 'add', '-A')

def chain(repo):
    rename(repo, staged=True)
    (repo / 'renamed.txt').rename(repo / 'final.txt')

def recreated(repo):
    git(repo, 'rm', '-q', 'a.txt')
    write(repo, 'a.txt', a + 'new workdir\n')

def untracked(repo):
    (repo / 'dir').mkdir()
    write(repo, 'dir/untracked.txt', 'untracked\n')
    (repo / 'ignored').mkdir()
    write(repo, 'ignored/skipped.txt', 'ignored\n')

def add_then_delete(repo):
    write(repo, 'new.txt', 'new\n')
    git(repo, 'add', 'new.txt')
    (repo / 'new.txt').unlink()

def type_change(repo, staged=False):
    (repo / 'a.txt').unlink()
    (repo / 'a.txt').symlink_to('b.txt')
    if staged:
        git(repo, 'add', 'a.txt')

def binary(repo):
    (repo / 'a.txt').write_bytes(b'\x00binary\xff\n')

def config_rename(repo, value):
    git(repo, 'config', 'diff.renames', value)
    rename(repo, staged=True)

def case_rename(repo):
    git(repo, 'config', 'core.ignorecase', 'true')
    (repo / 'a.txt').rename(repo / 'temp.txt')
    (repo / 'temp.txt').rename(repo / 'A.txt')

def conflict(repo):
    git(repo, 'checkout', '-qb', 'other')
    write(repo, 'a.txt', 'other side\n')
    git(repo, 'commit', '-qam', 'other')
    git(repo, 'checkout', '-q', 'main')
    write(repo, 'a.txt', 'main side\n')
    git(repo, 'commit', '-qam', 'main')
    git(repo, 'merge', 'other', check=False)

def filtered(repo):
    write(repo, '.gitattributes', 'a.txt filter=rot13\n')
    git(repo, 'config', 'filter.rot13.clean', "tr 'A-Za-z' 'N-ZA-Mn-za-m'")
    git(repo, 'config', 'filter.rot13.smudge', "tr 'A-Za-z' 'N-ZA-Mn-za-m'")
    git(repo, 'add', '.gitattributes', 'a.txt')
    git(repo, 'commit', '-qm', 'filtered')
    write(repo, 'a.txt', a + 'filter edit\n')

def submodule(repo):
    inner = root / 'inner'
    if not inner.exists():
        inner.mkdir()
        git(inner, 'init', '-q', '-b', 'main')
        write(inner, 'file', 'v1\n')
        git(inner, 'add', '.')
        git(inner, 'commit', '-qm', 'v1')
    git(repo, '-c', 'protocol.file.allow=always', 'submodule', 'add', '-q', str(inner), 'sub')
    git(repo, 'commit', '-qm', 'submodule')
    write(repo / 'sub', 'file', 'v2\n')
    git(repo / 'sub', 'commit', '-qam', 'v2')

def many_changes(repo):
    for i in range(1000):
        write(repo, f'bulk-{i:04d}.txt', f'initial {i}\n')
    git(repo, 'add', '.')
    git(repo, 'commit', '-qm', 'many files')
    for i in range(1000):
        write(repo, f'bulk-{i:04d}.txt', f'initial {i}\nstaged\n')
    git(repo, 'add', '.')
    for i in range(1000):
        write(repo, f'bulk-{i:04d}.txt', f'initial {i}\nstaged\nunstaged\n')

case('clean', lambda r: None)
case('unborn-untracked', lambda r: write(r, 'new.txt', 'new\n'), unborn=True)
case('unborn-staged', lambda r: (write(r, 'new.txt', 'new\n'), git(r, 'add', '.')), unborn=True)
case('detached', lambda r: (git(r, 'checkout', '-q', '--detach'), write(r, 'a.txt', a + 'edit\n')))
case('unstaged', lambda r: write(r, 'a.txt', a + 'edit\n'))
case('staged', staged)
case('both-columns', staged_then_edited)
case('cancelled-net-patch', cancelled)
case('untracked-and-ignored', untracked)
case('unstaged-delete', lambda r: (r / 'a.txt').unlink())
case('staged-delete', lambda r: git(r, 'rm', '-q', 'a.txt'))
case('staged-delete-recreated', recreated)
case('added-then-deleted', add_then_delete)
case('unstaged-rename', rename)
case('unstaged-edited-rename', lambda r: rename(r, edited=True))
case('staged-rename', lambda r: rename(r, staged=True))
case('staged-edited-rename', lambda r: rename(r, staged=True, edited=True))
case('chained-rename', chain)
case('rename-config-false', lambda r: config_rename(r, 'false'))
case('rename-config-copies', lambda r: config_rename(r, 'copies'))
case('case-only-rename', case_rename)
case('unstaged-type-change', type_change)
case('staged-type-change', lambda r: type_change(r, staged=True))
case('binary', binary)
case('conflict', conflict)
case('filtered', filtered)
case('submodule', submodule)
case('many-changed-files', many_changes)
case('retained-handle-staging-only', lambda r: write(r, 'a.txt', a + 'edit\n'), mode='refresh')
case('damaged-head', lambda r: write(r, '.git/refs/heads/main', 'not-an-object-id\n'), mode='head-error')
case('non-utf8-path', lambda r: (r / os.fsdecode(b'bad-\xff.txt')).write_bytes(b'new\n'))
(root / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
print(json.dumps({'root': str(root), 'passed': sum(r['passed'] is True for r in results),
                  'skipped': sum(r['passed'] is None for r in results), 'total': len(results)}))
sys.exit(0 if all(r['passed'] is not False for r in results) else 1)
