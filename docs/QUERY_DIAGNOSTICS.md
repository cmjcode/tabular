# Error checking in the query editor

Tabular checks your query while you type and underlines mistakes before you run it.
Errors get a red wavy underline, warnings a yellow one. A dot (error) or triangle
(warning) appears next to the line number, and a counter in the bottom-right corner of the
editor shows how many problems the query has.

Hover an underline to read the message, the hint and the suggested fix.

## What is checked

| Check | Example | Databases |
|---|---|---|
| Unterminated string, quoted name or block comment | `WHERE name = 'abc` | all SQL |
| Unbalanced parentheses | `COUNT(id FROM t` | all SQL |
| Stray or duplicate commas | `SELECT a, b, FROM t` | all SQL |
| Misspelled statement or clause keyword | `SELEC`, `FORM users`, `ODER BY` | all SQL |
| Syntax errors from the SQL parser for your dialect | `SELECT a FROM t WHERE` | PostgreSQL, MySQL, SQLite, SQL Server |
| Table or view does not exist | `FROM custmers` | all SQL |
| Column does not exist (qualified or not) | `o.totl`, `SELECT nmae FROM users` | all SQL |
| Unknown table alias | `x.id` with no `x` in FROM | all SQL |
| Table name used although it has an alias | `users.id` in `FROM users u` | all SQL |
| Ambiguous column | `id` when two joined tables have `id` | all SQL |
| Wrong letter case for a name created with quotes | `username` for `"userName"` | PostgreSQL |
| INSERT value count does not match the columns | `INSERT INTO t (a, b) VALUES (1)` | all SQL |
| Column listed twice in INSERT | `INSERT INTO t (id, id)` | all SQL |
| UNION/INTERSECT/EXCEPT branches return different column counts | | all SQL |
| Errors found by the server (types, functions, GROUP BY, permissions, …) | see below | PostgreSQL, MySQL, SQLite, SQL Server |
| Error from the last run, marked where it happened | | all |
| Invalid JSON (line and column), unknown query field, unknown `$operator`, unknown collection | `{"$mtch": …}` | MongoDB |
| Unknown command, command the editor does not run, wrong number of arguments | `GTE key`, `GET a b` | Redis |

Table and column checks use the schema Tabular already cached for the connection (the same
data autocomplete uses). If a table's columns are not cached yet, Tabular loads them in the
background and checks again. Nothing is marked while the schema is unknown, so a fresh
connection shows syntax problems first and schema problems a moment later.

Objects created earlier in the same script are understood: after
`CREATE TEMP TABLE scratch (...)` or `ALTER TABLE users ADD COLUMN nickname text`, later
statements that use them are not marked. CTEs, subqueries (including correlated ones),
derived tables, table functions such as `generate_series()`, temporary tables (`#tmp`) and
tables in other schemas or databases are not checked column by column.

## Confirming with the server

When the connection is open, Tabular also asks the server to prepare each changed
statement **without running it**:

- **PostgreSQL**: the statement is parsed and analysed inside a `READ ONLY` transaction
  with a short lock timeout, then rolled back.
- **MySQL**: the statement is prepared (`COM_STMT_PREPARE`) in the tab's database.
- **SQLite**: the statement is prepared and never stepped.
- **SQL Server**: the statement is compiled with `SET NOEXEC ON`. SQL Server does not
  report missing tables at compile time, so for SQL Server Tabular's own table check stays
  active.

The server's message replaces the local one and is shown at the token it points to. If the
server accepts a statement, local warnings for it are dropped, so dialect features that
Tabular's parser does not know are never marked. While the server is checking, uncertain
local marks wait for its answer instead of flickering.

Only idle pool connections are used. If all connections are busy with your queries,
the check waits and tries again later; it never delays a query you run.

Statements are not sent to the server when the text uses `{{KEY}}` or `${var}`
placeholders, when an earlier statement in the script creates or alters objects, or when
the statement is larger than 64 KB.

## Quick fixes

Many problems come with a suggestion, for example the closest column name. Apply it with
**Edit > Quick Fix** (Cmd+. / Ctrl+.), from the editor's right-click menu, or with the
buttons in the Problems list.

## Problems list and navigation

- Click the counter in the bottom-right corner of the editor, or use
  **Edit > Show Problems**, to list every problem with its line and column. Click a row to
  move the cursor there.
- **Edit > Next Problem** (F8) and **Edit > Previous Problem** (Shift+F8) move between
  problems.
- **Go to error** in the result area uses the same location as the error mark.

## Settings

**Preferences > Editor > Error checking**

- **Check queries while typing**: turns all marks on or off. Also available as
  **Edit > Toggle Error Checking While Typing**.
- **Confirm with the server**: *Except Production connections* (default), *All
  connections*, or *Never*. Connections are Production when their environment label says
  so.

Texts larger than 512 KB are not checked.
