import { showToast, Toast } from "@raycast/api";
import { groveRaw } from "./grove";

export default async function Command() {
  const toast = await showToast({ style: Toast.Style.Animated, title: "Syncing all repos" });
  try {
    await groveRaw(["sync"]);
    toast.style = Toast.Style.Success;
    toast.title = "All repos synced";
  } catch (e) {
    toast.style = Toast.Style.Failure;
    toast.title = "Sync failed";
    toast.message = String(e);
  }
}
