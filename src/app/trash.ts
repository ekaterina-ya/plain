// Plain Web's recycle bin (server/src/trashcan.rs): what `delete` took from
// the library in the last thirty days, put back from `file → restore from
// recycle bin…`. The desktop hands deleted files to the system's own.

import { invoke } from "@tauri-apps/api/core";
import { reloadTree } from "../library/tree";
import { fsError } from "./fs";
import { useStore } from "./store";

interface Deleted {
  id: string;
  path: string;
  deletedAt: number;
}

/** As many as the dialog shows without scrolling: the newest. */
const SHOWN = 10;

export function ago(at: number, now = Date.now()): string {
  const minutes = Math.max(0, Math.round((now - at) / 60_000));
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  return hours < 24 ? `${hours} h ago` : `${Math.round(hours / 24)} d ago`;
}

async function restore(entry: Deleted): Promise<void> {
  const store = useStore.getState();
  try {
    await invoke("trash_restore", { id: entry.id });
  } catch (error) {
    store.setMessage(`couldn't restore — ${fsError(error).message}`);
    return;
  }
  await reloadTree();
  store.setMessage("restored");
}

export async function openTrash(): Promise<void> {
  const store = useStore.getState();
  let deleted: Deleted[];
  try {
    deleted = await invoke<Deleted[]>("trash_list");
  } catch (error) {
    store.setMessage(`couldn't open the recycle bin — ${fsError(error).message}`);
    return;
  }
  if (deleted.length === 0) {
    store.setMessage("the recycle bin is empty");
    return;
  }
  const close = () => store.setDialog(null);
  store.setDialog({
    title: "restore from recycle bin",
    actions: [
      ...deleted.slice(0, SHOWN).map((entry) => ({
        label: `${entry.path.replace(/^\/library\//, "")} · ${ago(entry.deletedAt)}`,
        run: () => {
          close();
          void restore(entry);
        },
      })),
      { label: "cancel", run: close },
    ],
    cancel: close,
  });
}
