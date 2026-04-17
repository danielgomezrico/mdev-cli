# mdev

Rust CLI toolkit for Flutter/Android/iOS developers. Auto-detects your project and runs common dev tasks from within your project directory.

## Why this?

`adb` is powerful but the daily-driver workflow is full of papercuts:

- **`adb install` fails with "more than one device/emulator"** — as soon as you have a phone plugged in *and* an emulator running, every command needs an explicit `-s <serial>`. `mdev` fans out to all connected devices with `--all`, or targets one with `-d`.
- **Clearing app storage means tapping through the emulator UI** — Settings → Apps → pick app → Storage → Clear data. Minutes per cycle, repeated dozens of times a day. `mdev clear` reads the app id from your project and does it in one command.
- **`adb logcat` drowns you in noise from every app and system service** — the default stream is thousands of lines per second and filtering it down to just your app requires juggling `--pid`, tag filters, and `grep` ([ref](https://medium.com/@begunova/mastering-adb-logcat-options-filters-advanced-debugging-techniques-10331a73532f)).
- **Every action needs the package name first** — `adb shell pm clear`, `adb uninstall`, `pm grant` all take a package id, so you end up running `pm list packages | grep myapp` before the real command ([ref](https://www.repeato.app/how-to-delete-an-app-using-adb-without-knowing-its-package-name/)).
- **"unauthorized" / "offline" dance** — device drops off the bridge and you're back to `adb kill-server`, revoking USB debugging keys, replugging, and re-accepting the fingerprint prompt ([ref](https://www.repeato.app/troubleshooting-adb-device-unauthorized-issue/)).
- **Corrupted Gradle / pub / CocoaPods caches send you hunting across Stack Overflow** — a weird build failure and suddenly you need to remember the right incantation: `~/.gradle/caches`, `flutter clean && flutter pub cache repair`, `pod deintegrate`, `rm -rf ~/Library/Developer/Xcode/DerivedData`, `pod cache clean --all`… different path, different flag, same wasted afternoon. `mdev purge` knows all of them and supports `--dry-run` so you can see what's about to go.

## Commands

| Command | Description |
|---|---|
| `mdev uninstall` | Uninstall the app from connected devices/emulators |
| `mdev clear` | Clear app data and restart on connected devices |
| `mdev purge` | Purge build artifacts and caches (flutter, gradle, pub, pods, DerivedData) |
| `mdev keystore` | Interactively generate an Android signing keystore |
| `mdev emulator config` | Apply config tweaks (e.g. `showAVDManager=no`) to every local Android AVD |
| `mdev doctor` | Check development environment (flutter, adb, java, xcode, etc.) |

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

Run any command from within your Flutter/Android/iOS project directory.

```sh
# Uninstall from a specific device
mdev uninstall -d <device-id>

# Uninstall from all connected devices
mdev uninstall --all

# Clear app data and relaunch on all devices
mdev clear --all

# Purge all build caches (dry run first)
mdev purge --dry-run
mdev purge

# Purge only specific targets
mdev purge --flutter --gradle

# Generate a release keystore
mdev keystore

# Configure all local Android AVDs (default: showAVDManager=no)
mdev emulator config              # apply defaults
mdev emulator config -n           # dry run
mdev emulator config --set hw.keyboard=yes --backup
mdev emulator config --avd Pixel_9

# Check your dev environment
mdev doctor
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
