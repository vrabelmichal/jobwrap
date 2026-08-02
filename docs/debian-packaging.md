# Debian packaging

Two tracks are supported.

## Practical release package (cargo-deb)

`cargo-deb` builds binary `.deb` files from the Cargo project. It is suitable
for GitHub/GitLab releases and direct `apt install ./jobwrap_*.deb`.

Installed files:

```text
/usr/bin/jobwrap
/usr/libexec/jobwrap/jobwrapd
/usr/lib/systemd/user/jobwrapd.service
/usr/lib/systemd/user/jobwrapd.socket
/usr/share/doc/jobwrap/README.md.gz
/usr/share/doc/jobwrap/changelog.gz
/usr/share/man/man1/jobwrap.1.gz
/usr/share/man/man5/jobwrap.toml.5.gz
/usr/share/man/man8/jobwrapd.8.gz
/usr/share/bash-completion/completions/jobwrap
/usr/share/zsh/vendor-completions/_jobwrap
/usr/share/fish/vendor_completions.d/jobwrap.fish
```

No global writable state directory is installed. All runtime and persistent
state belongs to individual users under their XDG directories.

## Debian-policy package

For inclusion in Debian/Ubuntu, maintain proper source packaging
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

The per-user daemon starts only when that user invokes `jobwrap`, or through a
user systemd unit.

## systemd user units

```text
/usr/lib/systemd/user/jobwrapd.service
/usr/lib/systemd/user/jobwrapd.socket
```

The socket unit can activate the daemon; self-start is the default fallback.
