# Homebrew cask for the bookmill desktop app.
#
# Install (once this is published to the tap — see docs/DESKTOP.md):
#   brew tap framallo/tap
#   brew install --cask bookmill
#
# This file is the source of truth; on release, copy it into the tap repo
# (framallo/homebrew-tap) as Casks/bookmill.rb and fill in the real `version`
# and `sha256` (the release workflow prints the DMG sha256).
#
# TODO(publish): replace the placeholder version + sha256 with the values from
# the first GitHub Release of framallo/bookmill (see docs/DESKTOP.md §Release).
cask "bookmill" do
  version "0.1.0"
  sha256 :no_check # TODO: pin the real DMG sha256 once a release exists.

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
