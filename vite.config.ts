import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The about dialog asks Tauri for the version at runtime; in a browser there
// is no Tauri to ask, so the number comes from package.json at build time.
const { version } = JSON.parse(readFileSync("./package.json", "utf-8")) as { version: string };

// Plain Web (`--mode web`): every Tauri module is swapped for its stand-in
// in src/web, and /api goes to the server (server/).
const WEB_MODULES: Record<string, string> = {
  "@tauri-apps/api/core": "core",
  "@tauri-apps/api/event": "event",
  "@tauri-apps/api/window": "window",
  "@tauri-apps/api/webview": "webview",
  "@tauri-apps/api/path": "path",
  "@tauri-apps/api/app": "app",
  "@tauri-apps/plugin-fs": "fs",
  "@tauri-apps/plugin-dialog": "dialog",
  "@tauri-apps/plugin-opener": "opener",
  "@tauri-apps/plugin-clipboard-manager": "clipboard",
};
const webAlias = Object.entries(WEB_MODULES).map(([module, file]) => ({
  find: new RegExp(`^${module}$`),
  replacement: fileURLToPath(new URL(`./src/web/${file}.ts`, import.meta.url)),
}));

// Tauri drives this dev server; the port is fixed and must not wander.
// Plain Web on a phone's home screen (ТЗ-Plain-Web §5.5): the manifest and
// the icons, and the page told how to sit under the notch and not to zoom in
// on a field it focuses.
const WEB_HEAD = [
  '<link rel="manifest" href="/manifest.webmanifest" />',
  '<link rel="apple-touch-icon" href="/apple-touch-icon.png" />',
  // iOS names the icon after the tab's title, which is the open note's.
  '<meta name="apple-mobile-web-app-title" content="Plain" />',
  '<meta name="theme-color" content="#f7f6f3" media="(prefers-color-scheme: light)" />',
  '<meta name="theme-color" content="#141413" media="(prefers-color-scheme: dark)" />',
].join("\n    ");

const webPage = {
  name: "plain-web-head",
  transformIndexHtml: (html: string) =>
    html
      .replace(
        'content="width=device-width, initial-scale=1.0"',
        'content="width=device-width, initial-scale=1.0, maximum-scale=1, viewport-fit=cover"',
      )
      .replace("</head>", `  ${WEB_HEAD}\n  </head>`),
};

export default defineConfig(({ mode }) => ({
  plugins: mode === "web" ? [react(), webPage] : [react()],
  publicDir: mode === "web" ? "src/web/public" : "public",
  resolve: { alias: mode === "web" ? webAlias : [] },
  clearScreen: false,
  define: { __APP_VERSION__: JSON.stringify(version) },
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/server/**"] },
    proxy:
      mode === "web"
        ? {
            "/api": { target: "http://127.0.0.1:8642", ws: true },
            // Signing in and the devices page are the server's own (auth.rs).
            "^/(login|devices|logout|logout-all)(\\?|$)": "http://127.0.0.1:8642",
          }
        : undefined,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // WebView2 on Windows 11 is evergreen Chromium.
    target: "chrome120",
    sourcemap: false,
  },
  test: {
    // The suite must answer the same on every host: the setup file pins the
    // platform to Windows, and the macOS cases opt in (spec §13a).
    setupFiles: ["./vitest.setup.ts"],
  },
}));
