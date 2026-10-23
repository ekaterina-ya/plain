import { expect, it, vi } from "vitest";

// Plain Web: the commands that only make sense here, and the one that does not.
vi.mock("./env", () => ({ inTauri: true, inWeb: true }));

import { availableCommands } from "./registry";
import { makeDoc, useStore } from "./store";

it("offers the web's own commands and not reveal in explorer", () => {
  const doc = makeDoc({ id: "/library/a.md", path: "/library/a.md", text: "x" });
  useStore.setState({ docs: [doc], activeId: doc.id, treeSelected: "/library/a.md" });
  const ids = availableCommands().map((command) => command.id);
  expect(ids).toEqual(expect.arrayContaining(["file.import", "file.devices", "file.signOut"]));
  expect(ids).toContain("file.saveAs");
  expect(ids).toEqual(expect.arrayContaining(["file.download", "file.share", "file.exportHtml", "file.exportText"]));
  for (const desktop of ["file.reveal", "file.exit", "file.newWindow", "file.closeWindow", "view.alwaysOnTop"]) {
    expect(ids).not.toContain(desktop);
  }
});

it("leaves no desktop-only entry or dangling separator in the menu", async () => {
  const { menuModel } = await import("./menu");
  const keys: string[] = [];
  const walk = (items: { kind: string; key: string; items?: unknown[] }[]) => {
    items.forEach((item, index) => {
      keys.push(item.key);
      if (item.kind === "separator") {
        expect(index).toBeGreaterThan(0);
        expect(index).toBeLessThan(items.length - 1);
        expect(items[index - 1]?.kind).not.toBe("separator");
      }
      if (item.items) walk(item.items as never);
    });
  };
  for (const section of menuModel()) walk(section.items as never);
  expect(keys).toEqual(
    expect.arrayContaining(["file.import", "file.download", "file.share", "file.exportHtml", "file.exportPdf", "nav.quickSearch", "nav.searchInFolder", "nav.palette"]),
  );
  expect(keys.filter((key) => ["file.exit", "view.alwaysOnTop", "file.reveal"].includes(key))).toEqual([]);
});
