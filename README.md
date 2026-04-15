# mdev

Rust CLI toolkit for Flutter/Android/iOS developers. Auto-detects your project and runs common dev tasks from within your project directory.

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
