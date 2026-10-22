// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it } from "vitest";
import { useDrawer, useMedia } from "./media";
import { useStore } from "../app/store";

it("the drawer shuts when a document opens, and leaves the session's rail alone", () => {
  let api: [boolean, (open: boolean) => void] = [false, () => undefined];
  function Probe({ id }: { id: string | null }) {
    api = useDrawer(id);
    return null;
  }
  const host = document.createElement("div");
  const root = createRoot(host);
  act(() => root.render(<Probe id="a" />));
  const collapsed = useStore.getState().railCollapsed;
  act(() => api[1](true));
  expect(api[0]).toBe(true);
  act(() => root.render(<Probe id="b" />));
  expect(api[0]).toBe(false);
  expect(useStore.getState().railCollapsed).toBe(collapsed);
  act(() => root.unmount());
});

it("is never a phone outside the web build", () => {
  let phone = true;
  function Probe() {
    phone = useMedia("(max-width: 767px)");
    return null;
  }
  const root = createRoot(document.createElement("div"));
  act(() => root.render(<Probe />));
  expect(phone).toBe(false);
  act(() => root.unmount());
});

it("the drawer shuts on Esc and on a tap on a note, the open one too", () => {
  let api: [boolean, (open: boolean) => void] = [false, () => undefined];
  function Probe() {
    api = useDrawer("a");
    return null;
  }
  const root = createRoot(document.createElement("div"));
  act(() => root.render(<Probe />));
  act(() => api[1](true));
  act(() => void window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
  expect(api[0]).toBe(false);
  act(() => api[1](true));
  const row = document.createElement("button");
  row.className = "rail-item";
  document.body.append(row);
  act(() => row.click());
  expect(api[0]).toBe(false);
  row.remove();
  act(() => root.unmount());
});
