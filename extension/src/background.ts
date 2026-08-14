/**
 * The context menu, and only the context menu.
 *
 * Clicking the toolbar icon opens `popup.html`, which does its own shortening
 * and its own clipboard write — an extension page can do both without asking
 * anything of the page underneath. This file is what is left: right-clicking a
 * link has no window to open, so it still shortens in the background, still
 * injects to reach the clipboard, and still answers with a badge.
 *
 * That path is the weaker one and it is worth knowing why it is kept: injection
 * is refused on the browser's own pages and before `activeTab` is granted, so a
 * right-click can fail where the icon cannot. The compensation is that a
 * right-click on a link always happens *in* a page, which is exactly the case
 * injection handles well.
 */

import { api, shorten, type Failure } from "./api.js";

const MENU_ID = "oxid-shorten-link";

/** How long the badge stays before the icon goes quiet again. */
const BADGE_MS = 2500;

/**
 * Writes to the clipboard from the page, not from here.
 *
 * `navigator.clipboard` does not exist in an MV3 service worker — there is no
 * document to own the selection. The fallback matters more than it looks:
 * `writeText` rejects on a page that is not focused, and `execCommand` is
 * deprecated and still the only thing that works in that moment. Both need
 * `clipboardWrite`, which the manifest declares.
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

    if (result?.result !== true) {
      console.warn("oxid: the page refused the clipboard write", result);
      return false;
    }

    return true;
  } catch (cause) {
    console.error("oxid: could not inject into the tab", cause);
    return false;
  }
}

/** A badge is the only feedback available here, so it carries both outcomes:
 *  the code when it worked, a mark when it did not. */
async function badge(text: string, colour: string): Promise<void> {
  await api.action.setBadgeText({ text });
  await api.action.setBadgeBackgroundColor({ color: colour });

  setTimeout(() => {
    void api.action.setBadgeText({ text: "" });
  }, BADGE_MS);
}

/**
 * Missing credentials open the settings rather than showing an error.
 *
 * "You are not signed in" is not a failure anyone can act on from a badge, and
 * both a missing token and a missing permission are granted on the same screen.
 */
async function report(failure: Failure): Promise<void> {
  if (
    failure.kind === "no-token" ||
    failure.kind === "unauthorized" ||
    failure.kind === "no-permission"
  ) {
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
