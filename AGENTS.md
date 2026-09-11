# AGENTS.md

This file defines repository-wide engineering and workflow rules.

Keep it generic. Implementation plans, temporary research notes, handoff documents,
phase-specific decisions, and other working artifacts belong outside the repository
unless explicitly requested.

## Repository Discipline

- Inspect the current repository state before making changes.
- Treat the existing codebase as the source of truth.
- Do not assume files, configuration, branches, workflows, or behavior are absent without checking.
- Keep changes narrowly scoped to the task being worked on.
- Avoid unrelated refactors, formatting churn, or opportunistic rewrites.
- Prefer simple, auditable solutions over unnecessary abstraction or infrastructure.
- Do not introduce dependencies without a clear need.
- Do not commit secrets, credentials, tokens, private configuration, or sensitive data.

## Planning and Temporary Files

- Do not commit implementation plans, handoff documents, scratch notes, temporary research,
  local checklists, or other planning artifacts unless explicitly requested.
- Keep temporary work and investigation files local.
- Do not add generated or debug artifacts to the repository unless they are intentional project assets.
- Remove temporary files before considering a change complete.

## Branch Naming

Branches must describe the actual purpose of the work.

Use meaningful category-based names such as:

- `feat/<description>`
- `fix/<description>`
- `refactor/<description>`
- `perf/<description>`
- `test/<description>`
- `docs/<description>`
- `ci/<description>`
- `build/<description>`
- `chore/<description>`
- `hotfix/<description>`
- `release/<description>`

Examples:

- `feat/discord-presence`
- `fix/reconnect-state`
- `refactor/config-loading`
- `ci/rust-checks`

Do not create branches named after the tool, agent, model, or environment performing the work.

Forbidden patterns include, but are not limited to:

- `codex/...`
- `agent/...`
- `chatgpt/...`
- `ai/...`
- `claude/...`
- `copilot/...`

Avoid meaningless branch names such as:

- `test-branch`
- `changes`
- `update`
- `work`
- `temp`
- `new-branch`

The branch name should communicate the purpose of the change without requiring knowledge of who or what created it.

## Commit History

Maintain a clean, reviewable Git history.

- Commits should represent meaningful, coherent units of work.
- Avoid permanent commit noise such as:
  - `fix`
  - `oops`
  - `try again`
  - `small fix`
  - repeated formatting-only repair commits
  - multiple trivial follow-up commits for the same logical change
- Temporary, exploratory, or fixup commits are acceptable while actively developing.
- Before a branch is considered ready for review or merge, clean its history using interactive rebase,
  squash, fixup, or reword as appropriate.
- For a small correction to the latest logical commit, prefer amending that commit instead of creating
  another trivial commit.
- Do not squash unrelated changes merely to reduce commit count.
- Preserve meaningful logical boundaries between commits.
- Commit messages should describe what the change does, not the sequence of attempts used to arrive at it.

## History Rewriting

- Never rewrite the history of the default branch.
- Do not rewrite shared branch history without coordination.
- Rewriting a private or task-specific feature branch is allowed when cleaning its history.
- When updating an already-pushed feature branch after a rebase or amend, use `--force-with-lease`,
  never plain `--force`.

## Code Quality

- Prefer correctness and maintainability over cleverness.
- Keep modules and abstractions proportional to the current problem.
- Avoid speculative abstractions for features that do not exist yet.
- Handle external input and runtime failures explicitly.
- Do not silently swallow errors that affect correctness.
- Avoid unchecked assumptions around external data.
- Keep public interfaces intentionally small and explicit.
- Preserve backward compatibility where practical unless a breaking change is deliberate.

## Testing and Validation

Before considering a change complete:

- run the relevant formatter;
- run applicable static analysis and lints;
- run relevant automated tests;
- verify newly introduced behavior;
- verify important failure paths where practical.

Do not claim a check passed unless it was actually run successfully.

## Dependencies

- Add dependencies only when they provide clear value over a small local implementation or an already-used dependency.
- Prefer maintained, well-supported libraries.
- Avoid adding infrastructure merely for architectural aesthetics.
- Remove dependencies that become unused.

## Generated and Build Output

- Do not commit build output, caches, temporary logs, local environment files, or editor-generated artifacts.
- Commit generated files only when they are intentionally part of the repository and reproducibility or
  distribution requires them.

## Documentation

- Update repository documentation when behavior, setup, public interfaces, or operational procedures
  materially change.
- Do not duplicate large implementation plans inside repository documentation.
- Keep documentation consistent with the actual implementation.

## Self-Audit

After completing a task, re-audit the work before presenting it as finished.

The self-audit must review the actual final repository state, not merely the intended implementation.

At minimum, verify:

- the final diff matches the requested scope;
- no unrelated changes were introduced;
- no temporary, debug, planning, or local-only files were accidentally committed;
- no secrets or sensitive data were introduced;
- new dependencies are justified and actually used;
- error and failure paths remain sensible;
- public interfaces do not unintentionally expose internal state;
- relevant formatting, linting, tests, and validation have been run;
- documentation remains consistent with behavior where applicable;
- branch and commit history are clean enough for review;
- fixup or temporary commits have been amended, squashed, or rebased where appropriate;
- the working tree contains no unintended changes.

Review the final diff again after any cleanup, amend, or rebase because history cleanup itself can introduce mistakes.

If the audit finds a problem, fix it and repeat the relevant checks before declaring the task complete.

## Scope and Reviewability

A finished change should leave the repository in a state where another maintainer can:

1. understand what changed;
2. understand why it changed;
3. review the change without unrelated noise;
4. reproduce the relevant checks;
5. continue development without depending on hidden repository state.
