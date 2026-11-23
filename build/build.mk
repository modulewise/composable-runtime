# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# The build shared by the componentized repositories, synced from https://github.com/componentized/build
# at the version in build/VERSION. Nothing in build/ is edited here, see build/README.md.
#
# The repository's Makefile sets its settings, includes this file, then adds its own targets and
# prerequisites, e.g.
#
#   COMPONENT_TARGET := wasm32-wasip3
#   include build/build.mk
#   test-integration: test-valkey

SHELL := /bin/bash

export RUST_BACKTRACE ?= 1
export WASMTIME_BACKTRACE_DETAILS ?= 1
# the repository's wkg config, e.g. the registry of each namespace, optional: without one wkg uses its
# own, the user's config or its defaults. wkg fails when WKG_CONFIG_FILE names a missing file.
ifneq ($(wildcard .config/wasm-pkg/config.toml),)
export WKG_CONFIG_FILE ?= $(abspath .config/wasm-pkg/config.toml)
endif

# `make` without a goal runs all, otherwise the first target of an included file would run
.DEFAULT_GOAL := all

# the rust target components are built for, `wasm32-wasip3` builds components that run std threads on
# component model cooperative threads, see build/components.mk, `wasm32-wasip2` builds components
# rust links itself, e.g. a command. A component built with cargo may name
# its own in its Cargo.toml, `COMPONENT_TARGET` given on the command line overrides it, e.g.
#
#   [package.metadata.componentized]
#   target = "wasm32-wasip3"
COMPONENT_TARGET ?= wasm32-unknown-unknown

# the components and their directories, each `<name>=<dir>`, e.g. `example-hello=examples/hello`, found
# in components/ and each directory with a components.toml naming the prefix of its components, see
# build/scripts/components.sh. Read once while parsing, defines COMPONENTS, and for each component
# COMPONENT_DIR_<component>, its directory, and COMPONENT_BASE_<component>, the name of its directory
# its files are named for, e.g. hello for examples/hello/hello.wac
COMPONENT_DIRS := $(shell build/scripts/components.sh | tr ' ' '=')
COMPONENTS := $(sort $(foreach entry,$(COMPONENT_DIRS),$(firstword $(subst =, ,$(entry)))))
$(foreach name,$(COMPONENTS),$(if $(word 2,$(filter $(name)=%,$(COMPONENT_DIRS))),$(error the component $(name) is in more than one directory, $(patsubst $(name)=%,%,$(filter $(name)=%,$(COMPONENT_DIRS))), rename one or give its directory of components a prefix, see build/scripts/components.sh)))
$(foreach entry,$(COMPONENT_DIRS),$(eval COMPONENT_DIR_$(firstword $(subst =, ,$(entry))) := $(lastword $(subst =, ,$(entry)))))
$(foreach name,$(COMPONENTS),$(eval COMPONENT_BASE_$(name) := $(notdir $(COMPONENT_DIR_$(name)))))

# the components built with cargo, those with a Cargo.toml, and their packages, CARGO_PACKAGE_<component>,
# named in their Cargo.toml, e.g. examples/hello's may be example-hello, or hello
CARGO_COMPONENTS := $(foreach name,$(COMPONENTS),$(if $(wildcard $(COMPONENT_DIR_$(name))/Cargo.toml),$(name)))
$(foreach name,$(CARGO_COMPONENTS),$(eval CARGO_PACKAGE_$(name) := $(shell sed -n '/^\[package\]/,/^\[/s/^name *= *"\(.*\)"/\1/p' $(COMPONENT_DIR_$(name))/Cargo.toml)))

# read once while parsing, defines COMPONENT_TARGET_<component> for those that name one
$(foreach name,$(CARGO_COMPONENTS),$(eval COMPONENT_TARGET_$(name) := $(shell sed -n '/^\[package\.metadata\.componentized\]/,/^\[/s/^target *= *"\(.*\)"/\1/p' $(COMPONENT_DIR_$(name))/Cargo.toml)))

# the rust target a component is built for, e.g. `$(call component_target,trace)`
component_target = $(if $(filter command line,$(origin COMPONENT_TARGET)),$(COMPONENT_TARGET),$(or $(COMPONENT_TARGET_$(1)),$(COMPONENT_TARGET)))

# the targets the components built with cargo are built for
COMPONENT_TARGETS = $(sort $(foreach name,$(CARGO_COMPONENTS),$(call component_target,$(name))))

# the cli built from this repository, e.g. `constants`, released with an archive for each platform by
# the CI workflow. Empty for a repository without a cli.
CLI_BIN ?=
# the crates published to crates.io when a release is published, e.g. `componentized-constants
# componentized-constants-cli`, checked by the CI workflow. Empty for a repository without crates.
PUBLISH_CRATES ?=

# tools first, the others use the variables it defines for each tool when their rules are read
include build/tools.mk
include build/wit.mk
include build/components.mk
include build/sbom.mk
include build/publish.mk
include build/sync.mk


.PHONY: all ## Install the tools, generate, build the components and test them, `make` without a target
all: tools generate components test

# the help of a target follows `## ` on its .PHONY line, e.g. `.PHONY: tools ## Install the tools`, or
# on the line of its rule, e.g. `run: cli.wasm ## Run the cli`. A variable's line, with an `=`, is not
# a target.
.PHONY: help ## Display this help screen
help:
	@awk '/^[^#[:space:]][^=]*:[^=]*## / { \
		names = substr($$0, 1, index($$0, "## ") - 1); \
		if (sub(/^\.PHONY:/, "", names) == 0) sub(/:.*$$/, "", names); \
		text = substr($$0, index($$0, "## ") + 3); \
		count = split(names, list, /[[:space:]]+/); \
		for (i = 1; i <= count; i++) if (list[i] != "" && !(list[i] in seen)) { seen[list[i]] = 1; printf "\033[36m%-20s\033[0m %s\n", list[i], text } \
	}' $(MAKEFILE_LIST) | sort

# the repository adds what else it generates and commits as prerequisites, e.g. `generate: config`
.PHONY: generate ## Generate the files committed with the sources, the CI workflow fails when they drift
generate: wit
	@:

.PHONY: clean ## Remove the built components and the fetched wit dependencies
clean: clean-components clean-wit
	@:

.PHONY: clean-all ## Remove everything built, the installed tools and cargo's target directory too
clean-all: clean-components clean-tools clean-wit
	cargo clean

.PHONY: test ## Build the components and run the tests of the workspace
test: components
	cargo test --workspace

# the repository adds the tests that need more than cargo as prerequisites, e.g. a server in a
# container, `test-integration: test-valkey`
.PHONY: test-integration ## Run the tests that need more than cargo, run by the CI workflow after `make test`
test-integration:
	@:

# prints the value of a variable, e.g. `make -s print-CLI_BIN`, for the workflows
print-%:
	$(info $($*))
	@:
