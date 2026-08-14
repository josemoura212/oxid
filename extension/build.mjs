/**
 * Builds one unpacked extension per browser into `dist/<browser>/`.
 *
 * One source tree, three manifests. The code is identical everywhere — the
 * differences between the browsers live entirely in the manifest, and keeping
 * them there rather than behind runtime checks is what stops "works in Chrome"
 * from becoming a category of bug.
 *
 * No bundler. MV3 loads ES modules natively in all three, so `tsc` alone is the
 * whole toolchain, and this repository stays a Rust project with a small
 * TypeScript corner rather than one with a JavaScript build system in it.
 */

import { cp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import path from "node:path";

const run = promisify(execFile);
const root = path.dirname(new URL(import.meta.url).pathname);
const dist = path.join(root, "dist");

/** Every browser gets a directory named after it, so the packaging step and the
 *  store upload never have to guess which build they are holding. */
const BROWSERS = ["chrome", "firefox", "safari"];

async function main() {
  await rm(dist, { recursive: true, force: true });

  // Types are checked and JavaScript emitted in one pass. A failure here should
  // stop the build: shipping an extension that does not typecheck means finding
  // out from a store review a week later.
  await run("npx", ["tsc"], { cwd: root });

  const compiled = path.join(dist, ".tsc");
  const assets = ["popup.html", "popup.css"];

  for (const browser of BROWSERS) {
    const out = path.join(dist, browser);
    await mkdir(out, { recursive: true });

    for (const file of await readdir(compiled)) {
      await cp(path.join(compiled, file), path.join(out, file));
    }

    for (const asset of assets) {
      await cp(path.join(root, "src", asset), path.join(out, asset));
    }

    await cp(path.join(root, "icons"), path.join(out, "icons"), { recursive: true });

    const manifest = await readFile(path.join(root, "manifests", `${browser}.json`), "utf8");
    await writeFile(path.join(out, "manifest.json"), manifest);

    console.log(`built dist/${browser}`);
  }

  await rm(compiled, { recursive: true, force: true });
}

await main();
