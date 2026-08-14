/**
 * Where the token is pasted, and the only screen this extension has.
 *
 * It is also what the icon opens when there is no credential, which is why it
 * explains itself rather than presenting a bare field: for most people this page
 * appears because they clicked the icon expecting a short link, not because they
 * went looking for settings.
 */

import { DEFAULT_BASE, clearToken, readBase, readToken, writeBase, writeToken } from "./api.js";

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);

  if (found === null) {
    throw new Error(`missing element: ${id}`);
  }

  return found as T;
}

const form = element<HTMLFormElement>("form");
const token = element<HTMLInputElement>("token");
const base = element<HTMLInputElement>("base");
const status = element<HTMLParagraphElement>("status");
const forget = element<HTMLButtonElement>("forget");
const tokensLink = element<HTMLAnchorElement>("tokens-link");

/** Cleared on the next edit, so a stale "Saved" never sits under a changed
 *  field claiming something that is no longer true. */
function say(message: string, tone: "ok" | "bad" = "ok"): void {
  status.textContent = message;
  status.classList.toggle("status--error", tone === "bad");
}

function syncLink(): void {
  tokensLink.href = base.value || DEFAULT_BASE;
}

async function load(): Promise<void> {
  base.value = await readBase();
  syncLink();

  // The stored token is never written back into the field. Filling it would put
  // a live credential into the DOM every time the page opens, to no end — the
  // person is here to replace it or to leave, and neither needs it visible.
  const stored = await readToken();
  if (stored !== null) {
    token.placeholder = "•••••••• (um token já está salvo)";
  }
}

form.addEventListener("submit", (event) => {
  event.preventDefault();

  void (async () => {
    const value = token.value.trim();

    if (!value.startsWith("oxid_pat_")) {
      say("Isso não parece um token do oxid — eles começam com oxid_pat_.", "bad");
      return;
    }

    await writeBase(base.value || DEFAULT_BASE);
    await writeToken(value);

    token.value = "";
    token.placeholder = "•••••••• (um token já está salvo)";
    syncLink();
    say("Pronto. Clique no ícone em qualquer página para encurtá-la.");
  })();
});

forget.addEventListener("click", () => {
  void (async () => {
    await clearToken();
    token.value = "";
    token.placeholder = "oxid_pat_…";
    say("Token removido deste navegador. Ele continua válido no site até ser revogado lá.");
  })();
});

token.addEventListener("input", () => say(""));
base.addEventListener("input", syncLink);

void load();
