// Plain Web: files from this device into the library (plans/import.md). A
// browser cannot edit a file where it lies, so importing is a copy — picked
// in the system's file dialog (`file → import…`) or dropped on the window.
// The original on the device stays as it was.

import { writeFile } from "@tauri-apps/plugin-fs";
import { siblings } from "../library/ops";
import { uniqueName } from "../library/names";
import { joinPath, nodeAt, reloadTree } from "../library/tree";
import { deviceFile } from "../web/device";
import { openPaths } from "./commands";
import { fsError, readFile } from "./fs";
import { basename, dirname } from "./paths";
import { useStore } from "./store";

/** How many names to try before giving up; a folder is not an adversary. */
const TRIES = 50;
/** More than this, and only the first one opens: a tab each is a wall. */
const OPEN_AT_ONCE = 5;

/** The folder picked in the tree, the one around a picked file, or the root. */
function target(): string | null {
  const { libraryPath, tree, treeSelected } = useStore.getState();
  if (!libraryPath) return null;
  const node = treeSelected ? nodeAt(tree, treeSelected) : null;
  if (!node) return libraryPath;
  return node.dir ? node.path : dirname(node.path);
}

/** What the server says when a name is taken (server/src/api.rs `upload`). */
function isTaken(error: unknown): boolean {
  return typeof error === "object" && error !== null && (error as { kind?: string }).kind === "exists";
}

/** Under the first free name; the server never writes over a file. */
async function upload(folder: string, name: string, bytes: Uint8Array): Promise<string> {
  const taken = new Set(siblings(folder));
  for (let attempt = 0; attempt < TRIES; attempt += 1) {
    const free = uniqueName(name, taken);
    const path = joinPath(folder, free);
    try {
      await writeFile(path, bytes);
      return path;
    } catch (error) {
      if (!isTaken(error)) throw error;
      taken.add(free);
    }
  }
  throw new Error("no free name");
}

/** `into`: the folder the tree's menu was opened on; else as `target` picks. */
export async function importFiles(files: File[], into?: string): Promise<void> {
  const store = useStore.getState();
  const folder = into ?? target();
  if (files.length === 0) return;
  if (!folder) return store.setMessage("open a library first");
  // Settings may say `md` or `.MD`; the tree compares without case.
  const extensions = store.settings.library.extensions.map((e) => e.replace(/^\./, "").toLowerCase());
  const imported: string[] = [];
  const notes: string[] = [];
  for (const file of files) {
    // Safari on macOS may hand over `й` as two characters; the server would
    // keep both spellings as two files.
    const name = file.name.normalize("NFC");
    const dot = name.lastIndexOf(".");
    const extension = dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
    if (name.startsWith(".") || !extensions.includes(extension)) {
      notes.push(`skipped ${name}`);
      continue;
    }
    try {
      imported.push(await upload(folder, name, new Uint8Array(await file.arrayBuffer())));
    } catch (error) {
      notes.push(`couldn't import ${name} — ${fsError(error).message}`);
    }
  }
  const done = imported.length > 0 ? [`imported ${imported.length}`] : [];
  if (imported.length > OPEN_AT_ONCE) done.push("opened the first");
  if (notes.some((note) => note.startsWith("skipped"))) notes.push(`only .${extensions.join(", .")}`);
  // Before opening: a file that will not open says so over this.
  useStore.getState().setMessage([...done, ...notes].join(" · "));
  if (imported.length > 0) {
    await reloadTree();
    await openPaths(imported.length <= OPEN_AT_ONCE ? imported : imported.slice(0, 1));
  }
}

/**
 * Dropped on the window: the files behind the device paths (web/device.ts),
 * taken at once — the next drop or pick replaces what the paths stand for.
 */
export function droppedFiles(paths: string[]): File[] {
  return paths.map(deviceFile).filter((file): file is File => file !== undefined);
}

/**
 * The system's file dialog. Called straight from the menu, so the click is
 * still the user's — Safari opens no dialog for a script. No `accept`: iOS
 * greys out `.md` files it cannot match to a type, so the list is filtered
 * after the choice. The input sits in the page while it is open, or iOS may
 * never report the choice; without `cancel` (Safari before 16.4) a cancelled
 * dialog leaves it there, which is harmless.
 */
export function pickAndImport(into?: string): void {
  const input = document.createElement("input");
  input.type = "file";
  input.multiple = true;
  // Out of sight but not `display: none`, which some iOS versions will not open.
  input.style.cssText = "position:fixed;opacity:0;pointer-events:none;width:1px;height:1px";
  input.addEventListener("change", () => {
    const files = Array.from(input.files ?? []);
    input.remove();
    void importFiles(files, into);
  });
  input.addEventListener("cancel", () => input.remove());
  document.body.append(input);
  input.click();
}

/** A file onto this device: a download, from memory. */
export function toDevice(name: string, content: BlobPart, type: string): void {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * A note onto this device, as it is. Made here from its text rather than
 * linked to the server's file: a phone's home-screen app would open a link
 * in a viewer, and without a charset the Cyrillic in it.
 */
export async function downloadNote(path: string): Promise<void> {
  const store = useStore.getState();
  const open = store.docs.find((doc) => doc.path === path);
  try {
    const text = open ? open.text : (await readFile(path)).text;
    toDevice(basename(path), text, "text/markdown;charset=utf-8");
  } catch (error) {
    store.setMessage(`couldn't download — ${fsError(error).message}`);
  }
}
