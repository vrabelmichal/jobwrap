# GitHub releases

GitHub releases can host downloadable `.deb` assets. The repository includes
one command to build, verify, tag, upload, and publish a new release:

```sh
python3 packaging/release-github.py --maintainer 'Your Name <your-real-email>'
```

It publishes to the GitHub repository configured as the `origin` push remote
(currently `vrabelmichal/jobwrap`). The command uploads the native-architecture
application package, the architecture-independent optional Codex skills
package, and `SHA256SUMS`. GitHub also supplies source archives for the tag.
Downloadable assets appear on the release page under **Assets**.

Prerequisites are the [Debian build prerequisites](debian-packaging.md), `git`,
and authenticated `gh` with permission to push tags and create releases in the
repository. Authenticate with `gh auth login` if needed. The script never
installs packages or downloads Cargo dependencies; it uses the existing cache.

Before running it, update `[workspace.package].version` in `Cargo.toml` for a
new application version, update `CHANGELOG.md`, and commit all changes. A clean
working tree is required. The default release tag is `vVERSION`; the script
does not bump versions or commit on your behalf. A packaging-only release can
use `--revision 2`, producing tag `vVERSION-deb.2` and Debian version
`VERSION-2`. `--tag` overrides the tag explicitly.

Preview a release locally:

```sh
python3 packaging/release-github.py --dry-run
```

This builds and verifies the real packages and prints the release notes and
publication command. It does not contact GitHub, create tags, or push anything.
A placeholder maintainer is accepted only for this preview. Supply a real
maintainer when publishing; the repository's default address is a placeholder.

Upload a draft for review, or publish a prerelease:

```sh
python3 packaging/release-github.py --draft --maintainer 'Your Name <your-real-email>'
python3 packaging/release-github.py --prerelease --maintainer 'Your Name <your-real-email>'
```

`--draft` uploads the packages without making a public release. `--prerelease`
marks the release as a prerelease and prevents it from becoming Latest. Versions
with a prerelease suffix automatically use the same behavior. Add
`--notes-file path/inside/repository.md` to append release highlights to the
generated installation instructions. `--remote` chooses another GitHub remote.

The script builds from the current commit, verifies package contents and
completion behavior, writes release notes/checksums under `target/releases/`,
creates an annotated tag at that exact commit, and pushes only that tag. The
branch itself is not pushed. Existing tags must point to the same commit;
existing releases are rejected. There are no force pushes or asset overwrites.

If publication fails after the tag was pushed, that tag remains; rerunning can
reuse it if it still points to the same commit. If GitHub already created a
draft during a failed upload, inspect that draft in GitHub and finish or remove
it explicitly before retrying. The script never deletes releases or tags.

The release packages target the build machine's architecture and library
versions. Build on the oldest supported Debian/Ubuntu environment rather than
assuming binaries built on a newer machine will run on older distributions.
This command publishes one native architecture per release; it does not build
a cross-platform matrix or establish an APT repository.

The committed Cargo workspace version supplies the binary/API/footer version,
the `vVERSION` release tag, and the `VERSION-REVISION` Debian package version.
Release binaries embed the full source commit and report a clean build; the
package verification checks both executables against the package version and
current source commit. Source builds without Git metadata report `unknown`.

Verify the release orchestration without contacting GitHub:

```sh
python3 packaging/test-release-github.py
```
