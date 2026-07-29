# Configuration reference

Three files, separated by how far each one should travel.

| File | Lives in | Reaches | Holds |
| --- | --- | --- | --- |
| `config.toml` | `~/.jbsync/` | this machine only | how to reach the store, where JetBrains is |
| `sync.toml` | the store's root | every machine | what to sync and what counts as a setting |
| `machines/<id>.toml` | the store | one named machine | that machine's exceptions |

The store also holds two files jbsync maintains for itself, which you do not
edit: `manifest.toml`, the learned list of which files roam — what lets a
machine that never ran JetBrains' Backup and Sync still sync the right files —
and `defaults/<Product>.toml`, the factory defaults captured from installs
nobody had opened yet, which is how jbsync tells a choice from a shipped value.

Every file is optional. jbsync runs on defaults with no configuration at all,
and every key below shows its default. Unset keys are not merely tolerated —
they are the normal case.

Both `sync.toml` and `machines/` live *inside* the store, which is why they
replicate: they ride along with the settings.

---

## `~/.jbsync/config.toml`

Machine-local. Never synced, because it describes this machine.

```toml
# Complete example. Every value shown is the default unless noted.

[repo]
# Which backend implementation to use. Only "git" is implemented today;
# see docs/how-it-works.md for the contract a new one must meet.
backend = "git"

# Where to publish. Unset means the store never leaves this machine.
remote = "git@github.com:you/jetbrains-settings.git"   # default: unset

# Branch the git backend publishes to.
branch = "main"

# Where the store's working copy lives.
# path = "/Volumes/work/jbsync-store"                  # default: ~/.jbsync/data

[jetbrains]
# Where the JetBrains config root is. "auto" (or unset) uses the OS convention:
#   macOS    ~/Library/Application Support/JetBrains
#   Linux    $XDG_CONFIG_HOME/JetBrains, else ~/.config/JetBrains
#   Windows  %APPDATA%\JetBrains
# root = "/opt/jetbrains-config"                       # default: auto

# Extra directories to search for installed IDEs, used to find the launcher
# binary for plugin installation. Rarely needed.
install_roots = []

[machine]
# This machine's name in reports, and the machines/<id>.toml it reads.
# Defaults to the short hostname; the JBSYNC_MACHINE environment variable
# overrides that; this key overrides both.
# id = "work-laptop"
```

`jbsync init --remote <url> --machine <name>` writes this file for you, and
`jbsync repo set|unset <url>` edits `[repo]`.

### Choosing where the store lives

`repo.path` is useful when `~` is on a small or synced-by-something-else volume.
The store is small — canonical XML for the settings you actually changed — but
it is a Git repository, so it grows with history.

---

## `sync.toml`

Lives at the root of the store, so editing it on one machine and syncing
delivers it to all of them. This is the file for decisions that should be the
same everywhere.

```toml
# Complete example. Every value shown is the default unless noted.

# Schema version. Bumped only if a future release needs to migrate this file.
version = 1

[jetbrains]
# Which IDE config directories to sync, as globs against the directory name
# under the JetBrains root ("IntelliJIdea2026.2", "PyCharm2025.3", ...).
# The default matches any year-versioned IDE directory.
ides = ["*20??.*"]

# Copy each file to ~/.jbsync/backups/<timestamp>/ before overwriting it.
backups = true

# Apply the built-in exclusion list: caches, credentials, telemetry, window
# geometry, per-host trust, and JetBrains' own sync state. Turning this off is
# almost always a mistake.
use_default_excludes = true

# Additional files to never sync, as globs against the path inside an IDE's
# config directory.
exclude = []                                  # e.g. ["options/databaseSettings.xml"]

# Files to sync that the learned manifest did not select. This wins over both
# `exclude` and the manifest, so it is the escape hatch when jbsync will not
# pick something up.
explicit_include = []                         # e.g. ["*.vmoptions"]

# Extra patterns to add to the learned manifest. Unlike explicit_include, these
# are still subject to the exclusion list.
# include = ["templates/**"]                  # default: unset

# Map a product to the .vmoptions filename it uses, when the default guess is
# wrong. All products share one canonical idea.vmoptions in the store.
# vmoptions_names = { IntelliJIdea = "idea.vmoptions" }

[plugins]
# Record which third-party plugins you have, and report what is missing where.
enabled = true

# The IDE launcher used by `jbsync sync --install-plugins`. jbsync reads this
# from each IDE's own metadata; set it only when that is missing or wrong.
# launchers = { IntelliJIdea = "idea", PyCharm = "pycharm" }

[xml]
# Apply the built-in rules for what is not a user choice (registry keys the IDE
# set for itself, untouched tutorial progress, one-shot migration flags).
use_defaults = true
```

### `[[plugins.rule]]` — force a verdict

Overrides jbsync's compatibility check for one plugin. Useful when a plugin's
declared build range is stale but it works fine.

```toml
[[plugins.rule]]
id = "com.example.plugin"    # the plugin ID from its descriptor
ide = "*"                    # glob over IDE directory name / product; default "*"
action = "allow"             # "allow" or "deny"

[[plugins.rule]]
id = "com.heavy.profiler"
ide = "CLion*"
action = "deny"              # never offer this one to CLion
```

### `[[plugins.capability]]` — adjust what an IDE provides

A plugin declares the platform modules it needs. jbsync checks those against
what each IDE provides. Use this when that model is wrong for your setup.

```toml
[[plugins.capability]]
ide = "IntelliJIdea*"
add = ["com.intellij.modules.python"]     # pretend this IDE provides Python
remove = []
```

### `[[xml.omit]]` — stop sharing part of a file

The most useful block here. It removes matching elements from what goes into
the **shared store**; your IDEs keep whatever they already have. Nothing is
deleted from your settings.

```toml
# By option name — the common case. `<option name="..."/>`.
[[xml.omit]]
file = "options/editor.xml"          # glob against the path inside the store
component = "EditorSettings"         # the enclosing <component name="...">
option = "SHOW_INTENTION_BULB"

# Any file, not just one.
[[xml.omit]]
file = "options/*.xml"
option = "MIGRATE_OLD_SETTINGS"

# By attribute value, for elements that are not <option>.
[[xml.omit]]
file = "options/ide.general.xml"
component = "Registry"
element = "entry"                    # default is "option"
attribute = "source"
equals = "SYSTEM"
```

Fields:

| Field | Required | Meaning |
| --- | --- | --- |
| `file` | yes | Glob against the store-relative path, e.g. `options/editor.xml` |
| `component` | no | Only inside this `<component name="...">`; omit for any |
| `element` | no | Element name to match. Default `"option"` |
| `option` | no | Match `name="<value>"`. Takes precedence over `attribute` |
| `attribute` + `equals` | no | Match any attribute against a value |

Give either `option` **or** `attribute`+`equals`; a rule with neither matches
nothing. Rules apply at every depth within a matching file. After they run,
containers left holding nothing are removed too, so omitting the last entry of
a map does not leave an empty `<map/>` behind.

Verify a rule with `jbsync status --verbose`, which lists every removal and the
rule that caused it.

---

## `machines/<id>.toml`

Exceptions for one machine, stored in the store so the machine gets them
wherever it is set up. `<id>` is the machine name shown in report headers and
by `jbsync init`; sanitized to `[A-Za-z0-9._-]`.

```toml
# machines/work-laptop.toml

[jetbrains]
# Files this machine neither publishes nor accepts.
exclude = ["options/proxy.settings.xml"]

# Extra omit rules, in addition to those in sync.toml. Identical syntax.
[[xml.omit]]
file = "options/editor-font.xml"
component = "DefaultFontConfiguration"
option = "FONT_SIZE"
```

The classic use is a machine whose display makes one setting wrong everywhere
else — omit it, and each machine keeps its own value.

---

## Environment variables

| Variable | Effect |
| --- | --- |
| `JBSYNC_CONFIG_DIR` | Use a different root than `~/.jbsync`. Same as `--config-dir` |
| `JBSYNC_MACHINE` | Machine name, unless `machine.id` is set in `config.toml` |

`JBSYNC_CONFIG_DIR` is the safe way to experiment: point it at a scratch
directory and jbsync builds an entirely separate store, leaving your real one
alone.

---

## Precedence, in one place

**Which files sync**, in order:

1. `explicit_include` matches → **included**, no matter what follows.
2. The exclusion list matches → excluded. (Built-ins unless
   `use_default_excludes = false`, plus `jetbrains.exclude`, plus this
   machine's `jetbrains.exclude`.)
3. The manifest matches (learned from the IDEs, plus `jetbrains.include`) →
   included.
4. Otherwise excluded.

**Which settings are shared**: built-in omit rules (unless
`xml.use_defaults = false`), then `sync.toml`'s `[[xml.omit]]`, then this
machine's. All of them subtract; none can add something back.

**Which value wins a conflict**: `--prefer local` (the default), `remote`, or
`neither`. There is no config key for this — it is a per-run decision, made at
the machine you are sitting at.
