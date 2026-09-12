# Plan — #9 Document `version [--yes]` in CLI help and README

Parent epic: #1 (Maintain: quota-axi)
relates-to: thalixinc/codefactory#382

## Goal

The `version` verb shipped in #7 but its top-level help examples and the README
command table do not mention it. Close the documentation gap, docs-only.

## Files

1. `src/cli.ts` — `TOP_HELP.examples`: add `quota-axi version` and
   `quota-axi version --yes` (after the `models` examples, matching the
   `commands[4]` order: quota, auth, models, version).
2. `README.md` — CLI Reference command table: add a `version [--yes]` row
   after `update --check`.
3. `test/cli.test.ts` — extend the existing "prints the top-level help for
   --help" case to assert both example lines, guarding the help-string change.

`src/versionCommand.ts` `VERSION_HELP` already documents `version [--yes]` —
verify only, no change. `skills/quota-axi/SKILL.md` defers to `--help`, which
now covers the verb — no change.

## Order

1. Edit `src/cli.ts` examples.
2. Edit `README.md` command table.
3. Extend the help test assertion.
4. Build, test, format-check.

## Validation Strategy

- Unit: `test/cli.test.ts` "prints the top-level help for --help" now asserts
  `quota-axi version` and `quota-axi version --yes` appear in `--help` output.
- Smoke: `node dist/bin/quota-axi.js --help` lists both example lines.
- Behavior regression (verb already verified in #7): `version` and
  `version --yes` each print the bare version line with exit 0 when up to date
  (local 0.1.41 == npm latest 0.1.41).
- Full suite: `pnpm test` green.

## Proof

See `evidence/validate-2026-09-11.txt`.
