import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Palette = {
  bg: string;
  surface: string;
  text: string;
  muted: string;
  accent: string;
  selection: string;
  red: string;
  on_accent: string;
  is_dark: boolean;
};

export type Status = {
  signed_in: boolean;
  email: string | null;
  client_configured: boolean;
  error: string | null;
};

export type InboxItem = {
  id: string;
  thread_id: string;
  from: string;
  subject: string;
  date_ms: number | null;
  snippet: string;
  unread: boolean;
};

export type MessageView = InboxItem & {
  plain_text: string;
  html: string | null;
};

export const FALLBACK: Palette = {
  bg: "#1a1b26",
  surface: "#24283b",
  text: "#c0caf5",
  muted: "#9aa5ce",
  accent: "#7aa2f7",
  selection: "#283457",
  red: "#f7768e",
  on_accent: "#1a1b26",
  is_dark: true,
};

export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function applyPalette(palette: Palette) {
  const root = document.documentElement;
  root.style.setProperty("--bg", palette.bg);
  root.style.setProperty("--surface", palette.surface);
  root.style.setProperty("--text", palette.text);
  root.style.setProperty("--muted", palette.muted);
  root.style.setProperty("--accent", palette.accent);
  root.style.setProperty("--selection", palette.selection);
  root.style.setProperty("--red", palette.red);
  root.style.setProperty("--on-accent", palette.on_accent);
  root.dataset.theme = palette.is_dark ? "dark" : "light";
}

export async function loadTheme(): Promise<Palette> {
  if (!inTauri()) return FALLBACK;
  return invoke<Palette>("theme");
}

export function onTheme(handler: (palette: Palette) => void): Promise<UnlistenFn> {
  return listen<Palette>("theme-changed", (event) => handler(event.payload));
}

export const api = {
  status: () => invoke<Status>("status"),
  login: () => invoke<{ email: string }>("login"),
  logout: () => invoke<void>("logout"),
  sync: () => invoke<number>("sync"),
  inbox: () => invoke<InboxItem[]>("inbox"),
  message: (id: string) => invoke<MessageView>("message", { id }),
};
