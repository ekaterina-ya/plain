// `@tauri-apps/plugin-opener`.

export async function openUrl(url: string): Promise<void> {
  window.open(url, "_blank", "noopener");
}

// The server's folder is not this device's to show.
export async function openPath(_path: string): Promise<void> {}
export async function revealItemInDir(_path: string): Promise<void> {}
