import { Action, ActionPanel, Color, Icon, List, showToast, Toast } from "@raycast/api";
import { usePromise } from "@raycast/utils";
import { groveRaw, listRepos } from "./grove";

export default function Command() {
  const { data, isLoading, revalidate } = usePromise(async () => (await listRepos()).repos);

  async function sync(name?: string) {
    const label = name ?? "all repos";
    const toast = await showToast({ style: Toast.Style.Animated, title: `Syncing ${label}` });
    try {
      await groveRaw(name ? ["sync", name] : ["sync"]);
      toast.style = Toast.Style.Success;
      toast.title = `Synced ${label}`;
      revalidate();
    } catch (e) {
      toast.style = Toast.Style.Failure;
      toast.title = "Sync failed";
      toast.message = String(e);
    }
  }

  return (
    <List isLoading={isLoading} searchBarPlaceholder="Search repos">
      {(data ?? []).map((r) => (
        <List.Item
          key={r.name}
          title={r.name}
          subtitle={r.default_branch}
          icon={{ source: Icon.Folder, tintColor: r.exists ? Color.Green : Color.Red }}
          accessories={[{ date: r.last_synced_at ? new Date(r.last_synced_at) : undefined, tooltip: "Last synced" }]}
          actions={
            <ActionPanel>
              <Action title="Sync This Repo" icon={Icon.ArrowClockwise} onAction={() => sync(r.name)} />
              <Action title="Sync All" icon={Icon.ArrowClockwise} onAction={() => sync()} shortcut={{ modifiers: ["cmd"], key: "s" }} />
              <Action.CopyToClipboard title="Copy URL" content={r.url} />
            </ActionPanel>
          }
        />
      ))}
      <List.EmptyView title="No registered repos" description="Register with: grove register <name> <url>" />
    </List>
  );
}
