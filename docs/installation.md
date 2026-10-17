# Installation

TeXoria runs on macOS 10.15+ (Apple silicon and Intel), Windows (x64) and Linux with
glibc 2.35+ (x86_64 and aarch64; Ubuntu 22.04, Debian 12, Fedora 36 or newer).

Every package bundles [Tectonic](https://tectonic-typesetting.github.io), so you can build
right away without a TeX distribution. Optionally install:

- **TeX Live, MacTeX or MiKTeX**, to compile with latexmk (pdfLaTeX, XeLaTeX, LuaLaTeX) and your
  installed packages. TeXoria finds it even when it is not on the `PATH` of GUI apps.
- **Git**, for [source control](git.md) and [Overleaf](overleaf.md).

## Quick install

### macOS and Linux

```sh
curl -fsSL https://raw.githubusercontent.com/Herbrant/TeXoria/main/scripts/install.sh | sh
```

The script downloads the latest release, checks it against the release's `SHA256SUMS` and
installs it without root:

- **macOS**: `TeXoria.app` in `/Applications` (or `~/Applications` if that is not writable).
- **Linux**: `~/.local/bin/texoria`, its libraries in `~/.local/lib/texoria`, plus a menu entry
  and icons in `~/.local/share`.

TeXoria then [updates itself](#updating). It accepts a few environment variables (pass them to `sh`, as in
`curl … | TEXORIA_VERSION=0.1.0 sh`):

| Variable | Meaning |
|---|---|
| `TEXORIA_VERSION` | Version to install instead of the latest, e.g. `0.1.0` |
| `TEXORIA_APP_DIR` | macOS: where to put `TeXoria.app` |
| `TEXORIA_PREFIX` | Linux: install prefix instead of `~/.local` |

Prefer to read it first? Download [`scripts/install.sh`](../scripts/install.sh), check it, then
run `sh install.sh`.

### Windows

Download `texoria-<version>-windows-x86_64-setup.exe` from the
[latest release](https://github.com/Herbrant/TeXoria/releases/latest) and run it.

## Package managers

### Homebrew (macOS)

```sh
brew tap herbrant/tap
brew trust --cask herbrant/tap/texoria
brew install --cask texoria
```

Homebrew 7 and later only load casks from third-party taps you trust, hence the second command.
Update with `brew upgrade --cask texoria`.

### AUR (Arch Linux)

```sh
yay -S texoria-bin    # or paru, or any AUR helper
```

### Debian, Ubuntu

Download the `.deb` for your architecture from the
[latest release](https://github.com/Herbrant/TeXoria/releases/latest), then:

```sh
sudo apt install ./texoria_<version>_amd64.deb     # or _arm64
```

### Fedora, openSUSE

```sh
sudo dnf install ./texoria-<version>-1.x86_64.rpm   # or .aarch64; zypper install on openSUSE
```

### Other Linux distributions

Use the quick install above, or one of the release's portable builds:

- `texoria-<version>-linux-<arch>.AppImage`: `chmod +x` it and run it.
- `texoria-<version>-linux-<arch>.tar.gz`: a `usr/` tree to extract under any prefix
  (`/usr/local`, `~/.local`, …).

## Updating

TeXoria looks for a new release when it starts, at most once a day (turn it off in
[Settings](settings.md#updates)), and right away with **Check for Updates…** in the TeXoria menu.
When there is one, the status bar shows it on the right. What happens next depends on how you
installed TeXoria:

| Installed with | How it updates |
|---|---|
| `.dmg`, install script (macOS) | **Update to X.Y.Z** downloads the release; **Restart to Update** saves your files, replaces `TeXoria.app` and reopens the project |
| Windows installer | The same; the installer runs without questions once TeXoria has quit, and starts it again |
| AppImage | The same; the AppImage file is replaced |
| Install script (Linux) | The same; `bin/texoria` and `lib/texoria` under the prefix are replaced |
| Homebrew | `brew upgrade --cask texoria` |
| AUR, `.deb`, `.rpm` | Your package manager |

Before installing anything, TeXoria checks that the release's `SHA256SUMS` is signed with the
project's release key (a [minisign](https://jedisct1.github.io/minisign/) key whose public half
is built into the app) and that the download matches it.

An app it cannot replace — run straight from the disk image, from a folder you cannot write to,
or one macOS moved to a temporary location because it was never moved out of `Downloads` — is
only told about the new version: move it to Applications first, or download the release by hand.
Running the install script again also updates.

## Verifying downloads

Each release has a `SHA256SUMS` file, signed in `SHA256SUMS.minisig`. Check a download with:

```sh
sha256sum --check --ignore-missing SHA256SUMS            # Linux
shasum -a 256 --check --ignore-missing SHA256SUMS        # macOS
```

and, with [minisign](https://jedisct1.github.io/minisign/), that `SHA256SUMS` comes from the
release workflow:

```sh
minisign -V -m SHA256SUMS -P RWT1t4IPXYpPnfg23bOLpF8VMtWvMGCV2TASNWAGxMQgtBNxqqkYah15
```

## Code signing

TeXoria is not signed with an Apple Developer ID nor a Windows code-signing certificate.

- **macOS**: the install script and the Homebrew cask clear the quarantine flag for you. If you
  copy the app from the `.dmg` yourself, run this once, or macOS will refuse to open it:
  ```sh
  xattr -dr com.apple.quarantine /Applications/TeXoria.app
  ```
- **Windows**: SmartScreen asks for confirmation the first time (*More info* → *Run anyway*).

## Uninstalling

| Installed with | Remove with |
|---|---|
| Install script, macOS | `rm -rf /Applications/TeXoria.app` |
| Install script, Linux | `rm -rf ~/.local/bin/texoria ~/.local/lib/texoria ~/.local/share/licenses/texoria ~/.local/share/applications/texoria.desktop ~/.local/share/icons/hicolor/*/apps/texoria.png` |
| Homebrew | `brew uninstall --cask texoria` (`--zap` also removes the settings) |
| AUR | `sudo pacman -R texoria-bin` |
| `.deb` | `sudo apt remove texoria` |
| `.rpm` | `sudo dnf remove texoria` |
| Windows | *Settings* → *Apps* → *TeXoria* → *Uninstall* |

Settings and downloaded dictionaries stay in place: see [where TeXoria stores its
files](settings.md#where-files-are-stored) to remove them too.
