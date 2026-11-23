# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# sign published components with cosign, `SIGN=false` to push without signing, e.g. to a local registry
SIGN ?= true
# show what would be published without pushing or signing anything, `DRY_RUN=true`
DRY_RUN ?= false
# append each published file, its size in bytes and its image to this file, e.g. `gate.wasm 1234567 ghcr.io/componentized/http/gate:0.1.0@sha256:...`
PUBLISH_LOG ?=

# publish the interface, interface.wasm, `false` for a repository whose interface isn't one to depend
# on, e.g. componentized/build, whose interface is there to test the build
PUBLISH_INTERFACE ?= true

# the files that can be published, e.g. gate.wasm, published from target/components/gate/gate.wasm.
# Components are not published when they are pulled from elsewhere, `dep`, built here for use by
# other components, `internal`, or tests, `test`, named with it as a prefix or a suffix, e.g.
# `test-greeter` or `greeter-test`.
UNPUBLISHED_COMPONENTS ?= $(foreach kind,dep internal test,$(kind)-% %-$(kind))
PUBLISH_FILES := $(if $(filter true,$(PUBLISH_INTERFACE)),interface.wasm) $(foreach component,$(filter-out $(UNPUBLISHED_COMPONENTS),$(COMPONENTS)),$(component).wasm $(component).debug.wasm)

.PHONY: publish ## Publish each component in the target/components directory, `make publish-<file>` publishes one, e.g. publish-gate.wasm
publish: $(addprefix publish-,$(PUBLISH_FILES))

.PHONY: $(addprefix publish-,$(PUBLISH_FILES))
$(addprefix publish-,$(PUBLISH_FILES)): publish-%: | $(call tool,wkg) $(COSIGN_INSTALLED)
	@COSIGN="$(COSIGN)" VERSION="$(VERSION)" REPOSITORY="$(REPOSITORY)" COMPONENTS_DIR="$(COMPONENTS_DIR)" SIGN="$(SIGN)" PUBLISH_LOG="$(PUBLISH_LOG)" SBOM_DIR="$(PUBLISHED_SBOM_DIR)" DRY_RUN="$(DRY_RUN)" \
		COMPONENT_PATH="$(COMPONENT_DIR_$(patsubst %.debug,%,$(*:.wasm=)))" build/scripts/publish.sh $*
