import { closeMainWindow, showHUD } from "@raycast/api";
import { execFile } from "child_process";
import { promisify } from "util";
import { prefs } from "./grove";

const execFileAsync = promisify(execFile);

// Bring the configured terminal to the foreground (no new window/shell).
export async function focusTerminal() {
  const { terminalApp } = prefs();
  await execFileAsync("osascript", ["-e", `tell application "${terminalApp}" to activate`]);
}

// Spawn a fresh terminal window running an interactive grove command (e.g. init).
export async function spawnInTerminal(command: string) {
  const { terminalApp } = prefs();
  await closeMainWindow();

  if (terminalApp === "Ghostty") {
    // Ghostty has no AppleScript "do script"; launch a new window via its CLI.
    await execFileAsync("open", ["-na", "Ghostty", "--args", "-e", "zsh", "-lc", `${command}; exec zsh`]);
  } else {
    const escaped = command.replace(/"/g, '\\"');
    const script =
      terminalApp === "Terminal"
        ? `tell application "Terminal"
             activate
             do script "${escaped}"
           end tell`
        : `tell application "iTerm"
             activate
             create window with default profile
             tell current session of current window to write text "${escaped}"
           end tell`;
    await execFileAsync("osascript", ["-e", script]);
  }
  await showHUD(`Opened ${terminalApp}`);
}
