# Personal agent skill setup

The maintained jobwrap skill is in `skills/jobwrap/`. Install the skill from
the same release as the local `jobwrap` executable; check `jobwrap --version`.
The 0.2.x skill teaches explicit `jobwrap run` launches, detached `-d` servers,
read-only `show`/`status` queries, and the current monitoring lifetime limits.

## Personal discovery directories

Codex, Kilo, and OpenCode all discover personal skills from
`~/.agents/skills/`. Claude Code discovers personal skills only from
`~/.claude/skills/`. Install the complete directory layout in each location
used by your agents:

```text
~/.agents/skills/jobwrap/     Codex, Kilo, OpenCode
~/.claude/skills/jobwrap/     Claude Code
  SKILL.md
  references/
    cli.md
    api.md
```

Copy all three files from the selected release's `skills/jobwrap/` directory.
Preserve the layout because the manifest links to those relative references.
No provider settings, API keys, plugin registration, or extra skill paths are
needed for these standard locations. Updating the executable alone does not
refresh a personal skill copy.

Kilo and OpenCode also read `~/.claude/skills/` for Claude Code compatibility.
With both personal locations installed they log a duplicate-name warning and
use one copy; keep both locations identical (or make them links to one
directory) so the choice does not matter.

### Development checkout

On a machine that runs jobwrap from a source checkout, for example with
`~/.local/bin/jobwrap` linked to `target/release/jobwrap`, link the skill
directories to the checkout instead of copying them. The skill then follows the
checked-out source exactly like the executable does:

```sh
ln -s /path/to/jobwrap/skills/jobwrap ~/.agents/skills/jobwrap
ln -s /path/to/jobwrap/skills/jobwrap ~/.claude/skills/jobwrap
```

Codex, Kilo, OpenCode, and Claude Code all follow a linked skill directory.
Rebuild the executable after switching branches so both stay matched. Use
copies, or the Debian packages, on machines that run released packages.

## Migrating and refreshing

When migrating an existing `~/.codex/skills/jobwrap/` installation, first compare
its files with the release and preserve any local changes. Once the shared
copy is complete, move the old directory outside skill discovery, for example
under `~/.codex/skill-backups/`. Keep one discoverable copy per skill name:
Codex can show duplicate names rather than merging their instructions. Also
check native Kilo/OpenCode directories and project-specific overrides when
troubleshooting an older skill appearing instead of the shared version.

Codex detects skill updates automatically; start a new turn, or restart Codex
if the new path does not appear. In Kilo, start a new session or use `/reload`.
For OpenCode and Claude Code, start a new session after updating. To inspect
discovery without starting an agent task, the current Kilo and OpenCode CLIs
provide:

```sh
kilo debug skill --pure
opencode debug skill --pure
```

These commands list skill content as well as metadata. Redirect large output
to a file when checking it programmatically; some CLI versions truncate large
JSON output when stdout is a pipe. Confirm one `jobwrap` entry points to the
expected manifest, and compare all three installed files with the release. In
Claude Code, `/skills` lists the discovered skills.

## System-wide Debian packages

The optional Debian packages `jobwrap-codex-skills`, `jobwrap-claude-skills`,
`jobwrap-opencode-skills`, and `jobwrap-kilo-skills` are separate administrator
installations under `/etc/`; see [Debian packaging](debian-packaging.md).
Check for duplicate names when combining administrator and personal installs.

Official discovery references:

- [Codex local skills](https://learn.chatgpt.com/docs/build-skills)
- [Claude Code skill locations](https://code.claude.com/docs/en/skills)
- [Kilo skill locations and reload](https://kilo.ai/docs/customize/skills)
- [OpenCode skill discovery](https://opencode.ai/docs/skills/)
