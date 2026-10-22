// Recovery drafts (app/drafts.ts) stay on this device, in IndexedDB: they are
// written whether or not the server can be reached, and no other device
// sees them (ТЗ-Plain-Web §9.2). app/drafts.ts still speaks in paths under
// /data/drafts; fs.ts and core.ts send those here.
//
// Every tab keeps its own. Two tabs may have the same document open, and a
// save in one must not delete what the other has not saved. A tab's drafts
// are offered by another only once it has gone — closed, crashed or
// reloaded — which a Web Lock held for the tab's lifetime tells.

export const DRAFTS = "/data/drafts";

export function isDraft(path: string): boolean {
  return path === DRAFTS || path.startsWith(`${DRAFTS}/`);
}

interface Draft {
  key: string;
  tab: string;
  path: string;
  text: string;
}

export interface DraftStore {
  all(): Promise<Draft[]>;
  put(draft: Draft): Promise<void>;
  delete(key: string): Promise<void>;
}

const TAB = typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : String(Math.random());
const LOCK = "plain-tab-";

if (typeof navigator !== "undefined" && navigator.locks) {
  void navigator.locks.request(`${LOCK}${TAB}`, () => new Promise<never>(() => undefined));
}

let database: Promise<IDBDatabase> | null = null;

/** One connection for the tab's lifetime, opened on first use. */
function opened(): Promise<IDBDatabase> {
  database ??= new Promise((resolve, reject) => {
    const open = indexedDB.open("plain", 1);
    open.onupgradeneeded = () => open.result.createObjectStore("drafts", { keyPath: "key" });
    open.onerror = () => {
      database = null;
      reject(open.error?.message ?? "no IndexedDB");
    };
    open.onsuccess = () => resolve(open.result);
  });
  return database;
}

async function request<T>(run: (store: IDBObjectStore) => IDBRequest<T>, mode: IDBTransactionMode): Promise<T> {
  const db = await opened();
  return new Promise((resolve, reject) => {
    const asked = run(db.transaction("drafts", mode).objectStore("drafts"));
    asked.onsuccess = () => resolve(asked.result);
    asked.onerror = () => reject(asked.error?.message ?? "IndexedDB refused");
  });
}

let store: DraftStore = {
  all: () => request((s) => s.getAll() as IDBRequest<Draft[]>, "readonly"),
  put: async (draft) => void (await request((s) => s.put(draft), "readwrite")),
  delete: async (key) => void (await request((s) => s.delete(key), "readwrite")),
};

/** Tests only. */
export function setDraftStore(next: DraftStore): void {
  store = next;
}

/**
 * This tab's drafts, and those of tabs that are no longer there. Without
 * Web Locks (a page served over plain http) there is no telling, and every
 * other tab counts as open: a draft not offered is better than one deleted
 * from under the tab still editing it.
 */
async function visible(): Promise<Draft[]> {
  const all = await store.all();
  if (typeof navigator === "undefined" || !navigator.locks) return all.filter((d) => d.tab === TAB);
  const held = (await navigator.locks.query()).held ?? [];
  const alive = new Set(held.map((lock) => lock.name?.slice(LOCK.length)));
  alive.delete(TAB);
  return all.filter((draft) => !alive.has(draft.tab));
}

export async function writeDraft(path: string, text: string): Promise<void> {
  await store.put({ key: `${TAB}\n${path}`, tab: TAB, path, text });
}

/** This tab's own copy first. */
export async function readDraft(path: string): Promise<string> {
  const found = (await visible()).filter((draft) => draft.path === path);
  const draft = found.find((d) => d.tab === TAB) ?? found[0];
  if (!draft) throw `${path}: not found`;
  return draft.text;
}

export async function draftExists(path: string): Promise<boolean> {
  return path === DRAFTS || (await visible()).some((d) => d.path === path || d.path.startsWith(`${path}/`));
}

export async function listDrafts(folder: string) {
  const names = new Set<string>();
  for (const draft of await visible()) {
    const at = draft.path.lastIndexOf("/");
    if (draft.path.slice(0, at) === folder) names.add(draft.path.slice(at + 1));
  }
  return [...names].map((name) => ({ name, isFile: true, isDirectory: false }));
}

/** Saved, discarded or moved: gone from this tab, and from any gone tab that had it. */
export async function removeDraft(path: string): Promise<void> {
  for (const draft of await visible()) {
    if (draft.path === path) await store.delete(draft.key);
  }
}
