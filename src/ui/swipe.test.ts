// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from "vitest";

vi.mock("../app/env", async (original) => ({ ...(await original<object>()), inWeb: true }));

const { installSwipe } = await import("./media");

function touch(target: Element, type: string, x: number, y: number, fingers = 1) {
  const event = new Event(type, { bubbles: true });
  const point = { clientX: x, clientY: y };
  Object.assign(event, { touches: type === "touchend" ? [] : Array(fingers).fill(point), changedTouches: [point] });
  target.dispatchEvent(event);
}

function setup(coarse: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({ matches: coarse && query === "(pointer: coarse)" }));
  document.body.innerHTML = `<button class="tree-row"><span class="tree-name">a.md</span></button>`;
  const row = document.querySelector(".tree-row")!;
  const menus: MouseEvent[] = [];
  row.addEventListener("contextmenu", (event) => menus.push(event as MouseEvent));
  return { row, name: row.firstElementChild!, menus, stop: installSwipe() };
}

afterEach(() => vi.unstubAllGlobals());

it("a swipe left along a row opens its menu where the finger lifts", () => {
  const { name, menus, stop } = setup(true);
  touch(name, "touchstart", 200, 100);
  touch(name, "touchend", 120, 110);
  expect(menus).toHaveLength(1);
  expect([menus[0]!.clientX, menus[0]!.clientY]).toEqual([120, 110]);
  stop();
});

it("a tap, a swipe right, a scroll, two fingers and a mouse are not a swipe", () => {
  const { name, menus, stop } = setup(true);
  for (const [x, y, fingers] of [[200, 100, 1], [260, 100, 1], [180, 160, 1], [120, 100, 2]] as const) {
    touch(name, "touchstart", 200, 100, fingers);
    touch(name, "touchend", x, y);
  }
  expect(menus).toHaveLength(0);
  stop();
  const mouse = setup(false);
  touch(mouse.name, "touchstart", 200, 100);
  touch(mouse.name, "touchend", 120, 100);
  expect(mouse.menus).toHaveLength(0);
});

it("a swipe across a name being edited moves the caret, nothing else", () => {
  const { menus, stop } = setup(true);
  document.body.innerHTML = `<div class="tree-row"><input value="a.md" /></div>`;
  const field = document.querySelector("input")!;
  const menu: Event[] = [];
  document.body.addEventListener("contextmenu", (event) => menu.push(event));
  touch(field, "touchstart", 200, 100);
  touch(field, "touchend", 120, 100);
  expect([menus, menu]).toEqual([[], []]);
  stop();
});
