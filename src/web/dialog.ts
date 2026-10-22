// `@tauri-apps/plugin-dialog`: one library, always open; nothing to pick but
// a picture to insert (editor/images.ts), from this device.

import { register } from "./device";

interface OpenOptions {
  directory?: boolean;
  filters?: { name: string; extensions: string[] }[];
  [option: string]: unknown;
}

export async function open(options?: OpenOptions): Promise<string | null> {
  if (!options?.filters || options.directory) return null;
  const input = document.createElement("input");
  input.type = "file";
  input.accept = options.filters.flatMap((f) => f.extensions.map((e) => `.${e}`)).join(",");
  return new Promise((resolve) => {
    input.addEventListener("change", () => {
      const file = input.files?.[0];
      resolve(file ? (register([file])[0] ?? null) : null);
    });
    // Browsers without `cancel` (Safari before 16.4) leave this unanswered,
    // which only means the insert never happens.
    input.addEventListener("cancel", () => resolve(null));
    input.click();
  });
}

/**
 * Save As: a name, in the same folder of the library — what the copy of a
 * conflicting document needs most (save.ts). The server writes a name that
 * is taken only for the document's own file, never over another one.
 */
export async function save(options?: { defaultPath?: string; [option: string]: unknown }): Promise<string | null> {
  const suggested = (options?.defaultPath ?? "/library/Untitled.md").replace(/\\/g, "/");
  const slash = suggested.lastIndexOf("/");
  const folder = slash > 0 ? suggested.slice(0, slash) : "/library";
  const answer = window.prompt("save as — a name in this folder", suggested.slice(slash + 1))?.trim();
  if (!answer || /[\\/]/.test(answer) || answer.startsWith(".")) return null;
  return `${folder}/${answer.normalize("NFC")}`;
}
