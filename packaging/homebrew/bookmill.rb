# Homebrew cask for the bookmill desktop app.
#
# ┌───────────────────────────────────────────────────────────────────────────┐
# │ NOT INSTALLABLE YET. There is no published GitHub Release, no signed +      │
# │ notarized DMG, and no `framallo/homebrew-tap`, so `brew install --cask      │
# │ bookmill` will FAIL (404 on the url below). This file is the *source of      │
# │ truth* for when those exist — see docs/DESKTOP.md §"Human-only remaining     │
# │ steps" for exactly what is blocking (Apple Developer signing/notarization,  │
# │ a published Release, and the tap repo).                                     │
# └───────────────────────────────────────────────────────────────────────────┘
#
# On release: copy this into the tap repo (framallo/homebrew-tap) as
# Casks/bookmill.rb, bump `version`, and replace `sha256 :no_check` with the real
# DMG sha256 that the release workflow prints (see release-desktop.yml, which
# normalizes the release asset to the canonical `bookmill_<version>_universal.dmg`
# name this cask references).
#
# TODO(publish): pin the real sha256 + version from the first framallo/bookmill
# Release before publishing the tap. Until then this cask is a placeholder.
cask "bookmill" do
  version "0.1.0"
  # TODO(publish): replace with the DMG sha256 from the release workflow. `:no_check`
  # here is a deliberate placeholder — the cask is not meant to install as-is.
  sha256 :no_check

  # The release workflow uploads the universal DMG under exactly this name (see
  # release-desktop.yml → "Normalize DMG asset name"), so this url stays in
  # lockstep with the produced artifact.
  url "https://github.com/framallo/bookmill/releases/download/v#{version}/bookmill_#{version}_universal.dmg",
      verified: "github.com/framallo/bookmill/"
  name "bookmill"
  desc "Native desktop UI for the bookmill book-publishing pipeline"
  homepage "https://github.com/framallo/bookmill"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :catalina"

  app "bookmill.app"

  # Optional: symlink the bundled CLI onto PATH so `bookmill` works from a shell
  # after installing the cask. Remove if you install the CLI separately.
  binary "#{appdir}/bookmill.app/Contents/Resources/bookmill"

  zap trash: [
    "~/Library/Application Support/io.framallo.bookmill",
    "~/Library/Caches/io.framallo.bookmill",
    "~/Library/Logs/io.framallo.bookmill",
    "~/Library/WebKit/io.framallo.bookmill",
  ]
end
