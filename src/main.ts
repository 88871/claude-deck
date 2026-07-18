import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { homeDir } from "@tauri-apps/api/path";

const term = new Terminal({
  fontFamily: "Menlo, monospace",
  fontSize: 13,
  cursorBlink: true,
  theme: { background: "#0b0b0e" },
});
const fit = new FitAddon();
term.loadAddon(fit);
term.open(document.getElementById("terminal")!);
fit.fit();

let sessionId: string | null = null;

// Decode base64 PTY bytes → Uint8Array; xterm.write handles UTF-8 across
// chunk boundaries, so we never build a lossy string (Review fix #1).
function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

listen<{ id: string; b64: string }>("pty://data", (e) => {
  if (e.payload.id === sessionId) term.write(b64ToBytes(e.payload.b64));
});

term.onData((data) => {
  if (sessionId) invoke("write_to_pty", { id: sessionId, data });
});

function syncSize() {
  fit.fit();
  if (sessionId) invoke("resize_pty", { id: sessionId, cols: term.cols, rows: term.rows });
}
window.addEventListener("resize", syncSize);

// Temporary: start one session in the home directory on load (replaced in Task 4).
(async () => {
  sessionId = await invoke<string>("start_session", { cwd: await homeDir() });
  syncSize();
})();
