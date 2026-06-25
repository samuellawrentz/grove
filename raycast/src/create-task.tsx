import { Action, ActionPanel, Form, Icon, showToast, Toast, useNavigation } from "@raycast/api";
import { usePromise } from "@raycast/utils";
import { listRepos, prefs } from "./grove";
import { spawnInTerminal } from "./terminal";

interface Values {
  taskId: string;
  repos: string[];
  branch: string;
  base: string;
  context: string;
}

export default function Command() {
  const { pop } = useNavigation();
  const { data: repos, isLoading } = usePromise(async () => (await listRepos()).repos);

  async function submit(v: Values) {
    if (!v.taskId.trim()) {
      showToast({ style: Toast.Style.Failure, title: "Task id required" });
      return;
    }
    if (!v.repos.length) {
      showToast({ style: Toast.Style.Failure, title: "Pick at least one repo" });
      return;
    }
    // init launches tmux + Claude interactively, so hand off to a terminal.
    const parts = [prefs().grovePath, "init", v.taskId, ...v.repos];
    if (v.branch.trim()) parts.push("--branch", v.branch.trim());
    if (v.base.trim()) parts.push("--base", v.base.trim());
    if (v.context.trim()) parts.push("--context", `'${v.context.replace(/'/g, "'\\''")}'`);
    await spawnInTerminal(parts.join(" "));
    pop();
  }

  return (
    <Form
      isLoading={isLoading}
      actions={
        <ActionPanel>
          <Action.SubmitForm title="Create Task" icon={Icon.Plus} onSubmit={submit} />
        </ActionPanel>
      }
    >
      <Form.TextField id="taskId" title="Task ID" placeholder="add-billing" />
      <Form.TagPicker id="repos" title="Repos">
        {(repos ?? []).map((r) => (
          <Form.TagPicker.Item key={r.name} value={r.name} title={r.name} icon={Icon.Folder} />
        ))}
      </Form.TagPicker>
      <Form.TextField id="branch" title="Branch" placeholder="defaults to task id" />
      <Form.TextField id="base" title="Base Branch" placeholder="repo default branch" />
      <Form.TextArea id="context" title="Context" placeholder="Goes into CONTEXT.md" />
      <Form.Description text="Runs grove init in your terminal so the tmux/Claude session opens interactively." />
    </Form>
  );
}
