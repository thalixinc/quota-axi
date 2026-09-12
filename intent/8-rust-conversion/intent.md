# Intent: rust-conversion
Author: Brainstorm. Status: draft.
Issue: #8. Epic: #8.

## Problem
quota-axi ships as a Node/npm tool (package.json, `axi-sdk-js`'s `runAxiCli` for routing/help/version/error framing, npm publish). Every other owned tool in the thalixinc axi stack — cf, cf-queue, cmux-axi, sdlc-axi, brainstorm-axi, sbox-axi, herdr-axi — ships as Rust/cargo: a clap CLI, the #368 `version` → update-available + `[y/N]` + `--yes` convention, and `cargo install --git` / GitHub-release distribution. quota-axi is the lone Node holdout, so the crew carries two CLI patterns and the `axi-sdk-js` dependency for one tool.

## Proposed outcome
A faithful Rust/cargo port of quota-axi with the CLI surface preserved 1:1: the same commands (`quota` implicit, `auth`, `models`, `version`, `update`), the same 12 flags, the same TOON/JSON output, the same 0/1/2 exit-code contract, and the same help/error framing. `version`/`--yes` is reimplemented as the Rust #368 impl (GitHub release feed, `cargo install --git --tag v<latest> --force`, interactive `[y/N]` on a tty, non-tty hint that never blocks). npm install/publish is replaced by cargo build + install-from-git/release.

## Affected users and systems
- Every consumer of `quota-axi` (agents and humans reading local quota windows).
- The `bin/` + `src/` TypeScript graph, `skills/quota-axi/SKILL.md` (the stub stays a stub; it points at the CLI, not the language), `package.json`/`pnpm-lock.yaml`, and the release pipeline (`release-please-config.json`, `.github/workflows/release-please.yml`).
- The thalixinc cargo-install path in `cf doctor` (the quota-axi row flips from `npm install -g github:thalixinc/quota-axi` to `cargo install --git https://github.com/thalixinc/quota-axi --force`).

## Constraints
- **Runtime guarantees do not change** (AGENTS.md, VISION.md, README Security Posture): providers stay read-only, quota-axi never routes/ranks/orders providers, and delegated credential refresh stays the one carve-out. The Rust port must reproduce these, not relax or re-architect them.
- **Identical CLI UX**: command surface, flags, TOON/JSON shape, exit codes (0 success / 1 all-providers-failed / 2 validation), and error framing must match the Node build, byte-for-byte where the contract pins it.
- **No new verbs or behavior** beyond a faithful port. The npm package name mapping and the release-pipeline re-point (release-please `node` → `rust`, tag shape) are follow-on distribution questions, not in the port's behavior scope.

## Open questions
- Release tag shape: today release-please emits `quota-axi-v0.1.41` (component-prefixed) and publishes no GitHub Releases. The #368 impl reads `releases/latest` and installs `--tag v<latest>`. The release pipeline must be re-pointed to the `rust` release type (`v0.1.41` tags + GitHub Releases) as a follow-on; until then `version` correctly reports no newer release and `--yes` is a no-op at latest.
- Clean-cutover sequencing: the Node files are removed only when the Rust port reaches full parity (`quota`/`auth`/`models`/`--tui` all ported). Until then both trees coexist in the repo.

## Ticket breakdown (Plan)
1. Scaffold the cargo crate + port `version`/`update`/`-v`/`--version`/`--help` and the clap command surface (#8 first PR).
2. Port the `quota` data path: provider adapters (claude, codex, cursor, copilot, grok, kimi, zai, alibaba, opencode-go, agy), HTTP/proxy policy, credential selection, and cache.
3. Port interpretation + pace + TOON/JSON rendering (the normalized report model, schemaVersion 5).
4. Port the `auth` command.
5. Port the `models` command.
6. Port the `--tui` live terminal report.
7. Re-point the release pipeline to cargo (release-please `rust`, `cargo install --git`), update `cf doctor`, and remove the Node tree.
