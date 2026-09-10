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

- SDK/NDK download URLs in `youki-sdk-manager` are built from Google's
  known, confirmed naming pattern
  (`dl.google.com/android/repository/...`), but Google does change
  exact revision numbers (`_r01`, `_r02`...) over time. If an automatic
  download 404s, use `--version` to pin the exact number manually.
- The `requirements` field in `plugin.json` is an **addition from this
  tool only**, used at build time — `PluginManifestParser` inside
  YoukiShell itself ignores it completely at runtime (verified directly
  against the parser's source, and now explicitly documented in
  YoukiShell's own `PLUGIN_GUIDE.md` as of v5.0).
- ABI support currently covers all four Android ABIs (`arm64-v8a`,
  `armeabi-v7a`, `x86`, `x86_64`); the default when unspecified is
  `arm64-v8a` only, to avoid silently growing an existing plugin's
  build time/output size on upgrade.
- The permission model was significantly revised in YoukiShell v5.0:
  the manifest format itself is still `plugin.json` (no XML migration
  has shipped), but permissions now include any general-purpose
  `android.permission.*` string alongside the original 8
  YoukiShell-specific capabilities — see §12 for the confirmed details.
  Some internal source comments (in `AndroidPermissionCatalog.kt`)
  reference an XML `<uses-permission>` tag as a point of comparison;
  no XML manifest format has actually shipped as of this writing, and
  `plugin.json` remains the only format `PluginManifestParser` reads.

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
    ├── bin/                     ← executable ELF files (if using native.exec)
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

### 11.1 Official fields (read by YoukiShell at runtime)

| Field | Required? | Type | Notes |
|---|---|---|---|
| `pluginId` | **Yes** | string | Reverse-DNS format, globally unique, never changes across your plugin's versions |
| `displayName` | **Yes** | string | Shown on the card/row |
| `version` | **Yes** | string | Free-form text, shown as a subtitle |
| `enabled` | No | boolean | **Always read and ignored** — don't bother writing it |
| `permissions` | No (default `[]`) | array of strings | See §12 |
| `surface` | No (default `CARD_GRID`) | string | `"CARD_GRID"` or `"SETTINGS_LIST"`, case-insensitive |
| `entry` | **Yes** | object | See §13 |
| `cardBackground` | No | string (path) | Relative path to the card background image |
| `icon` | No | string (path) | Relative path to the plugin icon |
| `author` | No | string | Free-form, not currently displayed in the UI |
| `description` | No | string | Free-form, same note as `author` |

**Strict path rule:** any field that holds a path (`entry.dexPath`,
`entry.htmlEntry`, `cardBackground`, `icon`) is checked at load time,
and must point somewhere **inside** the plugin's own directory after
extraction. Using `../` isn't rejected with an error — the field is
simply ignored and falls back to its default (no icon, no background,
etc.), with no warning at all.

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

### 11.4 Large complete example (native_fragment, multiple permissions, C++/Rust)

```json
{
  "pluginId": "com.example.devicetoolkit",
  "displayName": "Device Toolkit Pro",
  "version": "2.3.1",
  "author": "Someone Studio",
  "description": "Diagnostic and tuning toolkit: reads sensors, manages Bluetooth, inspects installed packages.",

  "permissions": [
    "shell.exec", "native.exec", "code.load", "network.access",
    "youki.hardware", "youki.gamingpass", "youki.bluetooth"
  ],

  "surface": "CARD_GRID",

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

## 12. Permissions — the complete, confirmed model

```json
"permissions": ["shell.exec", "youki.hardware", "network.access", "android.permission.BLUETOOTH_CONNECT"]
```

**This section was updated against YoukiShell v5.0's actual source
code** (`AndroidPermissionCatalog.kt`, `SystemPermission.kt`,
`PermissionGate.kt`) — not a preview or a stated intention. The
generic `android.permission.*` model described below has shipped.

Two categories of strings are valid here.

### 12.1 Category A — YoukiShell-specific capabilities (8 strings, unchanged in shape)

```
shell.exec               native.exec              code.load
network.access           youki.securesettings     youki.storage.internal
youki.hardware           youki.gamingpass
```

These are the original, hand-built capabilities — each backed by its
own explicit case in `CapabilityBridge`, working exactly as documented
in earlier versions of this guide.

> **Note:** earlier `youki.*` names that used to be separate
> capabilities — `youki.installpackages`, `youki.deletepackages`,
> `youki.usagestats`, `youki.querypackages`, `youki.externalstorage`,
> `youki.media.images`, `youki.media.video`, `youki.notifications`,
> `youki.bluetooth` — **still work as accepted aliases** in
> `plugin.json` (confirmed via `SystemPermissionCatalog.systemPermissionIdFor()`),
> but each now internally maps to the real `android.permission.*`
> string it always corresponded to (e.g. `youki.bluetooth` →
> `android.permission.BLUETOOTH_CONNECT`). New plugins should declare
> the real `android.permission.*` name directly instead of the old
> alias — the alias exists for old plugins, not as the recommended
> spelling going forward.

### 12.2 Category B — general-purpose `android.permission.*` strings (134 confirmed)

This is the generic model that replaced the old idea of YoukiShell's
own code needing an update before a plugin could use a new Android
permission. Any string starting with `android.permission.` is accepted
by the parser; whether it actually *does* anything depends on which of
two kinds it is:

- **`SHELL_BACKED`** (the default — everything not explicitly listed as
  API_BACKED, confirmed via `AndroidPermissionCatalog.kindOf()`):
  resolves to a real shell command (`dumpsys`, `content query`, `pm`,
  `settings`) through the exact same `PermissionGate` → Watchdog →
  privilege-backend pipeline as every other request. Send it as a
  `capability` request with a `template` argument — see §12.4.

- **`API_BACKED`** (a fixed, confirmed set of ~60 permissions — camera,
  contacts, calendar, call log, SMS, phone calls, precise/background
  location, body sensors, biometrics, NFC, IR, several
  Companion-Device-Manager and system-dialog APIs, the sync framework,
  and several `BIND_*_SERVICE` permissions): these fundamentally
  require a live `Context`/system API call, which YoukiShell's
  architecture deliberately never hands to a plugin's own dex (that
  would break the "everything goes through ShellServer" design the
  whole app is built on). Declaring one is still valid and still shows
  a checkbox, but `CapabilityBridge.resolveGenericCommand()` returns
  `null` for it **unconditionally, by design** — there is no
  `args`/`template` combination that makes it do anything. The person
  additionally sees a separate, explicit warning identifying it as
  working differently, before the normal consent bubble.

**Important for your own plugin design:** a permission being
`API_BACKED` is not a temporary gap or a bug to work around — it's a
permanent architectural boundary. Don't build a feature around one of
these expecting it to eventually resolve to real functionality.

> **Caveat found during review, not yet confirmed as intentional:**
> `AndroidPermissionCatalog.isKnownAndroidPermission()` only checks
> that a string starts with `"android.permission."` — it does not
> verify the string is a real, existing Android permission constant.
> Declaring a made-up name like `android.permission.NOT_A_REAL_THING`
> is currently accepted as "known" by that specific check. In practice
> this doesn't grant any actual capability (it would simply resolve to
> nothing through `CapabilityBridge` either way), but don't rely on
> this function as a validity check for a permission name — it isn't
> one.

### 12.3 Strict rules around permissions

- **`code.load` is mandatory** for any `entry.type: "native_fragment"`
  — without it, YoukiShell refuses to load the dex at all, and the
  card simply won't open. Confirmed in source: `PermissionGate.checkCodeLoad()`
  always returns an honest `Rejected` for this specific permission,
  never the "faked" empty-result behavior described in §12.5 — there's
  no sensible fabricated Fragment the way there's a fabricated empty
  string for a shell command.
- **There is no automatic routing** — requesting a permission not
  declared in `permissions` is always explicitly rejected.
- **Network access is fully blocked without `network.access`** — there
  is no direct `java.net.Socket` or `HttpURLConnection` regardless of
  what other permissions are granted. `network.access` only means "you
  may send a `ShellRequest.NetworkFetch`"; the host app is the one that
  actually performs the request, after inspecting it.
- **Some permissions also need a separate, already-enabled
  system-level switch** (e.g. Bluetooth capabilities need
  `BLUETOOTH_CONNECT` enabled at the system level, checked via
  `SystemPermissionCatalog.systemPermissionIdFor()`) — disabling that
  switch cuts off every plugin relying on it at once, with an honest
  `Rejected` result (see §12.5), not a faked one.
- **`youki.securesettings` has no blocked-key list** — you can
  read/write any `settings` key, but that doesn't mean there's no
  protection: the real protection is the Shizuku switch itself,
  explicit user consent, plus a separate Watchdog layer that blocks
  certain dangerous keys (enabling accessibility services, device
  admin, etc.) regardless of any permission.
- Typos in either category (§12.1 or §12.2) are not corrected or
  warned about — an unrecognized string is simply a permission your
  plugin can never use, silently.

### 12.4 Sending a shell-backed `android.permission.*` capability call

```json
{
  "type": "capability",
  "requestId": "c1",
  "pluginId": "com.example.mytool",
  "capability": "android.permission.BATTERY_STATS",
  "args": { "template": "dumpsys", "section": "batterystats" }
}
```

`template` selects which generic command shape to build:

| `template` value | Required extra args | Resulting command |
|---|---|---|
| `dumpsys` | `section` | `dumpsys "<section>"` |
| `content_query` | `uri` | `content query --uri "<uri>"` |
| `pm_query` | `subcommand` | `pm <subcommand>` |
| `settings` | `action`, `namespace`, `key`, (`value` for write) | same shape as `youki.securesettings` |

If a permission is `API_BACKED`, sending a `capability` request for it
— with any `template` — always resolves to nothing, on purpose.

### 12.5 The three possible outcomes (confirmed from `PermissionGate.kt`)

Every request goes through `PermissionGate`, which returns exactly one
of three outcomes — the distinction between the last two matters a lot
for how you design your plugin:

- **`Pass`** — the permission is genuinely active: declared by your
  plugin, switched on for your plugin specifically, and not cut off by
  any system-wide switch.
- **`Fake`** — your plugin declared this permission, but the person
  switched *your plugin's own copy* of it off (from the per-plugin
  toggle screen, or simply never turned it on in the consent bubble).
  **Your plugin is never told "denied" here** — the router fabricates a
  plausible-looking empty/neutral response instead, so your plugin's
  sandbox has no way to distinguish "toggled off" from "actually
  running normally." An empty string, or `"0"` for a GPU frequency
  reading, is a normal, expected, permanent possible result — not an
  error condition to detect or work around.
- **`Rejected`** — an honest, explicit refusal. Only for a permanently
  banned `pluginId`, or a system-wide switch (Shizuku, or the specific
  Android permission's own system-level toggle) that's off — a
  whole-device decision, not something specific to your plugin, so
  it's told the truth plainly.

**Design implication:** never write logic that tries to detect a
`Fake` result — by construction, you cannot distinguish it from a
genuinely successful call returning an unusually empty answer. Treat
every empty/neutral result as acceptable. Only build retry or
error-handling logic around an explicit `Rejected`/`denied`/`error`
response type, never around "the data looked suspiciously empty."

## 13. The `entry` object — the exact shape for each type

**Type one: `native_fragment`** (compiled Kotlin/Java code)
```json
"entry": {
  "type": "native_fragment",
  "dexPath": "classes.dex",
  "mainClass": "com.example.mytool.MainEntry"
}
```
- `dexPath` — relative path to the resulting dex file. It doesn't have
  to be named `classes.dex`.
- `mainClass` — the fully-qualified name of the class implementing
  `PluginFragmentContract`:
  ```kotlin
  class MainEntry : PluginFragmentContract {
      override fun createFragment(): Fragment = MyPluginFragment()
  }
  ```
  Compile against `PluginFragmentContract.kt` as a compile-only
  dependency — don't bundle it into your own dex, the host app already
  has its own copy.

**Type two: `web`** (raw HTML/JS/CSS, no compilation)
```json
"entry": {
  "type": "web",
  "htmlEntry": "webview/index.html"
}
```

**Any other value for `entry.type`, or a missing `entry` altogether,
fails loading the whole plugin** with the message `"invalid or
unsupported 'entry' block"`.

## 14. Communication protocol with the host app

Your plugin holds no direct reference to any internal YoukiShell class.
All communication goes through a **socket on port 7171, JSON-over-TCP,
one message per line**. Every request must include your `pluginId` and
a `requestId` you generate yourself.

```json
{"type":"exec","requestId":"a1","pluginId":"com.example.gpuinspector","command":"echo hi"}
```
```json
{"type":"native_run","requestId":"a2","pluginId":"com.example.gpuinspector","runtime":"ELF","entry":"bin/mytool","argv":["--flag"]}
```
```json
{"type":"capability","requestId":"a3","pluginId":"com.example.gpuinspector","capability":"youki.gamingpass","args":{"query":"gpu_freq"}}
```

The response is always one of: `success` (has `output`), `denied` (has
`reason`), `error` (has `message`), `killed` (has `reason` — your
connection is about to close, see §16).

For `native_run`, `entry` is always relative to **your own plugin's**
installed directory — you can't point outside your own bundle.

## 15. Default state at app startup (important for your UI design)

After every real app startup (not resuming from background), any
request from your plugin is honestly rejected with "not started this
session" **until the user opens the Run tab themselves and presses the
green button manually**. There is no auto-start, and no state persists
from a previous session — this resets to false automatically on every
app start. **Design your Fragment/WebView to treat "the app hasn't
started yet" as a normal, expected initial state, not an error.**

## 16. What gets your plugin killed or permanently blocked

Every command/file path/permission call you send is checked against an
undisclosed blocklist, at two levels: a general one covering everyone,
and an additional one specific to each permission. **In general, do
not:**
- Send destructive commands (storage wipes, formatting, reboots,
  deleting other apps, `rm -rf /`, `dd if=`, `mkfs`, fork bombs).
- Try to read sensitive SMS/location/contacts data via raw shell
  commands.
- Write to raw kernel/sysfs paths for the hardware permission instead
  of only reading them.
- Name an executable in a way that suggests hiding a destructive
  process.

**First violation:** your current connection is killed immediately (a
warning, not a permanent ban — it can be reopened). **A second
violation, ever** (recorded permanently, not cleared by
reinstalling): your `pluginId` is permanently blocked, with no
automated or plugin-side way to lift the block — only the user,
manually, from advanced settings.

## 17. Design constraint: don't try to relaunch yourself automatically

Your Fragment cannot hold a reference to the host app's own
`FragmentManager` (since you only communicate through the socket, see
§14). If your Fragment opens a system Activity/Intent (to request a
device permission, for instance) and tries to immediately reload
itself on return, that attempt is rejected. Design permission-request
flows to wait for an explicit next tap from the user, rather than
assuming automatic recovery of control.

## 18. Modes and variants in `youki build`

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

## 19. SDK/NDK downloads — fully automatic

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

## 20. Multi-architecture (ABI) support

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

## 21. The build cache — how it works and what it looks like

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

## 22. Common commands (cheat sheet)

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

## 23. Final checklist before shipping

- [ ] `pluginId` is unique and never changes across your plugin's versions
- [ ] Every path in `entry`, `cardBackground`, `icon` is relative to your bundle's root, with no `../`
- [ ] `entry.type` is exactly `"native_fragment"` or `"web"`, lowercase
- [ ] If `native_fragment`: `code.load` is listed in `permissions`
- [ ] `permissions` contains only strings from §12 (the 8 YoukiShell-specific capabilities, or real `android.permission.*` names) — check spelling character by character
- [ ] If you declared an `API_BACKED` permission (§12.2), you understand it will never resolve to real functionality through `CapabilityBridge` — don't build a feature that depends on it working
- [ ] `surface` is either `"CARD_GRID"`, `"SETTINGS_LIST"`, or omitted
- [ ] If you use `requirements.abis`, you've actually tested the plugin on a device with each architecture listed
- [ ] `youki doctor` shows no missing tool before running `youki build`
- [ ] `youki build --mode nightmare` passes with zero warnings before final release

## 24. Note for AI models

If you (as an AI model) are asked to write a YoukiShell plugin:

1. Read this document in full first — don't assume your general
   Android knowledge is enough, since `plugin.json`, the socket
   protocol, and the permission model are all specific to this system
   and are not standard Android conventions.
2. Don't invent permission names — use only the 8 YoukiShell-specific
   capabilities or real `android.permission.*` names from §12.
3. Never assume your plugin's code has direct network access — every
   network operation goes through a `ShellRequest` after declaring
   `network.access`.
4. Don't write logic that tries to detect a `Fake` permission result (a
   toggled-off permission returns a normal-looking empty/neutral
   result by design — see §12.5). Only handle the explicit `Rejected`
   case as an actual failure.
5. If a permission you need is in the confirmed `API_BACKED` set
   (§12.2 — camera, contacts, precise location, SMS, biometrics, and
   similar live-API-only permissions), don't design a feature that
   depends on it actually returning data — it deliberately never does.
6. If the plugin needs C++/Rust code, explicitly ask which
   architectures are needed (`requirements.abis`) rather than assuming
   `arm64-v8a` alone, unless the user has stated they're targeting one
   specific, known device.
7. The manifest format is `plugin.json` — do not write XML, even if
   you encounter references to an XML `<uses-permission>` tag; no XML
   manifest format has shipped as of this writing (see §7).
