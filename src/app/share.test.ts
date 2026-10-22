// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../read/export", () => ({ exportHtml: vi.fn(async () => "<!doctype html><p>x</p>") }));
vi.mock("./fs", async (actual) => ({
  ...(await actual<typeof import("./fs")>()),
  readFile: vi.fn(async () => ({ text: "# Off disk\n\nbody" })),
}));
vi.mock("./import", () => ({ toDevice: vi.fn() }));

import { toDevice } from "./import";
import { prepare, shareNote } from "./share";
import { makeDoc, useStore } from "./store";

const shared: unknown[] = [];

function withSheet(accepts: (file: File) => boolean) {
  vi.stubGlobal("navigator", {
    ...navigator,
    share: vi.fn(async (data: unknown) => void shared.push(data)),
    canShare: (data: { files?: File[] }) => (data.files ?? []).every(accepts),
  });
}

beforeEach(() => {
  shared.length = 0;
  vi.mocked(toDevice).mockClear();
  const doc = makeDoc({ id: "/library/план.md", path: "/library/план.md", text: "# План\n\nтекст" });
  useStore.setState({ docs: [doc], activeId: doc.id, dialog: null, message: null });
});

afterEach(() => vi.unstubAllGlobals());

async function choose(label: string) {
  await shareNote("/library/план.md");
  const action = useStore.getState().dialog?.actions.find((a) => a.label === label);
  expect(action, label).toBeDefined();
  action?.run();
}

describe("sharing a note", () => {
  it("cuts the mail body on a whole character", async () => {
    vi.stubGlobal("navigator", { ...navigator, share: undefined, canShare: undefined });
    const doc = makeDoc({ id: "/library/план.md", path: "/library/план.md", text: "😀".repeat(2000) });
    useStore.setState({ docs: [doc], activeId: doc.id });
    const opened: string[] = [];
    vi.stubGlobal("open", vi.fn((url: string) => void opened.push(url)));
    await choose("telegram");
    expect(opened[0]).toMatch(/^https:\/\/t\.me\/share\/url\?url=/);
    expect(decodeURIComponent(opened[0]!.split("url=")[1]!)).toBe("😀".repeat(1500));
  });

  it("saves the file when the sheet refuses it", async () => {
    vi.stubGlobal("navigator", {
      ...navigator,
      canShare: () => true,
      share: vi.fn(async () => Promise.reject(Object.assign(new Error("no"), { name: "NotAllowedError" }))),
    });
    await choose("text");
    await vi.waitFor(() => expect(toDevice).toHaveBeenCalledWith("план.txt", expect.any(File), "text/plain;charset=utf-8"));
  });

  it("makes the files from the open note, or from the disk", async () => {
    const open = await prepare("/library/план.md");
    expect(open.md.name).toBe("план.md");
    expect(await open.md.text()).toBe("# План\n\nтекст");
    expect(open.html.type).toBe("text/html");
    expect(open.plain).toContain("текст");
    const other = await prepare("/library/other.md");
    expect(await other.md.text()).toBe("# Off disk\n\nbody");
  });

  it("hands the sheet the file alone, at once", async () => {
    withSheet(() => true);
    await choose("markdown");
    expect(shared).toEqual([{ files: [expect.objectContaining({ name: "план.md" })] }]);
    expect(useStore.getState().dialog).toBeNull();
  });

  it("gives a sheet that will not take .md a .txt", async () => {
    withSheet((file) => !file.name.endsWith(".md"));
    await choose("markdown");
    expect(shared).toEqual([{ files: [expect.objectContaining({ name: "план.md.txt" })] }]);
  });

  it("shares the text as a message", async () => {
    withSheet(() => true);
    await choose("as message");
    expect(shared[0]).toEqual({ text: expect.stringContaining("текст") });
  });

  it("without a sheet, saves the file and offers mail, Telegram and a copy", async () => {
    vi.stubGlobal("navigator", { ...navigator, share: undefined, canShare: undefined });
    await choose("html");
    expect(toDevice).toHaveBeenCalledWith("план.html", expect.any(File), "text/html;charset=utf-8");
    await shareNote("/library/план.md");
    const labels = useStore.getState().dialog?.actions.map((a) => a.label);
    expect(labels).toEqual(["markdown", "html", "text", "pdf", "mail", "telegram", "copy text", "cancel"]);
  });
});
