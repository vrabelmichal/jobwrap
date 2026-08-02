# Core rules

- The agent may only read or modify files inside the project root.
- It must never change the working directory above the project root.
- It must not access `~`, `/tmp`, `/etc`, `/usr`, or sibling repositories unless the user explicitly authorizes it.
- It must never use `rm -r`, `rm -rf`, recursive deletion APIs, or broad glob deletion.
- Every deletion must name one exact file path.
- Directory deletion is prohibited unless the directory is first shown to be empty and the exact path is approved.
- Symlinks must not be followed for writes or deletions.
- Before modifying a file, the agent must confirm that the resolved path remains inside the project root.
- Generated files must go only into designated directories such as `target/`, `dist/`, or `.artifacts/`.
- No commands using `sudo`.
- No installation of system packages.
- No modification of global Git, Cargo, shell, editor, or system configuration.
- No force pushes, history rewrites, branch deletion, or destructive Git cleanup.
- No `git clean`, `git reset --hard`, or `git checkout -- .`.
- No editing `.git/` contents directly.
- No network downloads unless explicitly authorized.
- No execution of scripts fetched from the network.
- No commands containing shell-expanded destructive globs such as `rm *.rs`.
- If a command could affect more than one file, the agent must list the affected files first.

## File deletion policy

A strong deletion rule could read:

1. Recursive deletion is forbidden.
2. `rm -r`, `rm -rf`, `find ... -delete`, `git clean`, and equivalent APIs are forbidden.
3. Each file deletion must specify one exact, normalized path.
4. The path must resolve inside the project root.
5. The path must not be a symlink.
6. Multiple deletions require one explicit operation per file.
7. Directories may only be removed with `rmdir` after verifying they are empty.

Agents are required to use a safe helper command instead of raw deletion:

```text
jobwrap-dev delete path/to/exact-file.rs
```

That helper would:

- canonicalize the path;
- reject paths outside the repository;
- reject symlinks;
- reject directories;
- reject wildcards;
- print the file being deleted;
- optionally require a Git-tracked or generated-file classification;
- append the deletion to an audit log.

The same idea should apply to writes. A small execution wrapper could run all agent commands inside a restricted environment:

```text
jobwrap-dev exec -- cargo test
```

The wrapper could enforce:

- fixed project root;
- sanitized environment;
- no privilege escalation;
- path containment;
- command denylist;
- optional network isolation;
- resource limits;
- complete command logging.

For stronger enforcement, run the coding agent in a container or bubblewrap sandbox with only the repository mounted writable:

```text
/project          writable
/usr              read-only
/home             unavailable
/tmp              isolated
network           disabled by default
```

That is much safer than relying solely on prompt instructions. The rules guide the agent; the sandbox prevents mistakes.

Add this section to the implementation-agent instructions:

## Filesystem safety rules

- Treat the project root as the complete writable filesystem.
- Never read, write, rename, move, or delete anything outside it.
- Resolve and validate every destination path before modifying it.
- Never follow symlinks during modification or deletion.
- Recursive deletion is forbidden.
- Delete files individually by exact path.
- Remove directories only with `rmdir` after confirming they are empty.
- Never run `git clean`, `git reset --hard`, or equivalent destructive commands.
- Never use `sudo`.
- Never modify user-level or system-level configuration.
- Stop and report the issue if completing a task would require violating these rules.

One subtle but important addition: build tools such as Cargo normally write to `target/`, and some tests may use `/tmp`. Either permit a project-local temporary directory like `.tmp/`, or provide an isolated temporary directory mounted specifically for the agent. Otherwise ordinary test suites may accidentally violate the policy.
