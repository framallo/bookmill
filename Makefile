# bookmill desktop app — build & run helpers.
#
# The desktop app is a Tauri v2 shell (desktop/src-tauri) that spawns `bookmill
# web` and shows it in a native macOS window. These targets stage the `bookmill`
# CLI as the app's sidecar, then run or bundle the app.
#
#   make run      # fast dev window (cargo tauri dev) on REPO
#   make app      # build bookmill.app bundle
#   make dmg      # build the installable .dmg
#   make open     # open the already-built .app on REPO
#   make install  # copy bookmill.app into /Applications
#
# Point it at a different book repo with:  make run REPO=~/path/to/repo

REPO    ?= $(HOME)/work/argentina_animal_libertaria
TAURI   := desktop/src-tauri
APP     := $(TAURI)/target/release/bundle/macos/bookmill.app
BIN      = $(abspath target/release/bookmill)

.PHONY: run dev app dmg open install sidecar cli clean help

help:
	@grep -E '^#   make ' Makefile | sed 's/^#   /  /'

## Build the release CLI and stage it as the app's sidecar.
cli:
	cargo build --release

sidecar: cli
	@mkdir -p $(TAURI)/resources
	cp target/release/bookmill $(TAURI)/resources/bookmill

## Fast dev loop: opens the native window with live Rust rebuilds. The dev shell
## PATH includes Homebrew, so the previewer's pdftoppm is found.
run dev: cli
	cd $(TAURI) && BOOKMILL_BIN=$(BIN) cargo tauri dev -- -- --repo "$(REPO)"

## Build the distributable .app (embeds the CLI sidecar).
app: sidecar
	cd $(TAURI) && cargo tauri build

## Build the .dmg (uses the headless-safe hdiutil script).
dmg: app
	./packaging/make-dmg.sh

## Open the already-built .app on REPO (build it first with `make app`).
open:
	@test -d "$(APP)" || { echo "no bundle yet — run 'make app'"; exit 1; }
	open "$(APP)" --args --repo "$(REPO)"

## Install the built app into /Applications.
install: app
	rm -rf /Applications/bookmill.app
	cp -R "$(APP)" /Applications/bookmill.app
	@echo "installed → /Applications/bookmill.app (launch 'bookmill' from Spotlight)"

clean:
	cd $(TAURI) && cargo clean
