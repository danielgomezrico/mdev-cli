# mdev

Rust CLI toolkit for Flutter/Android/iOS developers, with a tech-agnostic `purge` that also cleans Node, Rust, Go, Ruby/Rails, and Python build caches. Auto-detects your project and runs common dev tasks from within your project directory.

## Why this?

`adb` is powerful but the daily-driver workflow is full of papercuts:

- **`adb install` fails with "more than one device/emulator"** — as soon as you have a phone plugged in *and* an emulator running, every command needs an explicit `-s <serial>`. `mdev` fans out to all connected devices with `--all`, or targets one with `-d`.
- **Clearing app storage means tapping through the emulator UI** — Settings → Apps → pick app → Storage → Clear data. Minutes per cycle, repeated dozens of times a day. `mdev clear` reads the app id from your project and does it in one command.
- **`adb logcat` drowns you in noise from every app and system service** — the default stream is thousands of lines per second and filtering it down to just your app requires juggling `--pid`, tag filters, and `grep` ([ref](https://medium.com/@begunova/mastering-adb-logcat-options-filters-advanced-debugging-techniques-10331a73532f)).
- **Every action needs the package name first** — `adb shell pm clear`, `adb uninstall`, `pm grant` all take a package id, so you end up running `pm list packages | grep myapp` before the real command ([ref](https://www.repeato.app/how-to-delete-an-app-using-adb-without-knowing-its-package-name/)).
- **"unauthorized" / "offline" dance** — device drops off the bridge and you're back to `adb kill-server`, revoking USB debugging keys, replugging, and re-accepting the fingerprint prompt ([ref](https://www.repeato.app/troubleshooting-adb-device-unauthorized-issue/)).
- **Corrupted build caches send you hunting across Stack Overflow** — a weird build failure and suddenly you need to remember the right incantation: `~/.gradle/caches`, `flutter clean && flutter pub cache repair`, `pod deintegrate`, `rm -rf ~/Library/Developer/Xcode/DerivedData`, `pod cache clean --all`, `rm -rf node_modules`, `cargo clean`, `go clean -modcache`, `find . -name __pycache__ -exec rm -rf {} +`… different path, different flag, same wasted afternoon. `mdev purge` knows all of them across Flutter, Android, iOS, Node, Rust, Go, Ruby/Rails, and Python, and supports `--dry-run` so you can see what's about to go.

## Commands

Every command has a one-letter alias (e.g. `mdev u` == `mdev uninstall`).

| Command | Alias | Description |
|---|---|---|
| `mdev uninstall` | `u` | Uninstall the app from connected devices/emulators |
| `mdev clear` | `c` | Clear app data and restart on connected devices |
| `mdev kill` | `x` | Kill the running process for the current project — force-stops the app on devices (Flutter/Android/iOS) or kills the dev server (Node, Rust, Go, Ruby/Rails, Python) |
| `mdev reboot` | `r` | Restart the running process — relaunches the app on devices (Flutter/Android/iOS), or kills the dev server and prints its start command (Node, Rust, Go, Ruby/Rails, Python) |
| `mdev purge` | `p` | Purge build artifacts and caches across Flutter, Android, iOS, Node, Rust, Go, Ruby/Rails, and Python projects |
| `mdev keystore` | `k` | Interactively generate an Android signing keystore |
| `mdev emulator config` | `e c` | Apply config tweaks (e.g. `showAVDManager=no`) to every local Android AVD |
| `mdev emulator list` | `e l` | List known AVD config.ini tweaks |
| `mdev simulator ios` | `sim i` | Boot an iOS simulator (newest runtime, reuses a booted one) and open Simulator.app |
| `mdev simulator ios --off` | `sim i -o` | Shut down the booted simulator matching `--device` |
| `mdev simulator android` | `sim a` | Start an Android AVD and wait until it finishes booting (reuses a running AVD) |
| `mdev simulator android --off` | `sim a -o` | Stop the `--avd` emulator, or every running emulator when `--avd` is omitted |
| `mdev doctor` | `d` | Check development environment (flutter, adb, java, xcode, etc.) |
| `mdev doall` | `a` | Run a command in every immediate subfolder of a parent dir, in parallel |
| `mdev completions` | `s` | Generate shell completion script |

## Supported ecosystems

`mdev purge` auto-detects projects in the current directory (recursing into subdirectories up to 6 levels deep, so workspace layouts like `~/projects/<group>/<repo>` are all found) and applies per-ecosystem cleaners. Global caches list existing paths then prompt **None / All / Some (confirm each)** so you can wipe everything or pick individually (default **None**). Extra ecosystems (`--node-global`, `--rust-global`, …) use the same All/Some/None gate.

| Ecosystem | Anchor file(s) | Per-project paths | Global (gated) |
|---|---|---|---|
| Flutter | `pubspec.yaml` | `build/`, `.dart_tool/`, `android/build`, `ios/Pods`, `ios/Podfile.lock`, `ios/.symlinks`, … | `~/.pub-cache`, `<flutter>/bin/cache` for the active SDK **and** every FVM/asdf version (`~/fvm/versions/*/bin/cache`, `$FVM_CACHE_PATH/versions/*/bin/cache`, `~/.asdf/installs/flutter/*/bin/cache`) |
| Android | `app/build.gradle{,.kts}` | `app/build/`, `build/`, `.gradle/` | `~/.gradle/caches` |
| iOS | `*.xcodeproj` | `Pods/`, `Podfile.lock`, `*.xcworkspace`, `DerivedData` | CocoaPods cache, `~/Library/Developer/Xcode/DerivedData` |
| Node | `package.json` (+ lockfile) | `node_modules/`, `.next/`, `.nuxt/`, `.turbo/`, `.vite/`, `.parcel-cache/`, `dist/`, `build/`, `.svelte-kit/`, `.astro/`, `coverage/` | `~/.npm`, `~/.pnpm-store`, `~/.cache/yarn` or `~/Library/Caches/Yarn`, `~/.bun/install/cache` |
| Rust | `Cargo.toml` | `target/` | `~/.cargo/registry/cache`, `~/.cargo/registry/src`, `~/.cargo/git/db` |
| Go | `go.mod` | `bin/`, `pkg/` | `go clean -modcache`, `~/Library/Caches/go-build` or `~/.cache/go-build` |
| Ruby / Rails | `Gemfile` (+ `config/application.rb`) | `vendor/bundle/`, `.bundle/`, Rails: `tmp/cache/`, `log/*.log` | `~/.bundle/cache`, `~/.gem/cache` |
| Python (Django / FastAPI / generic) | `pyproject.toml`, `requirements.txt`, `Pipfile`, `uv.lock`, `poetry.lock`, `manage.py` | `__pycache__/`, `.pytest_cache/`, `.mypy_cache/`, `.ruff_cache/`, `.tox/`, `.coverage`, `htmlcov/`, Django: `staticfiles/`; opt-in: `.venv/`, `venv/`, `env/` | `~/Library/Caches/pip`/`pypoetry` (macOS), `~/.cache/pip`/`pypoetry` (Linux), `~/.cache/uv`, `~/.local/share/virtualenvs` |

Per-project paths are always cleaned for any detected project. Global caches are destructive and only fire when you pass the matching `--<eco>-global` flag, with an interactive confirmation prompt before any deletion (skipped in `--dry-run`).

### Extras (cross-platform junk)

For every detected project (and the current directory when nothing is detected), `mdev purge` also scans for size-hungry files that aren't tied to any specific ecosystem and that tend to pile up unnoticed:

- JVM heap dumps: `java_pid<NNN>.hprof`
- JVM crash logs: `hs_err_pid<NNN>.log`
- Core dumps: `core`, `core.<pid>`, `*.dmp`
- Package-manager debug logs: `npm-debug.log*`, `yarn-debug.log*`, `yarn-error.log*`, `lerna-debug.log*`
- Editor backups: `*~`, `.*.swp`, `.*.swo`, `.*.swn`
- OS metadata: `.DS_Store`, `Thumbs.db`
- Linter caches: `.eslintcache`, `.stylelintcache`
- Coverage / misc: `.nyc_output/`, `nohup.out`

The scanner lists each match with its size and a total, then prompts once before deleting. Subtrees owned by other cleaners (`node_modules/`, `target/`, `.venv/`, `.git/`, `build/`, …) are skipped to avoid double-walking. Pass `--no-extras` to disable this scan.

### Git worktrees

When `git` is on your `PATH`, `mdev purge` offers worktree cleanup in **two separate batches** per detected repo (each list + confirm, default **No**):

1. **Git-registered** linked worktrees (`git worktree list`) — `git worktree remove --force`. Main and **locked** worktrees are always skipped.
2. **FS-only** convention folders (e.g. `.worktree/`, `.claude/worktrees/`, `worktrees/feature`) not in git's list — confirm-gated free-form delete. Nested children under a listed parent are collapsed so one wipe covers the tree.

In `--dry-run` both batches are listed only. If `git` is unavailable, this step is a no-op.

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

# Purge all build caches (dry run first)
mdev purge --dry-run
mdev purge

# Purge only specific targets
mdev purge --flutter --gradle
mdev purge --rust --dry-run                # only Rust projects
mdev purge --node --node-global            # Node projects + global stores
mdev purge --python --python-venv          # Python + remove .venv/venv/env

# Generate a release keystore
mdev keystore

# Configure all local Android AVDs (default: showAVDManager=no)
mdev emulator config              # apply defaults
mdev emulator config -n           # dry run
mdev emulator config --set hw.keyboard=yes --backup
mdev emulator config --avd Pixel_9

# Check your dev environment
mdev doctor

# Run a command in every subfolder of the current dir (parallel)
mdev doall git status
mdev doall nexusindex
mdev doall -C ~/projects gitnexus analyze --embeddings --index-only
```

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

## Requirements

- **Android**: `adb` in PATH
- **iOS**: macOS + Xcode with `xcrun simctl`
- **Flutter**: `flutter` in PATH
- **Keystore**: JDK with `keytool`

## License

MIT
