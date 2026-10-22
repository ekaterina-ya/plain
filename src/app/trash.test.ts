// Plain Web's recycle bin: what the server kept, put back from a dialog.

import { beforeEach, expect, it, vi } from "vitest";

const invoke = vi.fn();
const reloadTree = vi.fn(async () => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke: (c: string, a?: unknown) => invoke(c, a) }));
vi.mock("../library/tree", () => ({ reloadTree: () => reloadTree() }));

const { ago, openTrash } = await import("./trash");
const { useStore } = await import("./store");

const HOUR = 3_600_000;

beforeEach(() => {
  invoke.mockReset();
  reloadTree.mockClear();
  useStore.setState({ dialog: null, message: null });
});

it("says when, roughly", () => {
  const now = 100 * 24 * HOUR;
  expect(ago(now - 5 * 60_000, now)).toBe("5 min ago");
  expect(ago(now - 3 * HOUR, now)).toBe("3 h ago");
  expect(ago(now - 50 * HOUR, now)).toBe("2 d ago");
});

it("lists the newest ten and puts the chosen one back", async () => {
  const deleted = Array.from({ length: 12 }, (_, i) => ({
    id: `id${i}`,
    path: `/library/notes/n${i}.md`,
    deletedAt: Date.now() - i * HOUR,
  }));
  invoke.mockImplementation(async (command: string) => (command === "trash_list" ? deleted : "/library/notes/n1.md"));
  await openTrash();
  const dialog = useStore.getState().dialog;
  expect(dialog?.actions).toHaveLength(11);
  expect(dialog?.actions[1]?.label).toBe("notes/n1.md · 1 h ago");
  expect(dialog?.actions[10]?.label).toBe("cancel");

  dialog?.actions[1]?.run();
  await vi.waitFor(() => expect(useStore.getState().message).toBe("restored"));
  expect(invoke).toHaveBeenCalledWith("trash_restore", { id: "id1" });
  expect(reloadTree).toHaveBeenCalled();
  expect(useStore.getState().dialog).toBeNull();
});

it("says so when there is nothing, or when the place is taken", async () => {
  invoke.mockResolvedValueOnce([]);
  await openTrash();
  expect(useStore.getState().dialog).toBeNull();
  expect(useStore.getState().message).toBe("the recycle bin is empty");

  invoke
    .mockResolvedValueOnce([{ id: "a", path: "/library/a.md", deletedAt: Date.now() }])
    .mockRejectedValueOnce("/library/a.md already exists");
  await openTrash();
  useStore.getState().dialog?.actions[0]?.run();
  await vi.waitFor(() =>
    expect(useStore.getState().message).toBe("couldn't restore — /library/a.md already exists"),
  );
  expect(reloadTree).not.toHaveBeenCalled();
});
