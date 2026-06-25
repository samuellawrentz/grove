import { getPreferenceValues } from "@raycast/api";
import { execFile } from "child_process";
import { promisify } from "util";

const execFileAsync = promisify(execFile);

interface Prefs {
  grovePath?: string;
  terminalApp?: string;
}

export function prefs(): Required<Prefs> {
  const p = getPreferenceValues<Prefs>();
  return {
    grovePath: p.grovePath?.trim() || "grove",
    terminalApp: p.terminalApp || "Ghostty",
  };
}

// grove may live in ~/.cargo/bin, which Raycast's PATH often misses.
function env() {
  const home = process.env.HOME ?? "";
  const extra = [`${home}/.cargo/bin`, "/opt/homebrew/bin", "/usr/local/bin"];
  const path = [...extra, process.env.PATH ?? ""].join(":");
  return { ...process.env, PATH: path };
}

export async function grove<T = unknown>(args: string[]): Promise<T> {
  const { grovePath } = prefs();
  const { stdout } = await execFileAsync(grovePath, [...args, "--json"], {
    env: env(),
    maxBuffer: 16 * 1024 * 1024,
  });
  return JSON.parse(stdout) as T;
}

// For commands without meaningful JSON we still want stderr surfaced on failure.
export async function groveRaw(args: string[]): Promise<string> {
  const { grovePath } = prefs();
  const { stdout } = await execFileAsync(grovePath, args, { env: env() });
  return stdout;
}

export interface Task {
  task_id: string;
  branch: string;
  path: string;
  repos: string[];
  repo_count?: number;
  created_at: string;
  exists?: boolean;
  tmux_alive: boolean;
  tmux_window?: string;
  claude_state: string;
  agent_state: string;
}

export interface Repo {
  name: string;
  url: string;
  path: string;
  default_branch: string;
  exists: boolean;
  last_synced_at?: string;
  registered_at: string;
}

export const listTasks = () => grove<{ ok: boolean; tasks: Task[] }>(["list"]);
export const listRepos = () => grove<{ ok: boolean; repos: Repo[] }>(["repos"]);
