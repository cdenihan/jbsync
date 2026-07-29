# jbsync

Settings and plugin sync for JetBrains IDEs, across every IDE on a machine and
across every machine you use.

`jbsync` stores only the settings you actually changed, merges them one setting
at a time so two IDEs editing the same file never collide, and owns the sync
repository itself so you never have to manage one.

```
$ jbsync sync
mac  ·  git@github.com:you/jetbrains-settings.git on main

2 settings into IDEs  ·  1 setting into the store  ·  1 conflict

IntelliJIdea2026.2  (IntelliJ IDEA)
  options/editor.xml
    to IDE    Editor/fontSize                     13 -> 15
    to store  CodeInsightSettings/AUTO_POPUP      false -> true

PyCharm2026.2  (PyCharm)
  options/editor.xml
    to IDE    Editor/fontSize                     13 -> 15
  options/laf.xml
    conflict  LafManager/laf/themeId
              this machine   Islands Dark  <- kept
              other machine  Light

CLion2026.2  (CLion)
  no changes

1 conflict resolved in favour of this machine. Re-run with --prefer remote to flip the choice.
Committed: 3 file(s) at 8f21c0a4
```

## Documentation

| | |
| --- | --- |
| [Getting started](docs/getting-started.md) | Install, first machine, second machine, troubleshooting |
| [Configuration](docs/configuration.md) | Every option, with complete annotated examples |
| [How it works](docs/how-it-works.md) | What gets synced, and how merging works |
| [Recipes](docs/recipes.md) | Short answers to common tasks |

## Install

Linux or macOS:

```console
curl -fsSL https://github.com/cdenihan/jbsync/releases/latest/download/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://github.com/cdenihan/jbsync/releases/latest/download/install.ps1 | iex
```

The installers detect the operating system and architecture, verify the release
SHA-256 file, and install atomically. After that, `jbsync update` replaces the
same executable in place.

## Getting started

```sh
jbsync init                  # create ~/.jbsync and a local store
jbsync status                # show what a sync would do, changing nothing
jbsync sync                  # reconcile every IDE, then publish
```

That is already useful with one machine: it keeps IntelliJ, PyCharm, CLion,
WebStorm and RustRover in step with each other.

To share across machines, point it at an empty Git repository you own:

```sh
jbsync repo set git@github.com:you/jetbrains-settings.git
jbsync sync
```

On the next machine, install `jbsync`, run the same two commands, and it adopts
what is already there. You never clone, commit, pull or push — `jbsync` owns
that repository. It lives under `~/.jbsync/data` and you can ignore it.

Finally, turn off JetBrains' own sync so the two do not overwrite each other:

```sh
jbsync disable-builtin-sync
```

## What actually gets synced

### Only what the platform roams

Any IDE that has run JetBrains' bundled Backup and Sync leaves a `settingsSync/`
directory, and that directory is the platform's own answer to what roams — it is
produced by the same `RoamingType` annotations the IDE uses internally.

`jbsync` pools those lists across every installed IDE and records the union in
the store, so a manifest learned once by any machine reaches all of them — even
one set up from scratch where Backup and Sync has never run. A curated built-in
list is unioned in as a floor. This matters: a plausible-looking `options/*.xml` rule
would sweep up `other.xml`, which is per-machine UI state, and the `llm.*.xml`
files, which are opaque JSON blobs that cannot be merged meaningfully. JetBrains
excludes both, so `jbsync` does too, without anyone maintaining a list.

Caches, credentials, telemetry, window geometry and per-host trust are excluded
in every case. An IDE that has never been launched is skipped rather than
harvested, so an installer's factory defaults never reach the store.

### Only settings you changed

The IntelliJ platform already does most of this work: when a component's state
equals the state its default constructor produces, nothing is written to the XML
at all. A live `options/*.xml` is therefore close to a diff against defaults
before `jbsync` sees it.

What is left is residue of three kinds, and each is handled by a declarative
rule rather than by code:

- components that serialize a whole map including untouched entries — tutorial
  progress where every lesson is `NOT_PASSED`, inlay-hint tables;
- values the IDE set for itself rather than for you — registry keys carrying
  `source="SYSTEM"` or `"MANAGER"`, one-shot migration flags;
- components that persist only a schema version and no settings.

Better still, jbsync *learns* the defaults. An IDE that has been installed but
never opened contains nothing but its product's factory settings, so jbsync
records them into the store before skipping it. After that, any setting still
holding its shipped value is known not to be a choice, on every machine.

`jbsync sync --verbose` lists exactly what was dropped and why. To cover
something new, add an `[[xml.omit]]` block to `sync.toml` — no code changes.

Pruning decides what is **shared**, never what your IDE keeps. Registry keys and
tutorial state stay in your IDE's own files untouched.

## Merging

Every reconciliation is a three-way merge between the last state both sides
agreed on, what this machine has now, and what the other side has now:

- both sides agree — nothing happens
- only the other side moved — take it, shown as `<` incoming
- only this side moved — keep it, shown as `>` outgoing
- both moved, differently — a conflict, resolved by policy and reported

The merge runs on individual **settings**, not on file text. Two IDEs changing
different settings in the same `editor.xml` is a non-event; Git's line merge
would call that a conflict and can produce XML that no longer parses.

Conflicts default to keeping the value on the machine you are sitting at.
`--prefer remote` flips that, and `--prefer neither` reports without changing
anything.

`.vmoptions` files are merged as a set of JVM flags: additions from both sides
are kept, and only two different values for the *same* flag conflict, so
`-Xmx4g` and `-Xmx16g` are caught while `-Xmx4g` and `-XX:+UseZGC` are not.

## Safety

- **Lossless storage.** Settings are stored as canonical XML, not converted to
  another format. Canonicalization is verified against the full corpus of a real
  installation: parsing, re-serializing and re-parsing must produce an identical
  document and an identical projection, and serialization must be idempotent.
- **Backups.** Every IDE file is copied to `~/.jbsync/backups/<timestamp>/`
  before it is overwritten.
- **Surgical writes.** Incoming settings are applied leaf by leaf to your IDE's
  real file, so anything `jbsync` does not manage is left exactly as it was.
- **Atomic writes.** Files are written to a temporary path and renamed, so an
  interrupted run cannot truncate a settings file.
- **One run at a time.** A sync takes an exclusive lock; a second run reports
  that one is already in progress rather than interleaving with it.
- **Dry runs are honest.** `--dry-run` buffers writes in memory and still runs
  the full reconciliation, so it reports what a real run would do — including
  changes that only appear once an earlier IDE has contributed.

## Plugins

`jbsync` records which third-party plugins you have in `plugins.json`, together
with the compatibility metadata from each descriptor, and other machines install
them from Marketplace through the IDE's own launcher.

Plugin directories are never copied. They contain compiled code and sometimes
native libraries, so copying them between machines — or between macOS and
Windows — is unsound.

Compatibility is checked before anything is installed: build ranges, required
modules, and `incompatible-with` declarations. A Python-only plugin is not
offered to CLion. `jbsync plugins` shows the manifest and the verdict per IDE;
installation is opt-in via `jbsync sync --install-plugins` because it launches
the IDE binary.

## Configuration

Two files, deliberately separated by whether they should travel.

`~/.jbsync/config.toml` is local to this machine and never synced:

```toml
[repo]
backend = "git"
remote = "git@github.com:you/jetbrains-settings.git"
branch = "main"
# path = "/somewhere/else"     # where the store's working copy lives

[jetbrains]
# root = "auto"                # detected per OS; override if you must
# install_roots = ["/opt/jetbrains"]

[machine]
# id = "work-laptop"           # defaults to the hostname
```

`sync.toml` lives *inside* the store, so it reaches every machine:

```toml
[jetbrains]
ides = ["*20??.*"]
backups = true
explicit_include = ["*.vmoptions"]
exclude = ["options/databaseSettings.xml"]

[plugins]
enabled = true
# launchers = { IntelliJIdea = "idea" }

# Stop sharing one setting. It keeps whatever value it already has in each IDE.
[[xml.omit]]
file = "options/editor.xml"
component = "EditorSettings"
option = "SHOW_INTENTION_BULB"
```

`machines/<id>.toml` inside the store holds overrides for a single machine.

Every key, with defaults and worked examples, is in the
[configuration reference](docs/configuration.md).

## Commands

| Command | Purpose |
| --- | --- |
| `jbsync init` | Create the local config and the store |
| `jbsync status` | Show what a sync would do, changing nothing |
| `jbsync sync` | Reconcile every IDE, then publish |
| `jbsync ides` | List the IDEs jbsync can see |
| `jbsync repo show\|set\|unset` | Inspect or change where the store lives |
| `jbsync plugins` | Show the plugin manifest and per-IDE verdicts |
| `jbsync disable-builtin-sync` | Turn off JetBrains' bundled Backup and Sync |
| `jbsync update` | Replace the running binary with the latest release |
| `jbsync completions <shell>` | Print a shell completion script |

Useful flags: `--dry-run`, `--verbose`, `--prefer local|remote|neither`,
`--ide <selector>` (repeatable), `--collect-only`, `--install-plugins`.

`jbsync update` prints a sentence; add `--json` for a machine-readable summary.

## Architecture

```
IDE config dirs ──▶ discovery ──▶ pruning ──▶ canonical XML ──┐
                                                              ▼
                                                   three-way merge
                                                              ▲
                              store working copy ◀── backend ──┘
```

- `src/xml/` — an order-preserving DOM, a deterministic serializer, and the flat
  projection that lets a merge address one setting at a time.
- `src/settings/` — which files sync, and which settings inside them are real
  user choices.
- `src/sync/` — the three-way merge, orchestration, and reporting.
- `src/backend/` — where the store lives and how it travels.
- `src/plugins.rs` — plugin descriptors, compatibility, and the manifest.

Two decisions worth knowing about, both explained in
[how it works](docs/how-it-works.md): the engine never talks to Git — it asks a
`Backend` for three views of the store and merges them itself, so Turso, a
custom HTTP service or Convex could replace it — and there is deliberately **no
IDE plugin**, because the platform offers no supported way to observe a setting
change or to make a running IDE reload one.

## Development

```sh
mise install        # pins the Rust toolchain
mise run ci         # fmt, clippy, tests — what CI runs
```

Tasks are defined in `mise.toml`.

Run the canonicalization check against your own installation:

```sh
JBSYNC_CORPUS="$HOME/Library/Application Support/JetBrains" \
  cargo test --test roundtrip -- --include-ignored
```

Dependencies are kept deliberately small; distribution, self-update and the
secure data directory come from
[rust-cli-release](https://github.com/cdenihan/rust-cli-release).

## License

MIT
