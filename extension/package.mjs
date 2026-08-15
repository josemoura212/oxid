/**
 * Zips each built extension into an archive a store will accept.
 *
 * Separate from `build.mjs` because the two answer different questions: building
 * is what you do to load an unpacked extension while working on it, and this is
 * what you do once, to upload. Running the zip on every build would produce a
 * file nobody asked for and hide the moment a release is actually cut.
 *
 * **The manifest has to sit at the root of the archive.** `zip -r out.zip dir`
 * stores `dir/manifest.json`, and addons.mozilla.org rejects that with an error
 * about a missing manifest that says nothing about the real cause. Zipping from
 * inside the directory is the whole trick.
 */

import { execFile } from "node:child_process";
import { readFile, readdir, rm } from "node:fs/promises";
import { promisify } from "node:util";
import path from "node:path";

const run = promisify(execFile);
const root = path.dirname(new URL(import.meta.url).pathname);
const dist = path.join(root, "dist");

/** Safari is not here on purpose: it ships as an app bundle produced by
 *  `xcrun safari-web-extension-converter`, not as a zip an upload form takes. */
const STORES = ["chrome", "firefox"];

async function main() {
  for (const browser of STORES) {
    const source = path.join(dist, browser);

    const manifest = JSON.parse(
      await readFile(path.join(source, "manifest.json"), "utf8"),
    );

    const archive = path.join(dist, `oxid-${browser}-${manifest.version}.zip`);
    await rm(archive, { force: true });

    // `-r .` from inside the directory, so paths in the archive are relative to
    // it. `-X` drops the extra file attributes some tools flag as noise.
    await run("zip", ["-r", "-X", "-q", archive, "."], { cwd: source });

    const entries = await readdir(source);
    console.log(`${path.relative(root, archive)}  (${entries.length} entries)`);
  }
}

await main();
