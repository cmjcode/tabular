# Projects

A project groups the three places where Tabular keeps work for one application:
a folder of connections, a folder of saved queries and an HTTP collection. It also
holds the environments (`.env` values for Development, Staging, Production and any
you add) and a memory that AI agents read and write.

## Create a project

1. Open the project menu on the right side of the header, next to the AI button. The
   pill beside the project name shows the active environment; click it to switch.
2. Choose **New project…**, type a name and keep the three **Create folders** boxes
   ticked.
3. Tabular creates a connection folder, a query folder and an HTTP collection with the
   project name, and makes the project active.

Folders that already exist with that name are reused. To turn an existing folder into
a project, right-click a top-level connection folder, a top-level query folder or an
HTTP collection and choose **Convert to Project…**.

The project owns everything inside its folders, including subfolders. Moving a
connection into the project folder adds it to the project.

## Environments

Every project starts with Development, Staging and Production. Edit them under
**Edit project… > Environments**:

- **Variables** work like a `.env` file. Write `{{KEY}}` in a query or an HTTP
  request and Tabular fills in the value of the active environment when it runs.
  Unknown placeholders are left as they are.
- **Secret** variables are kept in this computer's keychain. They are never written to
  the project file, never shared and never shown to agents. HTTP requests use them;
  SQL does not, because the executed SQL is stored in query history.
- **Connections** lists the connections in the project folder. Tick the ones each
  environment uses. Tabular colours those connections with the environment's colour.

Switch environment with the coloured pill next to the project name. If the active
query tab uses a connection of the project, the tab moves to the matching connection
of the new environment.

**Show only this project** in the project menu hides the other folders in the
Connections, Queries and HTTP lists.

## AI memory

Each project has a memory: short Markdown facts about the project, such as what a
status code means, how two tables join, or how the environments differ.

- The AI Assistant sees the current project, its environment, the variable names and
  the memory in every conversation.
- Agents connected through `tabular mcp` use `list_projects`, `project_context`,
  `save_project_memory` and `delete_project_memory` (see [MCP.md](MCP.md)).
- You can read and delete entries under **Edit project… > AI memory**.

Memory lives in `<data dir>/projects/<id>/memory/`, one file per fact plus a
`MEMORY.md` index. Values of the project's secret variables are replaced with
`[redacted]` before a fact is saved.

## Share with a team

Open **Collaborations > Projects**, pick a team and choose **Share**. Tabular shares
the connection folder, the query folder, the HTTP collection and the project itself
with that team, then re-encrypts the items that were already synced with the team key.
Subfolders are included.

Team members receive the project on their next sync, with its folders created for them.
They get the environments, the variable names and non-secret values, and the memory.
Each member fills in secret values on their own computer. Members with the *member*
role can read a shared project; owners and admins can also edit it.

**Stop sharing** removes all four shares. Renaming a shared project shares it again
under the new name.

## Where it is stored

| What | Where |
|---|---|
| Project file | `<data dir>/projects/<id>/project.json` |
| Memory | `<data dir>/projects/<id>/memory/` |
| Active project and filter | `<data dir>/projects/ui_state.json` |
| Secret values | OS keychain or Tabular's encrypted secrets file |
| On the sync server | One encrypted row per project; the server cannot read it |
