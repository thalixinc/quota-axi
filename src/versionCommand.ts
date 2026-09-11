import { spawnSync } from "node:child_process";
import { createInterface } from "node:readline/promises";
import { fetchLatestVersion, isUpdateAvailable } from "axi-sdk-js";
import { VERSION } from "./version.js";

export const VERSION_HELP = `usage: quota-axi version [--yes]
Print the installed version and report an available update (from the npm registry).

flags:
  --yes   upgrade now when a newer version is published (npm install -g quota-axi)

examples:
  quota-axi version
  quota-axi version --yes
`;

const PACKAGE_NAME = "quota-axi";

/**
 * `quota-axi version [--yes]` — mirror cf's `version` verb (#368): print the installed version,
 * then report `update available` and upgrade on `--yes`, or prompt `[y/N]` on an interactive
 * terminal (non-tty prints a hint and never blocks). The feed is the npm registry
 * (`fetchLatestVersion`, registry HTTP with an `npm view` fallback); a failed/offline lookup
 * degrades to the bare version line (exit 0, never a hard failure).
 */
export async function versionCommand(args: string[]): Promise<string> {
  const yes = args.includes("--yes");
  const lines: string[] = [`${PACKAGE_NAME} ${VERSION}`];

  let latest: string;
  try {
    latest = await fetchLatestVersion(PACKAGE_NAME);
  } catch {
    // Offline / registry down / not found: report nothing beyond the version line.
    return lines.join("\n");
  }

  if (!isUpdateAvailable(VERSION, latest)) {
    return lines.join("\n");
  }

  if (yes) {
    installGlobal();
    return `${lines.join("\n")}\nupdate: ${PACKAGE_NAME} upgraded ${VERSION} -> ${latest}`;
  }

  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    return `${lines.join("\n")}\nupdate available: ${latest} — run \`${PACKAGE_NAME} update\`, or \`${PACKAGE_NAME} version --yes\` to update now`;
  }

  // Interactive prompt: read a line from the real stdin (the command's return value renders
  // only after this completes, so the prompt draws itself).
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  process.stdout.write(
    `update available: ${latest} — run ${PACKAGE_NAME} update, or --yes to update now [y/N] `,
  );
  const answer = await rl.question("");
  rl.close();
  if (/^y(es)?$/i.test(answer.trim())) {
    installGlobal();
    return `${lines.join("\n")}\nupdate: ${PACKAGE_NAME} upgraded ${VERSION} -> ${latest}`;
  }
  return `${lines.join("\n")}\n(kept at ${VERSION})`;
}

function installGlobal(): void {
  const result = spawnSync("npm", ["install", "-g", PACKAGE_NAME], {
    stdio: "inherit",
  });
  if (result.status !== 0) {
    throw new Error(
      `npm install -g ${PACKAGE_NAME} failed with exit ${result.status ?? "unknown"}`,
    );
  }
}
