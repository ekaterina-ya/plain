// `@tauri-apps/api/core` in the web build (vite.config.ts, mode `web`): a
// command is a POST to the server, which answers the way Tauri would — the
// value, or the error as the rejection (server/src/api.rs).

import { isDraft, removeDraft, writeDraft } from "./drafts";
import { isDevice } from "./device";
import { lost, signedOut, watch, whenOnline } from "./link";
import { keepLocally, SETTINGS_FILE } from "./settings";

type Args = Record<string, unknown>;

/** Answered here: desktop window bookkeeping that a browser tab has no use for. */
const LOCAL: Record<string, (args: Args) => unknown> = {
  allow_asset_dir: () => null,
  print_page: () => window.print(),
  session_writer: () => null,
  set_open_docs: () => null,
  holding_window: () => [],
  quit_cancelled: () => null,
  file_association_registered: () => false,
  watch: (args) => watch(args.root as string | null, (args.files as string[]) ?? []),
};

/** Long enough for a large document; a dead line must not hold a save forever. */
const TIMEOUT_MS = 30_000;
/** What a proxy in front of the server answers while it is down. */
const GATEWAY = new Set([502, 503, 504]);

/** Every request that changes something says so (server/src/main.rs `ours`). */
export const HEADERS = { "X-Plain": "1" };

/** Drafts live on this device (drafts.ts); settings keep two values here too. */
async function routed(command: string, args: Args): Promise<{ value: unknown } | null> {
  const path = typeof args.path === "string" ? args.path : "";
  if (command === "write_text_atomic" && isDraft(path)) {
    return { value: await writeDraft(path, String(args.text)) };
  }
  if (command === "trash_path" && isDraft(path)) {
    // Nowhere to restore a draft from on the server: discarding it is final.
    return { value: await removeDraft(path) };
  }
  if (command === "write_text_atomic" && path === SETTINGS_FILE) keepLocally(String(args.text));
  // A file dropped from the device is not one of the server's (device.ts).
  if (command === "path_kind" && isDevice(path)) return { value: "missing" };
  return null;
}

async function post(command: string, args: Args): Promise<Response | null> {
  try {
    const response = await fetch(`/api/invoke/${command}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...HEADERS },
      body: JSON.stringify(args),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    // Signed out: the same as no line, until a sign-in in another tab.
    if (response.status === 401) signedOut();
    return GATEWAY.has(response.status) || response.status === 401 ? null : response;
  } catch {
    return null;
  }
}

export async function invoke<T>(command: string, args: Args = {}): Promise<T> {
  const local = LOCAL[command];
  if (local) return local(args) as T;
  const answer = await routed(command, args);
  if (answer) return answer.value as T;
  let response = await post(command, args);
  // No line. A save waits for it and goes again, base hash and all, so the
  // check against changes made meanwhile is the same one (server/src/api.rs
  // `write_document`). Anything else is refused.
  while (!response) {
    lost();
    if (command !== "write_file_atomic") throw `${command}: no connection`;
    await whenOnline();
    response = await post(command, args);
  }
  const text = await response.text();
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    // Nothing at all, or a page from somewhere: still a refusal, with words.
    throw `${command}: ${response.status} ${response.statusText}`.trim();
  }
  if (!response.ok) throw value;
  return value as T;
}

export function convertFileSrc(path: string): string {
  return `/api/asset?path=${encodeURIComponent(path)}`;
}
