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
- **Also generate business process** starts [business process
  generation](#business-processes-ai) for the linked diagrams that are open,
  right after the links are added. Diagrams that are not open get the links
  only; generate their processes from the diagram later.

**Add N endpoint(s)** creates the requests. Selected rows that already exist are
updated in place; tokens, passwords and API key values you already entered are
kept. Path parameters in the URL get their example value, and the route template
is kept with the request so later links still use `/users/{id}`.

**Link Endpoints to Diagram Tables** in the folder menu repeats the linking for
requests that already have tables, for example after you link a new diagram group.

## Endpoints in the database diagram

Linked endpoints are stored in the diagram, so they are saved to the diagram
file and the `diagram_by_tabular` table together with
groups and notes. A diagram that is not open is updated in its local file; open
it and save it to share the links.

By default endpoints are listed in the **API panel** on the left of the diagram
(the *Rail* mode), and the canvas stays a plain ERD:

- The panel groups endpoints by their main table (the table named in the path,
  otherwise the first table the endpoint writes), so `/panens`, `/panens/{id}`
  and `/panens_filter_date` all sit under `panens`. Each row shows the method,
  the full path and the operations the endpoint performs (**C**reate, **R**ead,
  **U**pdate, **D**elete).
- Click an endpoint to show its process card next to the tables it uses. The
  other tables are dimmed, the lines carry the operation and step numbers, and
  the process plays. Click the row again, or press Esc, to clear it.
- Hover a step to outline its table; hover a table to highlight the steps that
  use it.
- Each table header shows a badge such as **API 6 · CRUD**: six endpoints use
  the table, and together they create, read, update and delete it.
- The **CRUD matrix** tab counts endpoints per table and operation. Click a
  number to list those endpoints; click a table name to jump to it.
- Double-click a table name in the list to jump to that table. The filter box
  matches method, path, summary and table names. The chevron hides the panel.

The diagram menu controls how endpoints are shown:

| Setting | Options |
|---|---|
| **Show API endpoints** | Hides or shows cards and badges |
| **API endpoints as** | **Rail** (default): the API panel plus table badges. **Cards**: every endpoint as a card in an API band above its tables. **Badges**: only the blue **API n** badge on each table header. **Both**: cards and badges |
| **Process lines** | Cards and Both only. **Selected** (default): lines only for the selected, hovered or playing card. **All**: every card's lines, faded except the active one |
| **Arrange API cards** | Cards and Both only. Puts every card back in its automatic band |

Working with cards (in Rail mode only the selected endpoint has a card, and it
is placed automatically, so it cannot be dragged):

- Click a card to select it and draw its lines to the tables. Drag a card to
  move it; the position is saved with the diagram.
- Line colour follows the operation: blue for reads (arrow from the table),
  green for insert, update and upsert, red for delete (arrow to the table), grey
  when the operation is unknown. A table that is both read and written gets the
  write colour and a two-way arrow.
- Right-click a card for **Open Request**, **Collapse** / **Expand**, **Copy as
  Mermaid**, **Generate Business Process (AI)** / **Regenerate**, **Reset
  Position** and **Remove Card**. Removing a card also unlinks its endpoint
  from the tables.
- With badges on, click a badge for the endpoint panel: filter, open an
  endpoint's request in the HTTP client, show its card, or unlink it from the
  table. Unlinking the last table of a card that has no steps removes the card.
- Diagram search (Cmd+F) finds endpoints by method or path. With cards, the
  result jumps to the card; with badges only, it jumps to the table.
- **Esc** or a click on empty canvas clears the selection.
- Links to tables that disappear from the schema are removed on the next schema
  sync. Steps that pointed at such a table keep their text. Focus and group tabs
  show the cards that touch their tables, read-only.

## Business processes (AI)

A card can show the business process behind its endpoint: the steps the code
runs in order, such as auth, validation, database reads and writes, calls to
other services, queue messages and the response, with the table, columns and
`path/file:line` of each step.

Start it from the diagram:

- right-click a group title and choose **Generate Business Process (AI)** for
  every card of that group's repository;
- right-click a card and choose **Generate Business Process (AI)**, or
  **Regenerate** when it already has steps;
- press **Regenerate** in the card's Process panel.

The same code preparation and AI backend as endpoint generation are used.
Cards are sent in batches of 6, grouped by source file, and share the limit of
6 AI turns at once with the other jobs. A progress window shows each step; it
has **Cancel**, and **Run in Background** hides it until a notification says the
job is done. When the job ends, the window lists how many cards were generated,
how many were unchanged and which failed.

- Table names in the steps are matched to the diagram. A step whose table is
  not in the diagram keeps its text without a line. New tables found in the
  steps are linked to the endpoint.
- Tabular records the files the AI read and a hash of their content. The next
  run skips cards whose files have not changed; **Regenerate** on a card always
  runs it again.
- API providers see route snippets only, not the whole repository. Their cards
  are labelled **partial**; a CLI agent (Claude Code, Gemini CLI) gives more
  complete processes.
- Without an AI backend, cards still show their tables from the links and say
  **No business process yet**.
- The Mac App Store build cannot start external processes, so generation is not
  available there.

### Playing a process

Double-click a card to focus it: the other cards and unrelated tables fade, the
view zooms to the card and its tables, and the process plays. The request
enters the card, each step lights up in turn, a particle runs along the line
to its table in the direction of the data, and the table's header and columns
pulse. The response leaves the card at the end. Playback stops by itself.

The bar under the card has Play / Pause (**Space**), Previous step, Next step,
speed (0.5×, 1×, 2×), Replay and Close. A card without steps plays one step per
linked table.

The **Process** panel opens for the selected card. It shows the summary, the
commit and time the process was generated from, **Tables involved** with
C / R / U / D badges and step numbers (click to jump to the table), and every
step with its detail, condition and source (click to play from that step).

### Export

- **Copy as Mermaid** on a card copies its process as a Mermaid `flowchart`:
  steps in order, tables as cylinders, arrows in the direction of the data,
  and conditional steps as dotted arrows labelled with the condition.
- **Export → Business processes (Mermaid .md)…** in the diagram toolbar writes
  one Markdown file with a flowchart per card, ordered by path.
- AI agents read the same processes through the MCP tool `describe_diagram`
  ([MCP.md](MCP.md)).

### Compatibility

Cards and steps are saved in the diagram (`flow_cards`). Older Tabular versions
open these diagrams but drop the cards when they save them. The cards come back
from the endpoint links the next time a current version opens the diagram; the
steps need to be generated again.

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
