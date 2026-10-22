// Plain Web on a phone or a tablet (ТЗ-Plain-Web §5.2–5.4): which layout
// this screen gets. Only the web build asks — the desktop window keeps its
// own layout however narrow it is made.

import { useEffect, useState } from "react";
import { inWeb } from "../app/env";
import "./touch.css";

/** A phone in portrait or landscape. */
export const PHONE = "(max-width: 767px)";
/** The rail slides over the text: a phone, or a touch screen up to an iPad in portrait. */
export const DRAWER = "(max-width: 767px), (pointer: coarse) and (max-width: 900px)";
export const TOUCH = "(pointer: coarse)";

function matches(query: string): boolean {
  return inWeb && typeof matchMedia === "function" && matchMedia(query).matches;
}

export function useMedia(query: string): boolean {
  const [on, setOn] = useState(() => matches(query));
  useEffect(() => {
    if (!inWeb || typeof matchMedia !== "function") return;
    const list = matchMedia(query);
    const change = () => setOn(list.matches);
    change();
    list.addEventListener("change", change);
    return () => list.removeEventListener("change", change);
  }, [query]);
  return on;
}

/**
 * The drawer's own open-or-shut, kept on this device: the rail's state in
 * the store is the session's, and the session is every device's. Opening a
 * document shuts it — that is what it was opened for.
 */
export function useDrawer(activeId: string | null): [boolean, (open: boolean) => void] {
  const [open, setOpen] = useState(false);
  useEffect(() => setOpen(false), [activeId]);
  useEffect(() => {
    if (!open) return;
    const shut = () => setOpen(false);
    // Esc, and a tap on a note — the one already open too, which changes nothing.
    const key = (event: KeyboardEvent) => event.key === "Escape" && shut();
    const pick = (event: MouseEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest(".rail-item, .tree-row:not(.is-folder)")) shut();
    };
    window.addEventListener("keydown", key);
    document.addEventListener("click", pick);
    return () => {
      window.removeEventListener("keydown", key);
      document.removeEventListener("click", pick);
    };
  }, [open]);
  return [open, setOpen];
}

/**
 * On a touch screen the format bar sits on the keyboard. iOS keeps the page
 * the same height and lays the keyboard over it, so its height is what the
 * visual viewport lost — and the layout viewport may scroll under it too.
 */
export function installKeyboard(): () => void {
  const view = typeof window !== "undefined" ? window.visualViewport : null;
  if (!view || !matches(TOUCH)) return () => undefined;
  const root = document.documentElement;
  const place = () => {
    const covered = Math.max(0, window.innerHeight - view.height - view.offsetTop);
    root.style.setProperty("--kb", `${covered}px`);
    // A bar of browser chrome is not a keyboard.
    root.classList.toggle("kb-open", covered > 120);
  };
  place();
  view.addEventListener("resize", place);
  view.addEventListener("scroll", place);
  return () => {
    view.removeEventListener("resize", place);
    view.removeEventListener("scroll", place);
  };
}

/**
 * A swipe left along a tree row opens its menu, as a long press does — which
 * iOS often takes for selecting the name instead. Not the name being edited
 * in place: that row is a div.
 */
export function installSwipe(): () => void {
  if (!matches(TOUCH)) return () => undefined;
  let start: { row: Element; x: number; y: number } | null = null;
  const down = (event: TouchEvent) => {
    const touch = event.touches.length === 1 ? event.touches[0] : undefined;
    const row = touch && event.target instanceof Element ? event.target.closest("button.tree-row") : null;
    start = row && touch ? { row, x: touch.clientX, y: touch.clientY } : null;
  };
  const up = (event: TouchEvent) => {
    const touch = event.changedTouches[0];
    if (!start || !touch) return;
    const { row, x, y } = start;
    start = null;
    const dx = touch.clientX - x;
    if (dx < -40 && Math.abs(touch.clientY - y) < -dx / 2) {
      const at = { clientX: touch.clientX, clientY: touch.clientY };
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, ...at }));
    }
  };
  document.addEventListener("touchstart", down, { passive: true });
  document.addEventListener("touchend", up, { passive: true });
  return () => {
    document.removeEventListener("touchstart", down);
    document.removeEventListener("touchend", up);
  };
}
