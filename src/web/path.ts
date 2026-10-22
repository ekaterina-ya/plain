// `@tauri-apps/api/path`: the server's virtual roots are POSIX paths.

export async function appDataDir(): Promise<string> {
  return "/data";
}

export async function join(...parts: string[]): Promise<string> {
  return parts.join("/").replace(/\/+/g, "/");
}
