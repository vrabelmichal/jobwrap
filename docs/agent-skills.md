# Personal agent skill setup

The maintained jobwrap skill is in `skills/jobwrap/`. Install the skill from
the same release as the local `jobwrap` executable; check `jobwrap --version`.
The 0.2.0 skill teaches explicit `jobwrap run` launches, detached `-d` servers,
read-only `show`/`status` queries, and the current monitoring lifetime limits.

## Shared discovery directory

Codex, Kilo, and OpenCode all discover personal skills from
`~/.agents/skills/`. Install the complete directory layout:

```text
~/.agents/skills/jobwrap/
  SKILL.md
  references/
    cli.md
    api.md
```

Copy all three files from the selected release's `skills/jobwrap/` directory.
Preserve the layout because the manifest links to those relative references.
No provider settings, API keys, plugin registration, or extra skill paths are
needed for this standard location. Updating the executable alone does not
refresh a personal skill copy.

When migrating an existing `~/.codex/skills/jobwrap/` installation, first compare
its files with the release and preserve any local changes. Once the shared
copy is complete, move the old directory outside skill discovery, for example
under `~/.codex/skill-backups/`. Keep one discoverable copy per skill name:
Codex can show duplicate names rather than merging their instructions. Also
check native Kilo/OpenCode directories and project-specific overrides when
troubleshooting an older skill appearing instead of the shared version.

Codex detects skill updates automatically; start a new turn, or restart Codex
if the new path does not appear. In Kilo, start a new session or use `/reload`.
For OpenCode, start a new session after updating. To inspect discovery without
starting an agent task, the current Kilo and OpenCode CLIs provide:

```sh
kilo debug skill --pure
opencode debug skill --pure
```

These commands list skill content as well as metadata. Redirect large output
to a file when checking it programmatically; some CLI versions truncate large
JSON output when stdout is a pipe. Confirm one `jobwrap` entry points to the
shared manifest, and compare all three installed files with the release.

The optional Debian `jobwrap-codex-skills` package is a separate administrator
installation under `/etc/codex/skills/`; see [Debian packaging](debian-packaging.md).
Check for duplicate names when combining administrator and personal installs.

Official discovery references:

- [Codex local skills](https://learn.chatgpt.com/docs/build-skills)
- [Kilo skill locations and reload](https://kilo.ai/docs/customize/skills)
- [OpenCode skill discovery](https://opencode.ai/docs/skills/)
