# mdev

Rust CLI toolkit for Flutter/Android/iOS developers, with a tech-agnostic `purge` that finds everything eating your disk — project build artifacts (Flutter, Android, iOS, Node, Rust, Go, Ruby/Rails, Python), stale git worktrees, temp dirs, logs, Xcode and simulator leftovers, package-manager caches, Docker — shows it all in one tree, and deletes what you keep selected in parallel. Auto-detects your project and runs common dev tasks from within your project directory.

## Why this?

`adb` is powerful but the daily-driver workflow is full of papercuts:

- **`adb install` fails with "more than one device/emulator"** — as soon as you have a phone plugged in *and* an emulator running, every command needs an explicit `-s <serial>`. `mdev` fans out to all connected devices with `--all`, or targets one with `-d`.
- **Clearing app storage means tapping through the emulator UI** — Settings → Apps → pick app → Storage → Clear data. Minutes per cycle, repeated dozens of times a day. `mdev clear` reads the app id from your project and does it in one command.
- **`adb logcat` drowns you in noise from every app and system service** — the default stream is thousands of lines per second and filtering it down to just your app requires juggling `--pid`, tag filters, and `grep` ([ref](https://medium.com/@begunova/mastering-adb-logcat-options-filters-advanced-debugging-techniques-10331a73532f)).
- **Every action needs the package name first** — `adb shell pm clear`, `adb uninstall`, `pm grant` all take a package id, so you end up running `pm list packages | grep myapp` before the real command ([ref](https://www.repeato.app/how-to-delete-an-app-using-adb-without-knowing-its-package-name/)).
- **"unauthorized" / "offline" dance** — device drops off the bridge and you're back to `adb kill-server`, revoking USB debugging keys, replugging, and re-accepting the fingerprint prompt ([ref](https://www.repeato.app/troubleshooting-adb-device-unauthorized-issue/)).
- **Corrupted build caches send you hunting across Stack Overflow** — a weird build failure and suddenly you need to remember the right incantation: `~/.gradle/caches`, `flutter clean && flutter pub cache repair`, `pod deintegrate`, `rm -rf ~/Library/Developer/Xcode/DerivedData`, `pod cache clean --all`, `rm -rf node_modules`, `cargo clean`, `go clean -modcache`, `find . -name __pycache__ -exec rm -rf {} +`… different path, different flag, same wasted afternoon. `mdev purge` knows all of them across Flutter, Android, iOS, Node, Rust, Go, Ruby/Rails, and Python, plus temp dirs, logs, Xcode/simulator leftovers and stale worktrees, shows everything with sizes in one tree, and supports `--dry-run` so you can see what's about to go.

## Commands

Every command has a one-letter alias (e.g. `mdev u` == `mdev uninstall`).

| Command | Alias | Description |
|---|---|---|
| `mdev uninstall` | `u` | Uninstall the app from connected devices/emulators. Android is put in airplane mode first — `pm uninstall` only succeeds while the cell radio is off |
| `mdev clear` | `c` | Clear app data and restart on connected devices. Android is put in airplane mode first — `pm clear` only succeeds while the cell radio is off |
| `mdev kill` | `x` | Kill the running process for the current project — force-stops the app on devices (Flutter/Android/iOS) or kills the dev server (Node, Rust, Go, Ruby/Rails, Python) |
| `mdev reboot` | `r` | Restart the running process — relaunches the app on devices (Flutter/Android/iOS), or kills the dev server and prints its start command (Node, Rust, Go, Ruby/Rails, Python) |
| `mdev purge` | `p` | Interactive tree of everything purgeable — project artifacts, git worktrees, temp dirs, logs, Xcode, Android SDK, package caches, Docker — with sizes; deletes the selection in parallel and reports space freed |
| `mdev keystore` | `k` | Interactively generate an Android signing keystore |
| `mdev emulator config` | `e c` | Apply config tweaks (e.g. `showAVDManager=no`) to every local Android AVD |
| `mdev emulator list` | `e l` | List known AVD config.ini tweaks |
| `mdev simulator ios` | `sim i` | Boot an iOS simulator (newest runtime, reuses a booted one) and open Simulator.app |
| `mdev simulator ios --off` | `sim i -o` | Shut down the booted simulator matching `--device` |
| `mdev simulator android` | `sim a` | Start an Android AVD and wait until it finishes booting (reuses a running AVD) |
| `mdev simulator android --off` | `sim a -o` | Stop the `--avd` emulator, or every running emulator when `--avd` is omitted |
| `mdev fix android` | `f a` | Repair a running Android emulator that lost its network (Wi-Fi "!", nothing loads) |
| `mdev doctor` | `d` | Check development environment (flutter, adb, java, xcode, etc.) |
| `mdev doall` | `a` | Run a command in every immediate subfolder of a parent dir, in parallel |
| `mdev completions` | `s` | Generate shell completion script |

## Purge

`mdev purge [PATH...]` is the one command to keep a developer machine clean. It runs in four steps:

1. **Scan** — nine scanners run in parallel and collect candidates with their on-disk size. Project artifacts are looked for under `PATH...` (default: the current directory, recursing up to 6 levels so `~/projects/<group>/<repo>` layouts are found). Everything else is home-wide. While it runs you see what takes the time: a spinner with the directory count and current folder during project discovery, one live line per category showing the path or command it is on, then a bar while sizes are computed in parallel.
2. **Select** — a full-screen tree, Category → Group → Item, every row with its size, the header showing *selected of total*. Everything is pre-checked except the **◆ expensive-to-recreate** tier, which is listed but unchecked and never sized (shown as `?`) to keep the scan fast.
3. **Delete** — the selection is removed in parallel (a progress bar shows progress); command-backed items (`git worktree remove`, `go clean -modcache`, `xcrun simctl delete unavailable`, `docker … prune`) run one after another.
4. **Report** — per-category counts and bytes freed, a total, and any failures. Exit code is 1 when something failed.

### Keys

| Key | Action |
|---|---|
| `↑` `↓` (`j` `k`) | move |
| `Space` | toggle the node **and everything below it** — toggle a whole category or group in one keystroke; parents show `[-]` when partially selected |
| `←` `→` (`h` `l`) | collapse / expand |
| `a` / `n` | select all / none |
| `/` | filter by name or path (`Enter` keeps the filter, `Esc` clears it) |
| `Enter` | delete the selection |
| `q` / `Esc` | abort — nothing is deleted |

### What is scanned

| Category | Items | Default |
|---|---|---|
| Projects | per detected project: Flutter `build/ .dart_tool/ ios/Pods android/build …`, Android `build/ app/build .gradle`, iOS `Pods/ build/`, Node `node_modules/ .next/ dist/ …`, Rust `target/`, Go `bin/` (only when git-ignored), Ruby `vendor/bundle .bundle` (+ Rails `tmp/cache`, `log/*.log`), Python `__pycache__ .pytest_cache .mypy_cache .ruff_cache .tox .coverage htmlcov` (+ Django `staticfiles`) | checked; `.venv/ venv/ env/` ◆ |
| Extras | per project: JVM heap dumps / crash logs, core dumps, `npm-debug.log*`, editor backups, `.DS_Store`, `Thumbs.db`, linter caches, `.nyc_output/` | checked |
| Git worktrees | linked worktrees from `git worktree list` (removed with `git worktree remove --force`) and orphaned checkouts (gitdir gone) in convention folders (`.worktree/`, `worktrees/*`, `.claude/worktrees/*`, `.agents/wt/*`); a folder holding a registered worktree is never listed | checked; locked or with uncommitted changes ◆ |
| Temp dirs | entries directly under `/tmp`, `/var/tmp`, `$TMPDIR` owned by you with nothing modified within 2 levels for `--tmp-age` days (default 3), plus zombie worktree checkouts anywhere below them (not when the repo's `/Volumes/*` disk is unmounted). Sockets, FIFOs, symlinks, `com.apple.*`, lock files and live worktrees are never listed | checked |
| Logs | each folder under `~/Library/Logs`, `~/.npm/_logs` | checked |
| Xcode (macOS) | `DerivedData/*`, `Archives/*`, `iOS/watchOS/tvOS DeviceSupport/*`, Xcode caches, `CoreSimulator/Caches`, simulator device data (named via `simctl`, booted ones skipped), unavailable simulators, orphaned simulator dirs | checked; Archives, the 2 newest DeviceSupport versions, simulators, simulator data and DerivedData while Xcode/xcodebuild runs ◆ |
| Android SDK | `~/.android/cache`, `~/.android/build-cache`, `~/.gradle/daemon`, stale `*.lock` files in AVDs (only when no emulator runs) | checked; `~/.gradle/daemon` while a Gradle daemon runs ◆ |
| Package caches | `~/.npm`, `~/.pnpm-store`, Yarn, Bun, `~/.pub-cache`, Flutter SDK `bin/cache` (active, FVM and asdf versions), `~/.cargo/registry`, `~/.cargo/git/db`, `~/.gradle/caches`, `~/.gradle/wrapper/dists`, `~/.kotlin`, Go build cache + `go clean -modcache`, pip, poetry, uv, pipenv, bundler, gem, CocoaPods, Homebrew | all ◆ |
| Docker | `docker builder prune -f` (build cache), `docker container prune -f` (stopped containers) and `docker image prune -af` (all unused images), sized from `docker system df`; only when the daemon answers | build cache checked; containers, images ◆ |

Paths nested under another listed path are collapsed so one wipe covers the tree. Scan roots nested under another root are dropped; a git repo root that is itself a project (Cargo workspace, plugin + `example/`) is scanned along with its sub-projects. Scan-time tool calls (`git`, `xcrun`, `docker`, `go`…) give up after 10 s.

### Flags

```
mdev purge [PATH...] [-n|--dry-run] [-y|--yes] [--all] [--only <cats>] [--skip <cats>] [--tmp-age <days>] [-v]
```

- `-n` prints the whole tree with sizes and default marks, deletes nothing.
- `-y` skips the tree and deletes the default selection; add `--all` to include the ◆ tier.
- `--only` / `--skip` take comma-separated categories: `projects, extras, worktrees, temp, logs, xcode, android, package-caches, docker`.
- When stdout is not a terminal and `-y` is absent, the tree is printed and nothing is deleted (exit 2).

### Safety

Before any removal every path must be absolute, exist, live under your home, a scan root or a temp root, and must not be the home, a scan root, a project root, a temp root, `/`, `/Users`, `/Volumes/<disk>` or another system location. Symlinks are never followed when sizing or deleting. A temp directory's modification time only changes when entries are added or removed, so an old directory may still contain recently written files — review the Temp group before confirming.

## Installation

### Homebrew

```sh
brew tap <user>/tap
brew install mdev
```

### From source

Requires Rust. Run `make setup` if you don't have it.

```sh
git clone https://github.com/<user>/mdev
cd mdev
make install
```

## Usage

Run any command from within your project directory. `uninstall`, `clear`, `keystore`, `emulator`, and `doctor` target Flutter/Android/iOS; `kill`, `reboot`, and `purge` work across every supported ecosystem (see the table above).

```sh
# Uninstall from whatever is attached. Boots an Android emulator when nothing is.
mdev uninstall

# Uninstall from a specific device
mdev uninstall -d <device-id>

# Uninstall from all connected devices
mdev uninstall --all

# Clear app data and relaunch on all devices
mdev clear --all

# Kill the current project's process
mdev kill                                  # mobile: force-stop the app; server: kill the dev server
mdev kill -d <device-id>                    # force-stop on a specific device (mobile)

# Restart the current project's process
mdev reboot                                # mobile: relaunch the app; server: kill it + print the start command
mdev reboot -d <device-id>                  # relaunch on a specific device (mobile)

# Purge: see everything first, then pick in the tree
mdev purge -n                              # dry run — full tree with sizes, nothing deleted
mdev purge                                 # interactive tree for projects under . + home-wide caches
mdev purge ~/projects                      # scan all projects under ~/projects
mdev purge --only projects,extras -y       # non-interactive: delete default selection of two categories
mdev purge --skip docker,xcode             # leave Docker and Xcode alone
mdev purge --tmp-age 7 --all               # temp entries older than 7 days, globals pre-selected

# Generate a release keystore
mdev keystore

# Configure all local Android AVDs (default: showAVDManager=no)
mdev emulator config              # apply defaults
mdev emulator config -n           # dry run
mdev emulator config --set hw.keyboard=yes --backup
mdev emulator config --avd Pixel_9

# Fix an emulator whose Wi-Fi shows "!" and never loads anything
mdev fix android                           # every running emulator
mdev fix a --avd Pixel_9                   # just this one
mdev fix a --dns 8.8.8.8,1.1.1.1           # force these DNS servers on the restart
mdev fix a --no-restart                    # diagnose + guest-side fixes only
mdev fix a -y                              # do not ask before cold-booting

# Check your dev environment
mdev doctor

# Run a command in every subfolder of the current dir (parallel)
mdev doall git status
mdev doall nexusindex
mdev doall -C ~/projects gitnexus analyze --embeddings --index-only
```

## Fixing a broken emulator network

An Android emulator forwards DNS to whatever resolvers the host had **when it booted**. Change network, drop a VPN, or lose the resolver that was listening on `127.x`, and every lookup inside the guest times out: packets still flow (the gateway answers, `ping 8.8.8.8` works), but no name resolves, so the status bar shows Wi-Fi with a "!" and nothing loads.

`mdev fix android` probes for that state and applies the smallest repair that works, in order:

1. Enable airplane mode if it is off. The guest repair only succeeds while the cell radio is off, so this runs before every later step and again after a cold boot, which comes up with radios on.
2. Toggle Wi-Fi, which forces Android to re-run network validation and clears a stale "!".
3. Cold-boot the AVD with `-dns-server <host resolvers>` — the only fix for a dead resolver, since the emulator re-reads DNS at startup. Loopback resolvers are dropped (the guest cannot reach them) and `8.8.8.8,1.1.1.1` fill the remaining slots.

It targets every running emulator, or just `--avd <name>`, and asks before the restart unless you pass `-y`.

## Shell completions

`mdev completions <shell>` prints a completion script to stdout. Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`.

**zsh** (ensure `~/.zfunc` is on `fpath` and `autoload -U compinit && compinit` runs in `~/.zshrc`):

```sh
mkdir -p ~/.zfunc
mdev completions zsh > ~/.zfunc/_mdev
```

**bash**:

```sh
mdev completions bash > ~/.local/share/bash-completion/completions/mdev
```

**fish**:

```sh
mdev completions fish > ~/.config/fish/completions/mdev.fish
```

Restart the shell after installing.

## Flags

Most commands support:

- `-d / --device <id>` — target a specific device
- `-a / --all` — apply to all connected devices
- `-v / --verbose` — show detailed output
- `-n / --dry-run` (purge only) — preview what would be deleted
- `-y / --yes` (purge only) — skip the tree UI and delete the default selection

## Requirements

- **Android**: `adb` in PATH
- **iOS**: macOS + Xcode with `xcrun simctl`
- **Flutter**: `flutter` in PATH
- **Keystore**: JDK with `keytool`

## License

MIT
