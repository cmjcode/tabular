# Diagram groups linked to a code repository

A diagram group can point at the code that uses its tables. Tabular then reads
that code and suggests which tables from the diagram belong in the group.

## Linking a repository

Right-click the group title and choose **Set Repository…** (or **Edit
Repository…**). The dialog has two fields:

| Field | What it is | Where it is stored | When it is used |
|---|---|---|---|
| Git URL | `https://…`, `ssh://…`, `git@host:org/repo.git` or `file://…` | In the diagram, so it is shared with everyone who can open it (the `diagram_by_tabular` table and the local diagram file) | When you have no project folder for this group, or it is missing |
| Project folder | A local checkout, picked with **Browse…** | Only on this computer, in `{data dir}/diagram_repo_paths.json` for your OS user | First choice, whenever it exists |

The split is deliberate. Another user who opens the diagram gets the git URL
but never your local path, which would not exist on their machine. Anyone with
access to the database and to the git repository can run a scan. Each user can
set their own project folder for the same group.

Picking a folder fills the Git URL from its `.git/config` (remote `origin`,
otherwise the first remote). **Detect** re-reads it. Subfolders of a repository
and git worktrees are recognised.

If the folder does not exist on this computer but the URL is valid, the dialog
offers **Clone into this folder**, which runs a full `git clone` there. You can
also leave it: scans then use a private clone of the URL.

Because the URL is shared, a URL that contains a password or access token
(`https://user:secret@…`, `https://ghp_…@github.com/…`) cannot be saved. Use SSH
keys or a git credential helper instead. Any credentials that still appear in
git output are masked in tooltips, logs and errors.

**Open Folder Location** in the group menu opens the project folder in Finder,
Windows Explorer or the Linux file manager.

## Suggest Tables from Repo (AI)

Right-click the group title and choose **Suggest Tables from Repo (AI)**, or
press **Save & Suggest Tables** in the repository dialog.

1. **Prepare the code.** A local folder is read in place. A URL is cloned with
   depth 1 into `{data dir}/agent-workspace/repos/` and refreshed with
   `git fetch` on later scans; if the fetch fails (offline), the last copy is used.
2. **Text search.** Every file git knows about (tracked plus new, not ignored)
   is searched for the diagram's table names. Matches after `FROM`, `JOIN`,
   `INTO`, `UPDATE`, `TABLE`, `__tablename__`, `@Table(name = …)` and quoted
   names count; plain words in prose do not. Folders such as `node_modules`,
   `vendor`, `target` and `dist`, binaries, minified files and files over 1 MB
   are skipped. At most 20,000 files are read.
3. **AI review.** The AI backend selected in the AI Assistant panel reviews the
   code and confirms or rejects each table. It also maps ORM models that never
   name their table, for example a Laravel `class Invoice extends Model` to
   `invoices`.

| Backend | How it sees the code |
|---|---|
| Claude Code | Runs inside the repository with only `Read`, `Grep`, `Glob` and `LS` allowed |
| Gemini CLI | Runs inside the repository in read-only `plan` mode |
| Antigravity (agy), Custom CLI | Runs in a private clone, never in your folder. A local folder that is not a git repository falls back to snippets |
| API providers | Receive the matching lines from the text search only, not whole files |

If no AI backend is ready, or the AI step fails or times out, the window shows
the text search results and says so.

## Reviewing the suggestions

Each row shows the table, whether the AI confirmed it (**AI**) or only the text
search found it (**TEXT**), a confidence score, a short reason and the
`path:line` evidence. Tables that are already in the group are shown but cannot
be selected. Rows at 50% confidence or more start selected.

**Add N table(s)** adds the selection to the group. With **Arrange added tables
in the group** on, tables that belong to no other group are placed in a grid
below the group; tables from other groups keep their position. Names the AI
found in the code that are not in this diagram are listed separately.

**Rescan** repeats the scan. **Cancel** or closing the window stops a running
scan, including the git and AI processes.

## HTTP API endpoints on tables

A group is linked to every folder in the **APIs** sidebar that uses the same git
repository. Right-click the group title and choose **HTTP API Folders…** to
show it. When endpoints are generated from that folder's repository, each
endpoint appears as an API card in a lane beside the tables it reads or writes
(or, if you choose **Badges** in the diagram menu, as a blue **API n** badge on
each table header).

## Generate Business Process (AI)

Right-click the group title and choose **Generate Business Process (AI)**. The
AI follows each API card of the group's repository from its route handler into
services, repositories and SQL, and fills the card with the steps in order and
the tables each step reads or writes. Double-click a card to play the process
on the canvas. Cards whose source files have not changed since the last run
are skipped. While the job runs, the menu item becomes **Show Generation
Progress**.

The item is enabled when the group has a repository and at least one API card
comes from that repository. See
[HTTP_API_REPOSITORY.md](HTTP_API_REPOSITORY.md#business-processes-ai) for
cards, playback and Mermaid export.

## Notes

- `git` must be installed for URLs, private copies and `.gitignore` support.
  Private repositories use your existing git credentials; Tabular never prompts
  for them.
- The Mac App Store build cannot start external processes, so only local
  folders and the text search are available there.
- Tables from linked databases are not suggested, because their groups follow
  the source diagram.
