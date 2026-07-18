import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./styles.css";
import { openSession, writeToSession, setSidebarState, renderSidebar, fitActive } from "./sessions";

async function refreshSidebar() {
  const sessions = await invoke<{ id: string; label: string; state: string }[]>("list_sessions");
  renderSidebar(sessions);
}

listen<{ id: string; b64: string }>("pty://data", (e) => writeToSession(e.payload.id, e.payload.b64));
listen<{ id: string; state: string }>("session://state", (e) => setSidebarState(e.payload.id, e.payload.state));

// Reflow the focused terminal on window resize (Review fix #4).
window.addEventListener("resize", fitActive);

document.getElementById("new-session")!.addEventListener("click", async () => {
  const dir = await open({ directory: true, multiple: false });
  if (typeof dir !== "string") return;
  try {
    await openSession(dir);
    await refreshSidebar();
  } catch (err) {
    // Surfaces the §9 onboarding error (e.g. claude not installed / not logged in).
    // Foundation stopgap; Plan 2 replaces it with an inline onboarding panel.
    alert(String(err));
  }
});
