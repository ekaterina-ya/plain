// `@tauri-apps/api/event`: within this page, plus what link.ts brings in
// from the server and the other tabs, and sends out for them.

export type UnlistenFn = () => void;

export interface Event<T> {
  event: string;
  payload: T;
}

const handlers = new Map<string, Set<(event: Event<unknown>) => void>>();
let forward: ((event: string, payload: unknown) => void) | null = null;

export async function listen<T>(event: string, handler: (event: Event<T>) => void): Promise<UnlistenFn> {
  const set = handlers.get(event) ?? new Set();
  handlers.set(event, set);
  const wrapped = handler as (event: Event<unknown>) => void;
  set.add(wrapped);
  return () => void set.delete(wrapped);
}

/** To this page's listeners only. */
export function deliver(event: string, payload: unknown): void {
  for (const handler of [...(handlers.get(event) ?? [])]) handler({ event, payload });
}

export function forwardEmits(to: (event: string, payload: unknown) => void): void {
  forward = to;
}

export async function emit(event: string, payload?: unknown): Promise<void> {
  deliver(event, payload);
  forward?.(event, payload);
}
