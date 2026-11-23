<!-- Managed by componentized/build and synced into each repository by `make update-build`. Change it
in https://github.com/componentized/build, `make check-build` fails when a copy is changed. -->

# Build

The build shared by the componentized repositories, synced from [componentized/build](https://github.com/componentized/build) at the version in [`VERSION`](VERSION).

Nothing in this directory, nor the workflows synced with it, is edited here: `make check-build`, run by the CI workflow, fails when a synced file differs from [`MANIFEST`](MANIFEST). Changes go to componentized/build, and come back with its next release:

- `make update-build` updates to the latest release, `make update-build BUILD_VERSION=v0.2.0` to a given one, or `BUILD_VERSION=main` to a commit not yet released. A build synced from a commit is kept until a newer release.
- The `Update build` workflow opens a pull request each week when there is a newer release.

The repository's own `Makefile` sets the settings of the build, includes [`build.mk`](build.mk), then adds its own targets. See the [componentized/build README](https://github.com/componentized/build#readme) for the settings and the targets a repository can extend.
