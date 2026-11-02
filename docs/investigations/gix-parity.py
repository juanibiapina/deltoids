#!/usr/bin/env python3
"""Build git fixtures once, then compare `gitdump` outputs of two builds.

usage: parity.py make ROOT
       parity.py run ROOT BIN OUT.json
       parity.py diff A.json B.json
"""
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

env = dict(os.environ, GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull,
           GIT_AUTHOR_NAME='Probe', GIT_AUTHOR_EMAIL='probe@example.test',
           GIT_COMMITTER_NAME='Probe', GIT_COMMITTER_EMAIL='probe@example.test',
           GIT_AUTHOR_DATE='2026-01-01T00:00:00Z', GIT_COMMITTER_DATE='2026-01-01T00:00:00Z')


def git(repo, *args, check=True, input=None):
    return subprocess.run(['git', '-C', str(repo), *args], env=env,
                          capture_output=True, check=check, input=input)


a = ''.join(f'a line {i:02d}\n' for i in range(30))
b = ''.join(f'b other {i:02d}\n' for i in range(30))


def write(repo, path, text):
    p = repo / path
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text)


def make(root):
    root = Path(root)
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    base = root / '_base'
    base.mkdir()
    git(base, 'init', '-q', '-b', 'main')
    write(base, 'a.txt', a)
    write(base, 'b.txt', b)
    write(base, '.gitignore', 'ignored/\n')
    git(base, 'add', '.')
    git(base, 'commit', '-qm', 'base')
    cases = []

    def case(name, setup, unborn=False):
        repo = root / name
        if unborn:
            repo.mkdir()
            git(repo, 'init', '-q', '-b', 'main')
        else:
            shutil.copytree(base, repo, symlinks=True)
        try:
            setup(repo)
        except OSError as error:
            if error.errno == 92:
                shutil.rmtree(repo)
                return
            raise
        git(repo, 'update-index', '--refresh', check=False)
        cases.append(name)

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
        write(repo, 'dir/untracked.txt', 'untracked\n')
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
        (repo / 'a.txt').unlink()
        git(repo, 'checkout', '--', 'a.txt', check=False)
        write(repo, 'a.txt', a)
        git(repo, 'add', '.gitattributes', 'a.txt')
        git(repo, 'commit', '-qm', 'filtered')
        write(repo, 'a.txt', a + 'filter edit\n')

    def crlf(repo):
        git(repo, 'config', 'core.autocrlf', 'true')
        write(repo, 'a.txt', a.replace('\n', '\r\n') + 'crlf edit\r\n')

    def submodule(repo):
        inner = root / '_inner'
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
        for i in range(300):
            write(repo, f'bulk-{i:04d}.txt', f'initial {i}\n')
        git(repo, 'add', '.')
        git(repo, 'commit', '-qm', 'many files')
        for i in range(300):
            write(repo, f'bulk-{i:04d}.txt', f'initial {i}\nstaged\n')
        git(repo, 'add', '.')
        for i in range(300):
            write(repo, f'bulk-{i:04d}.txt', f'initial {i}\nstaged\nunstaged\n')

    def history(repo):
        write(repo, 'a.txt', a + 'second\n')
        git(repo, 'commit', '-qam', 'second\n\nbody text\n')
        git(repo, 'checkout', '-qb', 'feature')
        write(repo, 'f.txt', 'feature\n')
        git(repo, 'add', '.')
        git(repo, 'commit', '-qm', 'feature file')
        (repo / 'b.txt').rename(repo / 'b2.txt')
        os.chmod(repo / 'f.txt', 0o755)
        write(repo, 'nonl.txt', 'no newline')
        (repo / 'bin.dat').write_bytes(b'\x00\x01\x02')
        (repo / 'link').symlink_to('a.txt')
        write(repo, 'empty.txt', '')
        git(repo, 'add', '-A')
        git(repo, 'commit', '-qm', 'rename, chmod, binary, symlink')
        git(repo, 'checkout', '-q', 'main')
        write(repo, 'm.txt', 'main\n')
        git(repo, 'add', '.')
        git(repo, 'commit', '-qm', 'main file')
        git(repo, 'merge', '-q', '--no-edit', 'feature')
        write(repo, 'nonl.txt', 'no newline either')
        git(repo, 'commit', '-qam', 'edit without newline')
        git(repo, 'rm', '-q', 'bin.dat')
        git(repo, 'commit', '-qm', 'delete binary')

    def gitbutler_conflict(repo):
        tree = git(repo, 'rev-parse', 'HEAD^{tree}').stdout.decode().strip()
        write(repo, 'a.txt', a + 'resolved\n')
        git(repo, 'add', 'a.txt')
        resolved = git(repo, 'write-tree').stdout.decode().strip()
        git(repo, 'reset', '-q')
        write(repo, 'a.txt', a)
        listing = f'040000 tree {resolved}\t.auto-resolution\n040000 tree {tree}\t.conflict-base-0\n'
        outer = git(repo, 'mktree', input=listing.encode()).stdout.decode().strip()
        parent = git(repo, 'rev-parse', 'HEAD').stdout.decode().strip()
        body = (f'tree {outer}\nparent {parent}\n'
                'author Probe <probe@example.test> 1767225600 +0000\n'
                'committer Probe <probe@example.test> 1767225600 +0000\n'
                'gitbutler-conflicted 1\n\nconflicted\n\nGitButler-Conflict: yes\n')
        cid = git(repo, 'hash-object', '-t', 'commit', '-w', '--stdin', input=body.encode()).stdout.decode().strip()
        git(repo, 'update-ref', 'refs/heads/main', cid)
        git(repo, 'reset', '-q', '--hard')

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
    case('crlf', crlf)
    case('submodule', submodule)
    case('many-changed-files', many_changes)
    case('chmod', lambda r: os.chmod(r / 'a.txt', 0o755))
    case('no-trailing-newline', lambda r: write(r, 'a.txt', a + 'tail'))
    case('history', history)
    case('gitbutler-conflict', gitbutler_conflict)
    case('damaged-head', lambda r: write(r, '.git/refs/heads/main', 'not-an-object-id\n'))
    case('non-utf8-path', lambda r: (r / os.fsdecode(b'bad-\xff.txt')).write_bytes(b'new\n'))
    (root / 'cases.json').write_text(json.dumps(cases))
    print(f'{len(cases)} cases in {root}')


def run(root, binary, out):
    root = Path(root)
    cases = json.loads((root / 'cases.json').read_text())
    results = {}
    for name in cases:
        r = subprocess.run([binary, str(root / name)], env=env, capture_output=True)
        text = r.stdout.decode('utf-8', 'replace')
        try:
            results[name] = json.loads(text)
        except json.JSONDecodeError:
            results[name] = {'crash': r.returncode, 'stderr': r.stderr.decode('utf-8', 'replace')[-2000:]}
    Path(out).write_text(json.dumps(results, indent=1))


INDEX = re.compile(r'^(index )([0-9a-f]{7,40})\.\.([0-9a-f]{7,40})', re.M)


def norm(v):
    if isinstance(v, str):
        return INDEX.sub(lambda m: f'{m[1]}{m[2][:7]}..{m[3][:7]}', v)
    if isinstance(v, list):
        return [norm(x) for x in v]
    if isinstance(v, dict):
        return {k: norm(x) for k, x in v.items()}
    return v


def diff(a_path, b_path):
    A = json.loads(Path(a_path).read_text())
    B = json.loads(Path(b_path).read_text())
    bad = 0
    for name in A:
        x, y = norm(A[name]), norm(B.get(name))
        if x == y:
            continue
        bad += 1
        print(f'=== {name}')
        if not isinstance(y, dict) or 'crash' in y:
            print('  ', y)
            continue
        for k in sorted(set(x) | set(y)):
            if x.get(k) != y.get(k):
                print(f'  -- {k}')
                print('     base:', json.dumps(x.get(k))[:1500])
                print('     new: ', json.dumps(y.get(k))[:1500])
    print(f'{len(A) - bad}/{len(A)} cases equal')


if __name__ == '__main__':
    cmd = sys.argv[1]
    if cmd == 'make':
        make(sys.argv[2])
    elif cmd == 'run':
        run(*sys.argv[2:5])
    elif cmd == 'diff':
        diff(sys.argv[2], sys.argv[3])
