# Youki IDE — YoukiShell Plugin Build Tool

## 1. What this tool is

**Youki IDE** is a command-line tool written in Rust, invoked as
`youki`, **built for exactly one purpose: building plugins for the
YoukiShell app**.

It is **not**:
- A replacement for Android Studio or Gradle for building general
  Android apps.
- A tool for building YoukiShell itself (the host app).
- A general-purpose Android SDK — every command and default is shaped
  around `plugin.json`'s structure and the `.zip` bundle format that
  `PluginManifestParser` inside YoukiShell reads.

If your project isn't a YoukiShell plugin, this isn't the right tool.

## 2. Why Rust instead of Gradle?

Plugins used to be built through Gradle inside AndroidIDE, which
consumed enough RAM (JVM + Gradle daemon) that builds would fail
outright from memory exhaustion on resource-constrained devices —
especially Termux on Android.

`youki` is a full replacement: it calls the underlying compiler tools
directly (`kotlinc`, `aapt2`, `d8`/`r8`, `clang++`, `cargo`,
`zipalign`) with no JVM/Gradle layer on top at all. The final binary is
under 5 MB and runs comfortably on a single-core, 4 GB RAM machine.

## 3. Where this runs

- **Termux on Android** — the same environment the original AndroidIDE
  workflow used.
- **Regular Linux/desktop** (x86_64) — for developers who prefer
  building on a stronger machine and transferring the final `.zip` to
  the target device.

The same Rust source builds for both environments unchanged.

## 4. Repository layout

```
youki-build/
├── Cargo.toml                    ← workspace root
├── crates/
│   ├── youki-cli/                ← the final binary (commands: build, sdk, doctor, new)
│   ├── youki-build-engine/       ← the build pipeline: 7 stages, plus caching
│   ├── youki-sdk-manager/        ← downloads/manages the Android SDK and NDK without Android Studio
│   ├── youki-toolchain/          ← locates tool paths (kotlinc, clang++, cargo-ndk...) + ABI definitions
│   └── youki-manifest/           ← reads/interprets plugin.json (including the tool-specific `requirements` field)
```

## 5. Building from source

```bash
# needs a reasonably current Rust/cargo (some dependencies need edition 2024 support)
git clone <repository-url>
cd youki-build
cargo build --release
# resulting binary: target/release/youki
```

Move the `youki` binary onto your `$PATH` (or invoke it directly from
`target/release/youki`).

## 6. Quick command reference

```bash
youki new MyPlugin --package com.example.myplugin   # scaffold a new project
cd MyPlugin
youki doctor                                         # check which tools are available
youki sdk install-platform 35                        # download android.jar
youki sdk install-build-tools 35.0.0                 # download aapt2/d8/r8/zipalign
youki build                                          # build the plugin → output/plugin.zip
```

## 7. Project status and things to know

- SDK/NDK download URLs in `youki-sdk-manager` are resolved by
  probing Google's server (`dl.google.com/android/repository/...`)
  for the real filename rather than guessing a fixed revision number,
  since Google's exact revisions (`_r01`, `_r02`...) have no
  predictable pattern per API level. See §6 for the `--version`
  override if a lookup ever fails.
- The `requirements` field in `plugin.json` is an **addition from this
  tool only**, used at build time — `PluginManifestParser` inside
  YoukiShell itself ignores it completely at runtime (verified directly
  against the parser's source, and now explicitly documented in
  YoukiShell's own `PLUGIN_GUIDE.md` as of v5.0).
- ABI support currently covers all four Android ABIs (`arm64-v8a`,
  `armeabi-v7a`, `x86`, `x86_64`); the default when unspecified is
  `arm64-v8a` only, to avoid silently growing an existing plugin's
  build time/output size on upgrade.
- The manifest format is, and remains, `plugin.json` — no XML
  migration has ever shipped. `PluginManifestParser` (the real parser
  inside YoukiShell/Youki DEX) does not read a `permissions` array at
  all: a loaded plugin runs in-process as trusted code with direct
  access to whatever the host app itself can already do (including
  `RootManager`/`ShizukoManager` at runtime), the same way any other
  Fragment written directly into the app would. See §12 for exactly
  what this means for your plugin.

## 8. License

Apache-2.0. This tool is independent, original code — it is not built
on or derived from YoukiShell's own source or any GPL project. It only
consumes the publicly documented `plugin.json` format from YoukiShell's
`PLUGIN_GUIDE.md` as an integration contract.

---
---

# Plugin Developer Guide

Everything below is aimed at whoever is actually **writing a plugin**
— human developer or AI model alike. It contains everything you need
without consulting any other source: the required format, allowed
permissions, hard constraints, and how to get started.

> **Note for YoukiShell maintainers reading this:** the `requirements`
> field described below is an addition from the `youki` build tool
> only, used at build time, and is completely ignored at runtime by
> `PluginManifestParser`. Every other field (`pluginId`, `entry`,
> `permissions`, ...) is the official field set that
> `PluginManifestParser` in YoukiShell itself reads.

---

## 9. The very first thing you do — starting from scratch

```bash
youki new MyPlugin --package com.example.myplugin
cd MyPlugin
youki doctor          # checks which required tools are available on your machine
youki build           # produces output/plugin.zip
```

`youki new` generates:
```
MyPlugin/
├── plugin.json                                        ← ready with the essential fields
├── plugin/kotlin/com/example/myplugin/MainEntry.kt     ← default entry point
└── .gitignore
```

From here, edit `plugin.json`, add your code under `plugin/...`, and
rebuild with `youki build` each time.

## 10. Project directory layout (before building)

This is the project's shape during development — not the shape of the
final `.zip`:

```
MyPlugin/
├── plugin.json                  ← the manifest (always required)
└── plugin/
    ├── kotlin/                  ← Kotlin-only sources
    ├── java/                    ← Java AND Kotlin sources together
    ├── cpp/*.cpp                ← optional C++ code
    ├── rust/Cargo.toml          ← optional Rust project
    ├── qml/CMakeLists.txt       ← optional Qt/QML project
    ├── res/                     ← optional Android resources (XML layouts, etc.)
    ├── graphics/                ← icon and card background images
    ├── assets/                  ← any data files your code needs at runtime
    ├── bin/                     ← optional executable ELF helper binaries your plugin's code can invoke at runtime
    ├── webview/                 ← HTML/CSS/JS files (if entry.type == "web")
    └── proguard-rules.pro       ← optional proguard rules (if release + r8)
```

**Naming rule, deliberately strict — no fallback between the two:**
`plugin/kotlin/` is for Kotlin-only sources. `plugin/java/` accepts
**both** Java and Kotlin files together — naming your source folder
"java" is treated as a real declaration that interop code may be
present, so `kotlinc` (which compiles `.java` files referenced from
`.kt` just fine) is pointed at both extensions there. A project using
`kotlin/` is asserting no Java is in play; a stray `.java` file dropped
there is not specially discovered.

**General rule:** any folder above that actually exists is
auto-detected, and the matching build stage activates automatically (or
the folder is copied as-is, in the case of `assets/`, `graphics/`,
`bin/`, `webview/`). Nothing needs to be turned on manually beyond
placing files in the right spot.

**Important — this layout replaced an earlier `src/main/...`
convention entirely; there is no backward compatibility with it.** If
you're looking at an older plugin project using `src/main/java`,
`src/main/cpp`, etc., move those directories under `plugin/` (renaming
`src/main/java` to `plugin/kotlin` or `plugin/java` as appropriate)
before building with a current version of `youki`.

## 11. `plugin.json` — every field, exactly

### 11.1 Official fields (read by YoukiShell/Youki DEX at runtime)

| Field | Required? | Type | Notes |
|---|---|---|---|
| `pluginId` | **Yes** | string | Reverse-DNS format, globally unique, never changes across your plugin's versions. Also the name of the folder the plugin is installed under. |
| `displayName` | **Yes** | string | Shown on the card/row |
| `version` | **Yes** | string | Free-form text, shown as a subtitle |
| `entry` | **Yes** | object | See §13 |
| `surface` | No (default `CARD_GRID`) | string | `"CARD_GRID"` or `"SETTINGS_LIST"`, case-insensitive. See §13.1. |
| `cardBackground` | No | string (path) | Relative path to a `CARD_GRID` plugin's card background image. Ignored for `SETTINGS_LIST`. |
| `icon` | No | string (path) | Relative path to the plugin icon |
| `overrides` | No | string | The id of a built-in screen this plugin's Fragment replaces (`"workshop"` or `"help_about"` — see §13.4). Any other value, or omitting the field, means the plugin just shows its own screen. |
| `author` | No | string | Free-form, not currently displayed in the UI |
| `description` | No | string | Free-form, same note as `author` |

**No `permissions` field exists.** A plugin's compiled code (its
`classes.dex`) is loaded in-process as trusted guest code, the same
way a Fragment you wrote directly into the app itself would be — it
isn't sandboxed, and there's no per-plugin grant list to declare
against. See §12 for exactly what this means and doesn't mean.

**Path fields and zip-slip protection:** `entry.dexPath`,
`entry.htmlEntry`, `cardBackground`, and `icon` are each resolved
relative to the plugin's own installed directory, and the resolved
path is checked against that directory before use. A path that
resolves outside it (e.g. via `../`) is rejected: the parse either
fails outright (for `entry.dexPath`, since a plugin with no valid dex
can't load at all) or the field is silently dropped and its default
used instead (`icon`/`cardBackground` — falls back to no icon /
`AutoMaterial` background, with no warning shown to the user).

### 11.2 The `requirements` field (an addition from `youki` only, build-time)

```json
"requirements": {
  "compileSdk": 35,
  "minSdk": 26,
  "targetSdk": 35,
  "buildToolsVersion": "35.0.0",
  "ndkVersion": "27.2.12479018",
  "abis": ["arm64-v8a", "armeabi-v7a", "x86_64"]
}
```

| Field | Default if omitted | Used for |
|---|---|---|
| `compileSdk` | `35` | Which `android.jar` version the compiler builds against |
| `minSdk` | `26` | `d8`/`r8 --min-api`, and clang++'s `--target=` |
| `targetSdk` | = `compileSdk` | Informational only, currently |
| `buildToolsVersion` | `"35.0.0"` | Which aapt2/d8/r8/zipalign version to use |
| `ndkVersion` | none | Only if the project has `plugin/cpp` or `plugin/rust` |
| `abis` | `["arm64-v8a"]` | Which CPU architectures native (C++/Rust) code is compiled for |

**Important compatibility note:** `compileSdk` is not the same thing as
the minimum supported Android version. `compileSdk` is only the API
version the compiler runs against (normally the highest available),
while `minSdk` is what actually determines the oldest Android version
a device can run the plugin on. Confusing the two is the single most
common Android build misconfiguration — don't write one number and
assume it covers both.

### 11.3 Small complete example (no permissions, web plugin)

```json
{
  "pluginId": "com.example.aboutpage",
  "displayName": "About This Device",
  "version": "1.0.0",
  "surface": "SETTINGS_LIST",
  "entry": {
    "type": "web",
    "htmlEntry": "webview/index.html"
  },
  "icon": "graphics/icon.png"
}
```

### 11.4 Large complete example (native_fragment, C++/Rust, overriding a built-in screen)

```json
{
  "pluginId": "com.example.devicetoolkit",
  "displayName": "Device Toolkit Pro",
  "version": "2.3.1",
  "author": "Someone Studio",
  "description": "Diagnostic and tuning toolkit: reads sensors, manages Bluetooth, inspects installed packages.",

  "surface": "CARD_GRID",
  "overrides": "workshop",

  "entry": {
    "type": "native_fragment",
    "dexPath": "classes.dex",
    "mainClass": "com.example.devicetoolkit.MainEntry"
  },

  "cardBackground": "graphics/card_bg.png",
  "icon": "graphics/icon.png",

  "requirements": {
    "compileSdk": 35,
    "minSdk": 26,
    "buildToolsVersion": "35.0.0",
    "ndkVersion": "27.2.12479018",
    "abis": ["arm64-v8a", "armeabi-v7a", "x86_64"]
  }
}
```

## 12. Permissions — there is no permission model

Youki DEX/YoukiShell does not have a per-plugin permission or
capability system, and `plugin.json` has no `permissions` field.
`PluginManifest.kt`'s own source comment states this directly:

> there is no `permissions` list here. Youki DEX holds every Android
> permission itself (see `AndroidManifest.xml`), and a loaded plugin
> runs in-process as trusted guest code — it doesn't declare or
> request a subset up front, the same way a Fragment you write
> yourself doesn't.

Concretely, this means:

- Your plugin's `classes.dex` is loaded via Android's real
  `DexClassLoader` and instantiated directly (see §13) — there is no
  sandbox, no IPC boundary, and no permission check in front of that
  call.
- Whatever your plugin's code can call is exactly whatever the host
  app's own `AndroidManifest.xml` already declares — the same set
  every other screen in the app has. If you need a **runtime**
  permission the host hasn't been granted yet (camera, contacts,
  precise location, etc.), your Fragment requests it the normal
  Android way (`ActivityCompat.requestPermissions` / the system
  dialog) — same as writing that Fragment directly into the app
  itself.
- Root/Shizuku-backed operations are available the same way: call
  `RootManager.getInstance(context)` or
  `ShizukoManager.getInstance(context)` directly from your Fragment at
  runtime. There's no separate declaration step for this in
  `plugin.json` — whether it actually succeeds depends on whether the
  person running the app has root/Shizuku set up, exactly like any
  other feature in the app that uses those managers.
- **Do not build a plugin that assumes any kind of sandboxing,
  approval flow, or restricted execution.** Your plugin's code is
  fully trusted the moment it's loaded — write it with the same care
  you'd give code shipped directly inside the app, because
  functionally, that's what it becomes at runtime.

`youki-manifest` (this tool's own manifest crate) still accepts and
round-trips a `"permissions"` array in `plugin.json` if you write one
— it's a plain pass-through field kept for forward/backward
compatibility with other tooling, but **YoukiShell itself never reads
it.** Leaving it out entirely is equivalent to including an empty one.

## 13. The `entry` object — the exact shape for each type

**Type one: `native_fragment`** (compiled Kotlin/Java code)
```json
"entry": {
  "type": "native_fragment",
  "dexPath": "classes.dex",
  "mainClass": "com.example.mytool.MainEntry"
}
```
- `dexPath` — relative path to the resulting dex file inside your
  bundle. It doesn't have to be named `classes.dex` — d8/r8 always
  name their own output that, and `youki build`'s packaging stage
  renames it to whatever you declare here before zipping.
- `mainClass` — the fully-qualified name of a class implementing
  `PluginFragmentContract`:
  ```kotlin
  class MainEntry : PluginFragmentContract {
      override fun createFragment(): Fragment = MyPluginFragment()
  }
  ```
  Compile against `PluginFragmentContract.kt` as a compile-only
  dependency — don't bundle it into your own dex, the host app already
  has its own copy on its classpath, which your plugin's
  `DexClassLoader` is given as its parent specifically so it can see
  this interface (see §13.2).

**Type two: `web`** (raw HTML/JS/CSS, no compilation, no code loading)
```json
"entry": {
  "type": "web",
  "htmlEntry": "webview/index.html"
}
```
Runs inside a sandboxed WebView — the lighter-weight option for a
plugin that just needs a UI and no real code execution.

**Any other value for `entry.type`, a missing `entry` altogether, or a
`native_fragment` whose `dexPath` can't be resolved inside the
plugin's own directory, fails loading the whole plugin** with the
message `"invalid or unsupported 'entry' block"` at install time —
the plugin is simply refused, there's no partial/degraded install.

### 13.1 `surface` — which screen the plugin's tile appears on

- `"CARD_GRID"` (default) — a card in the plugin grid. Its background
  is your `cardBackground` image if you shipped one and it resolved
  successfully, otherwise it paints itself using the app's current
  Material color scheme automatically (no blank/placeholder tile).
- `"SETTINGS_LIST"` — a plain icon + title row, grouped alongside the
  app's own built-in Advanced Settings and Workshop entries.
  `cardBackground` is ignored in this mode.

### 13.2 How loading actually works — `DexPluginLoader`

When the person taps your plugin's card/row, the host app:

1. Instantiates a real `dalvik.system.DexClassLoader` for your
   `classes.dex`, with the app's own context `ClassLoader` as its
   parent (this is *why* your compiled code can reference
   `PluginFragmentContract` and any other public Youki DEX class
   without you bundling anything extra). This loader is cached per
   `pluginId` for the process lifetime — reopening your plugin doesn't
   reparse its dex from scratch.
2. Loads `mainClass`, instantiates it, and checks it actually
   implements `PluginFragmentContract`.
3. Calls `createFragment()` and displays the result.

**There is no sandbox, permission check, or approval step anywhere in
this path** — see §12. Your Fragment is a real, fully trusted Fragment
from this point on, exactly like one shipped inside the app itself,
including the ability to replace other parts of the app's UI (see
§13.4).

### 13.3 `SelfLaunchGuard` — the one real loop-prevention rule

The only thing checked before loading is a cooldown, not a trust
boundary: the **same** `pluginId` cannot be (re)loaded again within
1.5 seconds of its last load unless the new attempt is the direct,
immediate result of a person tapping its card/row in the host UI. This
exists purely to stop a plugin whose Fragment launches a system
Activity/Intent (e.g. to request a runtime permission) from
immediately re-triggering its own reload on return, looping. It is
**not** a sandbox and does not distinguish "trusted" from
"untrusted" — a genuine person-initiated reopen a second later works
completely normally.

**Design implication:** if your Fragment opens a system
Activity/Intent, don't try to reload/relaunch itself automatically the
instant control returns — wait for an explicit next tap from the
person instead. There's no socket, IPC layer, or special
"awaiting-consent" state to check; this cooldown is the entire
mechanism.

### 13.4 Replacing a built-in screen — `overrides`

A `native_fragment` plugin can optionally replace one of a small,
fixed set of built-in screens instead of (or in addition to) showing
its own tile, by setting `plugin.json`'s `"overrides"` to one of
exactly two recognized slot ids:

| Slot id | Replaces |
|---|---|
| `"workshop"` | The built-in Workshop tab |
| `"help_about"` | The built-in Help/About screen |

Any other string, or omitting the field, means the plugin doesn't
replace anything. Only one plugin may claim a given slot at a time —
if two enabled plugins both declare the same slot, the one whose
`pluginId` sorts alphabetically last wins (a deterministic tie-break,
not an error). A plugin that gets disabled or uninstalled
automatically releases whatever slot it held, with no extra code
needed on your side for that. **There is deliberately no slot for the
app's own main shell or its crash handler** — a broken plugin claiming
either of those could strand the person with no way back into the app
at all, so those two are simply never offered as override targets.

A replacement takes effect the next time the replaced part of the app
restarts (consistent with how theme/icon-pack/launcher-default changes
work elsewhere in the app) — not by hot-swapping a screen that's
already on screen.

## 14. Runtime capabilities — Root and Shizuku

Since a loaded plugin is trusted in-process code (§12), it can call
straight into the same root/Shizuku infrastructure any other part of
the app uses, at runtime, with no declaration step in `plugin.json`:

```kotlin
val root = RootManager.getInstance(context)
val shizuku = ShizukoManager.getInstance(context)
```

Whether a call through either of these actually succeeds depends
entirely on whether the person running your plugin has root or
Shizuku set up on their device — the exact same condition any other
root/Shizuku-backed feature in the app is subject to. There's no
plugin-specific grant, toggle, or consent bubble in front of this; if
the host app itself could do it, so can your plugin.

## 15. What actually blocks a plugin from loading or running

There is no behavior watchdog, command blocklist, or ban system for
plugin code — once installed and loaded, a plugin's Fragment runs with
the same trust as any other part of the app (§12). The real failure
points are structural, not policy-based:

- **Install time** (`PluginInstaller`): the zip must actually contain
  a valid `plugin.json` the parser accepts (§11), and every extracted
  entry's path is checked against the plugin's own destination
  directory before being written — an entry that would land outside it
  (a zip-slip payload) causes the **whole install** to be rejected, not
  just that one file.
- **Load time** (`DexPluginLoader`, §13.2): `mainClass` must resolve
  inside your dex and actually implement `PluginFragmentContract`, or
  the load fails with an explicit reason string shown to the host UI.
- **`SelfLaunchGuard`** (§13.3): the 1.5-second self-relaunch cooldown
  — the only "you can't do that right now" case that isn't simply
  pass/fail at install or load time.

Beyond these three points, nothing about your plugin's runtime
behavior is specially monitored or restricted by the host app — you
have the same responsibility for your code's correctness and safety
that you'd have writing that code directly into the app.

## 16. Uninstalling and reinstalling

`PluginInstaller.uninstall()` simply deletes the plugin's entire
per-plugin directory (keyed by `pluginId`, under the app's private
`youki_dex_plugins/` folder). Installing a zip whose `pluginId`
matches an already-installed plugin replaces it outright — the old
directory is deleted first, so there's no merge/upgrade step to design
around; treat every install as a full, clean replacement of anything
previously installed under that same `pluginId`.

## 17. Modes and variants in `youki build`

### Strictness modes (`--mode`)

| Mode | Kotlin/Java | C++ | Rust |
|---|---|---|---|
| `peaceful` (default) | warnings silenced (`-nowarn`) | warnings silenced (`-w`) | rustc's normal behavior |
| `dictator` | extra non-fatal checks | `-Wall -Wextra` | same as `peaceful` (rustc is already strict) |
| `nightmare` | `-Werror` (any warning = failure) | `-Wall -Wextra -Werror -Wpedantic` + sanitizers (`address,undefined`) | `cargo clippy -D warnings -D clippy::all` + `-D warnings` |

**Note:** in `nightmare`, any warning in any stage (even if the tool's
own exit code was 0) fails the entire build — the check is uniform
regardless of language.

### Variants (`--variant`)

| Variant | What happens at the dexing stage |
|---|---|
| `debug` (default) | `d8` directly — fast, no shrinking |
| `release` | `r8` instead of `d8` — same conversion + shrinking and obfuscation (uses `proguard-rules.pro` if present) |

### Full example command for a strict final release build:
```bash
youki build --variant release --mode nightmare
```

## 18. SDK/NDK downloads — fully automatic

`youki build` reads `requirements` from `plugin.json`, checks whether
the required components (compileSdk, buildToolsVersion, ndkVersion if
needed) are already installed under `$ANDROID_HOME` (or
`~/android-sdk` by default), and if anything is missing:

- **No flag** (interactive): asks "Download and install these now?
  [Y/n]"
- **`--yes`**: downloads automatically with no prompt (for CI scripts)
- **`--no-auto-install`**: fails immediately with a clear message
  listing everything missing, without attempting any download

If every required component is already present, no message appears at
all — the build proceeds directly.

To download specific components manually:
```bash
youki sdk install-platform 35
youki sdk install-build-tools 35.0.0
youki sdk install-ndk 27.2.12479018
youki sdk list                       # show everything currently installed
```

**How the download URL is found:** Google doesn't publish a stable
"latest" alias for a given platform/API level or build-tools version —
each one ships under an exact filename with a revision number that has
no predictable pattern (e.g. API 28 shipped as `platform-28_r06.zip`).
`youki sdk install-platform`/`install-build-tools` resolve the real
filename automatically by probing Google's server directly (a handful
of small `HEAD` requests, no large downloads wasted on a wrong guess)
rather than assuming a fixed revision. If this ever reports it
couldn't find the archive (Google occasionally jumps a revision number
higher than what's checked, or restructures a release), look up the
exact revision yourself at
`https://dl.google.com/android/repository/repository2-1.xml` and pass
it directly:
```bash
youki sdk install-platform 28 --version 6
youki sdk install-build-tools 34.0.0 --version-override 34.0.0-rc1
```
NDK installs don't need this — an NDK version string
(`27.2.12479018`, or the classic `r27c` form) already fully specifies
the release with nothing left to guess.

## 19. Multi-architecture (ABI) support

Native (C++/Rust) code is only built for the architectures listed in
`requirements.abis`. **The default is `["arm64-v8a"]` only** — if you
want your plugin to work on old 32-bit devices or x86 devices (rare,
mostly emulators), list them explicitly:

```json
"requirements": { "abis": ["arm64-v8a", "armeabi-v7a", "x86_64"] }
```

**Note:** ordinary Kotlin/Java code runs on every architecture
automatically (bytecode has no ABI) — this setting only affects parts
written in C++ or Rust.

## 20. The build cache — how it works and what it looks like

`youki build` caches every heavy compilation stage (Kotlin/Java, each
C++ ABI individually, Rust, and dexing) based on a fingerprint of that
stage's actual inputs: source file contents, relevant `plugin.json`
fields (like `minSdk`), and the toolchain binary's own path — so
upgrading a compiler invalidates the cache too, not just editing
source. A cache hit is only trusted if the expected output file is
also still physically present on disk; a fingerprint match alone is
never enough.

The cache lives under `build/.cache/` — never inside your source
directory.

**What you'll see:** output is intentionally similar to Gradle's own
task-based build output. Each actual tool invocation prints as its own
task line, with the exact command it ran directly beneath it — the
same thing you'd see if you'd typed that command yourself:

```
> Task :com.example.myplugin:compileDebugKotlin
  $ /usr/bin/kotlinc plugin/kotlin build/gen -cp android.jar -d build/classes -nowarn

> Task :com.example.myplugin:compileDebugCpp [arm64-v8a]
  $ /path/to/clang++ --target=aarch64-linux-android26 ...

> Task :com.example.myplugin:compileDebugCpp [armeabi-v7a]
  UP-TO-DATE

> Task :com.example.myplugin:dexDebug
  $ /path/to/d8 --debug --min-api 26 ...

BUILD SUCCESSFUL in 4s
/path/to/MyPlugin/output/plugin.zip
```

Each ABI a native stage builds for prints as its own independent task
line (`compileDebugCpp [arm64-v8a]`, `compileDebugCpp [armeabi-v7a]`,
...) so a partial cache hit — one architecture rebuilt, another
untouched — is visible per architecture, not hidden behind one
combined stage.

Use `--no-cache` to force every stage to rerun regardless of cache
state, without deleting `build/` entirely:
```bash
youki build --no-cache
```

## 21. Common commands (cheat sheet)

```bash
# new project
youki new MyPlugin --package com.example.myplugin

# check available tools on this machine
youki doctor

# quick development build (default)
youki build

# strict final release build, for every architecture declared
youki build --variant release --mode nightmare

# build with a custom SDK path
youki build --android-home /path/to/sdk

# build with no automatic download attempt at all (locked-down CI)
youki build --no-auto-install

# build a project at a path other than the current directory
youki build --path ./path/to/project

# force every stage to rerun, ignoring the cache
youki build --no-cache
```

## 22. Final checklist before shipping

- [ ] `pluginId` is unique and never changes across your plugin's versions
- [ ] `pluginId`, `displayName`, `version`, and `entry` are all present — these are the only required fields
- [ ] Every path in `entry.dexPath`/`entry.htmlEntry`, `cardBackground`, `icon` resolves inside your bundle's root (no `../` escape) — see §11.1
- [ ] `entry.type` is exactly `"native_fragment"` or `"web"`, lowercase
- [ ] If `native_fragment`: `mainClass` actually implements `PluginFragmentContract` and has a public no-arg constructor (§13.1–13.2)
- [ ] You have not declared or relied on a `permissions` field — it doesn't exist; see §12
- [ ] Your plugin doesn't assume any sandboxing, approval flow, or restricted execution — it runs as fully trusted in-process code
- [ ] `surface` is either `"CARD_GRID"`, `"SETTINGS_LIST"`, or omitted
- [ ] If set, `overrides` is exactly `"workshop"` or `"help_about"` — any other value is silently treated as not overriding anything
- [ ] If your Fragment launches a system Activity/Intent, it waits for an explicit next tap rather than auto-relaunching itself (§13.3)
- [ ] If you use `requirements.abis`, you've actually tested the plugin on a device with each architecture listed
- [ ] `youki doctor` shows no missing tool before running `youki build`
- [ ] `youki build --mode nightmare` passes with zero warnings before final release

## 23. Note for AI models

If you (as an AI model) are asked to write a YoukiShell/Youki DEX
plugin:

1. Read this document in full first — don't assume your general
   Android knowledge is enough, since `plugin.json`'s exact shape and
   the loading model (§12–13) are specific to this system.
2. **There is no permission system.** Do not invent a `permissions`
   array, capability strings, or anything resembling
   `android.permission.*` declarations in `plugin.json` — that field
   does not exist and is never read. A plugin's code is loaded and
   trusted exactly like a Fragment written directly into the app.
3. **There is no socket/IPC protocol.** Do not write a message
   envelope, `requestId`/`pluginId` fields, or any JSON-over-TCP
   exchange with the host app. A `native_fragment` plugin's code runs
   in-process; call Android APIs, and
   `RootManager`/`ShizukoManager` if you need root/Shizuku, directly
   (§14).
4. Do not gate your plugin's behavior on a "Run tab" or any kind of
   session-start state from the host app — no such gate exists. Your
   Fragment's lifecycle is the normal Android Fragment lifecycle.
5. The one real constraint on re-launching yourself is the 1.5-second
   `SelfLaunchGuard` cooldown after opening a system Activity/Intent
   (§13.3) — design around waiting for a person's next tap, not around
   a permission or approval system.
6. If the plugin needs C++/Rust code, explicitly ask which
   architectures are needed (`requirements.abis`) rather than assuming
   `arm64-v8a` alone, unless the user has stated they're targeting one
   specific, known device.
7. The manifest format is `plugin.json` — do not write XML. No XML
   manifest format has ever shipped for this system.
