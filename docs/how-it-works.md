# How it works

The design in enough detail to predict what jbsync will do, and to change it
safely.

```
IDE config dirs ──▶ discovery ──▶ pruning ──▶ canonical XML ──┐
                                                              ▼
                                                   three-way merge
                                                              ▲
                              store working copy ◀── backend ──┘
```

Four stages: decide **which files**, decide **which settings inside them**,
store them in a **stable form**, and **merge** what disagrees.

---

## 1. What gets synced

### The allowlist is learned, not guessed

JetBrains already knows which settings roam between machines. Every component
declares it in code, with `@State` and a `RoamingType`, and any IDE that has run
the bundled Backup and Sync writes the resulting file list into a `settingsSync/`
directory in its config folder.

jbsync reads those directories, pools them across every installed IDE, and
**records the union in `manifest.toml` at the root of the store**. Because that
file replicates like everything else there, a manifest learned once by any
machine reaches all of them — including a machine set up from scratch where
Backup and Sync has never run, and products like WebStorm that ship without a
tree even when their siblings have one.

The record only ever grows. An IDE being uninstalled, or Backup and Sync being
switched off, is not evidence that a file stopped being roamable.

This matters more than it sounds. The obvious rule — "sync `options/*.xml`" —
is wrong:

- `other.xml` holds per-machine UI state, search history and license details.
- The `llm.*.xml` files are opaque JSON blobs that cannot be merged.

JetBrains excludes both. Because jbsync derives its list from the platform's own
answer rather than from a hand-written one, it excludes them too, and keeps
doing so as JetBrains changes its mind — with nobody maintaining a list.

A built-in list is unioned in as well. It is a **floor, not a fallback**: it
applies always, so one IDE with a thin `settingsSync/` tree cannot narrow what
the others sync.

Every entry in that built-in list was checked against real trees. A file that
exists in an IDE's `options/` but never appears in that same IDE's
`settingsSync/` is one the platform declines to roam, and does not belong there
— that test removed `project.default.xml`, `find.xml`, `advancedSettings.xml`,
`console-font.xml`, `terminal-font.xml` and `textmate.xml`, each confirmed
across four products.

### IDEs that have never been launched

An installer such as Toolbox creates the config directory and fills `options/`
with the product's factory defaults before the IDE has ever run. jbsync records
those defaults (see above) but otherwise skips such a directory and says so,
using the platform's own test for a real config directory — the presence of `options/other.xml`, `options/ide.general.xml` or
`options/options.xml`, mirroring `ConfigImportHelper#OPTIONS`.

Two things go wrong otherwise: those factory defaults get harvested into the
store and pushed onto machines where you really did choose something, and
anything written into that directory races the IDE's first-run import wizard,
which may discard it. Launch the IDE once and it joins the next sync normally.

### Always excluded

Regardless of the manifest: caches and databases, `plugins/`, `system/`, `log/`,
credentials (`idea.key`, `ssl/`), telemetry, window geometry, recent projects,
path macros, proxy settings, SSH history, and the bundled sync's own state
(syncing that would make the two tools fight).

Add your own with `jetbrains.exclude`; override any of it with
`explicit_include`. See [configuration](configuration.md#synctoml).

### Filenames in the store

Each IDE's `.vmoptions` file has a product-specific name — `idea.vmoptions`,
`pycharm.vmoptions`, `clion64.vmoptions`. They are stored under one canonical
`idea.vmoptions` and written back under each product's real name, so JVM options
are shared rather than kept five times.

---

## 2. Only settings you changed

You asked for a store containing your choices, not a mirror of your config
directory. Most of that work is already done for you.

**The platform omits defaults at serialization time.** When a component's state
equals what its default constructor produced, nothing is written to the XML at
all. A live `options/editor.xml` is therefore already close to a diff against
defaults before jbsync sees it.

What remains is residue of three kinds, each handled by a declarative rule
rather than by code:

| Residue | Example |
| --- | --- |
| A map serialized whole, including untouched entries | tutorial progress where every lesson is `NOT_PASSED` |
| Values the IDE set for itself, not for you | registry entries with `source="SYSTEM"` or `"MANAGER"`, one-shot `MIGRATE_OLD_SETTINGS` flags |
| Components persisting only a schema version | `<component name="X" version="1"/>` |

### Learning a product's defaults

The rules above are judgement calls. There is a way to *know*.

A JetBrains installer creates the config directory and fills `options/` with the
product's factory defaults **before the IDE has ever run**. That directory is
the cleanest possible answer to "did the user choose this, or did it ship this
way?" — and it exists only in the window between installing an IDE and opening
it for the first time.

So jbsync captures it when it sees it. A never-launched IDE takes no part in the
sync, but its files are projected into `defaults/<Product>.toml` in the store,
which replicates like everything else there. From then on, any setting still
holding its recorded default is treated as not-a-choice and kept out of the
shared store — on every machine, for that product.

```
WebStorm2025.3
  skipped: never launched - recorded its factory defaults; start it once for it
           to take part
```

Defaults are keyed by product, never pooled across products: WebStorm and CLion
disagree about plenty. The build number is recorded for diagnosis but not used
for matching — a value that was this product's default at any point is not
evidence of a choice, and requiring an exact build match would make the capture
useless the first time the IDE updated.

This is the same judgement the platform already makes for itself. When a
component's state equals its default-constructed state, nothing is written at
all; capturing the defaults extends that to the components which write
themselves out regardless.

One consequence worth knowing: a setting you deliberately set *to* the default
value is indistinguishable from one you never touched, so it is not shared. That
is the platform's own semantics, and it is what makes "only what I chose"
possible at all.

After all the rules run, containers left holding nothing are removed bottom-up,
so emptying a map does not leave `<map/>` behind, and a component with no
settings left disappears rather than lingering as noise.

**Pruning decides what is shared, never what your IDE keeps.** Registry keys and
tutorial progress stay in your IDE's own files, untouched. It is a filter on the
way into the store, not an edit to your settings.

`jbsync status --verbose` lists every removal and the rule responsible. To add a
rule, write an `[[xml.omit]]` block — no code change, no release.

### A file that prunes to nothing

If everything in a file was residue, the file has no opinion — which is *not*
the same as the file being deleted. jbsync distinguishes the two deliberately.
Conflating them makes two IDEs oscillate forever, one adding a file the other
withdraws.

---

## 3. Canonical XML

Settings are stored as XML, in a normalized form: attributes sorted, child order
preserved, one declaration style, one indent, one escaping rule.

**Why not TOML or JSON?** A survey of 90 real roamable files found 80 distinct
element shapes, nesting seven deep, and elements carrying text content. A
general XML↔TOML mapping able to round-trip all of that would be an XML
serializer wearing a different syntax — more code, more risk, and a translation
layer to debug every time JetBrains ships a new shape. Storing XML as XML means
a bug can misplace a setting but cannot invent a representation for one.

Sorting attributes is what makes diffs meaningful: without it, an IDE rewriting
a file in a different attribute order looks like a change. Child order is
*preserved*, because in JetBrains XML a list's order is often the setting.

The guarantee is checked against a real installation, not a fixture set: parse →
serialize → parse must produce an identical document and an identical
projection, and serializing twice must produce identical bytes. Run it against
your own config:

```console
$ JBSYNC_CORPUS="$HOME/Library/Application Support/JetBrains" \
    cargo test --test roundtrip -- --include-ignored
```

### The flat projection

Merging XML trees directly is painful. Instead, each document is projected into
a flat list of `address → value` pairs:

```
component[name=Editor]/option[name=tabs]/@value      4
component[name=Editor]/option[name=wrap]/@value      true
```

Reports show a sugared form — `Editor/tabs` — but the full address is what the
merge operates on. Two documents become two maps, and merging maps is
straightforward and testable. Writes go back leaf by leaf into the real tree,
grafting missing ancestors, so applying an incoming setting cannot disturb
anything else in the file.

---

## 4. Merging

Every reconciliation is a **three-way merge** between:

- **base** — the last state both sides agreed on (`~/.jbsync/base/`)
- **local** — what this IDE has now
- **remote** — what the store has now

Per setting:

| base | local | remote | Result |
| --- | --- | --- | --- |
| any | same as remote | same as local | nothing |
| `A` | `A` | `B` | take `B` — incoming `<` |
| `A` | `B` | `A` | keep `B` — outgoing `>` |
| `A` | `B` | `C` | conflict `!` — policy decides |

The base is what makes the middle two rows distinguishable. Without it, "the
other side changed" and "I changed" look identical and everything is a conflict.
So the base is recorded **even when a sync was a no-op** for that IDE; otherwise
the next real divergence has no common ancestor.

Because the merge runs on individual settings rather than file text, two IDEs
changing different settings in the same `editor.xml` is a non-event. Git's line
merge would call that a conflict and can resolve it into XML that no longer
parses.

### Conflicts

Conflicts are reported precisely — which setting, which value on each side, and
which won:

```
conflict  LafManager/laf/themeId
          this machine   Islands Dark  <- kept
          other machine  Light
```

Resolution is per run, not stored: `--prefer local` (default), `--prefer remote`,
or `--prefer neither` to report and change nothing. The default keeps the value
on the machine you are sitting at, on the grounds that you can see it.

### `.vmoptions`

JVM options are lines, not XML, so they merge as a **set of flags** keyed by
flag name. Additions from both sides are kept; only two different values for the
*same* flag conflict. `-Xmx4g` versus `-Xmx16g` is caught; `-Xmx4g` versus
`-XX:+UseZGC` is not a disagreement and both are kept.

### Convergence

A sync reconciles every IDE against the store, but an earlier IDE's contribution
changes what a later one sees. The engine therefore loops until nothing moves,
folding the passes into one report. It settles in one or two passes in practice;
failing to settle is a bug, not a condition to be handled quietly.

---

## 5. The store and the backend

The store is a directory the CLI owns:

```
sync.toml            policy, shared with every machine
machines/<id>.toml   per-machine overrides
plugins.json         the plugin manifest
shared/              the settings, as canonical XML
```

The engine never talks to Git. It asks a `Backend` for three views — what this
machine has, what everyone else has, and the last state both agreed on — merges
them itself, and hands the result back.

### Other backends

That contract is the smallest one supporting a real three-way merge, and every
candidate can satisfy it:

| Backend | working copy | remote | base |
| --- | --- | --- | --- |
| Git (shipping) | the work tree | `origin/<branch>` | `git merge-base` |
| Turso / libSQL | local replica | `SELECT` after pull | last-synced snapshot |
| Custom HTTP | local cache | `GET /changes` | last-synced snapshot |
| Convex | local cache | `query()` | last-synced snapshot |

A backend that cannot name a common ancestor keeps a copy of the last state it
reconciled; that snapshot *is* the base. Reactivity is deliberately outside the
trait — only Convex offers real push, so it belongs in a separate opt-in
capability rather than forcing every backend to fake one. `repo.backend` in
`config.toml` already selects the implementation.

### Why Git is a subprocess

jbsync runs the `git` binary rather than linking a Git library. That keeps
authentication as whatever already works for you — SSH agent, Keychain, Windows
Credential Manager, `gh auth`, hardware keys — instead of reimplementing it, and
avoids a C dependency in every one of eight release targets.

When another machine has published since this one last looked, jbsync merges
semantically and then records the remote as a parent, so history stays honest
without Git's line merge ever touching a settings file.

---

## 6. Plugins

jbsync records which third-party plugins you have in `plugins.json`, with the
compatibility metadata from each descriptor. Other machines install them from
Marketplace through the IDE's own launcher.

**Plugin directories are never copied.** They contain compiled code and
sometimes native libraries; copying them between machines — let alone between
macOS and Windows — is unsound.

Compatibility is checked before anything is installed: build ranges (including
`.*` wildcards), required modules against what each IDE provides, and
`incompatible-with` declarations. A Python-only plugin is not offered to CLion.

A plugin the product **bundles** counts as already there, even though it lives
in the application directory rather than the config `plugins/` directory. One
IDE shipping TOML support in the box and another carrying it as a Marketplace
install is the normal case, and only the second needs anything done.

`jbsync plugins` shows the manifest and the verdict per IDE. A sync installs
what is missing; `jbsync sync --no-install-plugins` reduces that to a report.

Installing launches the IDE binary, so the first sync after a new plugin
appears is slower than usual and prints whatever that launcher decides to say.
The way to stop a plugin reaching a product is a rule, not the flag — see
[`[[plugins.rule]]`](configuration.md#pluginsrule--force-a-verdict) — because a
rule is a lasting decision and the flag only skips one run.

It also reports plugins that are **already installed but cannot load**, which is
a different question from what to install next. A plugin put there by hand — or
copied in by Toolbox while setting the IDE up — may declare a dependency the
product cannot satisfy:

```
Installed but cannot load:
  WebStorm2026.2: com.github.l34130.mise needs org.toml.lang
      fix: install org.toml.lang in WebStorm2026.2
```

That is the same failure the IDE reports at startup, found before you open it.
A dependency namespaced `com.intellij.modules.*` is a platform capability and
cannot be installed; anything else is a Marketplace plugin, and jbsync says so.

---

## 7. Safety

- **Backups.** Every IDE file is copied to `~/.jbsync/backups/<timestamp>/`
  before being overwritten. The ten most recent runs are kept; older ones are
  removed so the directory cannot grow without bound.
- **Permissions are preserved.** JetBrains keeps several roamable files at
  `0600`. Rewriting one through a temporary file would otherwise hand it back
  with the umask's permissions, undoing that.
- **Surgical writes.** Incoming settings are applied leaf by leaf to your real
  file, so anything jbsync does not manage is left exactly as it was — including
  the content pruning kept out of the store.
- **Atomic writes.** Files are written to a temporary path and renamed, so an
  interrupted run cannot truncate a settings file.
- **One run at a time.** A sync holds an exclusive lock for its whole duration;
  a second run reports that one is in progress rather than interleaving.
- **Honest dry runs.** `--dry-run` buffers writes in memory and runs the full
  reconciliation, including the convergence loop, so it reports what a real run
  would do — a property asserted by a test rather than assumed.

---

## Why there is no IDE plugin

A companion plugin was researched and deliberately not built.

The platform has no general "a setting changed" event. The deprecated Settings
Repository plugin achieved it by registering a `StreamProvider` into the
platform's persistence pipeline — undocumented internals that churn between
releases, as the move from `settings-repository` to today's `settingsSync`
module illustrates. Such a plugin would also have to stay binary-compatible with
every product line and every release, indefinitely, and pass Marketplace review
on each update.

The other direction is worse: there is no supported way to make a running IDE
reload arbitrary settings that another process rewrote. Even JetBrains' own
bundled sync largely waits for a restart.

An external tool reaches the same freshness with none of that risk, which is why
jbsync is a CLI you can run by hand, from a shell hook, or on a timer.

---

## Source map

| Path | Responsibility |
| --- | --- |
| `src/xml/dom.rs` | Order-preserving DOM and the deterministic serializer |
| `src/xml/project.rs` | The flat projection, and grafting leaves back into a tree |
| `src/settings/roamable.rs` | The learned manifest, exclusions, filename mapping |
| `src/settings/prune.rs` | What counts as a user choice |
| `src/sync/merge.rs` | Three-way merge for XML, text and `.vmoptions` |
| `src/sync/engine.rs` | Orchestration, staging, the convergence loop |
| `src/sync/report.rs` | What a run reports |
| `src/backend.rs` | The backend contract |
| `src/plugins.rs` | Descriptors, compatibility, the manifest |
