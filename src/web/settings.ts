// settings.json is the server's and every device shares it — except the
// font size and the column width, which belong to the screen in front of
// you (ТЗ-Plain-Web §9.2). Those two are kept in this browser too and win
// over the server's whenever settings are read here.

export const SETTINGS_FILE = "/data/settings.json";
const KEY = "plain:appearance";

interface Appearance {
  fontSize?: unknown;
  contentWidth?: unknown;
}

function stored(): Appearance | null {
  try {
    const text = localStorage.getItem(KEY);
    return text ? (JSON.parse(text) as Appearance) : null;
  } catch {
    return null;
  }
}

function store(appearance: Appearance): void {
  try {
    const { fontSize, contentWidth } = appearance;
    localStorage.setItem(KEY, JSON.stringify({ fontSize, contentWidth }));
  } catch {
    /* a private window: the server's values it is */
  }
}

function appearanceOf(settings: unknown): Appearance | null {
  const appearance = (settings as { appearance?: unknown } | null)?.appearance;
  return appearance && typeof appearance === "object" ? (appearance as Appearance) : null;
}

/** Settings as this device sees them. The first time, the server's two values become its own. */
export function withLocalAppearance(settings: unknown): unknown {
  const theirs = appearanceOf(settings);
  if (!theirs) return settings;
  const mine = stored();
  if (!mine) {
    store(theirs);
    return settings;
  }
  return { ...(settings as object), appearance: { ...theirs, ...mine } };
}

/** The file as read from the server, as this device sees it. */
export function readLocally(text: string): string {
  try {
    return JSON.stringify(withLocalAppearance(JSON.parse(text)), null, 2);
  } catch {
    return text;
  }
}

/** What is being written: its two values are this device's from now on. */
export function keepLocally(text: string): void {
  try {
    const appearance = appearanceOf(JSON.parse(text));
    if (appearance) store(appearance);
  } catch {
    /* not settings we could read; the server gets them as they are */
  }
}
