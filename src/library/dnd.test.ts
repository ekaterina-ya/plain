import { beforeEach, expect, it, vi } from "vitest";

const handlers: ((event: { payload: { type: string; paths?: string[] } }) => void)[] = [];

vi.mock("../app/env", () => ({ inTauri: true, inWeb: true }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async (handler: (typeof handlers)[number]) => {
      handlers.push(handler);
      return () => undefined;
    },
  }),
}));
vi.mock("../editor", () => ({ dropImagesInEditor: vi.fn(async () => false) }));
vi.mock("../app/import", () => ({
  droppedFiles: vi.fn((paths: string[]) => paths.map((p) => `file:${p}`)),
  importFiles: vi.fn(async () => undefined),
}));
vi.mock("../app/commands", () => ({ openPaths: vi.fn(), openLibraryPath: vi.fn() }));

import { openPaths } from "../app/commands";
import { droppedFiles, importFiles } from "../app/import";
import { dropImagesInEditor } from "../editor";
import { installDrop } from "./dnd";

const drop = async (paths: string[]) => {
  handlers[0]?.({ payload: { type: "drop", paths } });
  await new Promise((resolve) => setTimeout(resolve, 0));
};

beforeEach(() => {
  vi.clearAllMocks();
  handlers.length = 0;
  installDrop();
});

it("on the web, files from the device are imported, taken before anything waits", async () => {
  await drop(["/device/0/a.md"]);
  expect(droppedFiles).toHaveBeenCalledWith(["/device/0/a.md"]);
  expect(importFiles).toHaveBeenCalledWith(["file:/device/0/a.md"]);
  expect(openPaths).not.toHaveBeenCalled();
});

it("a picture dropped while editing still goes into the document", async () => {
  vi.mocked(dropImagesInEditor).mockResolvedValueOnce(true);
  await drop(["/device/0/p.png"]);
  expect(importFiles).not.toHaveBeenCalled();
});
