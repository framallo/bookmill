# bookmill desktop app

A native macOS window for bookmill, built with **Tauri v2**. It shows the same
UI as `bookmill web` — the Alexandria-style library grid, the cover editor, and
the interior previewer — in a real app window instead of a browser tab.

## How it works

The desktop app (`desktop/` — a separate Tauri crate, sibling to `web/`, so the
main `bookmill` binary build is untouched) is a thin shell:

1. On launch it resolves the `bookmill` CLI (env `BOOKMILL_BIN` → next to the
   app executable → the app bundle's `Resources/` → `PATH`). The packaged app
   **embeds the `bookmill` CLI as a sidecar** in `Contents/Resources/`, so it is
   self-contained — no separate `bookmill` install required.
2. It resolves the content repo (`--repo <dir>` / `BOOKMILL_REPO` → walk up from
   the working dir for a `bookmill.toml` → a native "choose folder" dialog).
3. It spawns `bookmill web --port <ephemeral> --repo <root>` and waits for the
   port to accept connections.
4. It navigates the native `WKWebView` window to `http://127.0.0.1:<port>/`.
5. When the window closes, the child `bookmill web` process is killed.

No web route is reimplemented — the app reuses the existing axum server 1:1.

## Running it

It's a normal macOS app — there is **no `bookmill` terminal command to launch
it**. Install and open it like any other app:

```bash
brew install --cask bookmill      # once published (see below)
```

Then launch **bookmill** from Applications or Spotlight. On first open it asks
you to choose your book repo (a folder with a `bookmill.toml`); it remembers the
last one. You can also open the built bundle directly:

```bash
open desktop/src-tauri/target/release/bundle/macos/bookmill.app
# or preselect a repo:
open desktop/src-tauri/target/release/bundle/macos/bookmill.app --args --repo ~/work/argentina_animal_libertaria
```

### Dev loop (editing the app)

```bash
cd desktop/src-tauri
cargo tauri dev -- -- --repo ~/work/argentina_animal_libertaria
# or set the repo via env:
BOOKMILL_REPO=~/work/argentina_animal_libertaria BOOKMILL_BIN=$(which bookmill) cargo tauri dev
```

The app needs a `bookmill` binary at runtime. In dev, `cargo install --path .`
puts it on `PATH`; or set `BOOKMILL_BIN=/path/to/bookmill`.

## Building the bundle (.app + .dmg)

```bash
# Stage the bookmill CLI the app bundles as a sidecar:
cargo build --release
mkdir -p desktop/src-tauri/resources
cp target/release/bookmill desktop/src-tauri/resources/bookmill

# Build the app:
cd desktop/src-tauri
cargo tauri build            # → target/release/bundle/macos/bookmill.app
                             #   target/release/bundle/dmg/bookmill_<ver>_*.dmg
```

For a universal (Intel + Apple Silicon) build, `lipo` the two CLI targets into
`resources/bookmill` first, then `cargo tauri build --target universal-apple-darwin`
(the release CI does exactly this — see `.github/workflows/release-desktop.yml`).

The bundle identifier is `io.framallo.bookmill`; the DMG is named
`bookmill_<version>_<arch>.dmg`.

### DMG in a headless session

`cargo tauri build` builds the `.app` fine but its DMG step (`create-dmg`)
drives Finder via **AppleScript** to style the DMG window — that fails in a
headless / no-GUI-automation session (`error running bundle_dmg.sh`). The `.app`
is still produced under `target/release/bundle/macos/`. To get a functional
(unstyled) DMG without AppleScript:

```bash
packaging/make-dmg.sh   # wraps the built .app into a drag-to-install .dmg via hdiutil
```

On a real GitHub `macos-14` runner (or an interactive Mac session) the default
`create-dmg` step works and produces the styled DMG; the helper is only needed
when AppleScript/Finder is unavailable.

**Status of local build:** `.app` bundle verified working (launches, spawns its
bundled `bookmill` sidecar, serves the library); a `.dmg` was produced locally
via `make-dmg.sh` (unsigned).

## Publishing a release + Homebrew cask

Target install experience:

```bash
brew install --cask bookmill
```

### 1. Cut a signed, notarized release (CI)

`.github/workflows/release-desktop.yml` triggers on a `v*` tag. It builds the
universal CLI sidecar, then runs `tauri-apps/tauri-action`, which builds the
`.app`/`.dmg`, (optionally) signs + notarizes, and uploads them to a **draft**
GitHub Release.

Signing/notarization runs **only if these repo secrets are set** (Apple Developer
Program account required):

| Secret | Meaning |
| --- | --- |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Federico Ramallo (TEAMID)` |
| `APPLE_CERTIFICATE` | base64 of the `.p12` Developer ID cert |
| `APPLE_CERTIFICATE_PASSWORD` | password for that `.p12` |
| `APPLE_ID` | Apple ID email (notarization) |
| `APPLE_PASSWORD` | app-specific password for that Apple ID |
| `APPLE_TEAM_ID` | 10-char team id |

Without them the workflow still produces an **unsigned** DMG (users open it via
right-click → Open, or `xattr -dr com.apple.quarantine bookmill.app`).

Tag and push:

```bash
git tag v0.1.0
git push origin v0.1.0
# then publish the draft release in the GitHub UI.
```

The workflow's last step prints the DMG `sha256` — copy it for the cask.

### 2. Publish the Homebrew cask

The cask source of truth is `packaging/homebrew/bookmill.rb`. To publish:

1. Create a tap repo **`framallo/homebrew-tap`** on GitHub.
2. Copy `packaging/homebrew/bookmill.rb` to `Casks/bookmill.rb` in that repo.
3. Fill in the real `version` and replace `sha256 :no_check` with the DMG
   `sha256` from the release step.
4. Commit + push the tap.

Users then:

```bash
brew tap framallo/tap
brew install --cask bookmill
```

`livecheck { strategy :github_latest }` lets `brew` track new GitHub releases,
so future versions only need the `version` + `sha256` bumped in the cask.

## Human-only remaining steps (checklist)

- [ ] Enroll in the Apple Developer Program; create a **Developer ID Application**
      certificate + an app-specific password, and add the six `APPLE_*` secrets.
- [ ] Create the **`framallo/homebrew-tap`** repo.
- [ ] Push a `v0.1.0` tag, publish the draft GitHub Release.
- [ ] Copy the printed DMG `sha256` into `Casks/bookmill.rb`, push the tap.
- [ ] Verify `brew install --cask bookmill` on a clean machine.

Until a signed release + tap exist, the cask's `url`/`sha256` are placeholders
(`sha256 :no_check`) — clearly marked with `TODO(publish)`.
