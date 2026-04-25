# Landing-page screenshots

Recipes for reproducing the four images under `public/screenshots/`. They drive
the comparison and section figures on `index.mdx`.

| File | Purpose | Capture style |
| ---- | ------- | ------------- |
| `delta.png` | "Default diff" half of the hero comparison. | `git show \| delta` → `freeze` |
| `deltoids.png` | "Whole enclosing function" half of the hero comparison. | `git show \| deltoids` → `freeze` |
| `lazygit.png` | Lazygit with `deltoids` as its pager (Use it anywhere section). | `tmux capture-pane` of a real `lazygit` → `freeze` |
| `edit-tui.png` | `edit-tui` browsing an agent trace (Coding agents section). | `tmux capture-pane` of a real `edit-tui` → `freeze` |

## Tooling

Install once:

```bash
brew install charmbracelet/tap/freeze   # ANSI → PNG renderer
brew install imagemagick                # post-resize / canvas extend
brew install lazygit                    # only for lazygit.png
# tmux is assumed (system or homebrew)
```

The recipes assume you're sitting at the repo root.

## Common rendering settings

All `freeze` invocations on this page use the same flags so window chrome
and font size match across screenshots:

```
freeze --language ansi --window \
       --margin 20 --padding 20 \
       --font.size 14 --line-height 1.4 \
       -o <out>.png < <input>.ansi
```

## 1 / 2. Hero comparison: `delta.png` and `deltoids.png`

Both render the same single hunk from commit
`09b3dff` (`fix: diff not showing when adding line at end of file`),
limited to `crates/deltoids/src/scope.rs` and trimmed to the first hunk so the
function-context expansion is the only visible difference.

```bash
mkdir -p /tmp/freeze-work
cd /tmp/freeze-work

# Strip the commit header (--format="") and keep just the first @@ hunk so
# the comparison stays focused on `collect_insert_lines`.
( cd ~/workspace/juanibiapina/deltoids \
  && git show --format="" 09b3dff -- crates/deltoids/src/scope.rs ) \
  | awk '/^@@/ { hunks++ } hunks <= 1 { print }' > scope-hunk1.diff

# delta keeps the user's git config (delta theme etc.); silence its bat
# warning with 2>/dev/null.
cat scope-hunk1.diff | delta --color-only 2>/dev/null > delta.ansi

# deltoids must run inside the repo for blob lookups via the `index` line.
( cd ~/workspace/juanibiapina/deltoids \
  && cat /tmp/freeze-work/scope-hunk1.diff | deltoids ) > deltoids.ansi

# deltoids also expands a second hunk it derives from the blob diff (the new
# test function); trim to the first scope block.
head -n 54 deltoids.ansi > deltoids-trim.ansi   # ends at closing `}` of collect_insert_lines

# Pad delta input to ~120 chars so freeze auto-sizes its window to the same
# width deltoids gets from its 120-char horizontal rule. Without this the
# two PNGs come out slightly different widths and CSS scales the window
# chrome unequally.
( printf '%120s\n' ""; cat delta.ansi ) > delta-padded.ansi

flags="--language ansi --window --margin 20 --padding 20 --font.size 14 --line-height 1.4"
freeze $flags -o delta-natural.png    < delta-padded.ansi    # 4353 x 1776
freeze $flags -o deltoids-natural.png < deltoids-trim.ansi   # 4920 x 4678 (or similar)

# Width-equalize via centered canvas extend.
magick deltoids-natural.png -background none -gravity center -extent 4353x4678 deltoids.png
cp delta-natural.png delta.png

# Drop into the website
cp delta.png    ~/workspace/juanibiapina/deltoids/website/public/screenshots/
cp deltoids.png ~/workspace/juanibiapina/deltoids/website/public/screenshots/
```

Final dimensions: both at 4353 wide. Heights differ on purpose (delta is
shorter; the contrast is the point).

## 3. `lazygit.png`

Captures real lazygit (with the user's tokyonight theme) on a temporary
worktree of commit `07b780c` (`fix: keep new sibling pair lines in enclosing
hunk`). The command log is hidden and `deltoids` is forced as the pager via a
layered config override. Wide tmux geometry (220 cols) keeps the right edge
of the diff pane intact.

```bash
# Worktree at the parent of the chosen commit, then re-apply the patch so
# the file shows up as an unstaged modification.
cd ~/workspace/juanibiapina/deltoids
git worktree add /tmp/lzg-shot 07b780c^
cd /tmp/lzg-shot
git diff 07b780c^ 07b780c -- crates/deltoids/src/scope.rs | git apply

# Layered overrides on top of the user's normal lazygit config.
cat > /tmp/lzg-overrides.yml <<'EOF'
gui:
  showCommandLog: false
git:
  paging:
    pager: deltoids
EOF

# Run lazygit headless in tmux at 220x60.
tmux new-session -d -s lzg -x 220 -y 60 \
  "cd /tmp/lzg-shot && LG_CONFIG_FILE=$HOME/.config/lazygit/config.yml,/tmp/lzg-overrides.yml lazygit"
sleep 3
tmux send-keys -t lzg Enter            # dismiss the welcome modal
sleep 1
tmux capture-pane -t lzg -p -e > /tmp/lzg.ansi
tmux kill-session -t lzg

# Render and resize. Native is ~7686x5145; resize down to 4000 wide for
# manageable file size while staying retina-sharp.
freeze --language ansi --window --margin 20 --padding 20 \
       --font.size 14 --line-height 1.4 \
       -o /tmp/lazygit-native.png < /tmp/lzg.ansi
magick /tmp/lazygit-native.png -resize 4000x -strip \
       ~/workspace/juanibiapina/deltoids/website/public/screenshots/lazygit.png

# Cleanup
git worktree remove --force /tmp/lzg-shot
rm -f /tmp/lzg-overrides.yml /tmp/lzg.ansi /tmp/lazygit-native.png
```

Final: 4000 × 2678, ~900 KB.

## 4. `edit-tui.png`

Captures `edit-tui` browsing the agent trace
`01KQ2EDBFEYE0C8ZTYV26ATPRS` (a TypeScript refactor in `pi-powerbar`).
Selected entry 10: "Skip redundant powerbar refreshes for unchanged segment
updates". Two hunks: a new `segmentEquals` helper and a modified
`createExtension` body — exercises scope expansion across two functions.

The capture method mirrors lazygit: launch in tmux at a wide geometry, send
keys to navigate to the right entry, capture the pane, render with `freeze`.

```bash
# edit-tui takes no positional args; it lists traces for the current cwd.
# `cd` into the directory whose traces you want to browse first.
tmux new-session -d -s etui -x 240 -y 60 \
  "cd /Users/juan/workspace/juanibiapina/pi-powerbar && edit-tui"
sleep 2

# Navigate to entry 10. Each `j` moves down one entry.
tmux send-keys -t etui j j j j j j j j j
sleep 1

tmux capture-pane -t etui -p -e > /tmp/etui.ansi
tmux kill-session -t etui

freeze --language ansi --window --margin 20 --padding 20 \
       --font.size 14 --line-height 1.4 \
       -o /tmp/edit-tui-native.png < /tmp/etui.ansi
magick /tmp/edit-tui-native.png -resize 4000x -strip \
       ~/workspace/juanibiapina/deltoids/website/public/screenshots/edit-tui.png

rm -f /tmp/etui.ansi /tmp/edit-tui-native.png
```

Final: 4000 × 2464, ~930 KB.

## Conventions

- **Same `freeze` flags everywhere.** Identical font size and window chrome
  keep visual rhythm consistent across the four images.
- **Width-match the hero pair.** `delta.png` and `deltoids.png` must share
  the same pixel width — CSS scales them by `width: 100%` in two grid
  columns; mismatched widths distort the traffic-light dots.
- **Resize after capture.** Native `freeze` outputs are huge (5–8K wide).
  `magick … -resize 4000x -strip` keeps them retina-sharp at half the bytes.
- **Use the user's real configs** (delta, lazygit theme) for visual
  authenticity. For one-off capture-time tweaks (hide command log, force
  deltoids as pager), use `LG_CONFIG_FILE` layering instead of editing the
  permanent config.
