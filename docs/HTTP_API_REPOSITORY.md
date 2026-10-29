# HTTP API folders linked to a code repository

A folder in the **APIs** sidebar can point at the code of the API it holds.
Tabular then reads that code to create every endpoint as a saved request,
links each endpoint to the database tables it uses in the diagram, and designs
integration tests that can span several APIs.

To create the folders, their repository links and endpoints for a whole repository
(one folder per microservice) in one step, see [REPO_SETUP.md](REPO_SETUP.md).

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
| **API endpoints as** | **Rail**: the API panel beside the diagram; the selected endpoint shows its process card next to its tables. **Badges**: only the blue **API n** badge on each table header. **Both** (default): the API panel and the badges |

Working with cards (only the endpoint selected in the API panel has a card; it
is placed next to its tables and always shows all its steps, at any zoom):

- Select an endpoint in the API panel to show its card and its lines to the
  tables. The view centres on the card at 50% zoom. Clicking the card plays its
  process and never hides it.
- Drag a card to move it. It keeps that place until the diagram tab is closed.
- Line colour follows the operation: blue for reads (arrow from the table),
  green for insert, update and upsert, red for delete (arrow to the table), grey
  when the operation is unknown. A table that is both read and written gets the
  write colour and a two-way arrow.
- Right-click a card for **Open Request**, **Copy as Mermaid**, **Generate
  Business Process (AI)** / **Regenerate** and **Remove Card**. Removing a card also unlinks its endpoint
  from the tables.
- With badges on, click a badge for the endpoint panel: filter, open an
  endpoint's request in the HTTP client, show its card, or unlink it from the
  table. Unlinking the last table of a card that has no steps removes the card.
- Diagram search (Cmd+F) finds endpoints by method or path. With the API panel
  on, the result opens the endpoint's card; with badges only, it jumps to the
  table.
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

## Integration tests

Integration tests live in a tab per project. Click the **Integration Testing**
button in the top bar, next to the project name. The tab opens the project's
most recent suite, or a new empty suite when the project has none yet. A suite
is an ordered list of HTTP steps that pass data to each other.

### Steps, phases and groups

Each step has a **phase**, and the list always shows the execution order:

1. **Setup** steps run first, for example login or master data the tests need.
2. **Test** steps run next, group by group.
3. **Cleanup** steps run last and remove the test data.

**Groups** split a suite into business areas, for example *Purchasing*,
*Plant* and *Sales* in an ERP. Add groups in the left column, rename or reorder
them from their context menu, and assign a step to a group in its details.
Select a group and click **Run Purchasing** to test only that group. Steps the
group depends on run too, even when they belong to another group. For example,
*Plant* needs the purchase order that *Purchasing* creates, so that step runs
as well. These steps are shown in italics under their own heading. Cleanup
runs for every step that created data.

The step list is split into blocks in execution order: **Setup**, **Tests**
per group and **Cleanup**. Each row shows the method, the step name with its
URL, the last result and the response time. Untick the box at the start of a
row to skip that step. The row menu, or a right-click, opens the step in the
HTTP client, moves it within its block, adds a cleanup step for it or deletes
it.

### Data flow between steps

A step passes values to later steps with **Save values for later steps**, for
example `po_id` taken from `json:$.id`. Later steps use the value as
`{{po_id}}` in the URL, headers, body or auth. A link icon next to a step name
shows where its input comes from and which steps use its output. The
**Data flow** section of the step details says the same in full sentences.

**Suite checks** below the step list lists problems before you run. Click it
to open the list, and click a step name to select that step:

- A step uses a variable that only a later step provides, or that nothing provides.
- A step creates data but no cleanup step removes it.
- Cleanup steps remove data in the wrong order. **Auto-order cleanup** in the
  **Settings** menu removes dependent data first, in reverse order of creation.
- A step has no assertions.

### Editing a step

Click a step to open its details on the right. They are ordered from what you
need most to what you need least:

1. **Request**: the method and URL, with **Edit request in HTTP client**.
2. **Last run**: the result, and for a failed step what was expected and what
   came back, the saved values and the response body.
3. **Expected result**: the checks, each as field, condition and value, for
   example `status` *equals* `200`. **Use last response** creates checks from
   the last answer.
4. **Save values for later steps**: the extract rules.
5. **Data flow**: which values the step needs and passes on.
6. **Settings**: phase, group, the step a cleanup step removes data of,
   whether the step runs, and the requirement it verifies.

With no step selected, the right column shows the suite description, the last
run and a short guide. Double-click a step, or use **Edit request in HTTP
client**, to open the request in a new HTTP client tab. Variables are filled
from the suite, the project environment and the last run, so you can send the
request from there. **Save** in that tab writes the request back to the step.

Add steps from the project's API collection with **Add step**. The host is
replaced by the folder's `base_url_<folder>` variable, and auth values are
replaced by variables such as `{{token}}`.

### Generating steps with AI

Click **Generate with AI** in the tab toolbar and describe the flow, for example
*"Purchasing: create vendor, create PO, approve, receive goods"*.

- **Add steps** appends steps to the suite, in the selected group. The AI
  reuses ids that existing steps already extract.
- **Add missing cleanup** adds cleanup steps for data that is not removed yet.
- **Design new suites** creates complete suites from all API endpoints of the
  project.

The AI receives the endpoint catalog (method, path, name, auth type, query
names, tables and redacted body examples, never auth values). It is asked to
use `{{run_id}}` in created names and codes, and to add a cleanup step for
every step that creates data. You can also start from a folder: right-click it
and choose **Generate Integration Tests (AI)…**.

### Validating generated steps

An endpoint catalog can be wrong about field names. For example, a scan may
guess `name` where the API expects `nama`. Generated steps are therefore
checked against the real server before you use them:

1. The suite runs against the project's active environment. Cleanup runs as
   usual, so test data is removed.
2. For each failing step, the AI receives the request, the server's status,
   response headers and body, and the source code of the route. The source code
   is the route line, the handler function and the request body type from the
   repository linked to the API folder. Steps that provide the failing step's
   variables are included too, for example the login step that sets a token
   cookie.
3. The AI fixes the steps and the suite runs again, up to 3 rounds.

This happens automatically after **Add steps**, **Add missing cleanup**,
**Design new suites** and **Generate Integration Tests (AI)…**. You can turn it
off with the checkbox in the AI bar, or run it any time with **Validate & fix**
in the toolbar. The suite is locked while it runs. Validation is refused when
the active environment is Production.

The toolbar shows the result: **Validated**, the number of steps that pass, or
**Changed since validation** after you edit the suite. Steps that still fail
are listed with their reason. They can be real API bugs, or need data or
variables the AI could not create. The AI is told never to weaken an assertion
that reveals a real bug.

Variables for accounts the suite creates itself, such as `test_password`, get a
generated value. Real secrets such as `token` or `api_key` are never filled in
by the AI. A step whose `extract` finds no value fails, because later steps
need that value.

### Step results

| Result | Meaning | What to do |
|---|---|---|
| Passed | The request was sent and every assertion held. | Nothing. |
| Failed | The server answered, but not as the step expects: status, body, or a value to extract. | Fix the step (Validate & fix), or fix the API code when the test is right. |
| Error | The request got no answer: connection refused or timeout. | Start the server or check the URL. |
| Blocked | Not sent, because a value it needs is missing. Usually an earlier step failed and did not extract an id or slug. | Fix the earlier failed step; the blocked steps then run. |
| Skipped | Disabled, or nothing to clean because the data was never created. | Nothing. |

A step can also fail because of an earlier failure. For example, a list that
should contain the created record fails when the create step failed.

When a step uses `Bearer {{token}}` and `token` is extracted from
`cookie:<name>`, Tabular also sends the token as that cookie. APIs that read the
session from a cookie then work without changing every step.

### Fixing the API code with AI

Sometimes the test is right and the API is wrong. Examples are a handler that
returns 500, a missing validation, or a wrong status code. Select a failed step
and click **Fix API code with AI…**. You can also use the button next to
**What did not pass** in the report, or the validation banner, to include every
failed step.

1. Tabular creates a separate git worktree on a new branch
   `tabular/fix-<suite>-<time>`. It contains your current working copy,
   including uncommitted changes, because that is what your server runs. Your
   own branch and files are not touched.
2. The AI receives the failing requests, the server's answers, the route,
   handler and request type code, and the full source of those files. Claude
   Code and Gemini CLI may also read the rest of the repository, read-only.
3. The AI answers with exact search-and-replace edits. Each edit must match
   exactly one place, or the AI is asked once more with the current file
   content.
4. **Review code fix** shows the summary and the diff. Choose:
   - **Apply to working copy** writes the change into your repository
     without committing. Restart the API server and run the suite again.
   - **Keep as branch** commits the change on the `tabular/fix-…` branch
     only, to review or merge later in the Git client.
   - **Run cargo check** (or `go build`, `tsc`, `mvn compile`) builds the
     change in the separate branch first.
   - **Discard** removes the worktree and the branch.

The local repository comes from the API folder's **Folder Repository…**
setting. Tabular never commits to your branch and never pushes.

### Running and cleaning up

**Run all** or **Run <group>** shows the number of steps, the steps that change
data, the cleanup steps, the environment and the target servers before
anything is sent. Running against an environment marked Production needs an
extra confirmation. Tabular first checks that every target server accepts
connections. When one does not, nothing is sent and the run reports which
server to start.

- Steps run strictly in order. A step whose variables are missing, because
  the step that provides them failed, is **Blocked** and not sent.
- A failing Setup step blocks the remaining tests. **Stop after first failure**
  in the **Settings** menu does the same for test steps.
- **Cleanup always runs**, also when tests fail or you press **Stop**. A
  cleanup step only runs when the data it removes was actually created. Network
  errors are retried once. Only **Abort** skips cleanup.
- Data that could not be removed is listed as left behind, with its ids.
- When the project has database connections and the steps are linked to
  tables, Tabular then counts the rows that still contain the run id in those
  tables. The query is read-only.

Runs continue in the background when you switch tabs. They appear in the
Background Tasks panel.

### Report and validation evidence

Switch between **Steps**, **Report** and **History** at the top right of the
tab. The **Report** view shows the last run from top to bottom:

1. The verdict with a one-sentence summary, and **Open full report** and
   **Export**.
2. Four numbers: passed, failed, not sent (blocked) and the pass rate against
   the pass criterion. **More statistics** opens the rest.
3. **Needs attention**: one card per failed or blocked step, with expected
   versus actual values. Click the step name to open it.
4. **All steps**, **By group**, **Test data cleanup**, **Endpoint coverage**
   and **Unstable steps**.

The **History** view lists earlier runs; **Open report** shows one of them.
Statistics use ISTQB terms and follow the ISO/IEC/IEEE 29119-3 test completion
report:

| Item | Meaning |
|---|---|
| Verdict | PASS, PASS WITH DEVIATIONS (blocked steps or test data left behind), FAIL (below the pass criterion, or a setup step failed) or INCOMPLETE (stopped) |
| Passed, Failed, Error, Blocked, Skipped | Failed means assertions failed; Error means no response |
| Pass rate | Passed divided by executed steps |
| Execution rate | Executed steps divided by planned steps, without disabled steps |
| Pass criterion | Minimum pass rate, 100% unless changed in the **Settings** menu |
| Per group | The same counts for each group |
| Response time | Minimum, average, p50, p90, p95 and maximum |
| Endpoint coverage | API endpoints of the project that a step exercises, and the ones without a test |
| Failures per endpoint | Endpoints that failed most |
| Flaky steps | Steps whose result changed between recent runs |

**Open full report** opens an HTML report you can print to PDF. **Export**
saves HTML, Markdown, JUnit XML or JSON. The report contains validation
evidence in the style of GAMP 5:

- who ran it, on which machine and when;
- the SHA-256 fingerprint of the suite definition that was run;
- expected versus actual results for every step;
- deviations and test data cleanup;
- a traceability matrix from step to endpoint, tables and requirement note;
- a review and approval table for signatures.

Tokens, passwords and other secret values are redacted in results and
reports.

### Command line and CI

```bash
tabular test list [--project <name>] [--json]
tabular test run "Procure to pay" --group Purchasing --junit report.xml --report report.html
```

`run` uses the project's active environment, or `--env <name>`. It refuses a
Production environment unless you pass `--allow-production`. The exit code is
0 for PASS and PASS WITH DEVIATIONS, and 1 otherwise.

### Database steps

Some bugs only show in the data: the response is 200 but a balance is wrong,
an async job wrote twice, or a delete left rows behind. A **database step**
(Add step > Database check, `"kind": "db_query"`) runs one read-only `SELECT`
and checks its result.

```json
{ "id": "balance_after", "kind": "db_query", "phase": "test",
  "db": { "connection_var": "db_main", "wait_until_ms": 5000,
          "sql": "SELECT qty_balance FROM kandang_stocks WHERE kandang_project_id = {{project_id}} AND product_id = {{product_id}} AND deleted_at IS NULL" },
  "assertions": [ { "target": "scalar", "op": "approx", "value": "{{balance_before}} + {{qty_sent}}" } ] }
```

- `connection_var` names a project environment variable whose value is the
  connection name (or id), so the same suite checks Dev in Development and
  Staging in Staging. The run checks every referenced connection with
  `SELECT 1` before the first step.
- `{{variables}}` in the SQL are bound as parameters (`?`, `$1`, `@P1`), never
  pasted into the text. A string literal that contains variables
  (`'%{{code}}%'`) becomes one text parameter. Variables inside quoted
  identifiers are refused.
- Only one `SELECT`/`WITH` statement is accepted, whatever the connection's
  access level. MySQL and PostgreSQL run it in a read-only transaction, SQLite
  with `query_only`; results are limited to 1000 rows and the query to 30 s.
- A connection marked Production is refused unless the run allows Production
  (the confirmation in the Run dialog, or `--allow-production`).
- `wait_until_ms` repeats the query every `poll_ms` (default 250) until all
  checks pass, for data written asynchronously after the response.
- Targets: `row_count`, `scalar` (first column of the first row),
  `cell:<row>.<column>` and `column:<column>` (all values, comma separated).
  `extract` works the same, e.g. `{ "var": "balance_before", "from": "scalar" }`.
- Assertion values may use simple arithmetic with spaces around the operator
  (`{{a}} + {{b}}`). `approx` compares numbers with a tolerance of 0.001, or
  the one given as `"100 +- 0.5"`.

### Concurrent steps

To reproduce a double submit or a race, add `concurrency` to an HTTP step:

```json
"concurrency": { "count": 5, "barrier": true, "rounds": 3 },
"assertions": [
  { "target": "count:status=2xx", "op": "equals", "value": "1" },
  { "target": "any:json:$.message", "op": "contains", "value": "already been received" } ]
```

All requests wait at a barrier and are sent at the same moment (`barrier:
false` sends them one after another as fast as possible). `count` is limited
to 2..50 and `rounds` to 20. Checks run over every response of a round, and the
step passes only when every round passes:

| Target | Meaning |
|---|---|
| `count:status=2xx` | number of 2xx responses (also `409`, `4xx`) |
| `count:json:$.status=success` | number of responses with this JSON value |
| `any:<target>` | at least one response matches |
| `all:<target>` or a plain target | every response matches |

`extract` reads the first successful response; the n-th successful response
also sets `{{var#n}}`. A cleanup step linked to a concurrent step runs once for
every record the burst created. The report lists each response per round.

### Data checks (Data Health)

A data check is a query that should return **no rows**; every row is
evidence of broken data. Suites test the code on Staging, data checks guard
the data on Production. Open them with Project > Data Health.

- Each check has a name, the rule it guards, a severity (Critical, Warning,
  Info), a connection variable, the SQL and an optional number of tolerated
  rows. The same read-only gate as database steps applies, so checks are safe
  on Production without `--allow-production`.
- The panel shows the last status, the number of rows and the trend since the
  previous run, opens the query in a new tab to see the rows, and can run all
  checks daily at a set time while Tabular is open (a notice appears when a
  healthy check starts failing).
- Generate with AI turns a business rule into a SQL check using the schema of
  the connection; you review it before it is saved.
- "Add stock checks" adds five MySQL checks from the VOLTUNES stock audit:
  duplicate receipts per document, balance vs transactions, active stock-outs
  of deleted stock-ins, stock-ins used beyond their quantity, and negative
  balances. Adjust them to your schema.

```bash
tabular check list --project VOLTUNES
tabular check seed --project VOLTUNES --preset voltunes-stock
tabular check run --project VOLTUNES --env Production --report health.html --junit health.xml
```

`check run` exits with 1 when a Critical check fails or cannot run.

### Storage

| What | Where |
|---|---|
| Suites | `{app data}/http_tests/<suite id>.json` |
| Run history (last 50 per suite) | `{app data}/http_tests/runs/<suite id>/<run id>.json` |
| Data checks per project | `{app data}/data_checks/<project id>.json` |
| Data check runs (last 100 per project) | `{app data}/data_checks/runs/<project id>/<run id>.json` |

Assertions and extracts use `status`, `time_ms`, `body`, `header:<Name>`,
`cookie:<name>` (a value from `Set-Cookie`) or `json:$.path.to[0].field`. For an
API that returns its token in a cookie, extract it with `cookie:<name>` and send
it with a `Cookie: <name>={{token}}` header. Assertions use `equals`, `not_equals`, `contains`,
`not_contains`, `exists`, `not_exists`, `less_than`, `greater_than` or `matches`
(regular expression).
