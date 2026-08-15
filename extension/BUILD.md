# Build instructions for reviewers

This archive is the complete source of the submitted extension. Following the
steps below reproduces the contents of `oxid-firefox-<version>.zip` exactly.

## Why source is required

The package ships JavaScript produced by the TypeScript compiler (`tsc`). Nothing
is bundled, minified or obfuscated — every `.js` in the upload is the direct
output of the `.ts` of the same name in `src/`.

## Build environment

| | |
|---|---|
| Operating system | Any that runs Node.js. Built and verified on Linux (Manjaro, kernel 6.18). macOS and Windows work — nothing here is platform-specific except `zip`, see below. |
| Node.js | **22.22.0** (any 22.x works). Install from https://nodejs.org or via `nvm install 22`. |
| npm | **10.x**, ships with Node 22. No separate install. |
| TypeScript | **5.7.2**, pinned in `package.json` and installed by `npm ci`. Not needed globally. |
| `zip` | Only for the packaging step. Present by default on Linux and macOS; on Windows use WSL, or skip it — see "Verifying without zip" below. |

No compiler toolchain, no network access beyond the npm registry, no other
system dependency.

## Steps

```bash
npm ci             # installs exactly what package-lock.json pins
npm run package    # compiles and produces the archives
```

`npm run package` is `node build.mjs && node package.mjs`:

1. **`build.mjs`** runs `npx tsc` (configured by `tsconfig.json`), then copies
   `src/popup.html`, `src/popup.css`, `icons/` and `manifests/firefox.json`
   into `dist/firefox/`, renaming the manifest to `manifest.json`.
2. **`package.mjs`** zips `dist/firefox/` from *inside* the directory, so
   `manifest.json` lands at the root of the archive.

Output: `dist/oxid-firefox-<version>.zip`.

Both scripts are plain Node, under 100 lines each, and are in this archive.

## Verifying without `zip`

If `zip` is unavailable, run only the first half:

```bash
npm ci
node build.mjs
```

`dist/firefox/` then holds the same files the submitted archive contains, and
each can be compared against it directly.

## A note on byte-for-byte comparison

The `.js` files are byte-identical to the upload for the same Node and TypeScript
versions. The `.zip` itself is not, because a zip stores modification times —
compare the extracted files rather than the archives.

## Source layout

| Path | What it is |
|---|---|
| `src/api.ts` | Every HTTP call, and the stored credential |
| `src/background.ts` | The context-menu path |
| `src/popup.ts` | The popup: shorten, copy, settings |
| `src/popup.html`, `src/popup.css` | The popup's markup and styles, copied verbatim |
| `manifests/firefox.json` | Becomes `manifest.json` in the archive |
| `build.mjs`, `package.mjs` | The build |
| `README.md` | Design notes — why the permissions are these, why a popup |
