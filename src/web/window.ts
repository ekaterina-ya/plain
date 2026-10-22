// `@tauri-apps/api/window`: a browser tab is the window.

import type { UnlistenFn } from "./event";

const current = {
  label: "main",
  async setTitle(title: string) {
    document.title = title;
  },
  async isFullscreen() {
    return document.fullscreenElement !== null;
  },
  async setFullscreen(on: boolean) {
    if (on) await document.documentElement.requestFullscreen();
    else if (document.fullscreenElement) await document.exitFullscreen();
  },
  async setAlwaysOnTop(_on: boolean) {},
  async close() {},
  async destroy() {},
  /**
   * The browser asks its own question on the way out, and only it can: the
   * app's unsaved-changes dialog cannot hold a tab open. So the handler is
   * not called; the browser is told whether to ask. Drafts and settings are
   * written on the way out and whenever the tab is hidden — a phone drops a
   * hidden tab without a word.
   */
  async onCloseRequested(_handler: (event: { preventDefault(): void }) => void): Promise<UnlistenFn> {
    const [{ useStore }, { flushActiveEditor, isDirty }, { flushDrafts }, { flushSettings }] =
      await Promise.all([
        import("../app/store"),
        import("../editor"),
        import("../app/drafts"),
        import("../app/settings"),
      ]);
    const flush = () => {
      flushActiveEditor();
      flushDrafts();
      void flushSettings();
    };
    const guard = (event: BeforeUnloadEvent) => {
      flush();
      if (useStore.getState().docs.some((doc) => isDirty(doc.id, doc.text))) {
        event.preventDefault();
        event.returnValue = "";
      }
    };
    const hidden = () => {
      if (document.visibilityState === "hidden") flush();
    };
    window.addEventListener("beforeunload", guard);
    document.addEventListener("visibilitychange", hidden);
    return () => {
      window.removeEventListener("beforeunload", guard);
      document.removeEventListener("visibilitychange", hidden);
    };
  },
};

export function getCurrentWindow(): typeof current {
  return current;
}
