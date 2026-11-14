# Sift — build, test, install into ~/.local (no root needed).
PREFIX ?= $(HOME)/.local
BINDIR  = $(PREFIX)/bin
APPDIR  = $(PREFIX)/share/applications
ICONROOT = $(PREFIX)/share/icons/hicolor
UNITDIR = $(HOME)/.config/systemd/user
CARGO  ?= cargo

.PHONY: all build test run run-cli install uninstall clean screenshots

all: build

build:
	$(CARGO) build --release

test:
	$(CARGO) test --workspace

run:
	$(CARGO) run -p sift-gui

run-cli:
	$(CARGO) run -p sift-cli -- status

install: build
	install -Dm755 target/release/sift $(BINDIR)/sift
	install -Dm755 target/release/sift-gui $(BINDIR)/sift-gui
	install -Dm644 data/dev.sift.Sift.desktop $(APPDIR)/dev.sift.Sift.desktop
	sed -i 's|^Exec=sift-gui|Exec=$(BINDIR)/sift-gui|' $(APPDIR)/dev.sift.Sift.desktop
	cd data/icons/hicolor && for f in */apps/dev.sift.Sift.*; do install -Dm644 "$$f" "$(ICONROOT)/$$f"; done
	# COSMIC's dock resolves absolute icon paths most reliably.
	sed -i 's|^Icon=dev.sift.Sift|Icon=$(ICONROOT)/256x256/apps/dev.sift.Sift.png|' $(APPDIR)/dev.sift.Sift.desktop
	install -Dm644 data/sift-sampler.service $(UNITDIR)/sift-sampler.service
	-update-desktop-database $(APPDIR) 2>/dev/null
	-systemctl --user daemon-reload 2>/dev/null
	@echo
	@echo "Installed. Launch Sift from your app launcher, or run: sift-gui"
	@echo "Background history: toggle it in Settings, or: systemctl --user enable --now sift-sampler.service"

uninstall:
	-systemctl --user disable --now sift-sampler.service 2>/dev/null
	rm -f $(BINDIR)/sift $(BINDIR)/sift-gui $(APPDIR)/dev.sift.Sift.desktop $(ICONROOT)/*/apps/dev.sift.Sift.* $(UNITDIR)/sift-sampler.service
	-systemctl --user daemon-reload 2>/dev/null
	@echo "Removed Sift. Your settings (~/.config/sift) and history (~/.local/state/sift) were left in place."

clean:
	$(CARGO) clean

# Render every page to PNG (needs a running desktop session).
screenshots: build
	SIFT_SCREENSHOT_DIR=$(CURDIR)/screenshots target/release/sift-gui
