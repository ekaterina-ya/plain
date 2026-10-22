// @vitest-environment happy-dom
//
// Split's editor is rich or markdown, picked at the end of the bar.

import { afterEach, beforeEach, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useStore } from "../app/store";
import { FormatBar } from "./FormatBar";

let host: HTMLDivElement;
let root: Root;
const initial = useStore.getState().settings;

function render(split: boolean) {
  act(() => root.render(<FormatBar split={split} />));
}

function toggle(): HTMLButtonElement[] {
  return Array.from(host.querySelectorAll(".fb-split button"));
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  useStore.setState({ splitRich: true, settings: initial });
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

it("is not there outside split", () => {
  render(false);
  expect(toggle()).toHaveLength(0);
});

it("shows rich as chosen and switches to md", () => {
  render(true);
  const [rich, md] = toggle();
  expect(rich?.textContent).toBe("rich");
  expect(rich?.classList.contains("is-active")).toBe(true);
  act(() => md?.click());
  expect(useStore.getState().splitRich).toBe(false);
  expect(md?.classList.contains("is-active")).toBe(true);
});

it("goes with the bar when the bar is off", () => {
  useStore.setState({ settings: { ...initial, edit: { ...initial.edit, toolbar: false } } });
  render(true);
  expect(toggle()).toHaveLength(0);
});
