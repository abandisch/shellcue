PLATFORM := $(shell uname -s)
ARCH ?= x86_64

$(warning PLATFORM is $(PLATFORM))

ifeq "$(PLATFORM)" "Darwin"
# Add brew executables to PATH
export PATH := /opt/homebrew/bin:/opt/homebrew/sbin:$(PATH)
endif

# Add cargo executables to PATH
export PATH := $(HOME)/.cargo/bin:$(PATH)

SHELL := env PATH=$(PATH) /bin/bash

# Check if cargo exists
CARGO := $(shell command -v cargo 2> /dev/null)

ifndef CARGO
    $(error "cargo is not in PATH $(PATH)")
endif

MAKE_DIR    ?= $(shell dirname $(realpath $(firstword $(MAKEFILE_LIST))))
TARGET_DIR  ?= $(shell cargo metadata --format-version 1 | jq -r .target_directory)

BUILT_PRODUCTS_DIR ?= $(MAKE_DIR)/build
OUT_DIR            := $(BUILT_PRODUCTS_DIR)
$(shell mkdir -p $(OUT_DIR))


CARGO_OPTIONS ?= --locked --release

BINARY_NAME ?= fig_desktop
CRATE_NAME  ?= fig_desktop

### Build for each platform

ifeq "$(PLATFORM)" "Darwin"
all: macos
endif
ifeq "$(PLATFORM)" "Linux"
all: linux
endif

macos: $(BINARY_NAME)-darwin-universal
	install $(OUT_DIR)/$< $(OUT_DIR)/$(BINARY_NAME)

$(BINARY_NAME)-x86_64-apple-darwin:
	cargo build --target=x86_64-apple-darwin $(CARGO_OPTIONS)
	install $(TARGET_DIR)/x86_64-apple-darwin/release/$(CRATE_NAME) $(OUT_DIR)/$@

$(BINARY_NAME)-aarch64-apple-darwin:
	cargo build --target=aarch64-apple-darwin $(CARGO_OPTIONS)
	install $(TARGET_DIR)/aarch64-apple-darwin/release/$(CRATE_NAME) $(OUT_DIR)/$@

$(BINARY_NAME)-darwin-universal:
	cargo build --target=x86_64-apple-darwin --target=aarch64-apple-darwin $(CARGO_OPTIONS)
	install $(TARGET_DIR)/x86_64-apple-darwin/release/$(CRATE_NAME) $(OUT_DIR)/$(BINARY_NAME)-x86_64-apple-darwin
	install $(TARGET_DIR)/aarch64-apple-darwin/release/$(CRATE_NAME) $(OUT_DIR)/$(BINARY_NAME)-aarch64-apple-darwin
	lipo -create -output $(OUT_DIR)/$@ $(OUT_DIR)/$(BINARY_NAME)-x86_64-apple-darwin $(OUT_DIR)/$(BINARY_NAME)-aarch64-apple-darwin

linux: $(BINARY_NAME)-$(ARCH)-unknown-linux-gnu
	install $(OUT_DIR)/$< $(OUT_DIR)/$(BINARY_NAME)

$(BINARY_NAME)-x86_64-unknown-linux-gnu:
	cargo build --target=x86_64-unknown-linux-gnu $(CARGO_OPTIONS)
	install $(TARGET_DIR)/x86_64-unknown-linux-gnu/release/$(CRATE_NAME) $(OUT_DIR)/$@

$(BINARY_NAME)-aarch64-unknown-linux-gnu:
	cargo build --target=aarch64-unknown-linux-gnu $(CARGO_OPTIONS)
	install $(TARGET_DIR)/aarch64-unknown-linux-gnu/release/$(CRATE_NAME) $(OUT_DIR)/$@

### Build and Install for local dev

build-native:
	cargo build

build-native-release:
	cargo build --release

install-native-usr: build-native
	sudo install $(TARGET_DIR)/debug/$(CRATE_NAME) /usr/bin/$(BINARY_NAME)

install-native-usr-release: build-native-release
	sudo install $(TARGET_DIR)/release/$(CRATE_NAME) /usr/bin/$(BINARY_NAME)

install-macos-bundle: build-native
	mkdir -p /Applications/ShellCue.app/Contents/MacOS/
	install $(TARGET_DIR)/debug/$(CRATE_NAME) /Applications/ShellCue.app/Contents/MacOS/shellcue-desktop

### Clean the project

clean:
	command -v cargo &>/dev/null && cargo clean
	rm -rf $(BUILT_PRODUCTS_DIR)

.PHONY: clean

