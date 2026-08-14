/**
 * The whole extension, in one small window.
 *
 * This replaced a design that shortened straight from the icon click and wrote
 * the clipboard by injecting a script into the page you were on. That worked on
 * about half the web: injection is refused on the browser's own pages, refused
 * before `activeTab` is granted, and — the one that actually bit — the clipboard
 * write inside an injected script needs the page focused and a permission that
 * was documented but never declared. Every one of those failures arrived as the
 * same silent `false`, and the only feedback was one character on a badge.
 *
 * A popup is an extension page. It owns its own document, it is focused by
 * definition, and it can say a sentence instead of a character. The one click it
 * costs buys away an entire class of bug.
 */

import {
  DEFAULT_BASE,
  api,
  clearToken,
  hasHostAccess,
  readBase,
  readToken,
  requestHostAccess,
  shorten,
  writeBase,
  writeToken,
  type Failure,
} from "./api.js";

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);

  if (found === null) {
    throw new Error(`missing element: ${id}`);
  }

  return found as T;
}

const heading = element<HTMLHeadingElement>("heading");
const subheading = element<HTMLSpanElement>("subheading");

const busy = element<HTMLParagraphElement>("busy");
const result = element<HTMLElement>("result");
const trouble = element<HTMLElement>("trouble");
const settings = element<HTMLElement>("settings");

const link = element<HTMLElement>("link");
const copyButton = element<HTMLButtonElement>("copy");
const note = element<HTMLParagraphElement>("note");

const troubleText = element<HTMLParagraphElement>("trouble-text");
const troubleAction = element<HTMLButtonElement>("trouble-action");

const form = element<HTMLFormElement>("form");
const token = element<HTMLInputElement>("token");
const base = element<HTMLInputElement>("base");
const feedback = element<HTMLParagraphElement>("feedback");

const tokensLink = element<HTMLAnchorElement>("tokens-link");
const settingsLink = element<HTMLButtonElement>("settings-link");
const forget = element<HTMLButtonElement>("forget");

type Screen = "busy" | "result" | "trouble" | "settings";

/** One screen at a time. Toggling `hidden` on all four from one place is what
 *  keeps two of them from ever being visible together. */
function show(which: Screen): void {
  busy.hidden = which !== "busy";
  result.hidden = which !== "result";
  trouble.hidden = which !== "trouble";
  settings.hidden = which !== "settings";

  settingsLink.hidden = which === "settings";
}

function say(message: string, tone: "ok" | "bad" = "ok"): void {
  feedback.textContent = message;
  feedback.classList.toggle("feedback--error", tone === "bad");
}

/**
 * `navigator.clipboard` first, `execCommand` when it refuses.
 *
 * The fallback is not superstition: `writeText` rejects without transient user
 * activation, and the activation from opening the popup is spent by the time the
 * server answers. `execCommand` is deprecated and still the only thing that
 * works in that moment — which is why `clipboardWrite` is in the manifest.
 */
async function toClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (cause) {
    console.warn("oxid: the clipboard API refused, falling back", cause);
  }

  const field = document.createElement("textarea");
  field.value = text;
  field.style.position = "fixed";
  field.style.opacity = "0";
  document.body.append(field);
  field.select();

  try {
    return document.execCommand("copy");
  } catch (cause) {
    console.error("oxid: could not write the clipboard", cause);
    return false;
  } finally {
    field.remove();
  }
}

/** Shown once the link exists, whether or not the clipboard cooperated. The
 *  link is on screen and selectable either way, so a failed copy is an
 *  inconvenience rather than a dead end. */
async function present(short: string, long: string): Promise<void> {
  heading.textContent = "Pronto";
  subheading.textContent = long;
  link.textContent = short;

  show("result");

  note.textContent = (await toClipboard(short))
    ? "Copiado para a área de transferência."
    : "Não consegui copiar daqui — use o botão acima.";
}

function troubleshoot(text: string, action: string, run: () => void): void {
  heading.textContent = "Não deu";
  subheading.textContent = "";
  troubleText.textContent = text;
  troubleAction.textContent = action;
  troubleAction.onclick = run;

  show("trouble");
}

/**
 * Every failure lands on a screen that offers the next step.
 *
 * A missing credential is not an error to report, it is a form to fill, so it
 * opens the form. A missing permission is a button, because the browser will
 * only grant it from a click. The rest are sentences with a retry.
 */
async function report(failure: Failure, retry: () => void): Promise<void> {
  if (failure.kind === "no-token") {
    await openSettings("Cole um token para começar.");
    return;
  }

  if (failure.kind === "unauthorized") {
    await openSettings("O servidor recusou este token. Ele pode ter sido revogado.", "bad");
    return;
  }

  if (failure.kind === "no-permission") {
    const host = await readBase();
    troubleshoot(
      `A extensão ainda não tem permissão para falar com ${host}.`,
      "Autorizar",
      () => {
        void (async () => {
          if (await requestHostAccess(host)) {
            retry();
          }
        })();
      },
    );
    return;
  }

  if (failure.kind === "offline") {
    troubleshoot("Não consegui falar com o servidor.", "Tentar de novo", retry);
    return;
  }

  troubleshoot(failure.message, "Tentar de novo", retry);
}

async function openSettings(message = "", tone: "ok" | "bad" = "ok"): Promise<void> {
  heading.textContent = "Configurações";
  subheading.textContent = "";

  base.value = await readBase();
  syncLink();

  // The stored token is never written back into the field. Filling it would put
  // a live credential into the DOM every time this opens, to no end — the person
  // is here to replace it or to leave, and neither needs it visible.
  const stored = await readToken();
  forget.hidden = stored === null;
  token.placeholder = stored === null ? "oxid_pat_…" : "•••••••• (um token já está salvo)";

  say(message, tone);
  show("settings");
  token.focus();
}

function syncLink(): void {
  tokensLink.href = base.value || DEFAULT_BASE;
}

/** The tab the popup was opened over. `activeTab` covers this: opening the popup
 *  is the invocation that grants it. */
async function currentTab(): Promise<chrome.tabs.Tab | undefined> {
  const [tab] = await api.tabs.query({ active: true, currentWindow: true });
  return tab;
}

async function run(): Promise<void> {
  heading.textContent = "Encurtando";
  subheading.textContent = "";
  show("busy");

  const tab = await currentTab();

  if (tab?.url === undefined) {
    troubleshoot("Não consegui ler o endereço desta aba.", "Configurações", () => {
      void openSettings();
    });
    return;
  }

  const outcome = await shorten(tab.url);

  if (outcome.ok) {
    await present(outcome.value.short_url, outcome.value.long_url);
    return;
  }

  await report(outcome.error, () => void run());
}

copyButton.addEventListener("click", () => {
  void (async () => {
    const value = link.textContent ?? "";
    note.textContent = (await toClipboard(value))
      ? "Copiado para a área de transferência."
      : "O navegador bloqueou a cópia. Selecione o link acima.";
  })();
});

form.addEventListener("submit", (event) => {
  event.preventDefault();

  void (async () => {
    const value = token.value.trim();

    if (!value.startsWith("oxid_pat_")) {
      say("Isso não parece um token do oxid — eles começam com oxid_pat_.", "bad");
      return;
    }

    const host = (base.value || DEFAULT_BASE).replace(/\/+$/, "");

    // Asked here because this click is the user gesture the browser requires,
    // and asked at all because declaring `host_permissions` is not holding it:
    // Chrome grants it at install, Firefox waits to be asked.
    if (!(await hasHostAccess(host)) && !(await requestHostAccess(host))) {
      say("Sem essa permissão a extensão não consegue encurtar nada. Tente salvar de novo.", "bad");
      return;
    }

    await writeBase(host);
    await writeToken(value);

    token.value = "";
    await run();
  })();
});

forget.addEventListener("click", () => {
  void (async () => {
    await clearToken();
    token.value = "";
    await openSettings("Token removido deste navegador. Ele continua válido no site até ser revogado lá.");
  })();
});

settingsLink.addEventListener("click", () => void openSettings());
token.addEventListener("input", () => say(""));
base.addEventListener("input", syncLink);

void (async () => {
  if ((await readToken()) === null) {
    await openSettings();
    return;
  }

  await run();
})();
