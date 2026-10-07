# Debian packaging

Release binary packaging is implemented. Debian distribution source packaging
remains a separate task.

## Build release packages

Prerequisites: the pinned Rust toolchain and cached Cargo dependencies, Python
3.8 or newer, and Debian's `dpkg`, `dpkg-dev`, and `binutils` tools. The builder
does not download dependencies or install anything. Populate the Cargo cache
before building on an offline machine.

```sh
python3 packaging/build-deb.py
# Only the application package:
python3 packaging/build-deb.py --codex-skills no
# Only the architecture-independent skill package:
python3 packaging/build-deb.py --skills-only
```

The default creates both `target/debian/jobwrap_VERSION_ARCH.deb` and
`target/debian/jobwrap-codex-skills_VERSION_all.deb`. Building the skill package
does not install or activate it. Options are explicit and never prompt, so
release automation can select `--codex-skills yes` or `--codex-skills no`.
Old artifacts are retained; select the intended versions rather than using a
wildcard that also matches previous builds.

The application is built with `cargo build --release --workspace --locked
--offline`. `--no-build` packages existing release binaries; use it only when
they are known to be current. `--revision` sets the Debian revision (default
`1`), and `--maintainer 'Name <email@example.org>'` supplies the real release
contact. The repository's default maintainer is still a placeholder and must
be replaced for publication.

The builder reads the application's assets from Cargo's deb metadata, computes
shared-library dependencies with `dpkg-shlibdeps`, compresses manpages and
changelogs, and uses `dpkg-deb --root-owner-group` so no root build is needed.
This builder targets the build machine's native architecture. Set
`SOURCE_DATE_EPOCH` to the source release timestamp for reproducible archive
timestamps. Staging directories remain under `target/debian` for inspection.

For example, inspect the current amd64 release without installing it:

```sh
python3 packaging/test-deb.py \
  target/debian/jobwrap_0.1.0-1_amd64.deb \
  target/debian/jobwrap-codex-skills_0.1.0-1_all.deb
lintian target/debian/jobwrap_0.1.0-1_amd64.deb \
  target/debian/jobwrap-codex-skills_0.1.0-1_all.deb
```

## Practical release package (cargo-deb)

The main Cargo manifest also supports `cargo-deb` as an alternative. Build both
workspace binaries first, because the daemon lives in a different crate:

```sh
cargo build --release --workspace --locked --offline
cargo deb --package jobwrap-cli --no-build
```

The Python builder is the tested path and additionally builds the optional
skills package; `cargo-deb` must be installed separately. Release binary
packages are suitable for direct `apt install ./jobwrap_VERSION_ARCH.deb`.

Installed files:

```text
/usr/bin/jobwrap
/usr/libexec/jobwrap/jobwrapd
/usr/lib/systemd/user/jobwrapd.service
/usr/share/doc/jobwrap/README.md.gz
/usr/share/doc/jobwrap/changelog.gz
/usr/share/doc/jobwrap/changelog.Debian.gz
/usr/share/man/man1/jobwrap.1.gz
/usr/share/man/man5/jobwrap.toml.5.gz
/usr/share/man/man8/jobwrapd.8.gz
/usr/share/bash-completion/completions/jobwrap
/usr/share/zsh/vendor-completions/_jobwrap
/usr/share/fish/vendor_completions.d/jobwrap.fish
```

No global writable state directory is installed. All runtime and persistent
state belongs to individual users under their XDG directories.

## Manpages and tab completion

All three manpages are installed in the standard man directories; use
`man jobwrap`, `man jobwrap.toml`, and `man jobwrapd` when a man reader is
installed. Bash, Zsh, and Fish completions are installed in their standard
vendor directories. Bash needs `bash-completion` enabled (a suggested package);
Zsh needs `compinit`, and Fish discovers its vendor completions automatically.
Open a new shell after installation. Packaging does not edit shell startup
files. Completions cover subcommands and common options, and do not start the
daemon to query job IDs.

## Optional Codex skills

The established package-selection pattern is a separate optional package.
`jobwrap` only **Suggests** `jobwrap-codex-skills`, so normal APT installation
does not select it automatically. Install one or both packages as desired.
The skill package depends on the matching application version.

Run these as an administrator (substitute the actual version/architecture):

```sh
# Application only; no skill installation:
apt install ./jobwrap_0.1.0-1_amd64.deb
# Application and skill:
apt install ./jobwrap_0.1.0-1_amd64.deb ./jobwrap-codex-skills_0.1.0-1_all.deb
# Unattended application and skill installation:
DEBIAN_FRONTEND=noninteractive apt-get install -y \
  ./jobwrap_0.1.0-1_amd64.deb ./jobwrap-codex-skills_0.1.0-1_all.deb
# Unattended application-only installation: omit the skill package above.
```

The package installs `SKILL.md` and both references under
`/etc/codex/skills/jobwrap/`, the documented
[Codex administrator skill location](https://developers.openai.com/codex/skills).
This makes the skill available for **all users** on the machine. No home directories or
Codex user configuration are modified. Existing personal copies can cause
duplicate skills; choose the system package or a personal copy deliberately.

Files in `/etc` are registered as Debian conffiles, preserving administrator
edits during upgrades. As with other conffiles, local edits may trigger dpkg's
standard upgrade question; `DEBIAN_FRONTEND` controls debconf, not those
questions. For unattended upgrades, select your configuration-management
policy, for example `-o Dpkg::Options::=--force-confdef -o
Dpkg::Options::=--force-confold` to retain local edits when needed.
`apt remove jobwrap-codex-skills` retains these configuration files and therefore
may leave the skill discoverable; use `apt purge jobwrap-codex-skills` to remove
the system skill completely. Packaging never removes personal skill copies.

There is no custom installer prompt or custom `apt`/`dpkg` parameter.
[Debian Policy §3.9.1](https://www.debian.org/doc/debian-policy/ch-binary.html#prompting-in-maintainer-scripts)
uses debconf for necessary package configuration questions; unattended answers
use debconf preseeding and `DEBIAN_FRONTEND=noninteractive`. That machinery is
appropriate for required configuration, but optional integration can be
selected directly as a package. Neither package has maintainer scripts.

## Debian-policy package

For inclusion in Debian/Ubuntu, still add proper source packaging
(`debian/control`, `debian/rules`, `debian/changelog`, `debian/copyright`)
using `dh-cargo`/`debcargo`. The repository is structured so this can be added
without changing runtime layout.

## Ubuntu 20.04 compatibility

* build the release binary on Ubuntu 20.04 (or an equally old glibc);
* test on a clean Ubuntu 20.04 image;
* PTY behavior should be tested in a real VM, not only in containers.

## Package scripts

No root-level post-install behavior:

* do not start a system-wide daemon;
* do not create users;
* do not open firewall ports;
* do not create global secrets;
* do not enable remote access.

The per-user daemon starts only when that user invokes `jobwrap`, or explicitly
enables the user systemd service.

## systemd user units

```text
/usr/lib/systemd/user/jobwrapd.service
```

The service runs the daemon in the foreground with `Type=simple`. The daemon
does not implement systemd readiness notification or socket activation, so the
prototype socket unit is not shipped. Self-start through `jobwrap` remains the
default. Users who want systemd supervision can run:

```sh
jobwrap daemon stop
systemctl --user enable --now jobwrapd.service
```

With that service enabled, use `systemctl --user restart jobwrapd.service` to
restart it. Package installation does not enable the service for any user.
