# Claude Code Integration

This directory contains the Claude Code plugin for deltoids.

## Requirements

The `deltoids` binary must be installed and available on PATH:

```bash
brew install juanibiapina/taps/deltoids
```

…or `cargo install --git https://github.com/juanibiapina/deltoids deltoids-cli`.

## Install

The deltoids repo doubles as a Claude Code plugin marketplace. Add the
marketplace once, then install the plugin:

```bash
claude plugin marketplace add juanibiapina/deltoids
claude plugin install deltoids@deltoids
```

Or, equivalently, from inside an interactive Claude Code session:

```
/plugin marketplace add juanibiapina/deltoids
/plugin install deltoids@deltoids
```

The first argument's `juanibiapina/deltoids` is the GitHub repo
shorthand; the `@deltoids` in `deltoids@deltoids` is the marketplace
name (defined in [`.claude-plugin/marketplace.json`](../../.claude-plugin/marketplace.json)).
The plugin is installed at user scope (`~/.claude/plugins/`) and is
enabled automatically. Confirm with `claude plugin list`.

To pick up new plugin releases:

```bash
claude plugin marketplace update deltoids
claude plugin update deltoids@deltoids
```

To remove:

```bash
claude plugin uninstall deltoids@deltoids
```

### Local development

To iterate on the plugin without going through the marketplace, point
Claude Code at this directory directly for one session:

```bash
claude --plugin-dir /path/to/deltoids/plugins/claude-code
```

## What It Does

The plugin registers a single `PostToolUse` hook on the `Write` and
`Edit` tools. After every successful file mutation Claude Code
performs, the hook spawns `deltoids hook claude-code` and pipes the
hook envelope to it. The subcommand records a trace entry under
`$XDG_DATA_HOME/edit/traces/<session_id>/entries.jsonl`.

The Claude `session_id` is used directly as the deltoids trace id, so
every edit in one Claude session lands in a single trace. Continuing
the session with `claude --continue` keeps writing to that same trace.

Browse the recorded traces with:

```bash
deltoids tui
```

(The TUI opens on the working-tree diff; press `[` or `]` to toggle to
the trace browser. Traces are filtered by current working directory, so
run it from the same directory the Claude session was running in.)

## How It Works

For each `PostToolUse` event matching `Write|Edit`:

1. Claude Code pipes the JSON envelope to `deltoids hook claude-code`.
2. The subcommand:
   - Resolves `before` from `tool_response.originalFile` (empty for
     newly-created files).
   - Resolves `after` from `tool_input.content` (Write) or by reading
     the file from disk (Edit), so any cascading post-write mutations
     (formatters, other hooks) are also captured.
   - Computes the diff with the deltoids engine.
   - Appends a trace entry keyed on `session_id`.
3. The hook always exits 0 on success and 1 on failure (never 2), so
   it never blocks the agent or pollutes its context with stderr.

## Caveats

- **No agent-supplied reason.** Claude Code's built-in `Edit` and
  `Write` tools do not carry a `reason` field, so trace entries use a
  synthesized reason of the form `"Claude Code <ToolName>"`. If you
  want richer "why" annotations, look at the pi integration in
  `plugins/pi/`, which overrides the agent's edit tools and forces a
  reason.
- **Plugin hook delivery quirk.** Older Claude Code builds ship with a
  bug ([anthropics/claude-code#34573][bug-34573]) where `PreToolUse`
  and `PostToolUse` command hooks distributed via plugins are silently
  dropped. We only register `PostToolUse`, but if you hit that bug,
  copy the matcher block into `~/.claude/settings.json` directly:

  ```json
  {
    "hooks": {
      "PostToolUse": [
        {
          "matcher": "Write|Edit",
          "hooks": [
            { "type": "command", "command": "deltoids hook claude-code" }
          ]
        }
      ]
    }
  }
  ```

  This achieves the same result without relying on plugin hook
  delivery.

[bug-34573]: https://github.com/anthropics/claude-code/issues/34573
