/**
 * The whole extension: click the icon, the page you are on becomes a short link
 * on your clipboard.
 *
 * There is no popup. A popup would be a window to dismiss between wanting the
 * link and having it, and the four steps this replaces — leave the page, open
 * oxid, paste, copy — are the entire reason it exists.
 */

import { api, shorten, type Failure } from "./api.js";

const MENU_ID = "oxid-shorten-link";

/** How long the badge stays before the icon goes quiet again. */
const BADGE_MS = 2500;

/**
 * Writes to the clipboard from the page, not from here.
 *
 * `navigator.clipboard` does not exist in an MV3 service worker — there is no
 * document to own the selection. Injecting into the active tab is the way that
 * works in all three browsers, and `activeTab` grants it only for the tab the
 * person just clicked on, which is exactly the scope this needs.
 *
 * The fallback matters more than it looks: `navigator.clipboard.writeText`
 * rejects on a page that is not focused, and a tab can lose focus between the
 * click and the response arriving. `execCommand` is deprecated and still the
 * only thing that works in that moment.
 */
async function copy(tabId: number, text: string): Promise<boolean> {
  try {
    const [result] = await api.scripting.executeScript({
      target: { tabId },
      func: async (value: string) => {
        try {
          await navigator.clipboard.writeText(value);
          return true;
        } catch {
          const field = document.createElement("textarea");
          field.value = value;
          field.style.position = "fixed";
          field.style.opacity = "0";
          document.body.append(field);
          field.select();
          const copied = document.execCommand("copy");
          field.remove();
          return copied;
        }
      },
      args: [text],
    });

    return result?.result === true;
  } catch {
    // Injection is refused on the browser's own pages — the new-tab page, the
    // store, `about:` — and there is nothing to be done about it from here.
    return false;
  }
}

/** A badge is the only feedback available without a popup, so it carries both
 *  outcomes: the code when it worked, a mark when it did not. */
async function badge(text: string, colour: string): Promise<void> {
  await api.action.setBadgeText({ text });
  await api.action.setBadgeBackgroundColor({ color: colour });

  setTimeout(() => {
    void api.action.setBadgeText({ text: "" });
  }, BADGE_MS);
}

/**
 * Missing credentials open the options page rather than showing an error.
 *
 * "You are not signed in" is not a failure the person can act on from a badge,
 * and the stage's own goal says it: signed out, the click should invite you in
 * instead of failing.
 */
async function report(failure: Failure): Promise<void> {
  if (failure.kind === "no-token" || failure.kind === "unauthorized") {
    await api.runtime.openOptionsPage();
    return;
  }

  await badge("!", "#f0656a");
}

async function run(url: string | undefined, tabId: number | undefined): Promise<void> {
  if (url === undefined || tabId === undefined) {
    return;
  }

  const result = await shorten(url);

  if (!result.ok) {
    await report(result.error);
    return;
  }

  const copied = await copy(tabId, result.value.short_url);

  // The code itself, truncated to what a badge holds. It is a receipt: the
  // person can see the link was made even when the clipboard could not be
  // written, which is the case on pages the browser will not let us into.
  await badge(copied ? result.value.code.slice(0, 4) : "?", "#d4693a");
}

api.action.onClicked.addListener((tab) => {
  void run(tab.url, tab.id);
});

// The context menu covers the other half of the job: a link on the page rather
// than the page itself. Same path, same feedback.
api.runtime.onInstalled.addListener(() => {
  api.contextMenus.create({
    id: MENU_ID,
    title: "Encurtar este link com oxid",
    contexts: ["link"],
  });
});

api.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === MENU_ID) {
    void run(info.linkUrl, tab?.id);
  }
});
