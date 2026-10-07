#!/usr/bin/env bash
# Regenerate the Kao capture archives used by tests/record_cli.rs.
# Requires `kao` and Git on PATH. Run from anywhere.
set -euo pipefail

out="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

git_commit() {
  git add -A
  git -c user.name=fixture -c user.email=fixture@example.invalid commit -qm fixture
}

# changes.tar: every kind of change Kao captures, from a command that exits 7.
repo="$work/changes"
mkdir -p "$repo/src" && cd "$repo" && git init -q
printf 'fn main() {\n    let x = 1;\n}\n' > src/app.rs
printf 'old\r\nnext\r\n' > crlf.txt
printf 'gone\n' > deleted.txt
printf '\000old\377\n' > binary.dat
printf '#!/bin/sh\n' > script.sh
printf 'odd\n' > 'we*ird[1].txt'
ln -s src/app.rs link
git_commit
printf 'fn main() {\n    let x = 2;\n}\n' > src/app.rs
kao run -- bash -c '
  printf "fn main() {\n    let x = 3;\n    let y = 4;\n}\n" > src/app.rs
  printf "new\r\nnext" > crlf.txt
  rm deleted.txt
  printf "created\nno newline" > created.txt
  : > empty.txt
  printf "\000new\377" > binary.dat
  chmod +x script.sh
  printf "x\n" > "we*ird[1].txt"
  ln -sfn crlf.txt link
  exit 7
' 3>"$out/changes.tar" || true

# incomplete.tar: an embedded repository makes the capture incomplete.
repo="$work/incomplete"
mkdir -p "$repo" && cd "$repo" && git init -q
printf 'a\n' > notes.txt
git_commit
kao run -- bash -c '
  printf "b\n" > notes.txt
  git init -q nested && touch nested/x
  git -C nested add x
  git -C nested -c user.name=f -c user.email=f@x commit -qm x
' 3>"$out/incomplete.tar" 2>/dev/null || true

# nochange.tar: a read-only command.
cd "$work/changes"
kao run -- true 3>"$out/nochange.tar"
