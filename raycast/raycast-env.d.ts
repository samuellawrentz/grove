/// <reference types="@raycast/api">

/* 🚧 🚧 🚧
 * This file is auto-generated from the extension's manifest.
 * Do not modify manually. Instead, update the `package.json` file.
 * 🚧 🚧 🚧 */

/* eslint-disable @typescript-eslint/ban-types */

type ExtensionPreferences = {
  /** Grove Binary Path - Absolute path to the grove executable. */
  "grovePath": string,
  /** Terminal App - Terminal where tmux runs; focused on attach. */
  "terminalApp": "Ghostty" | "iTerm" | "Terminal"
}

/** Preferences accessible in all the extension's commands */
declare type Preferences = ExtensionPreferences

declare namespace Preferences {
  /** Preferences accessible in the `list-tasks` command */
  export type ListTasks = ExtensionPreferences & {}
  /** Preferences accessible in the `create-task` command */
  export type CreateTask = ExtensionPreferences & {}
  /** Preferences accessible in the `repos` command */
  export type Repos = ExtensionPreferences & {}
  /** Preferences accessible in the `sync-all` command */
  export type SyncAll = ExtensionPreferences & {}
}

declare namespace Arguments {
  /** Arguments passed to the `list-tasks` command */
  export type ListTasks = {}
  /** Arguments passed to the `create-task` command */
  export type CreateTask = {}
  /** Arguments passed to the `repos` command */
  export type Repos = {}
  /** Arguments passed to the `sync-all` command */
  export type SyncAll = {}
}

