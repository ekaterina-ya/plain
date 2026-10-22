// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { convertFileSrc, invoke } from "./core";
import { register } from "./device";
import { setDraftStore, type DraftStore } from "./drafts";
import { emit, listen } from "./event";
import { copyFile, exists, mkdir, readDir, readTextFile, remove, stat } from "./fs";
import { keepLocally, readLocally, withLocalAppearance } from "./settings";
import { getCurrentWebview } from "./webview";
import { getCurrentWindow } from "./window";

function answer(status: number, body: string) {
  const fetch = vi.fn(async (_url: string, _init?: RequestInit) => new Response(body, { status }));
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

function memoryDrafts(): Map<string, { key: string; tab: string; path: string; text: string }> {
  const rows = new Map<string, { key: string; tab: string; path: string; text: string }>();
  const store: DraftStore = {
    all: async () => [...rows.values()],
    put: async (draft) => void rows.set(draft.key, draft),
    delete: async (key) => void rows.delete(key),
  };
  setDraftStore(store);
  return rows;
}

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe("invoke", () => {
  it("posts the arguments to the command, saying it is the app", async () => {
    const fetch = answer(200, '{"text":"a"}');
    await expect(invoke("read_file", { path: "/library/a.md" })).resolves.toEqual({ text: "a" });
    expect(fetch).toHaveBeenCalledWith(
      "/api/invoke/read_file",
      expect.objectContaining({
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Plain": "1" },
        body: '{"path":"/library/a.md"}',
      }),
    );
  });

  it("sends an empty object when there are no arguments", async () => {
    const fetch = answer(200, "null");
    await invoke("take_opening");
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ body: "{}" });
  });

  it("rejects with the error the server sent, as Tauri does", async () => {
    answer(400, '{"kind":"missing","message":"/library/a.md is gone"}');
    await expect(invoke("read_file", { path: "/library/a.md" })).rejects.toEqual({
      kind: "missing",
      message: "/library/a.md is gone",
    });
    answer(400, '"not a folder: /library/x"');
    await expect(invoke("read_tree", { root: "/library/x" })).rejects.toBe("not a folder: /library/x");
  });

  it("rejects with words when the answer is not JSON", async () => {
    answer(500, "<html>oops</html>");
    await expect(invoke("read_tree")).rejects.toMatch(/^read_tree: 500/);
  });

  it("answers desktop window commands without the network", async () => {
    const fetch = answer(200, "null");
    await expect(invoke("holding_window", { keys: [] })).resolves.toEqual([]);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("says a dropped device file is not one of the server's", async () => {
    const fetch = answer(200, '"file"');
    await expect(invoke("path_kind", { path: "/device/0/a.md" })).resolves.toBe("missing");
    expect(fetch).not.toHaveBeenCalled();
  });
});

it("convertFileSrc keeps spaces and Cyrillic intact", () => {
  const url = convertFileSrc("/library/папка/my pic.png");
  expect(url).toBe("/api/asset?path=%2Flibrary%2F%D0%BF%D0%B0%D0%BF%D0%BA%D0%B0%2Fmy%20pic.png");
  expect(decodeURIComponent(url.split("=")[1] ?? "")).toBe("/library/папка/my pic.png");
});

it("events reach their listeners until unlisten", async () => {
  const seen: unknown[] = [];
  const off = await listen<number>("tick", (event) => seen.push(event.payload));
  await emit("tick", 1);
  off();
  await emit("tick", 2);
  expect(seen).toEqual([1]);
});

it("stat gives mtime as a Date", async () => {
  answer(200, '{"isFile":true,"isDirectory":false,"size":3,"mtime":1700000000000}');
  const info = await stat("/library/a.md");
  expect(info.mtime).toEqual(new Date(1700000000000));
});

describe("drafts stay on this device", () => {
  it("are written, listed, read and removed without the server", async () => {
    const fetch = answer(500, "");
    memoryDrafts();
    await invoke("write_text_atomic", { path: "/data/drafts/a1.json", text: "{}" });
    await invoke("write_text_atomic", { path: "/data/drafts/deferred/b2.json", text: "[]" });
    expect(await exists("/data/drafts")).toBe(true);
    expect(await exists("/data/drafts/deferred")).toBe(true);
    expect(await readDir("/data/drafts")).toEqual([{ name: "a1.json", isFile: true, isDirectory: false }]);
    expect(await readDir("/data/drafts/deferred")).toEqual([
      { name: "b2.json", isFile: true, isDirectory: false },
    ]);
    expect(await readTextFile("/data/drafts/a1.json")).toBe("{}");
    await remove("/data/drafts/a1.json");
    expect(await exists("/data/drafts/a1.json")).toBe(false);
    // `discard` on the recovery screen.
    await invoke("trash_path", { path: "/data/drafts/deferred/b2.json" });
    expect(await exists("/data/drafts/deferred/b2.json")).toBe(false);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("from a tab that has gone are offered, from a live one are not", async () => {
    const rows = memoryDrafts();
    rows.set("live", { key: "live", tab: "live", path: "/data/drafts/x.json", text: "open elsewhere" });
    rows.set("gone", { key: "gone", tab: "gone", path: "/data/drafts/y.json", text: "left behind" });
    vi.stubGlobal("navigator", {
      locks: { query: async () => ({ held: [{ name: "plain-tab-live" }] }) },
    });
    expect(await readDir("/data/drafts")).toEqual([{ name: "y.json", isFile: true, isDirectory: false }]);
    await expect(readTextFile("/data/drafts/x.json")).rejects.toMatch(/not found/);
    // Restored here and saved: the gone tab's copy goes, the live one's stays.
    await remove("/data/drafts/y.json");
    await remove("/data/drafts/x.json");
    expect([...rows.keys()]).toEqual(["live"]);
  });

  it("of other tabs are left alone where tabs cannot be told apart", async () => {
    const rows = memoryDrafts();
    rows.set("other", { key: "other", tab: "other", path: "/data/drafts/x.json", text: "theirs" });
    vi.stubGlobal("navigator", {});
    expect(await readDir("/data/drafts")).toEqual([]);
    await remove("/data/drafts/x.json");
    expect(rows.has("other")).toBe(true);
  });

  it("of the same document in two tabs are two drafts", async () => {
    const rows = memoryDrafts();
    rows.set("other", { key: "other", tab: "other", path: "/data/drafts/x.json", text: "theirs" });
    vi.stubGlobal("navigator", {
      locks: { query: async () => ({ held: [{ name: "plain-tab-other" }] }) },
    });
    await invoke("write_text_atomic", { path: "/data/drafts/x.json", text: "mine" });
    expect(await readTextFile("/data/drafts/x.json")).toBe("mine");
    await remove("/data/drafts/x.json");
    expect(rows.get("other")?.text).toBe("theirs");
  });
});

describe("font size and column width belong to the device", () => {
  const file = (fontSize: number, contentWidth: number, theme = "dark") =>
    JSON.stringify({ appearance: { theme, fontSize, contentWidth } });

  it("are taken from the server the first time, and kept here after", () => {
    expect(JSON.parse(readLocally(file(14, 600)))).toMatchObject({ appearance: { fontSize: 14 } });
    // Another device has changed them since: this one keeps its own.
    expect(JSON.parse(readLocally(file(18, 900, "light")))).toEqual({
      appearance: { theme: "light", fontSize: 14, contentWidth: 600 },
    });
  });

  it("follow what this device writes", async () => {
    answer(200, "null");
    await invoke("write_text_atomic", { path: "/data/settings.json", text: file(16, 700) });
    expect(withLocalAppearance(JSON.parse(file(12, 500)))).toEqual({
      appearance: { theme: "dark", fontSize: 16, contentWidth: 700 },
    });
    keepLocally("not json");
    expect(JSON.parse(readLocally(file(12, 500)))).toMatchObject({ appearance: { fontSize: 16 } });
  });

  it("are read from the file the settings come in", async () => {
    keepLocally(file(17, 800));
    answer(200, JSON.stringify({ text: file(12, 500) }));
    expect(JSON.parse(await readTextFile("/data/settings.json"))).toMatchObject({
      appearance: { fontSize: 17, contentWidth: 800 },
    });
  });
});

describe("pictures", () => {
  it("go to the server with forward slashes, never over a file", async () => {
    const fetch = answer(200, "null");
    await mkdir("/library/notes\\assets");
    expect(fetch.mock.calls[0]?.[1]?.body).toBe('{"path":"/library/notes/assets"}');
    const [path] = register([new File(["png"], "my pic.png")]);
    await copyFile(path ?? "", "/library/notes\\assets\\my pic.png");
    const [url, init] = fetch.mock.calls[1] ?? [];
    expect(url).toBe("/api/upload?path=%2Flibrary%2Fnotes%2Fassets%2Fmy%20pic.png");
    expect(init).toMatchObject({ method: "POST", headers: { "X-Plain": "1" } });
    expect(await (init?.body as File).text()).toBe("png");
  });

  it("from anywhere but this device are not copied", async () => {
    await expect(copyFile("/library/a.png", "/library/b.png")).rejects.toMatch(/not on this device/);
  });

  it("dropped on the page arrive as device paths, and only files are caught", async () => {
    const seen: string[][] = [];
    const off = await getCurrentWebview().onDragDropEvent((event) => seen.push(event.payload.paths ?? []));
    const drop = (types: string[], files: File[]) => {
      const event = new Event("drop", { cancelable: true });
      Object.defineProperty(event, "dataTransfer", { value: { types, files } });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    };
    expect(drop(["Files"], [new File(["x"], "a.png")])).toBe(true);
    expect(drop(["text/plain"], [])).toBe(false);
    off();
    expect(seen).toEqual([["/device/0/a.png"]]);
  });
});

describe("leaving the page", () => {
  it("asks only when something is unsaved", async () => {
    const { useStore, makeDoc } = await import("../app/store");
    const off = await getCurrentWindow().onCloseRequested(() => undefined);
    const leave = () => {
      const event = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    };
    expect(leave()).toBe(false);
    useStore.setState({ docs: [makeDoc({ id: "a", path: null, text: "edited", savedText: "saved" })] });
    expect(leave()).toBe(true);
    off();
    expect(leave()).toBe(false);
    useStore.setState({ docs: [] });
  });
});

describe("save as", () => {
  it("asks for a name in the same folder, and refuses a way out of it", async () => {
    const { save } = await import("./dialog");
    vi.stubGlobal("prompt", vi.fn(() => "копия.md"));
    await expect(save({ defaultPath: "/library/notes/a.md" })).resolves.toBe("/library/notes/копия.md");
    vi.stubGlobal("prompt", vi.fn(() => "../x.md"));
    await expect(save({ defaultPath: "/library/notes/a.md" })).resolves.toBeNull();
    vi.stubGlobal("prompt", vi.fn(() => null));
    await expect(save({ defaultPath: "/library/notes/a.md" })).resolves.toBeNull();
  });
});
