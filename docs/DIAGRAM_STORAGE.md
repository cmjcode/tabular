# Diagram storage

A database diagram (table positions, groups, virtual relations, notes, flow
cards, linked databases and endpoints) is stored **in the database itself**, in
the table `diagram_by_tabular`. Everyone who connects to that database sees the
same diagram. MySQL, PostgreSQL, SQLite and SQL Server are supported.

Tabular also keeps a local copy in `<data dir>/diagrams/` so a diagram opens
instantly and keeps working while the database is unreachable. The local copy
is a cache; the database is the source of truth.

Diagrams are no longer synced to Tabular Cloud.

## Saving

- **Save** (Cmd S) writes the diagram to `diagram_by_tabular` right away.
- With **Auto save** on, changes are written to the database about 3 seconds
  after you stop editing.
- The first save creates the table. An older table without the `revision`
  column is upgraded with `ALTER TABLE` on the next save.
- When the database cannot be reached, or your database user cannot write to
  it, the diagram is saved locally only. The toolbar shows a warning icon; click
  it or press Save to try again.

The Save menu shows the current state, for example
`Saved in database · revision 12 · by alice · 2026-09-30 10:42`.

## Opening a diagram and merging

Every save increases the row's `revision`. Tabular remembers the last revision
it read (`conn_<id>_<db>.base.json` next to the local copy) and compares three
versions when the diagram is opened, when a save finds a newer revision, or when
you pick **Sync with database now**:

| Your copy | Database | Result |
|---|---|---|
| unchanged | unchanged | Nothing to do |
| unchanged | changed | The database version is loaded |
| changed | unchanged | Your changes are saved to the database |
| changed | changed | Changes to different items are merged automatically |

When both sides changed **the same item** (the same table moved to different
places, a group renamed on one side and deleted on the other, and so on), the
**Merge** window lists those items. Pick **Mine** or **Database** for each one, or
use **Keep all mine** / **Take all from database**, then **Apply merge**. The
result is saved to the database.

Closing the window postpones the merge. Your edits stay local, and saving to
the database is paused until you resolve it with the merge button in the
diagram toolbar.

Pan, zoom and display toggles (grid, relations, notes visibility) are personal
and never cause a merge. Column lists come from the live schema, so they never
conflict either.
