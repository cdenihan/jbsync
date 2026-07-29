# Getting started

This walks through a first machine, a second machine, and what to do when
something looks wrong. It assumes nothing about Git.

## Install

Linux or macOS:

```console
curl -fsSL https://github.com/cdenihan/jbsync/releases/latest/download/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://github.com/cdenihan/jbsync/releases/latest/download/install.ps1 | iex
```

Check it landed:

```console
$ jbsync --version
jbsync 2026.07.29.1
```

From then on `jbsync update` replaces that same executable in place.

## Your first machine

### 1. Look before you leap

```console
$ jbsync ides
WebStorm2026.2          WebStorm        build 262.8665.341
IntelliJIdea2026.2      IntelliJIdea    build 262.8665.337
RustRover2026.2         RustRover       build 262.8665.323
CLion2026.2             CLion           build 262.8665.321
PyCharm2026.2           PyCharm         build 262.8665.369
```

If an IDE you use is missing here, it has not been run yet (JetBrains creates
the config directory on first launch), or it lives somewhere unusual — see
[configuration](configuration.md).

### 2. Set up

```console
$ jbsync init
config  /Users/you/.jbsync/config.toml
store   /Users/you/.jbsync/data
machine mac
found   5 IDE(s)

Next: jbsync status
```

That scan is all it did: no settings were read, nothing was written to your
IDEs, and nothing left your machine.

### 3. Preview

```console
$ jbsync status
```

`status` is a full sync that throws away its writes. It reports exactly what
`jbsync sync` would do — that equivalence is enforced by a test — so read it
before the first real run. See [reading a report](#reading-a-report) below.

### 4. Sync

```console
$ jbsync sync
```

Now your IDEs share settings with each other. That is already useful on one
machine: change your font in IntelliJ and PyCharm picks it up on the next sync.

Before every overwrite, the original file is copied to
`~/.jbsync/backups/<timestamp>/`.

### 5. Turn off JetBrains' own sync

Two tools writing the same files will undo each other.

```console
$ jbsync disable-builtin-sync
```

Use `--dry-run` first if you want to see which IDEs it touches.

## Sharing across machines

Create an **empty** repository you own — private is the sensible default, since
these are your editor settings — and point jbsync at it:

```console
$ gh repo create you/jetbrains-settings --private
$ jbsync repo set git@github.com:you/jetbrains-settings.git
$ jbsync sync
```

You never clone, commit, pull or push that repository. jbsync owns it. Its
working copy is `~/.jbsync/data` and you can ignore that directory exists.

On the next machine:

```console
$ jbsync init --remote git@github.com:you/jetbrains-settings.git
$ jbsync status      # see what it wants to adopt
$ jbsync sync
```

The second machine adopts what is already published, and contributes anything
it has that the first machine did not.

Authentication is whatever already works for you — SSH agent, macOS Keychain,
Windows Credential Manager, `gh auth`, a hardware key. jbsync runs the `git`
you have installed rather than embedding its own client, so there are no
separate credentials to configure.

## Day to day

```console
$ jbsync sync
```

That is the whole workflow. It is safe to run often; when nothing changed it
says so and writes nothing. It is safe to run while an IDE is open, but the IDE
will not notice incoming changes until it restarts — no supported API exists to
make a running JetBrains IDE reload settings another process rewrote.

Some people wire it into a shell hook or a timer. It takes an exclusive lock, so
a scheduled run and a manual one cannot interleave; the second reports that a
run is already in progress.

## Reading a report

```
machine mac  |  git (git@github.com:you/jetbrains-settings.git on main)

Legend: < incoming   > outgoing   ! conflict

IntelliJIdea2026.2 (IntelliJIdea)
  options/editor.xml
      > CodeInsightSettings/AUTO_POPUP_JAVADOC_INFO    true
      < Editor/fontSize                                13 -> 15

PyCharm2026.2 (PyCharm)
  options/laf.xml
      ! LafManager/themeId    here Islands Dark / there Light -> kept this machine's value

CLion2026.2 (CLion)
  no changes

1 conflict(s) resolved. Re-run with --prefer remote to flip the choice.
Committed: 3 file(s) at 8f21c0a4 (local store only - `jbsync repo set <url>` to share)
```

| Symbol | Meaning |
| --- | --- |
| `>` | This IDE had a value the shared store did not. It is being published. |
| `<` | The store had a value this IDE did not. It is being written into the IDE. |
| `!` | Both sides changed the same setting since they last agreed. Policy decided. |

Settings are named `Component/setting`, which is where they live in the XML.
`(default)` on either side of an arrow means the setting was absent — reverting
something to its default value propagates as a removal, not as a value.

`--verbose` adds a `pruned` section listing what was left out of the shared
store for not being a user choice, and why.

## When something is not syncing

Work down this list.

1. **Is the IDE visible?** `jbsync ides`. If not, `jetbrains.ides` in
   `sync.toml` does not match its directory name.
2. **Has the IDE ever been launched?** A report saying
   `skipped: never launched` means the installer created the directory but the
   IDE has never run, so it has only factory defaults. Start it once.
3. **Is the file eligible?** `jbsync status --verbose` shows what was considered
   and what was pruned. jbsync syncs the files the JetBrains platform itself
   roams — see [how it works](how-it-works.md#1-what-gets-synced). Deliberate
   omissions include `other.xml` and the `llm.*.xml` files.
4. **Is it a per-machine file?** Window geometry, recent projects, path macros,
   proxy settings and SSH history are excluded on purpose.
5. **Does it need a restart?** The IDE holds settings in memory and writes them
   on exit. Quit the IDE, then sync.
6. **Is JetBrains' own sync still on?** `jbsync disable-builtin-sync`.

To force a file in that jbsync is not picking up:

```toml
# sync.toml
[jetbrains]
explicit_include = ["options/mycustom.xml"]
```

`explicit_include` overrides both the exclusion list and the learned manifest,
so use it deliberately.

## Undoing things

- **A bad incoming value.** Fix it in the IDE and sync; your value publishes.
- **A whole bad sync.** Every overwritten file is in
  `~/.jbsync/backups/<timestamp>/`. Copy back what you want, then sync.
- **Stop sharing one setting** without changing it anywhere:
  see [recipes](recipes.md#stop-sharing-one-setting).
- **Start over.** Delete `~/.jbsync` and run `jbsync init` again. Your IDE
  settings are untouched by that; only jbsync's own state goes.

## Where things live

| Path | What it is |
| --- | --- |
| `~/.jbsync/config.toml` | This machine's config. Never synced. |
| `~/.jbsync/data/` | The store's working copy. jbsync owns it. |
| `~/.jbsync/data/shared/` | The settings themselves, as canonical XML. |
| `~/.jbsync/data/sync.toml` | Policy shared with every machine. |
| `~/.jbsync/data/machines/<id>.toml` | Overrides for one machine. |
| `~/.jbsync/data/plugins.json` | The plugin manifest. |
| `~/.jbsync/data/manifest.toml` | Which files roam, learned and shared. |
| `~/.jbsync/base/` | Last state each IDE and the store agreed on. |
| `~/.jbsync/backups/<timestamp>/` | Copies taken before overwriting. |

`--config-dir` (or `JBSYNC_CONFIG_DIR`) moves all of that somewhere else, which
is handy for trying jbsync out against a copy of your config without touching
the real one.

## Next

- [Configuration reference](configuration.md) — every option, with examples
- [How it works](how-it-works.md) — what gets synced, and how merging works
- [Recipes](recipes.md) — common tasks
