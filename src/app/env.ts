// The UI also has to render in a plain browser during `vite dev`, so every
// Tauri call goes behind this flag. Plain Web takes the Tauri path too: its
// build swaps the Tauri modules for the server's (src/web).
export const inWeb = import.meta.env.MODE === "web";
export const inTauri =
  inWeb ||
  (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window);
