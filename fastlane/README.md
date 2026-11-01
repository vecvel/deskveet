fastlane documentation
----

# Installation

Make sure you have the latest version of the Xcode command line tools installed:

```sh
xcode-select --install
```

For _fastlane_ installation instructions, see [Installing _fastlane_](https://docs.fastlane.tools/#installing-fastlane)

# Available Actions

## Mac

### mac build

```sh
[bundle exec] fastlane mac build
```

Build the universal app and disk image

### mac notarize_dmg

```sh
[bundle exec] fastlane mac notarize_dmg
```

Notarise and staple the disk image

### mac release

```sh
[bundle exec] fastlane mac release
```

Tag the current version and push the tag (GitHub Actions publishes the release)

### mac store_metadata

```sh
[bundle exec] fastlane mac store_metadata
```

Upload the App Store listing text and screenshots

----

This README.md is auto-generated and will be re-generated every time [_fastlane_](https://fastlane.tools) is run.

More information about _fastlane_ can be found on [fastlane.tools](https://fastlane.tools).

The documentation of _fastlane_ can be found on [docs.fastlane.tools](https://docs.fastlane.tools).
