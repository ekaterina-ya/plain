// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";

const written: string[] = [];
let taken = new Set<string>();

vi.mock("@tauri-apps/plugin-fs", () => ({
  writeFile: vi.fn(async (path: string) => {
    if (taken.has(path)) throw { kind: "exists", message: `${path} exists` };
    written.push(path);
  }),
}));
vi.mock("./commands", () => ({ openPaths: vi.fn(async () => true) }));
vi.mock("../library/tree", async (actual) => ({
  ...(await actual<typeof import("../library/tree")>()),
  reloadTree: vi.fn(async () => undefined),
}));
vi.mock("../library/ops", () => ({
  siblings: () => useStore.getState().tree.map((node) => node.name),
}));

import { openPaths } from "./commands";
import { droppedFiles, importFiles } from "./import";
import { useStore, type TreeNode } from "./store";
import { register } from "../web/device";

const node = (path: string, dir = false, children: TreeNode[] = []): TreeNode =>
  ({ name: path.split("/").pop(), path, rel: path, dir, unreadable: false, mtimeMs: 0, ctimeMs: 0, children }) as TreeNode;

const file = (name: string, text = "# x") => new File([text], name);

beforeEach(() => {
  written.length = 0;
  taken = new Set();
  vi.mocked(openPaths).mockClear();
  const settings = useStore.getState().settings;
  useStore.setState({
    libraryPath: "/library",
    tree: [node("/library/Заметка.md"), node("/library/notes", true, [node("/library/notes/a.md")])],
    treeSelected: null,
    settings: { ...settings, library: { ...settings.library, extensions: [".md", "markdown"] } },
  });
});

describe("importing from the device", () => {
  it("copies markdown to the root and opens it", async () => {
    await importFiles([file("plan.md")]);
    expect(written).toEqual(["/library/plan.md"]);
    expect(openPaths).toHaveBeenCalledWith(["/library/plan.md"]);
    expect(useStore.getState().message).toBe("imported 1");
  });

  it("lands in the folder picked in the tree, or around the file picked", async () => {
    useStore.setState({ treeSelected: "/library/notes" });
    await importFiles([file("b.md")]);
    useStore.setState({ treeSelected: "/library/notes/a.md" });
    await importFiles([file("c.MD")]);
    expect(written).toEqual(["/library/notes/b.md", "/library/notes/c.MD"]);
  });

  it("never takes a name that is there, whatever its case", async () => {
    await importFiles([file("заметка.md")]);
    expect(written).toEqual(["/library/заметка 2.md"]);
    // Taken on the server since the tree was read: the next name.
    taken.add("/library/new.md");
    await importFiles([file("new.md")]);
    expect(written).toContain("/library/new 2.md");
  });

  it("says what failed and goes on with the rest", async () => {
    taken.add("/library/bad.md");
    const { writeFile } = await import("@tauri-apps/plugin-fs");
    vi.mocked(writeFile).mockRejectedValueOnce({ kind: "io", message: "disk full" });
    await importFiles([file("bad.md"), file("good.md")]);
    expect(written).toEqual(["/library/good.md"]);
    expect(useStore.getState().message).toBe("imported 1 · couldn't import bad.md — disk full");
  });

  it("opens only the first of many, and says so", async () => {
    await importFiles(["a", "b", "c", "d", "e", "f"].map((n) => file(`${n}.md`)));
    expect(openPaths).toHaveBeenCalledWith(["/library/a.md"]);
    expect(useStore.getState().message).toBe("imported 6 · opened the first");
  });

  it("lands in the folder the tree's menu was opened on", async () => {
    useStore.setState({ treeSelected: "/library/Заметка.md" });
    await importFiles([file("x.md")], "/library/notes");
    expect(written).toEqual(["/library/notes/x.md"]);
  });

  it("composes names macOS hands over decomposed", async () => {
    await importFiles([file("йдея.md")]);
    expect(written).toEqual(["/library/йдея.md"]);
  });

  it("skips what the library would not show, and says so", async () => {
    await importFiles([file("scan.pdf"), file(".hidden.md"), file("ok.markdown")]);
    expect(written).toEqual(["/library/ok.markdown"]);
    expect(useStore.getState().message).toBe(
      "imported 1 · skipped scan.pdf · skipped .hidden.md · only .md, .markdown",
    );
  });

  it("takes dropped files as they were when dropped", async () => {
    const paths = register([file("one.md"), file("two.md")]);
    const importing = importFiles(droppedFiles(paths));
    // Another drop at once: this batch already holds its files.
    register([file("other.md")]);
    await importing;
    expect(written).toEqual(["/library/one.md", "/library/two.md"]);
  });
});

describe("downloading a note", () => {
  it("hands the device the note's own text, under its own name", async () => {
    const { downloadNote } = await import("./import");
    const { makeDoc } = await import("./store");
    useStore.setState({ docs: [makeDoc({ id: "/library/план.md", path: "/library/план.md", text: "# план" })] });
    const made: Blob[] = [];
    vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => (made.push(blob as Blob), "blob:x"));
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const clicked: HTMLAnchorElement[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      clicked.push(this);
    });
    await downloadNote("/library/план.md");
    vi.restoreAllMocks();
    expect(clicked[0]?.download).toBe("план.md");
    expect(made[0]?.type).toBe("text/markdown;charset=utf-8");
    expect(await made[0]?.text()).toBe("# план");
  });
});
