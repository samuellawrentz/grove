import {
  Action,
  ActionPanel,
  Alert,
  Color,
  Icon,
  List,
  closeMainWindow,
  confirmAlert,
  showToast,
  Toast,
} from "@raycast/api";
import { usePromise } from "@raycast/utils";
import { groveRaw, listTasks, Task } from "./grove";
import { focusTerminal } from "./terminal";

function stateColor(s: string): Color {
  if (s === "not running") return Color.SecondaryText;
  if (s.includes("working") || s.includes("running")) return Color.Green;
  return Color.Yellow;
}

export default function Command() {
  const { data, isLoading, revalidate } = usePromise(async () => (await listTasks()).tasks);

  async function attach(task: Task) {
    if (!task.tmux_alive) {
      await showToast({ style: Toast.Style.Failure, title: "tmux not running", message: "Recreate the task with tmux." });
      return;
    }
    try {
      // select-window switches the existing client; then bring Ghostty forward.
      await groveRaw(["attach", task.task_id]);
      await focusTerminal();
      await closeMainWindow();
    } catch (e) {
      await showToast({ style: Toast.Style.Failure, title: "Attach failed", message: String(e) });
    }
  }

  async function close(task: Task) {
    const ok = await confirmAlert({
      title: `Close ${task.task_id}?`,
      message: "Removes worktrees and tmux session. Merged branches are deleted.",
      primaryAction: { title: "Close", style: Alert.ActionStyle.Destructive },
    });
    if (!ok) return;
    const toast = await showToast({ style: Toast.Style.Animated, title: `Closing ${task.task_id}` });
    try {
      await groveRaw(["close", task.task_id]);
      toast.style = Toast.Style.Success;
      toast.title = `Closed ${task.task_id}`;
      revalidate();
    } catch (e) {
      toast.style = Toast.Style.Failure;
      toast.title = "Close failed";
      toast.message = String(e);
    }
  }

  return (
    <List isLoading={isLoading} searchBarPlaceholder="Search tasks">
      {(data ?? []).map((t) => (
        <List.Item
          key={t.task_id}
          title={t.task_id}
          subtitle={t.branch}
          icon={{
            source: t.tmux_alive ? Icon.CircleFilled : Icon.Circle,
            tintColor: t.tmux_alive ? Color.Green : Color.SecondaryText,
          }}
          accessories={[
            { tag: { value: `${t.repos.length} repos`, color: Color.Blue } },
            { tag: { value: t.claude_state, color: stateColor(t.claude_state) } },
          ]}
          actions={
            <ActionPanel>
              <Action title="Attach" icon={Icon.Terminal} onAction={() => attach(t)} />
              <Action.Push
                title="View Repos"
                icon={Icon.List}
                target={
                  <List navigationTitle={t.task_id}>
                    {t.repos.map((r) => (
                      <List.Item key={r} title={r} icon={Icon.Folder} />
                    ))}
                  </List>
                }
              />
              <Action.Open title="Open Task Folder" target={t.path} icon={Icon.Finder} />
              <Action.CopyToClipboard title="Copy Path" content={t.path} />
              <Action
                title="Close Task"
                icon={Icon.Trash}
                style={Action.Style.Destructive}
                shortcut={{ modifiers: ["ctrl"], key: "x" }}
                onAction={() => close(t)}
              />
              <Action title="Refresh" icon={Icon.ArrowClockwise} onAction={revalidate} shortcut={{ modifiers: ["cmd"], key: "r" }} />
            </ActionPanel>
          }
        />
      ))}
      <List.EmptyView title="No active tasks" description="Create one with the Create Grove Task command." />
    </List>
  );
}
