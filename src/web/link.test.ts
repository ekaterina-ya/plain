// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from "vitest";

/** A socket the test opens and closes by hand. */
class FakeSocket {
  static all: FakeSocket[] = [];
  static OPEN = 1;
  readyState = 0;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((message: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(public url: string) {
    FakeSocket.all.push(this);
  }
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  close() {
    this.readyState = 3;
    this.onclose?.();
  }
  send(text: string) {
    this.sent.push(text);
  }
  hear(event: string, payload: unknown) {
    this.onmessage?.({ data: JSON.stringify({ event, payload }) });
  }
}

const latest = () => FakeSocket.all[FakeSocket.all.length - 1] as FakeSocket;

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  FakeSocket.all = [];
  vi.stubGlobal("WebSocket", FakeSocket);
  // No server here: a closed socket's probe (link.ts) finds nothing.
  vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  localStorage.clear();
});

async function connected() {
  const core = await import("./core");
  const event = await import("./event");
  const { useStore } = await import("../app/store");
  await core.invoke("watch", { serial: 1, root: "/library", files: ["/library/a.md"] });
  latest().open();
  return { ...core, ...event, useStore };
}

it("a save without a line waits for it and goes again, base hash and all", async () => {
  const { invoke, useStore } = await connected();
  const bodies: string[] = [];
  let up = false;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_url: string, init?: RequestInit) => {
      bodies.push(String(init?.body));
      if (!up) throw new TypeError("Failed to fetch");
      return new Response('{"hash":"h2","snapshotError":null}');
    }),
  );
  const request = { request: { path: "/library/a.md", text: "x", baseHash: "h1" } };
  let done: unknown = null;
  const saving = invoke("write_file_atomic", request).then((value) => (done = value));
  await vi.advanceTimersByTimeAsync(0);
  expect(done).toBeNull();
  expect(useStore.getState().banners.map((b) => b.id)).toContain("offline");

  // Anything else is refused at once rather than left hanging.
  await expect(invoke("read_tree", { root: "/library" })).rejects.toBe("read_tree: no connection");

  up = true;
  await vi.advanceTimersByTimeAsync(1000);
  latest().open();
  await saving;
  expect(done).toEqual({ hash: "h2", snapshotError: null });
  expect(bodies[0]).toBe(bodies[bodies.length - 1]);
  await vi.advanceTimersByTimeAsync(0);
  expect(useStore.getState().banners.map((b) => b.id)).not.toContain("offline");
});

it("back after a gap, the tree and every open file are checked again", async () => {
  const { listen } = await connected();
  const heard: unknown[] = [];
  await listen("fs-change", (event) => heard.push(event.payload));
  latest().hear("fs-change", { paths: ["/library/b.md"] });
  latest().close();
  await vi.advanceTimersByTimeAsync(1000);
  latest().open();
  expect(heard).toEqual([{ paths: ["/library/b.md"] }, { paths: ["/library", "/library/a.md"] }]);
});

it("settings go to the other tabs, and come from them as somebody else's", async () => {
  const { emit, listen } = await connected();
  localStorage.setItem("plain:appearance", JSON.stringify({ fontSize: 15, contentWidth: 640 }));
  const heard: unknown[] = [];
  await listen("plain:settings-changed", (event) => heard.push(event.payload));
  await emit("plain:settings-changed", { from: "main", settings: { appearance: { fontSize: 13 } } });
  await emit("plain:tree-changed", {});
  expect(latest().sent).toEqual([
    JSON.stringify({ event: "plain:settings-changed", payload: { from: "main", settings: { appearance: { fontSize: 13 } } } }),
  ]);
  latest().hear("plain:settings-changed", {
    from: "main",
    settings: { appearance: { theme: "dark", fontSize: 20, contentWidth: 900 } },
  });
  expect(heard[1]).toEqual({
    from: "remote",
    settings: { appearance: { theme: "dark", fontSize: 15, contentWidth: 640 } },
  });
});

it("signed out, a save waits for a sign-in in another tab, and says so", async () => {
  const { invoke, useStore } = await connected();
  let signedIn = false;
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      signedIn ? new Response('{"hash":"h2","snapshotError":null}') : new Response("", { status: 401 }),
    ),
  );
  let done: unknown = null;
  const saving = invoke("write_file_atomic", { request: { path: "/library/a.md", baseHash: "h1" } }).then(
    (value) => (done = value),
  );
  await vi.advanceTimersByTimeAsync(0);
  expect(done).toBeNull();
  const banner = useStore.getState().banners.find((b) => b.id === "offline");
  expect(banner?.text).toMatch(/^signed out/);
  expect(banner?.actions?.[0]?.label).toBe("sign in");
  await expect(invoke("read_tree", { root: "/library" })).rejects.toBe("read_tree: no connection");

  // Signed in elsewhere: the cookie is shared, the socket gets through.
  signedIn = true;
  await vi.advanceTimersByTimeAsync(1000);
  latest().open();
  await saving;
  expect(done).toEqual({ hash: "h2", snapshotError: null });
});

it("a socket closed by a sign-out elsewhere is told apart from a dead line", async () => {
  const { useStore } = await connected();
  vi.stubGlobal("fetch", vi.fn(async () => new Response("", { status: 401 })));
  latest().close();
  await vi.advanceTimersByTimeAsync(0);
  expect(useStore.getState().banners.find((b) => b.id === "offline")?.text).toMatch(/^signed out/);
});
