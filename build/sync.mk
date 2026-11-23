# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# the release of componentized/build to update to, e.g. v0.2.0, or a branch, tag, commit or ref, e.g.
# main, the latest release by default, unless the build is of a newer commit
BUILD_VERSION ?=

.PHONY: update-build ## Update the build to a release or commit of componentized/build, e.g. `make update-build BUILD_VERSION=v0.2.0` or `BUILD_VERSION=main`
update-build:
	build/scripts/update-build.sh $(BUILD_VERSION)

.PHONY: check-build ## Check the files synced from componentized/build are unchanged since `make update-build`
check-build:
	build/scripts/check-build.sh
