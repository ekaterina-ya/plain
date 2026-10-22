// `@tauri-apps/api/webview`.

import { register } from "./device";
import type { UnlistenFn } from "./event";

interface DragDrop {
  payload: { type: string; paths?: string[] };
}

const carriesFiles = (event: DragEvent) => event.dataTransfer?.types.includes("Files") === true;

const current = {
  async setZoom(factor: number) {
    document.documentElement.style.zoom = String(factor);
  },
  /**
   * Files dropped from the device, the way Tauri hands them over: as paths,
   * here ones that stand for the files until they are uploaded (device.ts).
   * Caught before CodeMirror, which would paste a file's bytes as text, and
   * before the browser, which would open it in place of the app. Text and
   * the tree's own rows are dragged as before.
   */
  async onDragDropEvent(handler: (event: DragDrop) => void): Promise<UnlistenFn> {
    const over = (event: DragEvent) => {
      if (carriesFiles(event)) event.preventDefault();
    };
    const drop = (event: DragEvent) => {
      if (!carriesFiles(event)) return;
      event.preventDefault();
      event.stopPropagation();
      handler({ payload: { type: "drop", paths: register([...(event.dataTransfer?.files ?? [])]) } });
    };
    window.addEventListener("dragover", over, true);
    window.addEventListener("drop", drop, true);
    return () => {
      window.removeEventListener("dragover", over, true);
      window.removeEventListener("drop", drop, true);
    };
  },
};

export function getCurrentWebview(): typeof current {
  return current;
}
