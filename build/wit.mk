# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# a path relative to the root of the repository, e.g. `wit` for `components/../wit`
relpath = $(if $(filter $(CURDIR),$(abspath $(1))),.,$(patsubst $(CURDIR)/%,%,$(abspath $(1))))

# directories with a wkg.toml, each fetches the dependencies of its wit directory into wit/deps,
# e.g. `.` and `components`
WKG_DIRS := $(sort $(patsubst ./%,%,$(patsubst %/,%,$(dir $(shell find . -name wkg.toml -not -path './target/*' -not -path '*/wit/deps/*')))))

# the wit/deps directory of a directory with a wkg.toml, e.g. `wit/deps` for `.`
wit_deps = $(patsubst ./%,%,$(1)/wit/deps)

WIT_DEPS := $(foreach dir,$(WKG_DIRS),$(call wit_deps,$(dir)))

# the prerequisites of what's made from WIT paths, relative to the repository, e.g. `components/wit`:
# a directory's deps, when a wkg.toml beside the directory fetches them, otherwise its files, e.g. a
# directory without a wkg.toml, implicitly up to date, or one updated by another tool. A path that
# doesn't exist has none.
wit_prerequisites = $(foreach path,$(1),$(if $(filter $(path)/deps,$(WIT_DEPS)),$(path)/deps,$(shell find $(path) -type f 2> /dev/null)))

# the cache wkg fetches packages into, in place of the system cache. wkg caches a package by its
# version, so a dev version fetched again would come from the cache, stale, rather than the registry.
# Exported for every wkg the build runs, absolute as wkg also runs in other directories, e.g.
# `cd components && wkg fetch`. `make clean-wit` removes it.
export WKG_CACHE_DIR := $(abspath target/wkg-cache)

# runs wkg with the arguments in the directory with a wkg.toml, e.g. `$(call wkg,components,fetch)`.
# wkg rewrites wkg.lock even when nothing changed, which would leave everything made from the lock
# looking out of date, so the lock is put back as it was, with its time, when its content is unchanged. A
# missing lock is tolerated, wkg writes it.
wkg = if [ -f $(1)/wkg.lock ]; then cp -p $(1)/wkg.lock $(1)/wkg.lock.orig; fi && \
	(cd $(1) && $(WKG) $(2)); status=$$?; \
	if [ -f $(1)/wkg.lock.orig ]; then \
		if cmp -s $(1)/wkg.lock.orig $(1)/wkg.lock; then mv $(1)/wkg.lock.orig $(1)/wkg.lock; else rm $(1)/wkg.lock.orig; fi; \
	fi; \
	exit $$status

.PHONY: clean-wit ## Remove the fetched wit dependencies and the wkg cache, fetched again by `make wit`
clean-wit:
	rm -rf $(WIT_DEPS) $(WKG_CACHE_DIR)

.PHONY: wit ## Fetch the wit dependencies of each wkg.toml into its wit/deps
wit: $(WIT_DEPS)

define FETCH_WIT

# the dependencies are touched after fetching, newer than the lock they were fetched with even when
# the fetch changed nothing in them
#
# a package overridden with a local path, e.g. `{ path = "../wit" }`, has its dependencies fetched first,
# an override without dependencies of its own is skipped, e.g. `{ path = "./wit/overrides/wasi-muppets" }`.
# The lock is optional, the fetch writes it.
$(call wit_deps,$1): $1/wkg.toml $(wildcard $1/wkg.lock) $(shell find $1/wit -type f -name "*.wit" -not -path "*/deps/*") $(filter $(WIT_DEPS),$(foreach path,$(shell sed -n 's/.*path *= *"\(.*\)".*/\1/p' $1/wkg.toml),$(call relpath,$1/$(path))/deps)) | $(call tool,wkg)
	$$(call wkg,$1,fetch)
	@touch $(call wit_deps,$1)

endef

$(foreach dir,$(WKG_DIRS),$(eval $(call FETCH_WIT,$(dir))))
