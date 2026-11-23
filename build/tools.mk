# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

TOOLS_DIR := target/tools/$(shell rustc --print host-tuple)
# absolute, tools also run from other directories, e.g. `cd components && wkg fetch`
TOOLS_BIN := $(abspath $(TOOLS_DIR))/bin
# for the scripts and tests the recipes run, they look up the tools on the PATH
export PATH := $(TOOLS_BIN):$(PATH)

# the tools the build provides, pinned in build/tools/Cargo.toml, and those the repository pins in
# its own tools/Cargo.toml, optional: a tool it pins overrides the build's, others it only uses
# itself. Generated when either changes, make restarts after generating it. Defines TOOLS,
# TOOLS_PINNED with the version of each tool, e.g. `wkg@0.16.1`, and a variable for each binary a
# tool installs, e.g. `WKG := $(TOOLS_BIN)/wkg`. Recipes run the pinned tools by path, make 3.81 runs
# a simple command without a shell and looks it up on its own PATH, before the export above, which
# would find a tool already on the system. A binary set on the command line is run in its place,
# e.g. `make WKG=../wkg/target/release/wkg`.
TOOLS_MANIFESTS := build/tools/Cargo.toml $(wildcard tools/Cargo.toml)
TOOLS_MK := target/tools/pinned.mk
include $(TOOLS_MK)
# generated again when tools/Cargo.toml is added or removed, it records the manifests it read
ifneq ($(strip $(TOOLS_MANIFESTS_PINNED)),$(strip $(TOOLS_MANIFESTS)))
$(TOOLS_MK): tools-manifests-changed
.PHONY: tools-manifests-changed
endif

# the tools built from this repository rather than pinned, e.g. a cli the
# components are built with, `LOCAL_TOOLS := componentized-constants-cli`. Each is a package of the
# workspace, installed with the pinned tools, and installed again when its sources change, its own or
# those of the workspace packages it depends on. A local tool replaces a pinned tool of the same name,
# one the build provides too, e.g. componentized/constants builds its own constants cli.
# Generated when the workspace changes, defines LOCAL_TOOL_PATH_<package>, LOCAL_TOOL_SOURCES_<package>
# and a variable for each binary the package installs, like the pinned tools, e.g.
# `CONSTANTS := $(TOOLS_BIN)/constants`.
LOCAL_TOOLS ?=
LOCAL_TOOLS_MK := target/tools/local.mk
ifneq ($(strip $(LOCAL_TOOLS)),)
# quietly, the first time make generates it before reading it
-include $(LOCAL_TOOLS_MK)
endif

# the toolchain pinned in rust-toolchain.toml, named for `cargo install`, otherwise rustup warns that
# the toolchain file overrides the default
RUST_TOOLCHAIN := $(shell sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml 2> /dev/null)


.PHONY: clean-tools ## Remove the installed tools, installed again by `make tools`
clean-tools:
	rm -rf ${TOOLS_DIR}

# each tool is a dependency of a tools package, the binaries it installs are listed in
# [package.metadata.bins]. The repository's tools/Cargo.toml is read last, overriding the build's.
$(TOOLS_MK): $(TOOLS_MANIFESTS) build/pinned.jq
	@mkdir -p $(@D)
	@set -o pipefail; for manifest in $(TOOLS_MANIFESTS); do cargo metadata --manifest-path $$manifest --no-deps --format-version 1; done | jq -rs -f build/pinned.jq > $@.tmp
	@echo "TOOLS_MANIFESTS_PINNED := $(TOOLS_MANIFESTS)" >> $@.tmp
	@mv $@.tmp $@

# the binaries that print shell completions and their arguments, read by build/scripts/completions.sh
# as a shell starts, see [package.metadata.completions] in build/tools/Cargo.toml. Generated with the
# tools, the repository's tools/Cargo.toml is read last, overriding the build's.
TOOLS_COMPLETIONS := target/tools/completions

tools: $(TOOLS_COMPLETIONS)

$(TOOLS_COMPLETIONS): $(TOOLS_MANIFESTS) build/completions.jq
	@mkdir -p $(@D)
	@set -o pipefail; for manifest in $(TOOLS_MANIFESTS); do cargo metadata --manifest-path $$manifest --no-deps --format-version 1; done | jq -rs -f build/completions.jq > $@.tmp
	@mv $@.tmp $@

# the packages of the workspace, generated again when LOCAL_TOOLS changes. The workspace's members may
# change without changing Cargo.toml or Cargo.lock, `make clean-tools` generates it again.
$(LOCAL_TOOLS_MK): Cargo.toml Cargo.lock $(filter-out $(TOOLS_MK) $(LOCAL_TOOLS_MK),$(MAKEFILE_LIST)) build/local.jq
	@mkdir -p $(@D)
	@set -o pipefail; cargo metadata --no-deps --format-version 1 | jq -r --arg tools "$(LOCAL_TOOLS)" -f build/local.jq > $@.tmp
	@mv $@.tmp $@

tool_version = $(if $(filter $(1),$(LOCAL_TOOLS)),local,$(patsubst $(1)@%,%,$(filter $(1)@%,$(TOOLS_PINNED))))
# a stamp naming the version of a tool installed in $(TOOLS_DIR)/bin, e.g. `wkg@0.16.1`, the binary
# does not say which version it is. Bumping the pinned version names a stamp that does not exist yet,
# so the tool is installed again. A local tool's stamp is `<package>@local`.
tool = $(TOOLS_DIR)/.installed/$(1)@$(call tool_version,$(1))

# a tool a recipe runs: a local tool is a prerequisite, what's made with it is made again when it
# changes, a pinned tool only has to be installed first, e.g. `$(call tool_prerequisite,wkg)`. Last
# among the prerequisites, it may be order-only.
tool_prerequisite = $(if $(filter $(1),$(LOCAL_TOOLS)),$(call tool,$(1)),| $(call tool,$(1)))

.PHONY: tools ## Install the cli tools the build provides, those pinned in tools/Cargo.toml, and those built from the workspace
tools: $(foreach name,$(sort $(TOOLS) $(LOCAL_TOOLS)),$(call tool,$(name)))

.PHONY: tools-path ## Print the directory of the installed tools for this platform, to add to the PATH
tools-path:
	@echo $(TOOLS_BIN)

# installed first, it installs the others. The binary of the release for the host, see
# build/scripts/install-cargo-binstall.sh, or else, for a host without one, built with cargo install.
$(call tool,cargo-binstall): build/scripts/install-cargo-binstall.sh
	build/scripts/install-cargo-binstall.sh $(call tool_version,cargo-binstall) $(abspath $(TOOLS_DIR)) $(notdir $(TOOLS_DIR)) \
		|| cargo $(if $(RUST_TOOLCHAIN),+$(RUST_TOOLCHAIN)) install --locked --root $(TOOLS_DIR) --version $(call tool_version,cargo-binstall) cargo-binstall
	@mkdir -p $(@D)
	@rm -f $(@D)/cargo-binstall@*
	@touch $@

define INSTALL_TOOL

$(call tool,$1): | $(call tool,cargo-binstall)
	$(CARGO_BINSTALL) --no-confirm --disable-telemetry --locked --root $(TOOLS_DIR) --version $(call tool_version,$1) $1
	@mkdir -p $$(@D)
	@# only the installed version has a stamp, so going back to a previous version installs it again
	@rm -f $$(@D)/$1@*
	@touch $$@

endef

$(foreach name,$(filter-out cargo-binstall $(LOCAL_TOOLS),$(TOOLS)),$(eval $(call INSTALL_TOOL,$(name))))

define INSTALL_LOCAL_TOOL

$(call tool,$1): Cargo.toml Cargo.lock $(wildcard rust-toolchain.toml) $(if $(LOCAL_TOOL_SOURCES_$1),$(shell find $(LOCAL_TOOL_SOURCES_$1) -type f))
	@# forced, make only installs when the sources change, and the binary may belong to another package
	cargo $(if $(RUST_TOOLCHAIN),+$(RUST_TOOLCHAIN)) install --force --locked --root $(TOOLS_DIR) --path $(LOCAL_TOOL_PATH_$1)
	@mkdir -p $$(@D)
	@# going back to the pinned version installs it again
	@rm -f $$(@D)/$1@*
	@touch $$@

endef

$(foreach name,$(LOCAL_TOOLS),$(eval $(call INSTALL_LOCAL_TOOL,$(name))))

# cosign, which signs the published components and attests their SBOMs, see build/scripts/publish.sh,
# and verifies the SBOM attested to a component pulled with wkg, see build/scripts/sbom.sh. A binary
# of the release for the host, checked against the release's checksums, run as `$(COSIGN)`.
COSIGN_VERSION := 3.1.4
COSIGN_HOST := $(or $(word 2,$(subst :, ,$(filter $(shell rustc --print host-tuple):%,aarch64-apple-darwin:darwin-arm64 x86_64-apple-darwin:darwin-amd64 aarch64-unknown-linux-gnu:linux-arm64 x86_64-unknown-linux-gnu:linux-amd64))),unsupported-host)
COSIGN := $(TOOLS_BIN)/cosign
COSIGN_INSTALLED := $(TOOLS_DIR)/.installed/cosign@$(COSIGN_VERSION)

tools: $(COSIGN_INSTALLED)

$(COSIGN_INSTALLED):
	@mkdir -p $(TOOLS_BIN) $(@D)
	@curl -fsSL -o $(COSIGN).download https://github.com/sigstore/cosign/releases/download/v$(COSIGN_VERSION)/cosign-$(COSIGN_HOST)
	@expected=$$(curl -fsSL https://github.com/sigstore/cosign/releases/download/v$(COSIGN_VERSION)/cosign_checksums.txt | awk '$$2 == "cosign-$(COSIGN_HOST)" { print $$1 }'); \
		actual=$$( (sha256sum $(COSIGN).download 2> /dev/null || shasum -a 256 $(COSIGN).download) | cut -d' ' -f1); \
		if [ -z "$$expected" ] || [ "$$expected" != "$$actual" ]; then \
			rm -f $(COSIGN).download; \
			echo "cosign-$(COSIGN_HOST) $(COSIGN_VERSION) has the SHA-256 $$actual, not the $${expected:-unknown} of the release's checksums" >&2; \
			exit 1; \
		fi
	@chmod +x $(COSIGN).download
	@mv $(COSIGN).download $(COSIGN)
	@rm -f $(@D)/cosign@*
	@touch $@

ifneq ($(filter wasm32-wasip3,$(COMPONENT_TARGETS)),)

# wasi-sdk, for its experimental sysroot with a wasi-libc built for component model cooperative
# threads, see build/components.mk. Installed with the tools, set WASI_SDK_PATH to use an existing install.
WASI_SDK_VERSION := 34.0
WASI_SDK_HOST := $(or $(word 2,$(subst :, ,$(filter $(shell rustc --print host-tuple):%,aarch64-apple-darwin:arm64-macos x86_64-apple-darwin:x86_64-macos aarch64-unknown-linux-gnu:arm64-linux x86_64-unknown-linux-gnu:x86_64-linux))),unsupported-host)
WASI_SDK_DIR := $(abspath $(TOOLS_DIR))/wasi-sdk-$(WASI_SDK_VERSION)
WASI_SDK_PATH ?= $(WASI_SDK_DIR)
WASI_SDK := $(TOOLS_DIR)/.installed/wasi-sdk@$(WASI_SDK_VERSION)

tools: $(WASI_SDK)

$(WASI_SDK):
	@mkdir -p $(WASI_SDK_DIR) $(@D)
	@set -o pipefail; curl -fsSL https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-$(basename $(WASI_SDK_VERSION))/wasi-sdk-$(WASI_SDK_VERSION)-$(WASI_SDK_HOST).tar.gz | tar xz --strip-components 1 -C $(WASI_SDK_DIR)
	@rm -f $(@D)/wasi-sdk@*
	@touch $@

# a wasi-libc commit built for component model cooperative threads, in place of wasi-sdk's
# experimental sysroot, for fixes not yet in a wasi-sdk release, e.g. `WASI_LIBC_VERSION :=
# f6e9006376e6f8e8f91ab84fc956be4e345588bc` for WebAssembly/wasi-libc#884. Empty uses wasi-sdk's.
WASI_LIBC_VERSION ?=
# the WASI 0.3 version the wasi-libc built from WASI_LIBC_VERSION imports, in place of the one it's
# generated for, e.g. `0.3.0` to match the WIT of the components. The WIT of the patch releases of
# WASI 0.3 is the same but for docs, only the versions in the generated bindings are replaced.
# Empty keeps wasi-libc's.
WASI_LIBC_WASI_VERSION ?=

ifeq ($(WASI_LIBC_VERSION),)

WASI_SYSROOT := $(WASI_SDK_PATH)/share/wasi-sysroot/experimental-coop-threads
WASI_LIBC :=

else

WASI_LIBC_BUILD := $(WASI_LIBC_VERSION)$(if $(WASI_LIBC_WASI_VERSION),-wasi-$(WASI_LIBC_WASI_VERSION))
WASI_SYSROOT := $(abspath $(TOOLS_DIR))/wasi-libc-$(WASI_LIBC_BUILD)
WASI_LIBC := $(TOOLS_DIR)/.installed/wasi-libc@$(WASI_LIBC_BUILD)

# the bindings wasi-libc generates for WASI 0.3 from its WIT, checked in, and the version they're
# generated for
WASI_LIBC_WASIP3_BINDINGS := libc-bottom-half/headers/public/wasi/__generated_wasip3.h libc-bottom-half/sources/wasip3.c libc-bottom-half/sources/wasip3_component_type.o libc-bottom-half/crt/crt1-command.c
wasi_libc_wasip3_version = $$(sed -n 's/^set(wasip3-version \(.*\))$$/\1/p' $(1)/CMakeLists.txt)

# cmake and ninja build wasi-libc, installed with the tools
CMAKE_VERSION := 4.4.4
CMAKE_HOST := $(or $(word 2,$(subst :, ,$(filter $(shell rustc --print host-tuple):%,aarch64-apple-darwin:macos-universal x86_64-apple-darwin:macos-universal aarch64-unknown-linux-gnu:linux-aarch64 x86_64-unknown-linux-gnu:linux-x86_64))),unsupported-host)
CMAKE_DIR := $(abspath $(TOOLS_DIR))/cmake-$(CMAKE_VERSION)
# the macos archive holds an app bundle
CMAKE := $(CMAKE_DIR)/$(if $(filter macos-%,$(CMAKE_HOST)),CMake.app/Contents/)bin/cmake
CMAKE_INSTALLED := $(TOOLS_DIR)/.installed/cmake@$(CMAKE_VERSION)

NINJA_VERSION := 1.13.2
NINJA_HOST := $(or $(word 2,$(subst :, ,$(filter $(shell rustc --print host-tuple):%,aarch64-apple-darwin:mac x86_64-apple-darwin:mac aarch64-unknown-linux-gnu:linux-aarch64 x86_64-unknown-linux-gnu:linux))),unsupported-host)
NINJA_DIR := $(abspath $(TOOLS_DIR))/ninja-$(NINJA_VERSION)
NINJA := $(NINJA_DIR)/ninja
NINJA_INSTALLED := $(TOOLS_DIR)/.installed/ninja@$(NINJA_VERSION)

tools: $(WASI_LIBC)

$(CMAKE_INSTALLED):
	@rm -rf $(CMAKE_DIR)
	@mkdir -p $(CMAKE_DIR) $(@D)
	@set -o pipefail; curl -fsSL https://github.com/Kitware/CMake/releases/download/v$(CMAKE_VERSION)/cmake-$(CMAKE_VERSION)-$(CMAKE_HOST).tar.gz | tar xz --strip-components 1 -C $(CMAKE_DIR)
	@rm -f $(@D)/cmake@*
	@touch $@

$(NINJA_INSTALLED):
	@rm -rf $(NINJA_DIR)
	@mkdir -p $(NINJA_DIR) $(@D)
	@curl -fsSL https://github.com/ninja-build/ninja/releases/download/v$(NINJA_VERSION)/ninja-$(NINJA_HOST).zip -o $(NINJA_DIR)/ninja.zip
	@unzip -q -o $(NINJA_DIR)/ninja.zip -d $(NINJA_DIR)
	@rm $(NINJA_DIR)/ninja.zip
	@rm -f $(@D)/ninja@*
	@touch $@

# built with wasi-sdk's clang, static libraries only, rust links libc statically and wasi-sdk's
# compiler-rt builtins, which a shared libc links, aren't built for cooperative threads
$(WASI_LIBC): | $(WASI_SDK) $(CMAKE_INSTALLED) $(NINJA_INSTALLED)
	@rm -rf $(WASI_SYSROOT) $(WASI_SYSROOT)-src
	@mkdir -p $(WASI_SYSROOT)-src $(@D)
	@set -o pipefail; curl -fsSL https://github.com/WebAssembly/wasi-libc/archive/$(WASI_LIBC_VERSION).tar.gz | tar xz --strip-components 1 -C $(WASI_SYSROOT)-src
ifneq ($(WASI_LIBC_WASI_VERSION),)
	@# the type section of the bindings is binary, a version of the same length keeps it valid
	@from=$(call wasi_libc_wasip3_version,$(WASI_SYSROOT)-src); to=$(WASI_LIBC_WASI_VERSION); \
		if [ -z "$$from" ] || [ $${#from} -ne $${#to} ]; then echo "can't replace WASI $$from of wasi-libc with $$to" >&2; exit 1; fi; \
		cd $(WASI_SYSROOT)-src && perl -0777 -pi -e "s/\@\Q$$from\E/\@$$to/g" $(WASI_LIBC_WASIP3_BINDINGS)
endif
	$(CMAKE) -S $(WASI_SYSROOT)-src -B $(WASI_SYSROOT)-src/build -G Ninja -DCMAKE_MAKE_PROGRAM=$(NINJA) \
		-DCMAKE_C_COMPILER=$(WASI_SDK_PATH)/bin/clang -DCMAKE_AR=$(WASI_SDK_PATH)/bin/llvm-ar \
		-DCMAKE_NM=$(WASI_SDK_PATH)/bin/llvm-nm -DCMAKE_RANLIB=$(WASI_SDK_PATH)/bin/llvm-ranlib \
		-DTARGET_TRIPLE=wasm32-wasip3 -DENABLE_COOP_THREADS=ON -DBUILD_SHARED=OFF \
		-DBUILTINS_LIB=$$(ls $(WASI_SDK_PATH)/lib/clang/*/lib/wasm32-unknown-wasip3/libclang_rt.builtins.a) \
		-DCMAKE_INSTALL_PREFIX=$(WASI_SYSROOT)
	$(NINJA) -C $(WASI_SYSROOT)-src/build install
	@rm -rf $(WASI_SYSROOT)-src
	@rm -f $(@D)/wasi-libc@*
	@touch $@

endif

endif
