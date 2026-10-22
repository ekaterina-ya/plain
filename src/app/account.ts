// Plain Web's sign-in (server/src/auth.rs): the devices page and signing out
// of this one. The desktop has neither.

/** In a tab of its own: this one keeps its documents as they are. */
export function openDevices(): void {
  window.open("/devices", "_blank");
}

/** Unsaved work gets the browser's own question on the way out. */
export async function signOut(): Promise<void> {
  await fetch("/logout", { method: "POST", headers: { "X-Plain": "1" } }).catch(() => undefined);
  location.assign("/login");
}
