/**
 * Every call the extension makes, and the credential it makes them with.
 *
 * The token is not the session cookie, and could not be: extensions do not share
 * cookies with the site in any way that holds across browsers. It is also better
 * on its own terms — revocable by itself, so uninstalling here does not sign
 * anyone out of the site.
 */

/** Where the shortener lives. Overridable so a development build can point at a
 *  local API without editing source. */
export const DEFAULT_BASE = "https://oxid.uk";

const TOKEN_KEY = "token";
const BASE_KEY = "base";

/** What a shorten produced. Mirrors `oxid_shared::ShortenResponse`. */
export interface Shortened {
  code: string;
  short_url: string;
  long_url: string;
}

/**
 * The reasons a shorten does not produce a link, kept apart from each other
 * because the extension answers each one differently: missing credentials open
 * the options page, a rejected one says so, and a network failure is worth
 * retrying.
 */
export type Failure =
  | { kind: "no-token" }
  | { kind: "unauthorized" }
  | { kind: "refused"; message: string }
  | { kind: "offline" };

export type Result = { ok: true; value: Shortened } | { ok: false; error: Failure };

/**
 * `browser` in Firefox and Safari, `chrome` in Chrome and Edge. Firefox and
 * Safari also expose `chrome` for compatibility, but only the `browser`
 * namespace is promise-based there without a polyfill — so preferring it means
 * the same `await` works everywhere and no shim ships.
 */
const api: typeof chrome =
  (globalThis as { browser?: typeof chrome }).browser ?? chrome;

export { api };

export async function readToken(): Promise<string | null> {
  const stored = await api.storage.local.get(TOKEN_KEY);
  const token = stored[TOKEN_KEY];

  return typeof token === "string" && token.length > 0 ? token : null;
}

export async function writeToken(token: string): Promise<void> {
  await api.storage.local.set({ [TOKEN_KEY]: token.trim() });
}

export async function clearToken(): Promise<void> {
  await api.storage.local.remove(TOKEN_KEY);
}

export async function readBase(): Promise<string> {
  const stored = await api.storage.local.get(BASE_KEY);
  const base = stored[BASE_KEY];

  return typeof base === "string" && base.length > 0 ? base : DEFAULT_BASE;
}

export async function writeBase(base: string): Promise<void> {
  // Trailing slash stripped here rather than at every call site: the API paths
  // all start with one, and `https://oxid.uk//v1/shorten` is a 404 nobody would
  // think to look for in storage.
  await api.storage.local.set({ [BASE_KEY]: base.trim().replace(/\/+$/, "") });
}

/**
 * Shortens one URL.
 *
 * Only http and https are sent. A browser tab can be sitting on `about:blank`,
 * a `file://` path or the extension's own options page, and the server would
 * reject all three — asking it is a round trip to learn something already
 * known, and the answer would arrive as a generic error.
 */
export async function shorten(url: string): Promise<Result> {
  if (!/^https?:\/\//i.test(url)) {
    return { ok: false, error: { kind: "refused", message: "unsupported page" } };
  }

  const token = await readToken();
  if (token === null) {
    return { ok: false, error: { kind: "no-token" } };
  }

  const base = await readBase();

  let response: Response;
  try {
    response = await fetch(`${base}/v1/shorten`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ url }),
    });
  } catch {
    return { ok: false, error: { kind: "offline" } };
  }

  if (response.status === 401) {
    return { ok: false, error: { kind: "unauthorized" } };
  }

  if (!response.ok) {
    // The API answers RFC 9457, so there is usually a sentence worth showing.
    // Falling back to the status matters because a proxy in the way may answer
    // with something that is not JSON at all.
    const message = await response
      .json()
      .then((body: { detail?: string; title?: string }) => body.detail ?? body.title ?? "")
      .catch(() => "");

    return {
      ok: false,
      error: { kind: "refused", message: message || `${response.status}` },
    };
  }

  return { ok: true, value: (await response.json()) as Shortened };
}
