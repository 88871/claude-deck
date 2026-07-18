import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";

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
window.addEventListener("resize", () => fit.fit());
term.writeln("claude-deck: terminal ready");
