// Plain Web: a note out to other apps (ТЗ-Plain-Web §5.6) — the system's share
// sheet on a phone, a tablet and a Mac (Telegram, Mail, AirDrop, Files), a
// download where a browser has none. Everything is made before the choice
// is shown: Safari opens the sheet only straight from a tap.

import { flushActiveEditor } from "../editor";
import { exportHtml } from "../read/export";
import { exportPdf, htmlToPlainText, openPaths } from "./commands";
import { copyText } from "./platform";
import { fsError, readFile } from "./fs";
import { toDevice } from "./import";
import { basename, dirname } from "./paths";
import { render } from "../read/pipeline";
import { activeDoc, useStore } from "./store";

const MAIL_LIMIT = 1500;

type ShareNavigator = Navigator & {
  canShare?: (data: { files?: File[] }) => boolean;
  share?: (data: { files?: File[]; text?: string; title?: string }) => Promise<void>;
};

const nav = () => navigator as ShareNavigator;

function canShareFile(file: File): boolean {
  return typeof nav().share === "function" && nav().canShare?.({ files: [file] }) === true;
}

/** `text/…` downloads say UTF-8, or a phone shows Cyrillic as noise. */
const typed = (type: string) => (type.startsWith("text/") ? `${type};charset=utf-8` : type);

/**
 * The sheet with the file, or the file onto the device. Cancelling the sheet
 * is no error; a sheet that refuses (activation lost, a type it will not
 * take) is, and the file is saved instead.
 */
function deliver(file: File): void {
  if (!canShareFile(file)) return toDevice(file.name, file, typed(file.type));
  nav()
    .share!({ files: [file] })
    .catch((error: unknown) => {
      if ((error as { name?: string })?.name !== "AbortError") toDevice(file.name, file, typed(file.type));
    });
}

/** At most `limit` characters, never half of one (an emoji is two code units). */
const cut = (text: string, limit: number) => Array.from(text).slice(0, limit).join("");

/** `.md` where the sheet takes it; Chromium's does not, and gets `.txt`. */
function markdownFile(stem: string, text: string): File {
  const md = new File([text], `${stem}.md`, { type: "text/markdown" });
  // Not `stem.txt`: that is the plain-text choice, with other contents.
  const txt = new File([text], `${stem}.md.txt`, { type: "text/plain" });
  return typeof nav().share === "function" && !canShareFile(md) && canShareFile(txt) ? txt : md;
}

export interface Prepared {
  path: string;
  stem: string;
  md: File;
  html: File;
  text: File;
  plain: string;
}

/** The three files and the plain text, made from what is on screen or on disk. */
export async function prepare(path: string): Promise<Prepared> {
  const state = useStore.getState();
  const open = state.docs.find((doc) => doc.path === path);
  const source = open ? open.text : (await readFile(path)).text;
  const stem = basename(path).replace(/\.[^.]+$/, "");
  const html = await exportHtml(source, stem, dirname(path), {
    useDrawnDiagrams: open !== undefined && open.id === state.activeId,
  });
  const plain = htmlToPlainText(render(source).html);
  return {
    path,
    stem,
    md: markdownFile(stem, source),
    html: new File([html], `${stem}.html`, { type: "text/html" }),
    text: new File([plain], `${stem}.txt`, { type: "text/plain" }),
    plain,
  };
}

/** PDF is what the print sheet makes: on an iPhone and an iPad it shares it on. */
async function viaPrint(path: string): Promise<void> {
  try {
    if (activeDoc(useStore.getState())?.path !== path) await openPaths([path]);
    await exportPdf();
  } catch (error) {
    useStore.getState().setMessage(`couldn't print — ${fsError(error).message}`);
  }
}

/** `file → share…`, and the tree's: the choice, in Plain's own dialog. */
export async function shareNote(path: string): Promise<void> {
  // Whatever was typed a moment ago goes too.
  flushActiveEditor();
  const store = useStore.getState();
  store.setMessage("preparing…");
  let ready: Prepared;
  try {
    ready = await prepare(path);
  } catch (error) {
    store.setMessage(`couldn't prepare — ${fsError(error).message}`);
    return;
  }
  store.setMessage(null);
  const sheet = typeof nav().share === "function";
  const close = () => useStore.getState().setDialog(null);
  const act = (label: string, run: () => void) => ({
    label,
    run: () => {
      close();
      run();
    },
  });
  const message = sheet
    ? [
        act("as message", () => {
          nav()
            .share!({ text: ready.plain })
            .catch((error: unknown) => {
              if ((error as { name?: string })?.name !== "AbortError") store.setMessage("couldn't share");
            });
        }),
      ]
    : [
        act("mail", () => {
          const body = encodeURIComponent(cut(ready.plain, MAIL_LIMIT));
          location.href = `mailto:?subject=${encodeURIComponent(ready.stem)}&body=${body}`;
        }),
        act("telegram", () => {
          // Telegram wants `url`; the text itself serves.
          const text = encodeURIComponent(cut(ready.plain, MAIL_LIMIT));
          window.open(`https://t.me/share/url?url=${text}`, "_blank", "noopener");
        }),
        act("copy text", () =>
          void copyText(ready.plain).then(
            () => store.setMessage("copied"),
            () => store.setMessage("couldn't copy"),
          ),
        ),
      ];
  store.setDialog({
    title: sheet ? `share ${ready.stem}` : `download ${ready.stem}`,
    lines: sheet ? ["as a file, or the text as a message"] : ["this browser has no share sheet: the file is saved instead"],
    actions: [
      act("markdown", () => deliver(ready.md)),
      act("html", () => deliver(ready.html)),
      act("text", () => deliver(ready.text)),
      act("pdf", () => void viaPrint(path)),
      ...message,
      { label: "cancel", run: close },
    ],
    cancel: close,
  });
}
