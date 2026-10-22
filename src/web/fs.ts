// `@tauri-apps/plugin-fs`: the calls Plain makes, by way of the server —
// except drafts, which stay on this device (drafts.ts).

import { convertFileSrc, HEADERS, invoke } from "./core";
import { signedOut } from "./link";
import { deviceFile } from "./device";
import { draftExists, isDraft, listDrafts, readDraft, removeDraft } from "./drafts";
import { readLocally, SETTINGS_FILE } from "./settings";

interface Stat {
  isFile: boolean;
  isDirectory: boolean;
  size: number;
  mtime: number | null;
}

/** editor/images.ts builds `assets\x.png` the Windows way; the server takes `/` only. */
const slash = (path: string) => path.replace(/\\/g, "/");

async function statOf(path: string): Promise<Stat | null> {
  return invoke<Stat | null>("fs_stat", { path: slash(path) });
}

export async function exists(path: string): Promise<boolean> {
  if (isDraft(path)) return draftExists(path);
  return (await statOf(path)) !== null;
}

export async function stat(path: string) {
  const info = await statOf(path);
  if (!info) throw `${path}: not found`;
  return { ...info, mtime: info.mtime === null ? null : new Date(info.mtime) };
}

export async function readTextFile(path: string): Promise<string> {
  if (isDraft(path)) return readDraft(path);
  const { text } = await invoke<{ text: string }>("read_file", { path: slash(path) });
  return path === SETTINGS_FILE ? readLocally(text) : text;
}

export async function readFile(path: string): Promise<Uint8Array> {
  const response = await fetch(convertFileSrc(slash(path)));
  if (response.status === 401) signedOut();
  if (!response.ok) throw `${path}: ${response.status}`;
  return new Uint8Array(await response.arrayBuffer());
}

export async function readDir(path: string) {
  if (isDraft(path)) return listDrafts(path);
  return invoke<{ name: string; isFile: boolean; isDirectory: boolean }[]>("fs_read_dir", { path: slash(path) });
}

/** Only drafts are removed this way, and they are on this device. */
export async function remove(path: string, _options?: { recursive?: boolean }): Promise<void> {
  if (!isDraft(path)) throw `${path}: not available`;
  await removeDraft(path);
}

/** The `assets` folder beside a document (editor/images.ts). */
export async function mkdir(path: string, _options?: { recursive?: boolean }): Promise<void> {
  await invoke("create_dir", { path: slash(path) });
}

/** A pasted picture, into the library; never over a file that is there. */
export async function writeFile(path: string, bytes: Uint8Array | Blob): Promise<void> {
  const response = await fetch(`/api/upload?path=${encodeURIComponent(slash(path))}`, {
    method: "POST",
    headers: HEADERS,
    body: bytes as BodyInit,
  });
  if (response.status === 401) signedOut();
  if (!response.ok) throw await response.json().catch(() => `${path}: ${response.status}`);
}

/** Only a file from this device, dropped or picked (device.ts), is copied in. */
export async function copyFile(from: string, to: string): Promise<void> {
  const file = deviceFile(from);
  if (!file) throw `${from}: not on this device`;
  await writeFile(to, file);
}
