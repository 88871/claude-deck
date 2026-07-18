import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import "@xterm/xterm/css/xterm.css";

interface Pane { term: Terminal; fit: FitAddon; el: HTMLElement; }
const panes = new Map<string, Pane>();
const glyphs: Record<string, string> = {
  starting: "○", running: "⏳", waitingOnYou: "◍", idle: "✓", parked: "◌",
  closed: "⏹", error: "✗", // closed = clean exit (/exit, Ctrl-D); error = abnormal
};
let activeId: string | null = null;

// base64 PTY bytes → Uint8Array; xterm handles UTF-8 across chunks (Review fix #1).
function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

export async function openSession(cwd: string): Promise<string> {
  const id = await invoke<string>("start_session", { cwd });
  const el = document.createElement("div");
  el.className = "term-pane hidden";
  document.getElementById("terminals")!.appendChild(el);

  const term = new Terminal({ fontFamily: "Menlo, monospace", fontSize: 13, cursorBlink: true, theme: { background: "#0b0b0e" } });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(el);
  term.onData((data) => invoke("write_to_pty", { id, data }));
  panes.set(id, { term, fit, el });
  focusSession(id);
  return id;
}

export function writeToSession(id: string, b64: string) {
  panes.get(id)?.term.write(b64ToBytes(b64));
}

export function focusSession(id: string) {
  activeId = id;
  for (const [pid, p] of panes) p.el.classList.toggle("hidden", pid !== id);
  const p = panes.get(id);
  if (p) {
    p.fit.fit();
    invoke("resize_pty", { id, cols: p.term.cols, rows: p.term.rows });
    p.term.focus();
  }
  for (const row of document.querySelectorAll(".session-row"))
    row.classList.toggle("active", (row as HTMLElement).dataset.id === id);
}

// Refit the focused pane and push the new size to its PTY. Bound to window
// resize so terminals reflow (Review fix #4 — Task 3's resize handler was
// dropped when main.ts was replaced).
export function fitActive() {
  if (!activeId) return;
  const p = panes.get(activeId);
  if (!p) return;
  p.fit.fit();
  invoke("resize_pty", { id: activeId, cols: p.term.cols, rows: p.term.rows });
}

export function renderSidebar(sessions: { id: string; label: string; state: string }[]) {
  const list = document.getElementById("session-list")!;
  list.innerHTML = "";
  for (const s of sessions) {
    const li = document.createElement("li");
    li.className = "session-row" + (s.id === activeId ? " active" : "");
    li.dataset.id = s.id;
    // Build with textContent — directory-derived labels may contain < & etc.
    // (Review fix #5; never innerHTML untrusted text).
    const glyph = document.createElement("span");
    glyph.className = "glyph";
    glyph.textContent = glyphs[s.state] ?? "○";
    const label = document.createElement("span");
    label.textContent = s.label;
    li.append(glyph, label);
    li.addEventListener("click", () => focusSession(s.id));
    list.appendChild(li);
  }
}

export function setSidebarState(id: string, state: string) {
  const row = document.querySelector<HTMLElement>(`.session-row[data-id="${id}"] .glyph`);
  if (row) row.textContent = glyphs[state] ?? "○";
}

export function activeSession() { return activeId; }
