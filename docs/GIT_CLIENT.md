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
line under the picker tells you which diagram groups, API folders and projects use it.

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
- **History**: the latest 200 commits, with **Load more**. Click a commit to see its
  message and changed files.
- The header shows the current branch, commits ahead/behind, and **Fetch**, **Pull**
  and **Push**. Pull only fast-forwards unless you turn on **Pull with rebase** in
  Preferences > Git. Push sets the upstream on the first push of a new branch.

Git never prompts inside Tabular. Fetch, pull, push and clone use your credential
helper or SSH key; if they need a password, the error tells you to set one up in a
terminal first.

## Merge Review

1. Open **Preferences > Git** and paste a token:
   - GitHub: a classic token with the `repo` scope, or a fine-grained token with
     *Pull requests: read & write*.
   - GitLab: a personal access token with the `api` scope. Change **GitLab URL** for a
     self-hosted instance.

   Tokens are stored in the OS keychain.
2. In the Git tab choose **Review**. **Assigned to me** lists open requests where you
   are a requested reviewer, an assignee or the author, across all repositories.
   **This repository** lists the requests of the selected repository.
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

## Not included yet

Stash, rebase and cherry-pick, tags, blame, a conflict editor (conflicted files are
listed; resolve them in your editor and stage them) and syntax highlighting in diffs.
