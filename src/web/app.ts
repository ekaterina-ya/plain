// `@tauri-apps/api/app`.

export async function getVersion(): Promise<string> {
  return __APP_VERSION__;
}
