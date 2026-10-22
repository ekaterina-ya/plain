// Dropping on the window (spec §6): a file opens, a folder becomes the
// library. Tauri hands over the paths; only the kind has to be asked for.

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { openLibraryPath, openPaths } from "../app/commands";
import { dropImagesInEditor } from "../editor";
import { inTauri, inWeb } from "../app/env";
import { droppedFiles, importFiles } from "../app/import";
import { useStore } from "../app/store";
import { pathKind } from "./tree";

async function accept(paths: string[]): Promise<void> {
  // Plain Web: what was dropped, taken before anything waits (app/import.ts).
  const dropped = inWeb ? droppedFiles(paths) : [];
  // Images dropped while editing go into the document, not into a tab (§2a).
  if (await dropImagesInEditor(paths)) return;
  // Plain Web: from the device, so into the library as a copy.
  if (inWeb) return importFiles(dropped);
  const files: string[] = [];
  let folder: string | null = null;
  for (const path of paths) {
    const kind = await pathKind(path);
    if (kind === "dir") folder ??= path;
    else if (kind === "file") files.push(path);
  }
  // The library first, so files dropped with it land inside a known root.
  if (folder) await openLibraryPath(folder);
  if (files.length > 0 && (await openPaths(files)) && !folder) {
    useStore.getState().setRailCollapsed(true);
  }
}

export function installDrop(): () => void {
  if (!inTauri) return () => undefined;
  const unlisten = getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type !== "drop") return;
    void accept(event.payload.paths ?? []);
  });
  return () => void unlisten.then((off) => off());
}
