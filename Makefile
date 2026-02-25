# Harman Kardon Invoke Community Firmware — Top-level Makefile
#
# All build outputs go to build/ in the repo root.
#
# On Windows, Linux tasks route through WSL automatically.
# On Linux (native or CI), everything runs directly.

SHELL := bash

# Auto-detect OS: on Windows, route Linux commands through WSL.
# On Linux (native or CI), run directly.
IS_WINDOWS := $(shell uname -s 2>/dev/null | grep -qi mingw && echo 1 || (uname -s 2>/dev/null | grep -qi msys && echo 1 || echo 0))

ifeq ($(IS_WINDOWS),1)
  # Windows: delegate Linux tasks to WSL with login shell for PATH
  WSL := wsl.exe -d Ubuntu -u root -- bash -lc
  REPO_WIN := $(CURDIR)
  REPO_WSL = $(shell wsl.exe -d Ubuntu -- bash -c "wslpath '$(CURDIR)'" 2>/dev/null)
  RUN_LINUX = $(WSL) "cd '$(REPO_WSL)' && $(1)"
else
  # Linux (native, CI, WSL shell): run directly
  RUN_LINUX = bash -lc "cd '$(CURDIR)' && $(1)"
endif

.PHONY: help test firmware wasm encore app-dist app-icons app app-dev app-android app-android-dev kernel kernel-vendor kexec download clean clean-kernel clean-kernel-vendor clean-kexec clean-all distclean verify check-wsl

# Default target — show usage
.DEFAULT_GOAL := help

# Verify WSL is available (Windows only — Linux skips this)
check-wsl:
ifeq ($(IS_WINDOWS),1)
	@if [ -z "$(REPO_WSL)" ]; then \
		echo "ERROR: Could not detect WSL path for $(REPO_WIN)"; \
		echo "  Ensure WSL Ubuntu is running: wsl -d Ubuntu echo ok"; \
		echo "  Or set REPO_WSL manually: make REPO_WSL=/mnt/g/HKInvoke firmware"; \
		exit 1; \
	fi
endif

help:
	@echo "Harman Kardon Invoke Community Firmware"
	@echo ""
	@echo "Targets:"
	@echo "  make encore           Build WASM dashboard + ARM binary"
	@echo "  make firmware         Build full firmware image"
	@echo "  make wasm             Build WASM web dashboard only"
	@echo "  make test             Run Rust tests"
	@echo "  make app              Build desktop app"
	@echo "  make app-dev          Run desktop app in dev mode"
	@echo "  make app-android      Build Android app"
	@echo "  make app-android-dev  Run Android app in dev mode"
	@echo "  make kernel           Build 6.1 kernel for kexec"
	@echo "  make kernel-vendor    Build vendor 3.8.13 kernel"
	@echo "  make kexec            Build kexec module + tools"
	@echo "  make download         Set up stock firmware image"
	@echo "  make verify           Check for credential leaks"
	@echo "  make clean            Remove build/ and Cargo artifacts"
	@echo "  make clean-all        Clean everything"
	@echo ""
	@echo "Workflow:  make download, make firmware, flash"
	@echo "Output:    build/encore, build/firmware/, build/desktop/, build/android/"

test:
	cd encore && cargo test --workspace --exclude encore-app --exclude encore-wasm

firmware: encore check-wsl
	$(call RUN_LINUX,bash scripts/build/build_firmware.sh)

wasm:
	wasm-pack build encore/crates/encore-wasm --target web --out-dir ../../web/pkg

encore: wasm
	bash scripts/build/build_encore.sh

# Assemble web dashboard + branding into build/app-dist/ for Tauri
app-dist: wasm
	@echo "=== Assembling app-dist ==="
	rm -rf build/app-dist
	mkdir -p build/app-dist/branding
	cp -r encore/web/* build/app-dist/
	cp branding/*.png build/app-dist/branding/

app-icons:
	@echo "=== Generating Tauri app icons ==="
	cd encore/crates/encore-app && cargo tauri icon ../../../branding/pwa-icon-512-dark.png

app: app-dist app-icons
	@# Clear WebView2 cache to prevent stale frontend content
	-taskkill //F //IM msedgewebview2.exe 2>/dev/null; true
	rm -rf "$(LOCALAPPDATA)/com.encore.speaker" 2>/dev/null; true
	cd encore/crates/encore-app && cargo tauri build
	@echo "=== Collecting desktop build outputs ==="
	mkdir -p build/desktop
	cp -f encore/target/release/bundle/msi/*.msi build/desktop/ 2>/dev/null; true
	cp -f encore/target/release/bundle/nsis/*-setup.exe build/desktop/ 2>/dev/null; true
	cp -f encore/target/release/bundle/dmg/*.dmg build/desktop/ 2>/dev/null; true
	cp -f encore/target/release/bundle/deb/*.deb build/desktop/ 2>/dev/null; true
	cp -f encore/target/release/bundle/appimage/*.AppImage build/desktop/ 2>/dev/null; true
	@echo "  Output: build/desktop/"
	@ls -lh build/desktop/

app-dev: app-dist app-icons
	cd encore/crates/encore-app && cargo tauri dev

ANDROID_BUILD_TOOLS = $(shell ls -d "$(LOCALAPPDATA)/Android/Sdk/build-tools"/*/ 2>/dev/null | tail -1)

app-android: app-dist app-icons
	cd encore/crates/encore-app && cargo tauri android build
	@echo "=== Signing and collecting Android build ==="
	mkdir -p build/android
	@if [ ! -f build/encore-debug.keystore ]; then \
		echo "  Generating debug keystore..."; \
		keytool -genkeypair \
			-keystore "$(CURDIR)\build\encore-debug.keystore" \
			-alias encore -keyalg RSA -keysize 2048 -validity 10000 \
			-storepass encore123 -keypass encore123 \
			-dname "CN=Encore Debug, O=Encore, C=US" 2>/dev/null; \
	fi
	"$(ANDROID_BUILD_TOOLS)zipalign.exe" -f 4 \
		"$(CURDIR)\encore\crates\encore-app\gen\android\app\build\outputs\apk\universal\release\app-universal-release-unsigned.apk" \
		"$(CURDIR)\build\android\app-aligned.apk"
	"$(ANDROID_BUILD_TOOLS)apksigner.bat" sign \
		--ks "$(CURDIR)\build\encore-debug.keystore" \
		--ks-key-alias encore --ks-pass pass:encore123 --key-pass pass:encore123 \
		--out "$(CURDIR)\build\android\Encore.apk" \
		"$(CURDIR)\build\android\app-aligned.apk"
	rm -f build/android/app-aligned.apk
	cp -f encore/crates/encore-app/gen/android/app/build/outputs/bundle/universalRelease/*.aab build/android/ 2>/dev/null; true
	@echo "  Output: build/android/"
	@ls -lh build/android/

app-android-dev: app-dist app-icons
	cd encore/crates/encore-app && cargo tauri android dev

kernel: check-wsl
	$(call RUN_LINUX,bash scripts/build/build_kernel.sh)

kernel-vendor: check-wsl
	$(call RUN_LINUX,bash scripts/build/build_kernel_vendor.sh)

kexec: check-wsl
	$(call RUN_LINUX,bash scripts/build/build_kexec.sh)

download: check-wsl
	$(call RUN_LINUX,bash scripts/build/download_firmware.sh)

clean:
	rm -rf build
	cd encore && cargo clean 2>/dev/null; true
	rm -rf encore/web/pkg

clean-kernel: check-wsl
	$(call RUN_LINUX,bash scripts/build/build_kernel.sh clean)

clean-kernel-vendor: check-wsl
	$(call RUN_LINUX,bash scripts/build/build_kernel_vendor.sh clean)

clean-kexec: check-wsl
	$(call RUN_LINUX,bash scripts/build/build_kexec.sh clean)

clean-all: clean clean-kernel clean-kernel-vendor clean-kexec

distclean: clean-all
	@echo "=== Removing build directory ==="
	$(call RUN_LINUX,rm -rf ~/hkinvoke-build)
	@echo "Done"

verify:
	bash scripts/verify.sh
