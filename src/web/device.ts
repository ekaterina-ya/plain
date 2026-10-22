// Files from this device — dropped on the page (webview.ts) or picked in the
// browser's file dialog (dialog.ts) — under a path of their own until
// `copyFile` uploads them (fs.ts). Only the latest batch is kept.

const files = new Map<string, File>();

export function isDevice(path: string): boolean {
  return path.startsWith("/device/");
}

export function register(list: File[]): string[] {
  files.clear();
  return list.map((file, index) => {
    const path = `/device/${index}/${file.name}`;
    files.set(path, file);
    return path;
  });
}

export function deviceFile(path: string): File | undefined {
  return files.get(path);
}
