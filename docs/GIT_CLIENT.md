# Git client

The **Git** tab in the sidebar is a small git client for the code that goes with your
databases and APIs, plus **Merge Review** for GitHub pull requests and GitLab merge
requests with an AI review.

It uses the `git` installed on your computer. If git is missing, Preferences > Git says
so and the local features stay disabled.

## Repositories

The repository list is shared with the rest of Tabular. A repository shows up when:

- a diagram group has a repository (group menu > Repository),
- an HTTP API folder has a repository,
- a project has a repository URL, or
- you add it in the Git tab with **+ > Add local folder…**, **Clone repository…** or
  **Init repository…**.

Entries that point to the same repository are merged by their git URL, so
`git@github.com:org/app.git` and `https://github.com/org/app` are one repository. The
line under a repository tells you which diagram groups, API folders and projects use it.

### One section per repository

Like Source Control in VS Code, the sidebar shows every repository of the project
selected in the project switcher at once. Each repository is a section you can open
and close. Its header shows the current branch, commits behind/ahead, the number of
changes, and buttons for **Git Graph** and **Fetch**. Right-click the header for
Pull, Push, Reveal in file manager, Copy path, and Remove from list.

An open section has its own **Changes / Branches / History / Review** buttons, and
remembers which one you used. Operations run per repository, so a fetch in one
repository does not block a commit in another.

A repository belongs to a project when the project's repository URL points to it, when
a diagram group of the project's connections or an HTTP API folder of its workspace
uses it, or when you added it with **+** while that project was selected. Other
repositories are listed under **Other repositories**; right-click one and choose
**Add to project** to move it. With no project selected, every repository is listed.
The **Fetch all** button in the sidebar header fetches every repository of the project.

If a repository is known only by its URL (for example a teammate set it on a diagram
group), the Git tab offers **Clone…** or **Choose folder…**. The folder you pick or
clone into is also saved for those diagram groups and API folders on this computer.
When a repository already has a folder but some linked items do not, use **Use this
folder**.

Folders are personal to this computer and are not synced. Repositories you add in the
Git tab and the Git settings are stored in `git_repos.json` in the Tabular data folder.

## Changes

- Click a file to see its diff in the editor area, side by side or unified.
- **+** stages a file, **−** unstages it. Right-click a file or a section header for
  more: stage all, unstage all, discard changes, copy path.
- Write a message and click **Commit** (or press Ctrl/Cmd+Enter). With nothing staged,
  the button says **Commit all** and commits every change, like VS Code.
- **Amend** rewrites the last commit. Leave the message empty to keep the old one.
- The ✨ button asks your AI backend to write the commit message from the diff.

Discarding changes and removing untracked files always asks for confirmation.

## Branches and history

- **Branches**: create a branch from HEAD, double-click a branch to check it out (a
  remote branch becomes a local tracking branch), right-click to remove it.
- **History**: the latest 200 commits of the current branch with a small graph, and
  **Load more**. Click a commit to see its message and changed files, or click
  **Open Git Graph** for all branches.
- Pull only fast-forwards unless you turn on **Pull with rebase** in Preferences > Git.
  Push sets the upstream on the first push of a new branch.
- When a merge, rebase, cherry-pick or revert stops on conflicts, the section and Git
  Graph show **Continue** and **Abort**.

Git never prompts inside Tabular. Fetch, pull, push and clone use your credential
helper or SSH key; if they need a password, the error tells you to set one up in a
terminal first.

## Git Graph

**Git Graph** opens a repository's history in the editor area, modeled on the Git Graph
extension for VS Code:

- A colored graph of all local branches, remote branches, tags and stashes, with an
  **Uncommitted Changes** row on top. Branches that also exist on a remote show as one
  label (`main | origin`). Columns: Description, Date, Author, Commit. Drag a column
  edge to resize it.
- **Branches** filters the graph to the branches you tick; **Show Remote Branches**
  hides or shows remote branches. More commits load while you scroll.
- Click a commit for its details at the bottom: hash, parents (click to jump), author,
  committer, signature, labels, and the full message with Markdown, emoji shortcodes
  and issue links. The file list (list or tree) shows added/removed lines; click a file
  for its diff, double-click to open it in a tab.
- Ctrl/Cmd+click a second commit to compare the two. **Compare with Working Tree** is
  in the commit menu.
- **Start Code Review** marks every file you open as reviewed for that commit or
  comparison, so you can see what is left. The marks are kept between sessions.
- Find (Ctrl/Cmd+F) searches messages, hashes, authors and branch names, with match
  case and regular expression options. Ctrl/Cmd+R refreshes, Ctrl/Cmd+H jumps to HEAD,
  and the arrow keys move between commits while the details are open.
- **Remotes** (toolbar) adds, edits, renames, fetches, prunes and removes remotes.

Right-click for actions:

| Where | Actions |
|---|---|
| Commit | Add Tag, Create Branch, Checkout, Cherry Pick, Revert, Drop, Merge into current branch, Rebase current branch on this commit, Reset current branch to this commit (soft, mixed, hard), Compare, Copy hash or subject, Create Archive |
| Local branch | Checkout, Rename, Delete (optionally on the remote too), Merge, Rebase, Push (normal, force with lease, force), Create Pull Request, Create Archive, Select in filter, Copy name |
| Remote branch | Checkout as a local tracking branch, Delete on remote, Fetch into a local branch, Merge, Pull into current branch, Create Pull Request |
| Tag | View details, Delete (optionally on the remote too), Push, Create Archive |
| Stash | Apply, Pop, Drop, Create Branch from stash |
| Uncommitted Changes | Stash, Reset (mixed or hard), Clean untracked files |

Double-click a branch label to check it out. Every action that can lose work asks
for confirmation first, with a red button.

Graph settings are in the toolbar (ordering by commit date, author date or topology;
rounded or angular lines; date format; columns; tags, stashes, uncommitted changes,
first-parent only, reflog commits, muted merge commits) and in Preferences > Git
(author avatars, commits per page, commit signatures, prune on fetch, issue links).

## Merge Review

1. Open **Preferences > Git** and paste a token:
   - GitHub: a classic token with the `repo` scope, or a fine-grained token with
     *Pull requests: read & write*.
   - GitLab: a personal access token with the `api` scope. Change **GitLab URL** for a
     self-hosted instance.

   Tokens are stored in the OS keychain.
2. Open a repository section and choose **Review**. **This repository** lists the
   requests of that repository. **Assigned to me** lists open requests where you are a
   requested reviewer, an assignee or the author, across all repositories.
3. Click a request to open it in the editor area: changed files, side-by-side diff,
   description, labels and reviewers.
4. **Analyze with AI** reviews every changed file and ends with APPROVE, APPROVE WITH
   SUGGESTIONS or REQUEST CHANGES. It uses the backend chosen in Preferences > AI
   Assistant (an API key or a CLI agent such as Claude Code). The language is set in
   Preferences > Git.
5. Comment (Markdown, with preview), use the AI review as a comment, **Merge** (merge
   commit, squash, or rebase on GitHub) or **Close**. Merge and close ask for
   confirmation. Draft requests cannot be merged.

The list refreshes every 5 minutes by default; change it or turn it off in
Preferences > Git.

### What is sent where

- Diffs sent to an AI API provider go through the same secret redaction as the rest of
  Tabular, and the AI Assistant switch in Preferences > Privacy blocks them.
- The first 40,000 characters of the diff go into the review prompt; later files are
  listed as not shown.
- Git and the GitHub/GitLab API are contacted only for actions you start, listed under
  Preferences > Privacy > Always initiated by you.
- Author avatars are initials by default. If you choose **Gravatar** in Preferences >
  Git, an MD5 hash of each author's email is sent to gravatar.com; the images are
  cached in the `git_avatars` folder in the Tabular data folder.

## Not included yet

Blame, interactive rebase, a conflict editor (conflicted files are listed; resolve them
in your editor, stage them, then choose **Continue**) and syntax highlighting in diffs.
