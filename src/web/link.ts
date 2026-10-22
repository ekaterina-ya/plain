// The page's line to the server (ТЗ-Plain-Web §9.2). Changes in the library
// and settings from other tabs come over one WebSocket; a lost socket, or a
// request that fails on the way (core.ts), means offline until a socket opens
// again. Meanwhile the text stays in the editor and in its draft on this
// device, and a save waits for the line to come back (core.ts).

import { deliver, forwardEmits } from "./event";
import { withLocalAppearance } from "./settings";

/** What other tabs are told; the rest of the page's events stay in it. */
const SHARED = new Set(["plain:settings-changed"]);
const RETRY_MS = [1000, 1000, 2000, 3000, 5000];

let socket: WebSocket | null = null;
let started = false;
let opened = false;
let failures = 0;
let online = true;
/** The server said 401: this device's session is over (server/src/auth.rs). */
let signedIn = true;
let waiting: (() => void)[] = [];
/** The library and the open files, as watcher.ts last asked for them. */
let watched: string[] = [];

function banner(): void {
  void import("../app/store").then(({ useStore }) => {
    const store = useStore.getState();
    if (online) store.dismissBanner("offline");
    else if (!signedIn)
      // Signing in again happens in another tab, so this one keeps its text;
      // the cookie is shared, and the socket's next try gets through.
      store.showBanner({
        id: "offline",
        text: "signed out — edits stay on this device, saving waits until you sign in",
        actions: [{ label: "sign in", run: () => void window.open("/login", "_blank") }],
      });
    else
      store.showBanner({
        id: "offline",
        text: "no connection — edits stay on this device, saving waits for the connection",
      });
  });
}

function setOnline(next: boolean): void {
  if (online === next) return;
  online = next;
  banner();
  if (!next) return;
  const ready = waiting;
  waiting = [];
  ready.forEach((resume) => resume());
}

export function whenOnline(): Promise<void> {
  return online ? Promise.resolve() : new Promise((resume) => waiting.push(resume));
}

/** A request was turned away for want of a session: offline, and why. */
export function signedOut(): void {
  if (signedIn) {
    signedIn = false;
    if (!online) banner();
  }
  lost();
}

/** A request did not get through. The socket may not know yet: start over. */
export function lost(): void {
  // Before the first socket there is none to come back through.
  if (!started) return;
  setOnline(false);
  socket?.close();
}

/** Another tab's settings, with this device's own font size and column width. */
function incoming(event: string, payload: unknown): unknown {
  if (event !== "plain:settings-changed" || !payload || typeof payload !== "object") return payload;
  const { settings } = payload as { settings: unknown };
  // Every tab is `main`; this tells windows.ts the change is not its own.
  return { from: "remote", settings: withLocalAppearance(settings) };
}

function connect(): void {
  const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/events`);
  ws.onopen = () => {
    socket = ws;
    failures = 0;
    signedIn = true;
    setOnline(true);
    // Back after a gap: whatever changed meanwhile was not heard. The
    // library root and every open file, the way watcher.ts takes a change —
    // the tree is read again, each document checks its hash.
    if (opened && watched.length > 0) deliver("fs-change", { paths: watched });
    opened = true;
  };
  ws.onmessage = (message) => {
    try {
      const { event, payload } = JSON.parse(String(message.data)) as { event: string; payload: unknown };
      deliver(event, incoming(event, payload));
    } catch {
      /* not ours to read */
    }
  };
  ws.onclose = () => {
    if (socket === ws) socket = null;
    if (opened) setOnline(false);
    // A socket cannot say why it was refused; a plain request can — signed
    // out elsewhere (`/devices`) is a 401, not a dead line.
    void fetch("/api/events")
      .then((answer) => answer.status === 401 && signedIn && signedOut())
      .catch(() => undefined);
    setTimeout(connect, RETRY_MS[Math.min(failures++, RETRY_MS.length - 1)]);
  };
}

/** watcher.ts arms its watcher here (core.ts `watch`); the first call connects. */
export function watch(root: string | null, files: string[]): void {
  watched = root ? [root, ...files] : files;
  if (started || typeof WebSocket === "undefined") return;
  started = true;
  forwardEmits((event, payload) => {
    if (SHARED.has(event) && socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ event, payload }));
    }
  });
  connect();
}
