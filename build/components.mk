# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

COMPONENTS_DIR := target/components

# the WIT packages of components/wit/deps, when it has any, encoded where `wac compose` looks for a package a
# composition names without instantiating it, e.g. `import stderr: wasi:cli/stderr@0.3.0;`
WAC_DEPS_DIR := target/wac-deps

# COMPONENTS, the components, and CARGO_COMPONENTS, those built with cargo, are found by
# build/build.mk, with the directory of each, COMPONENT_DIR_<component>

# the WIT paths of the wit_bindgen::generate! of each component with a Cargo.toml, in its sources and
# in those of the workspace crates it depends on, e.g. a crate of shared bindings, see
# build/scripts/wit-paths.sh. Defines WIT_BINDGEN_PATHS_<package>, for the component's package,
# CARGO_PACKAGE_<component>. Generated when a Rust source or the workspace changes, make restarts
# after generating it.
WIT_PATHS_MK := target/wit-paths.mk
ifneq ($(CARGO_COMPONENTS),)
# quietly, the first time make generates it before reading it
-include $(WIT_PATHS_MK)
endif

$(WIT_PATHS_MK): Cargo.toml Cargo.lock $(foreach name,$(CARGO_COMPONENTS),$(COMPONENT_DIR_$(name))/Cargo.toml) build/scripts/wit-paths.sh $(shell find . -name '*.rs' -not -path '*/target/*' -not -path '*/wit/deps/*')
	@mkdir -p $(@D)
	@build/scripts/wit-paths.sh $(foreach name,$(CARGO_COMPONENTS),$(CARGO_PACKAGE_$(name))) > $@.tmp
	@mv $@.tmp $@

# the cargo profiles components are built with, `component` and `component-debug` when Cargo.toml
# defines them, e.g. to build smaller components than the release profile a cli is built with
COMPONENT_PROFILE ?= $(if $(shell grep -s '^\[profile\.component\]' Cargo.toml),component,release)
COMPONENT_DEBUG_PROFILE ?= $(if $(shell grep -s '^\[profile\.component-debug\]' Cargo.toml),component-debug,dev)

# the core module cargo built for a component with a profile, e.g. `$(call cargo_wasm,gate,dev)` for
# target/wasm32-unknown-unknown/debug/gate.wasm, `$(call cargo_wasm,gate,release,.opt)` for the
# module wasm-opt optimized. For wasm32-wasip2 cargo builds a component rather than a core module.
cargo_wasm = target/$(call component_target,$(1))/$(if $(filter dev,$(2)),debug,$(2))/$(call cargo_artifact,$(1))$(3).wasm

# the name of the wasm file cargo builds for a component's package, a library's has `_` for each `-`, a
# binary keeps the package's name, e.g. a command for wasm32-wasip2 with a src/main.rs and no src/lib.rs
cargo_artifact = $(if $(and $(wildcard $(COMPONENT_DIR_$(1))/src/main.rs),$(if $(wildcard $(COMPONENT_DIR_$(1))/src/lib.rs),,bin)),$(CARGO_PACKAGE_$(1)),$(subst -,_,$(CARGO_PACKAGE_$(1))))

# the files the components cargo builds need besides the sources, e.g. a component the library
# embeds, `CARGO_COMPONENT_DEPS := crates/componentized-static-config/constants-config.wasm`
CARGO_COMPONENT_DEPS ?=

ifneq ($(filter wasm32-wasip3,$(COMPONENT_TARGETS)),)
# wasm32-wasip3 runs std threads on component model cooperative threads, which needs a wasi-libc built
# for them from wasi-sdk 34+'s experimental sysroot, installed with the tools or from WASI_SDK_PATH, or
# built from WASI_LIBC_VERSION, see build/tools.mk, and a rust toolchain with cooperative thread
# support, 1.100+. `make components COMPONENT_TARGET=wasm32-unknown-unknown` builds without.
WASI_SYSROOT_LIB := $(WASI_SYSROOT)/lib/wasm32-wasip3
# linked with wasm-component-ld, like the target's default, against the coop threads wasi-libc in
# place of the libc vendored with std. Linked by clang, a cdylib is a shared library, not a component.
# It links a core module, like wasm32-unknown-unknown, wasm-opt removes the code the release build
# doesn't reach, and with it the imports only that code uses, before it's made a component.
export CARGO_TARGET_WASM32_WASIP3_LINKER := $(WASI_SDK_PATH)/bin/wasm-component-ld
export CARGO_TARGET_WASM32_WASIP3_RUSTFLAGS := -Clink-self-contained=n -Clink-arg=-L$(WASI_SYSROOT_LIB) -Clink-arg=$(WASI_SYSROOT_LIB)/crt1-reactor.o -Clink-arg=--export=cabi_realloc -Clink-arg=--skip-wit-component
endif

# turn the core module cargo built for a component into a component, e.g. `$(call
# componentize,gate,<module>,<component>)`. For wasm32-wasip2 rust links a component itself, with
# wasm-component-ld, which is copied.
componentize = $(if $(filter wasm32-wasip2,$(call component_target,$(1))),cp $(2) $(3),$(WASM_TOOLS) component new $(2) -o $(3))

# the release build of a component is optimized again for size with wasm-opt when set, e.g.
# `COMPONENT_OPTIMIZE := true`, the build provides wasm-opt to every repository. wasm-opt optimizes
# core modules, a component built for wasm32-wasip2 is not optimized again.
COMPONENT_OPTIMIZE ?=

# whether the release build of a component is optimized again, e.g. `$(call component_optimize,gate)`
component_optimize = $(if $(COMPONENT_OPTIMIZE),$(filter-out wasm32-wasip2,$(call component_target,$(1))))

# the wasm features rust enables for wasm32-unknown-unknown and wasm32-wasip3, `rustc --print cfg
# --target wasm32-unknown-unknown`. The stripped modules don't say which features they use, wasm-opt
# is limited to these so it doesn't introduce instructions rust wouldn't emit.
WASM_OPT_FEATURES ?= --enable-bulk-memory --enable-multivalue --enable-mutable-globals --enable-nontrapping-float-to-int --enable-reference-types --enable-sign-ext

.PHONY: clean-components ## Remove the built components
clean-components: clean-wit
	rm -rf ${COMPONENTS_DIR} ${WAC_DEPS_DIR}

.PHONY: components ## Build the interface and each component into target/components, `make components/<name>` builds one, `make <dir>` those in a directory, e.g. `make examples`
components: ${COMPONENTS_DIR}/interface.wasm $(foreach component,$(COMPONENTS),${COMPONENTS_DIR}/$(component)/$(component).wasm ${COMPONENTS_DIR}/$(component)/$(component).debug.wasm)

# the components whose release and debug builds may have different WIT, not checked by `make
# check-components`, e.g. a wasm32-wasip3 component whose release build wasm-opt optimizes, see
# COMPONENT_OPTIMIZE, dropping imports of the standard library the debug build keeps:
# `CHECK_COMPONENTS_SKIP := gate`. `make check-components/<name>` still checks one.
CHECK_COMPONENTS_SKIP ?=

.PHONY: check-components ## Check the release and debug build of each component have the same WIT, `make check-components/<name>` checks one
check-components: $(foreach component,$(filter-out $(CHECK_COMPONENTS_SKIP),$(COMPONENTS)),check-components/$(component))

.PHONY: validate-components ## Validate the release and debug build of each component, `make validate-components/<name>` checks one
validate-components: $(foreach component,$(COMPONENTS),validate-components/$(component))

define BUILD_COMPONENT

.PHONY: components/$1
components/$1: ${COMPONENTS_DIR}/$1/$1.wasm ${COMPONENTS_DIR}/$1/$1.debug.wasm

# the same imports and exports, e.g. a debug build that isn't optimized may keep imports the release
# build doesn't use, the order of the WIT may differ, see build/scripts/wit-canonical.sh
.PHONY: check-components/$1
check-components/$1: ${COMPONENTS_DIR}/$1/$1.wasm ${COMPONENTS_DIR}/$1/$1.debug.wasm build/scripts/wit-canonical.sh | $(call tool,wasm-tools)
	@diff -u --label $1.wasm --label $1.debug.wasm \
		<(WASM_TOOLS=$(WASM_TOOLS) build/scripts/wit-canonical.sh ${COMPONENTS_DIR}/$1/$1.wasm) \
		<(WASM_TOOLS=$(WASM_TOOLS) build/scripts/wit-canonical.sh ${COMPONENTS_DIR}/$1/$1.debug.wasm) \
		|| { echo "the WIT of the release and debug build of $1 differ" >&2; exit 1; }

.PHONY: validate-components/$1
validate-components/$1: ${COMPONENTS_DIR}/$1/$1.wasm | $(call tool,wasm-tools)
	@$(WASM_TOOLS) validate --features all ${COMPONENTS_DIR}/$1/$1.wasm || { echo "$1.wasm is not a valid component"; exit 1; }
	@$(WASM_TOOLS) validate --features all ${COMPONENTS_DIR}/$1/$1.debug.wasm || { echo "$1.debug.wasm is not a valid component"; exit 1; }

ifneq ($(wildcard $(COMPONENT_DIR_$1)/wit/$(COMPONENT_BASE_$1).constants.wit),)

${COMPONENTS_DIR}/$1/$1.wasm: $(call wit_prerequisites,$(COMPONENT_DIR_$1)/wit) ${COMPONENTS_DIR}/$1/README.md $(call tool_prerequisite,componentized-constants-cli)
	$(CONSTANTS) --wit $(COMPONENT_DIR_$1)/wit -o ${COMPONENTS_DIR}/$1/$1.wasm

${COMPONENTS_DIR}/$1/$1.debug.wasm: $(call wit_prerequisites,$(COMPONENT_DIR_$1)/wit) ${COMPONENTS_DIR}/$1/README.md $(call tool_prerequisite,componentized-constants-cli)
	$(CONSTANTS) --wit $(COMPONENT_DIR_$1)/wit -o ${COMPONENTS_DIR}/$1/$1.debug.wasm

else ifneq ($(wildcard $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).properties),)

${COMPONENTS_DIR}/$1/$1.wasm: $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).properties ${COMPONENTS_DIR}/$1/README.md $(call tool_prerequisite,componentized-static-config-cli)
	$(STATIC_CONFIG) -f $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).properties -o ${COMPONENTS_DIR}/$1/$1.wasm

${COMPONENTS_DIR}/$1/$1.debug.wasm: $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).properties ${COMPONENTS_DIR}/$1/README.md $(call tool_prerequisite,componentized-static-config-cli)
	$(STATIC_CONFIG) -f $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).properties -o ${COMPONENTS_DIR}/$1/$1.debug.wasm

else ifneq ($(wildcard $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wac),)

# the local packages the composition instantiates, e.g. `new local:latch-n2 { ... }`
WAC_DEPS_$1 := $$(shell grep -v '^\s*//' $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wac | grep -oE 'local:[a-z0-9-]+' | sed 's/^local://' | sort -u)

${COMPONENTS_DIR}/$1/$1.wasm: $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wac $$(foreach component,$$(WAC_DEPS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).wasm) ${COMPONENTS_DIR}/$1/README.md ${WAC_DEPS_DIR} | $(call tool,wac-cli)
	$(WAC) compose --deps-dir ${WAC_DEPS_DIR} $$(foreach component,$$(WAC_DEPS_$1),-d local:$$(component)=$${COMPONENTS_DIR}/$$(component)/$$(component).wasm) -o ${COMPONENTS_DIR}/$1/$1.wasm $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wac

${COMPONENTS_DIR}/$1/$1.debug.wasm: $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wac $$(foreach component,$$(WAC_DEPS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).debug.wasm) ${COMPONENTS_DIR}/$1/README.md ${WAC_DEPS_DIR} | $(call tool,wac-cli)
	$(WAC) compose --deps-dir ${WAC_DEPS_DIR} $$(foreach component,$$(WAC_DEPS_$1),-d local:$$(component)=$${COMPONENTS_DIR}/$$(component)/$$(component).debug.wasm) -o ${COMPONENTS_DIR}/$1/$1.debug.wasm $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wac

else ifneq ($(wildcard $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).plug),)

# the components plugged together with `wac plug`, each a component of the repository named on a line
# of its own, the socket first, then each plug, `#` starts a comment, e.g.
#
#   greeter
#   defaults
#
# Where the socket and a plug import different versions of the same interface, e.g. wasi:io@0.2.6 and
# wasi:io@0.2.12, the plugged component imports the newer one, a composition the one it imports first.
PLUG_COMPONENTS_$1 := $$(shell sed 's/\#.*//' $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).plug)
PLUG_SOCKET_$1 := $$(firstword $$(PLUG_COMPONENTS_$1))
PLUG_PLUGS_$1 := $$(wordlist 2,$$(words $$(PLUG_COMPONENTS_$1)),$$(PLUG_COMPONENTS_$1))

${COMPONENTS_DIR}/$1/$1.wasm: $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).plug $$(foreach component,$$(PLUG_COMPONENTS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).wasm) ${COMPONENTS_DIR}/$1/README.md | $(call tool,wac-cli)
	$(WAC) plug $${COMPONENTS_DIR}/$$(PLUG_SOCKET_$1)/$$(PLUG_SOCKET_$1).wasm $$(foreach component,$$(PLUG_PLUGS_$1),--plug $${COMPONENTS_DIR}/$$(component)/$$(component).wasm) -o ${COMPONENTS_DIR}/$1/$1.wasm

${COMPONENTS_DIR}/$1/$1.debug.wasm: $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).plug $$(foreach component,$$(PLUG_COMPONENTS_$1),$${COMPONENTS_DIR}/$$(component)/$$(component).debug.wasm) ${COMPONENTS_DIR}/$1/README.md | $(call tool,wac-cli)
	$(WAC) plug $${COMPONENTS_DIR}/$$(PLUG_SOCKET_$1)/$$(PLUG_SOCKET_$1).debug.wasm $$(foreach component,$$(PLUG_PLUGS_$1),--plug $${COMPONENTS_DIR}/$$(component)/$$(component).debug.wasm) -o ${COMPONENTS_DIR}/$1/$1.debug.wasm

else ifneq ($(wildcard $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg),)

${COMPONENTS_DIR}/$1/$1.wasm: $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg ${COMPONENTS_DIR}/$1/README.md | $(call tool,wkg)
	$(WKG) oci pull $(shell cat $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg 2> /dev/null | head -1) -o ${COMPONENTS_DIR}/$1/$1.wasm

${COMPONENTS_DIR}/$1/$1.debug.wasm: $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg ${COMPONENTS_DIR}/$1/README.md | $(call tool,wkg)
	$(WKG) oci pull $(shell cat $(COMPONENT_DIR_$1)/$(COMPONENT_BASE_$1).wkg  2> /dev/null | tail -1 2> /dev/null) -o ${COMPONENTS_DIR}/$1/$1.debug.wasm

# cargo is checked last, other strategies may have a Cargo.toml for tests of non-rust sources
else ifneq ($(wildcard $(COMPONENT_DIR_$1)/Cargo.toml),)

# the WIT the bindings are generated from, the path of each wit_bindgen::generate! of the component
# and the workspace crates it depends on, e.g. `path: "../wit"` for components/wit, or else the wit
# beside the component's directory, e.g. components/wit for components/gate, e.g. bindings generated
# other than with wit-bindgen's macro
WIT_PATHS_$1 := $$(foreach path,$$(or $$(WIT_BINDGEN_PATHS_$(CARGO_PACKAGE_$1)),$$(wildcard $(patsubst %/,%,$(dir $(COMPONENT_DIR_$1)))/wit)),$$(call relpath,$$(path)))

${COMPONENTS_DIR}/$1/$1.wasm: $(CARGO_COMPONENT_DEPS) Cargo.toml Cargo.lock $$(call wit_prerequisites,$$(WIT_PATHS_$1)) $(shell find $(COMPONENT_DIR_$1) -type f) $(shell find crates -type f 2> /dev/null) ${COMPONENTS_DIR}/$1/README.md | $(call tool,wasm-tools) $(if $(call component_optimize,$1),$(call tool,wasm-opt)) $(if $(filter wasm32-wasip3,$(call component_target,$1)),$(WASI_SDK) $(WASI_LIBC))
	cargo build -p $(CARGO_PACKAGE_$1) --target $(call component_target,$1) --profile $(COMPONENT_PROFILE)
ifneq ($(call component_optimize,$1),)
	$(WASM_OPT) -Oz $(WASM_OPT_FEATURES) $(call cargo_wasm,$1,$(COMPONENT_PROFILE)) -o $(call cargo_wasm,$1,$(COMPONENT_PROFILE),.opt)
	$(call componentize,$1,$(call cargo_wasm,$1,$(COMPONENT_PROFILE),.opt),${COMPONENTS_DIR}/$1/$1.wasm)
else
	$(call componentize,$1,$(call cargo_wasm,$1,$(COMPONENT_PROFILE)),${COMPONENTS_DIR}/$1/$1.wasm)
endif

${COMPONENTS_DIR}/$1/$1.debug.wasm: $(CARGO_COMPONENT_DEPS) Cargo.toml Cargo.lock $$(call wit_prerequisites,$$(WIT_PATHS_$1)) $(shell find $(COMPONENT_DIR_$1) -type f) $(shell find crates -type f 2> /dev/null) ${COMPONENTS_DIR}/$1/README.md | $(call tool,wasm-tools) $(if $(filter wasm32-wasip3,$(call component_target,$1)),$(WASI_SDK) $(WASI_LIBC))
	cargo build -p $(CARGO_PACKAGE_$1) --target $(call component_target,$1) --profile $(COMPONENT_DEBUG_PROFILE)
	$(call componentize,$1,$(call cargo_wasm,$1,$(COMPONENT_DEBUG_PROFILE)),${COMPONENTS_DIR}/$1/$1.debug.wasm)

endif

${COMPONENTS_DIR}/$1/README.md: $(COMPONENT_DIR_$1)/README.md
	@mkdir -p ${COMPONENTS_DIR}/$1
	@cp $(COMPONENT_DIR_$1)/README.md ${COMPONENTS_DIR}/$1/README.md

endef

$(foreach component,$(COMPONENTS),$(eval $(call BUILD_COMPONENT,$(component))))

# a target for each directory holding components, a component's directory and each directory it's in,
# building the components in it and under it, with their SBOMs, e.g. `make examples`, `make
# components/gate` for gate and its group, gate-internal and gate-config, or `make components/deps`
# for those of components/deps/componentized/components.toml. `make components` builds every
# component, those of components/ and of every other directory of components.
COMPONENT_DIR_TARGETS = $(sort $(foreach component,$(COMPONENTS),$(call dir_ancestors,$(COMPONENT_DIR_$(component)))))

# a directory and each directory it's in, but the repository's, e.g. components/gate/internal,
# components/gate and components for components/gate/internal
dir_ancestors = $(if $(filter-out .,$(1)),$(1) $(call dir_ancestors,$(patsubst %/,%,$(dir $(1)))))

# the components in a directory or under it, e.g. `$(call components_in,components/gate)`
components_in = $(foreach component,$(COMPONENTS),$(if $(filter $(1) $(1)/%,$(COMPONENT_DIR_$(component))),$(component)))

define COMPONENT_DIR_TARGET
.PHONY: $1
$1: $(foreach component,$(call components_in,$1),$(foreach build,$(component) $(component).debug,${COMPONENTS_DIR}/$(component)/$(build).wasm ${COMPONENTS_DIR}/$(component)/$(build).cdx.json))

.PHONY: $1/
$1/: $1
endef

$(foreach dir,$(COMPONENT_DIR_TARGETS),$(eval $(call COMPONENT_DIR_TARGET,$(dir))))

${WAC_DEPS_DIR}: $(call wit_prerequisites,components/wit) build/scripts/wac-deps.sh | $(call tool,wasm-tools)
	WASM_TOOLS=$(WASM_TOOLS) build/scripts/wac-deps.sh components/wit/deps ${WAC_DEPS_DIR}

${COMPONENTS_DIR}/interface.wasm: wit/deps README.md | $(call tool,wkg)
	@mkdir -p ${COMPONENTS_DIR}
	$(call wkg,.,build -o $(abspath ${COMPONENTS_DIR})/interface.wasm)
	@cp README.md ${COMPONENTS_DIR}/README.md
