# Recipes

Short answers to things people actually want to do. Config file locations are
in the [configuration reference](configuration.md).

## Stop sharing one setting

You like a different font size per machine, but everything else should sync.
Remove it from the store; each IDE keeps whatever it already has.

```toml
# sync.toml
[[xml.omit]]
file = "options/editor-font.xml"
component = "DefaultFontConfiguration"
option = "FONT_SIZE"
```

Find the component and option names by opening the file in your IDE config
directory — the address in a jbsync report (`DefaultFontConfiguration/FONT_SIZE`)
is exactly `component`/`option`.

Check it took:

```console
$ jbsync status --verbose
```

## Keep one machine out of one file

A work laptop that must not share its database settings, without changing the
policy for everything else:

```toml
# machines/work-laptop.toml, inside the store
[jetbrains]
exclude = ["options/databaseSettings.xml"]
```

The machine name is the one in the report header, and in `jbsync init` output.

## Sync a file jbsync ignores

```toml
# sync.toml
[jetbrains]
explicit_include = ["options/mycustom.xml"]
```

`explicit_include` beats both the exclusion list and the learned manifest. That
makes it the right tool when jbsync will not pick something up — and the wrong
tool for anything containing credentials or machine-local state.

## Sync JVM options

```toml
# sync.toml
[jetbrains]
explicit_include = ["*.vmoptions"]
```

All products share one canonical `idea.vmoptions` in the store, written back
under each product's own filename. Flags merge as a set: additions from both
sides are kept, and only two values for the same flag conflict.

## Limit a run to one IDE

```console
$ jbsync sync --ide IntelliJIdea2026.2      # by directory name
$ jbsync sync --ide 'PyCharm*'              # by glob
$ jbsync sync --ide RustRover               # by product
```

Repeatable. Selectors match the directory name, the product, or the IDE's
display name; an absolute path matches that exact config directory.

## Exclude an IDE permanently

```toml
# sync.toml
[jetbrains]
ides = ["IntelliJIdea20??.*", "PyCharm20??.*"]
```

Globs match directory names under the JetBrains root. Replacing the default
`["*20??.*"]` with an explicit list is how you opt in rather than out.

## Publish without changing local IDEs

```console
$ jbsync sync --collect-only
```

Gathers settings into the store and publishes, but writes nothing back. Useful
on the machine whose configuration you consider authoritative, when you are
seeding a store for the first time.

## Take the other machine's side

```console
$ jbsync sync --prefer remote
```

Or `--prefer neither` to report conflicts and change nothing at all — the
conservative choice when returning to a machine after a long gap.

## Move the store somewhere else

```toml
# ~/.jbsync/config.toml
[repo]
path = "/Volumes/work/jbsync-store"
```

Then run `jbsync sync`. It rebuilds the store there from your remote; nothing is
lost, because the remote is the source of truth once one is configured.

## Try jbsync without touching anything

```console
$ export JBSYNC_CONFIG_DIR=/tmp/jbsync-trial
$ jbsync init
$ jbsync status
```

A completely separate store and config. `jbsync status` never writes anyway, but
this also keeps a stray `sync` off your real settings. Delete the directory when
done.

## Run it automatically

A shell hook, so every new terminal syncs at most once an hour:

```sh
# ~/.zshrc
_jbsync_periodic() {
  local stamp=~/.cache/jbsync-last
  [[ -f $stamp && $(date +%s) -lt $(( $(date -r "$stamp" +%s) + 3600 )) ]] && return
  touch "$stamp"
  (jbsync sync >/dev/null 2>&1 &)
}
_jbsync_periodic
```

Or a systemd timer, a launchd agent, or Task Scheduler. jbsync takes an
exclusive lock, so a scheduled run and a manual one cannot interleave.

Remember that a running IDE will not notice incoming changes until it restarts.

## Recover from a bad sync

Every overwritten file was copied first:

```console
$ ls ~/.jbsync/backups/
20260729-084512/
$ cp -r ~/.jbsync/backups/20260729-084512/IntelliJIdea2026.2/. \
      ~/Library/Application\ Support/JetBrains/IntelliJIdea2026.2/
```

Then fix the setting at the source and sync, so your value is the one that
propagates.

## Start over

```console
$ rm -rf ~/.jbsync
$ jbsync init
```

Your IDE settings are untouched — only jbsync's own state goes. If a remote is
configured, re-adding it re-adopts everything already published.

To start over *including* the shared history, delete the remote repository's
contents too, then `jbsync sync` from the machine you trust most.

## See what jbsync is leaving out

```console
$ jbsync status --verbose
```

The `pruned` section lists every dropped element and the rule that dropped it —
built-in rules and your own `[[xml.omit]]` blocks alike. This is the fastest way
to confirm a rule matches what you meant.

## Check the store is intact

The store is an ordinary Git repository. jbsync owns it, but nothing stops you
looking:

```console
$ git -C ~/.jbsync/data log --oneline
$ git -C ~/.jbsync/data show --stat HEAD
```

Read freely. Committing by hand is asking for trouble; jbsync expects to be the
only writer.
