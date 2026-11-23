# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# the SBOM of each component and of the interface, CycloneDX JSON beside the wasm file it describes,
# e.g. target/components/greeter/greeter.cdx.json for greeter.wasm and greeter.debug.cdx.json for
# greeter.debug.wasm, built with the components and published with them, see build/scripts/sbom.sh

# the SBOMs cargo-cyclonedx generates for the packages of the workspace, a directory for each target
CARGO_SBOM_DIR := target/sbom/cargo

# the SBOM of each file published, with its version and image, see build/scripts/publish.sh, laid out
# like the components directory, e.g. greeter/greeter.debug.cdx.json for greeter.debug.wasm. The CI
# workflow packs them with the components into the components.tar of the release it drafts, in place
# of the SBOMs of the build.
PUBLISHED_SBOM_DIR := target/sbom/published

# the time the SBOMs are generated at, the time of the commit, so the SBOMs of a commit are the same
# each time they are generated
SBOM_EPOCH := $(or $(shell git log -1 --format=%ct 2> /dev/null),0)

# the version of the cli a component is generated with, a local tool's is its package's version, e.g.
# `$(call tool_crate_version,componentized-constants-cli)`
tool_crate_version = $(if $(filter $(1),$(LOCAL_TOOLS)),$(shell cargo pkgid -p $(1) | sed 's/.*[\#@]//'),$(call tool_version,$(1)))

.PHONY: sboms ## Generate the SBOM of the interface and of each component, built with them by `make components`
sboms: ${COMPONENTS_DIR}/interface.cdx.json $(foreach component,$(COMPONENTS),${COMPONENTS_DIR}/$(component)/$(component).cdx.json ${COMPONENTS_DIR}/$(component)/$(component).debug.cdx.json)

components: sboms

.PHONY: clean-sboms ## Remove the SBOMs cargo-cyclonedx generated and those published, `make clean-components` removes the others
clean-sboms:
	rm -rf $(CARGO_SBOM_DIR) $(PUBLISHED_SBOM_DIR)

clean-components: clean-sboms

# the SBOM of each package of the workspace built for the target, e.g. target/sbom/cargo/wasm32-wasip2.
# A stamp, the directory isn't made newer when an SBOM in it is replaced.
$(CARGO_SBOM_DIR)/%.stamp: Cargo.toml Cargo.lock $(shell find . -name Cargo.toml -not -path './target/*' -not -path './build/tools/*' -not -path './tools/*') build/scripts/sbom-cargo.sh | $(call tool,cargo-cyclonedx)
	CARGO_CYCLONEDX=$(CARGO_CYCLONEDX) SOURCE_DATE_EPOCH=$(SBOM_EPOCH) build/scripts/sbom-cargo.sh $* $(CARGO_SBOM_DIR)/$*
	@touch $@

# `$(call sbom,<kind>,<wasm>,<out>,<args>)`
sbom = SOURCE_DATE_EPOCH=$(SBOM_EPOCH) build/scripts/sbom.sh $(1) $(2) $(3) $(4)

define BUILD_SBOM

ifneq ($(or $(wildcard $(COMPONENT_DIR_$1)/wit/$(COMPONENT_BASE_$1).constants.wit),$(wildcard $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).properties)),)

# a component generated with a cli embeds the cli's code, e.g. the constants cli's
SBOM_CLI_$1 := $(if $(wildcard $(COMPONENT_DIR_$1)/wit/$(COMPONENT_BASE_$1).constants.wit),componentized-constants-cli,componentized-static-config-cli)

${COMPONENTS_DIR}/$1/$1.cdx.json ${COMPONENTS_DIR}/$1/$1.debug.cdx.json: ${COMPONENTS_DIR}/$1/%.cdx.json: ${COMPONENTS_DIR}/$1/%.wasm build/scripts/sbom.sh
	$$(call sbom,generated,$$<,$$@,$$(SBOM_CLI_$1) $$(call tool_crate_version,$$(SBOM_CLI_$1)))

else ifneq ($(wildcard $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wac),)

${COMPONENTS_DIR}/$1/$1.cdx.json: ${COMPONENTS_DIR}/$1/$1.wasm $$(foreach component,$$(WAC_DEPS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).cdx.json) build/scripts/sbom.sh
	$$(call sbom,composition,$$<,$$@,$$(foreach component,$$(WAC_DEPS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).cdx.json))

${COMPONENTS_DIR}/$1/$1.debug.cdx.json: ${COMPONENTS_DIR}/$1/$1.debug.wasm $$(foreach component,$$(WAC_DEPS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).debug.cdx.json) build/scripts/sbom.sh
	$$(call sbom,composition,$$<,$$@,$$(foreach component,$$(WAC_DEPS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).debug.cdx.json))

else ifneq ($(wildcard $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).plug),)

${COMPONENTS_DIR}/$1/$1.cdx.json: ${COMPONENTS_DIR}/$1/$1.wasm $$(foreach component,$$(PLUG_COMPONENTS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).cdx.json) build/scripts/sbom.sh
	$$(call sbom,composition,$$<,$$@,$$(foreach component,$$(PLUG_COMPONENTS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).cdx.json))

${COMPONENTS_DIR}/$1/$1.debug.cdx.json: ${COMPONENTS_DIR}/$1/$1.debug.wasm $$(foreach component,$$(PLUG_COMPONENTS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).debug.cdx.json) build/scripts/sbom.sh
	$$(call sbom,composition,$$<,$$@,$$(foreach component,$$(PLUG_COMPONENTS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).debug.cdx.json))

else ifneq ($(wildcard $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg),)

# the image the release build is pulled from is the first line of the .wkg file, the debug build's
# the last. cosign verifies the SBOM attested to it, the base of its SBOM.
${COMPONENTS_DIR}/$1/$1.cdx.json: ${COMPONENTS_DIR}/$1/$1.wasm $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg build/scripts/sbom.sh | $(COSIGN_INSTALLED)
	COSIGN=$(COSIGN) $$(call sbom,oci,$$<,$$@,$$(shell head -1 $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg))

${COMPONENTS_DIR}/$1/$1.debug.cdx.json: ${COMPONENTS_DIR}/$1/$1.debug.wasm $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg build/scripts/sbom.sh | $(COSIGN_INSTALLED)
	COSIGN=$(COSIGN) $$(call sbom,oci,$$<,$$@,$$(shell tail -1 $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg))

else ifneq ($(wildcard $(COMPONENT_DIR_$1)/Cargo.toml),)

# the release and debug builds depend on the same crates. A wasm32-wasip3 component links the
# wasi-libc of the wasi-sdk sysroot or the one built from WASI_LIBC_VERSION, see build/tools.mk.
${COMPONENTS_DIR}/$1/$1.cdx.json ${COMPONENTS_DIR}/$1/$1.debug.cdx.json: ${COMPONENTS_DIR}/$1/%.cdx.json: ${COMPONENTS_DIR}/$1/%.wasm $(CARGO_SBOM_DIR)/$(call component_target,$1).stamp build/scripts/sbom.sh | $(call tool,wasm-tools) $(if $(filter wasm32-wasip3,$(call component_target,$1)),$(WASI_SDK) $(WASI_LIBC))
	WASM_TOOLS=$(WASM_TOOLS) WASI_SDK_PATH=$(WASI_SDK_PATH) WASI_LIBC_VERSION=$(WASI_LIBC_VERSION) WASI_LIBC_WASI_VERSION=$(WASI_LIBC_WASI_VERSION) \
		$$(call sbom,cargo,$$<,$$@,$(CARGO_SBOM_DIR)/$(call component_target,$1)/$(CARGO_PACKAGE_$1).cdx.json $(call component_target,$1))

endif

endef

$(foreach component,$(COMPONENTS),$(eval $(call BUILD_SBOM,$(component))))

# wkg.lock is optional, written when the interface is built
${COMPONENTS_DIR}/interface.cdx.json: ${COMPONENTS_DIR}/interface.wasm $(wildcard wkg.lock) build/scripts/sbom.sh
	$(call sbom,interface,$<,$@,wkg.lock)
