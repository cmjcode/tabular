# HTTP API folders linked to a code repository

A folder in the **APIs** sidebar can point at the code of the API it holds.
Tabular then reads that code to create every endpoint as a saved request,
links each endpoint to the database tables it uses in the diagram, and designs
integration tests that can span several APIs.

## Linking a repository

Right-click a folder and choose **Set Repository…** (or **Edit Repository…**).
The dialog is the same as the one for diagram groups
([DIAGRAM_GROUP_REPOSITORY.md](DIAGRAM_GROUP_REPOSITORY.md)):

| Field | Where it is stored | Notes |
|---|---|---|
| Git URL | In the HTTP collection (`http_collections/<workspace>.json`), exported with **Export All Data** | URLs with a password or token cannot be saved |
| Project folder | Only on this computer, in `{data dir}/diagram_repo_paths.json` | Used first when it exists; otherwise a private clone of the URL is used |

Folders with a repository show a git icon in the sidebar. **Open Folder
Location** opens the local project folder.

Server sync of HTTP collections sends requests and folder names only, so the
git URL of a folder stays on the devices where it was set or imported.

## The link to diagram groups

An HTTP API folder and a diagram group are linked when they point at the same
repository. Tabular compares the git URLs after normalising them, so
`https://github.com/Org/App.git`, `git@github.com:org/app` and
`ssh://git@github.com/org/app` are the same repository. When only a project
folder is set, its `origin` remote is used.

- In the APIs sidebar, **Linked Diagram Groups…** opens the diagram and focuses
  the linked group. With several groups, a list lets you pick one.
- In the diagram, right-click a group title and choose **HTTP API Folders…** to
  show the linked folder in the sidebar.

## Generate Endpoints from Repo (AI)

Right-click the folder and choose **Generate Endpoints from Repo (AI)**, or
press **Save & Generate Endpoints** in the repository dialog.

1. **Prepare the code.** Same as the diagram group scan: a local folder is read
   in place, a URL is cloned with depth 1 into `{data dir}/agent-workspace/repos/`.
2. **Find routes.** Every source file git knows about is searched for route
   definitions. Test files, `node_modules`, `vendor`, build output and files over
   1 MB are skipped. Supported without AI:

   | Language | Frameworks |
   |---|---|
   | JavaScript / TypeScript | Express, Koa, Fastify, Hono, NestJS, Next.js (`app/**/route.ts`, `pages/api`), SvelteKit (`+server.ts`) |
   | Python | FastAPI, Flask (including blueprints), Django `urls.py`, Django REST Framework routers |
   | PHP | Laravel (`routes/api.php` adds `/api`, `Route::prefix()->group`, `resource`, `apiResource`), Symfony attributes |
   | Ruby | Rails `config/routes.rb` (`namespace`, `scope`, `resources … only:`) |
   | Go | Gin, Echo, Fiber, chi, gorilla/mux, `net/http` (Go 1.22 `"GET /path"`) |
   | Rust | Axum, Actix Web, Rocket |
   | Java / Kotlin | Spring (`@RequestMapping` on the class plus `@GetMapping` …), JAX-RS |
   | C# | ASP.NET controllers (`[Route("api/[controller]")]`, `[HttpGet]`), minimal APIs with `MapGroup` |

3. **Find missed endpoints (AI).** When the AI backend can read the repository
   (Claude Code, Gemini CLI, or agy/Custom CLI in a private copy), it lists the
   endpoints the route search missed, such as routers mounted in another file.
4. **Document every endpoint (AI).** Endpoints are sent in batches of 20,
   grouped by file, so large APIs are documented completely. Three batches run
   at the same time by default; change **Parallel AI batches** (1 to 6) next to
   **Rescan** for the next run. Higher values are faster but use more of your AI
   quota. For each endpoint
   the AI fills in a name, description, path and query parameters, headers,
   auth (bearer, basic or API key), body type and example, a response example,
   status codes, the source line and the database tables it reads or writes.
   Table names are matched to the tables of the linked diagrams.

API providers (not CLI agents) cannot open files. They receive the source of
the route files, up to 60 KB per batch, with string literals that look like
passwords, tokens or keys replaced by `«redacted»`.

If no AI backend is ready or a batch fails, the endpoints from the route search
are still listed with their method and path; the window says so.

### Several folders at once

Each folder has its own generation job and window, so you can generate
endpoints for several folders or repositories at the same time. Starting a
folder again while its job is running only brings its window back. Close a
window to keep its job running in the background: a notification appears and
the window opens again when the endpoints are ready. **Cancel** stops the job.
All jobs together run at most 6 AI turns at once; extra batches wait for a
free turn.

### Reviewing and adding

Each row shows the method, path, name, the tables it uses, **AI** or **TEXT**
(route search only) and **exists** when the folder already has a request for the
same method and path. Click a row to see its parameters, body and response
examples. New endpoints start selected; existing ones do not.

- **Base URL** is detected from the code when possible (for example the port the
  server listens on), otherwise it follows the framework default.
- **Group into sub-folders by resource** creates one sub-folder per resource,
  such as `users` for `/api/v1/users/{id}`.
- **Link endpoints to diagram tables** is on when a linked diagram group exists.

**Add N endpoint(s)** creates the requests. Selected rows that already exist are
updated in place; tokens, passwords and API key values you already entered are
kept. Path parameters in the URL get their example value, and the route template
is kept with the request so later links still use `/users/{id}`.

**Link Endpoints to Diagram Tables** in the folder menu repeats the linking for
requests that already have tables, for example after you link a new diagram group.

## Endpoints in the database diagram

Linked endpoints are stored in the diagram, so they are saved to the diagram
file, the `diagram_by_tabular` table, the vault and cloud sync together with
groups and notes. A diagram that is not open is updated in its local file; open
it and save it to share the links.

- A blue **API n** badge on a table header shows how many endpoints use the
  table. Hover it for the list of `METHOD /path`.
- Click the badge for the endpoint panel: filter, open an endpoint's request in
  the HTTP client, or unlink it from the table.
- **Show API endpoints** in the diagram menu hides or shows the badges.
- Diagram search (Cmd+F) finds endpoints by method or path; the result jumps to
  the table the endpoint uses.
- Links to tables that disappear from the schema are removed on the next schema
  sync. Focus and group tabs keep the links of the tables they show.

## Integration tests (AI)

Right-click a folder or a workspace and choose **Generate Integration Tests
(AI)…**. Select one or more folders, from the same or different repositories,
and optionally describe what to focus on. The AI receives the endpoint catalog
(method, path, name, auth type, query names, tables and redacted body examples,
never auth values) and designs suites that:

- create data, read it back, update, list and clean up;
- cover negative cases such as validation errors, not found and unauthorized;
- pass values between steps and across services with `extract` and
  `{{variables}}`.

Suites are saved in `{app data}/http_tests/` and open in **Integration Tests…**
(workspace menu).

### Running a suite

- **Variables**: each folder gets a `base_url_<folder>` variable with the host
  its requests use. Tokens, passwords and API keys start empty; fill them in
  before running. `{{run_id}}` is unique per run, useful for unique emails.
- Uncheck a step to skip it.
- **Run** sends the steps in order. If the suite has steps that create, change
  or remove data, Tabular asks for confirmation and shows the target servers.
  Use a development or test environment.
- Each step shows pass or fail, status and time. Expand it for assertions with
  the actual value on failure, extracted variables, unresolved `{{variables}}`
  and the start of the response body. **Open in HTTP client** opens the step with
  the variables filled in.

Assertions use `status`, `time_ms`, `body`, `header:<Name>` or
`json:$.path.to[0].field` with `equals`, `not_equals`, `contains`,
`not_contains`, `exists`, `not_exists`, `less_than`, `greater_than` or `matches`
(regular expression).
