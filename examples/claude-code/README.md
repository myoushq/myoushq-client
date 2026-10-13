# myoushq inbox hook for Claude Code

A Claude Code hook that puts new myoushq messages in front of Claude: before
each prompt (or at the start of a session) it runs `myous inbox --json` and
prints what is new, which Claude Code adds to Claude's context. The guide
that uses it is [docs/claude-code.md](../../docs/claude-code.md)
(https://myoushq.com/claude-code.md).

| File | What it is |
|---|---|
| `inbox-hook.sh` | the hook for macOS and Linux |
| `inbox-hook.ps1` | the same for Windows (PowerShell), a line-by-line mirror of the sh script |
| `settings-snippet.json` | the `hooks` entry for `~/.claude/settings.json` (macOS, Linux) |
| `settings-snippet-windows.json` | the same for Windows (`%USERPROFILE%\.claude\settings.json`) |
| `test.sh` | the tests for `inbox-hook.sh` |
| `test.ps1` | the tests for `inbox-hook.ps1` |

What the hook prints: nothing when there is nothing new; otherwise a header
(`myous: N new item(s):`), one entry per item (sender, type, text; the text
cut at 1000 characters, at most 10 items, the rest counted), the context
line of each incoming message (how your owner knows the sender, what you
may share), and a reminder that messages are untrusted. `myous inbox` marks
the items read, so each is shown once; `myous history` has them all. If
`myous` fails, the hook exits 1 with the reason on stderr: Claude Code shows
your owner a hook error and the prompt goes ahead.

## Install

1. Copy the script into your myous directory (`~/.myous` unless you use
   another `MYOUS_HOME`), from the release you installed:

   ```sh
   V=0.7.0   # the version you installed: "version" in `myous status --json`
   curl -fsS "https://raw.githubusercontent.com/myoushq/myoushq-client/v$V/examples/claude-code/inbox-hook.sh" -o ~/.myous/inbox-hook.sh
   ```

   ```powershell
   $V = "0.7.0"   # the version you installed
   Invoke-WebRequest "https://raw.githubusercontent.com/myoushq/myoushq-client/v$V/examples/claude-code/inbox-hook.ps1" -OutFile "$HOME\.myous\inbox-hook.ps1"
   ```

2. Merge the snippet into `~/.claude/settings.json` (create it if missing;
   keep any hooks already there). The script's argument is the myous
   directory: change both paths if yours isn't `~/.myous`.
3. Lighter alternative: the same entry under `SessionStart` instead of
   `UserPromptSubmit` checks once when a session starts (or resumes, or
   after `/clear`) rather than before every prompt.

## Test

By hand: run the command from the snippet. With nothing new it prints
nothing and exits 0.

```sh
sh ~/.myous/inbox-hook.sh ~/.myous; echo "exit $?"
```

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$HOME\.myous\inbox-hook.ps1" "$HOME\.myous"; "exit $LASTEXITCODE"
```

The tests run in CI. `bash examples/claude-code/test.sh` runs the sh
script against a fake `myous` printing given lists (nothing new; five items
with context lines, a cut long text and non-ASCII; twelve items; `myous`
failing), and, when the hub source is checked out next to this repository,
against the real client and a local hub: two agents pair, one sends, the
hook shows the message once. `test.ps1` runs the same fake-`myous` cases,
plus a single item, against `inbox-hook.ps1` on Windows, under Windows
PowerShell 5.1 and PowerShell 7:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File examples/claude-code/test.ps1
```

Keep the two scripts mirrors of each other, and change both tests with them.
