# Import, export and transfer

This page covers everything that moves data in or out of Tabular: opening files as tables,
exporting results, importing into a table, copying tables between connections, comparing data,
and the extra backup engines.

| Feature | Where to find it |
|---|---|
| Open a data file as a table | Settings menu > **Open Data File...**, command palette **Data: Open Data File**, or drop the file on the window |
| Export a result with options | Result grid context menu > **Export with Options...**, or **Data: Export with Options** |
| Import into a table | Table context menu > **Import Data (CSV, JSON, Excel)...** |
| Transfer tables | Table or database context menu > **Transfer To...**, or **Data: Transfer Tables** |
| Export objects as SQL | Table or database context menu > **Export as SQL...** / **Export Objects as SQL...** |
| Compare data | Table or database context menu > **Compare Data...**, or **Data: Compare Data** |
| Decrypt an exported file | Settings menu > **Decrypt Exported File...** |
| MongoDB / SQL Server backup | Database context menu > **Backup Database...** / **Restore Database...** |

## Data Files

**Open Data File** loads a file into a local SQLite workspace and opens it as a table. Nothing is
written to your own databases. The workspace appears in the sidebar as a connection named
**Data Files**, so the grid, filters, charts, exports and the SQL editor all work on it, and you
can join several files in one query.

- Formats: CSV, TSV and other delimited text, JSON, NDJSON, Excel (`.xlsx`, `.xlsm`, `.xlsb`,
  `.xls`), OpenDocument (`.ods`), and Parquet.
- Wrappers: `.gz`, `.zip` (the first entry with a known format) and `.zst`, plus encrypted
  `.enc` exports made by Tabular. Encrypted files ask for the passphrase.
- Each file becomes one table named after the file. A workbook with several sheets becomes one
  table per sheet (`file_sheet`).
- Column types are inferred from the content: whole numbers, decimals, booleans, dates and
  timestamps; everything else is text. Values with leading zeros such as `00123` stay text.
- Opening the same file again reloads its table. A different file with the same name gets a
  numbered table (`sales_2`). A table you created yourself in the workspace is never replaced.
- The workspace is `data_files.sqlite` in the Tabular data directory. `data_files.json` next to
  it records which file each table came from.

The file is copied into the workspace when you open it; later edits to the file are picked up
only when you open it again. The whole file is read into memory while loading.

Size and `NULL` rules that apply to every file reader (Data Files and the import wizard):

- A file may hold at most 4 GiB of content after decompression. A `.gz`, `.zst` or `.zip` that
  expands beyond that is rejected with an error instead of exhausting memory; decompress or
  split it first.
- Previews of plain (not compressed, not encrypted) CSV/TSV and NDJSON files read only the
  beginning of the file, so opening the wizard on a multi-gigabyte file is immediate. Other
  formats are still read completely for a preview.
- In delimited files every cell is text. An empty cell is an empty string and a cell containing
  the four letters `NULL` is the string `NULL`; neither is SQL `NULL` unless you ask for it (see
  the import wizard below). A column missing from a short row, JSON `null`, an empty spreadsheet
  cell and a Parquet null are SQL `NULL`. Data Files therefore loads a CSV column that contains
  the text `NULL` as a text column.

## Exporting a result

**Export with Options** exports the rows currently loaded in the grid.

| Format | Notes |
|---|---|
| CSV, TSV | |
| JSON, NDJSON | Numbers and `NULL` keep their JSON types |
| Markdown | Pipe table |
| HTML | Standalone page with one table |
| XML | `<table><row><field name="col">value</field></row></table>`; `NULL` is `null="true"` |
| Excel (XLSX) | |
| Parquet | Snappy compression; columns are INT64, DOUBLE, BOOLEAN or string, all nullable |
| SQL INSERT | Multi-row statements; numeric columns are written unquoted |

Options:

- **Encoding** for text formats: UTF-8, UTF-16 LE, UTF-16 BE or Windows-1252, with an optional
  byte order mark. Characters that Windows-1252 cannot represent are written as `?`; the export
  reports how many were replaced.
- **Max INSERT size** for SQL INSERT: a size in KB and a row count. A new statement starts when
  either limit is reached. SQL Server is always capped at 1000 rows per statement.
- **Encrypt with a passphrase**: see [Encrypted exports](#encrypted-exports).

The quick export items in the grid menu (CSV, XLSX, JSON, Markdown, SQL) are unchanged.

## Importing into a table

The import wizard accepts the same formats and wrappers as Data Files. It detects the format,
the delimiter and the text encoding; you can override the encoding (UTF-8, UTF-16 LE/BE,
Windows-1252), pick the sheet of a workbook, and enter the passphrase of an encrypted file.
JSON `null`, empty spreadsheet cells and Parquet nulls are imported as `NULL`.

**NULL representation** decides which text becomes SQL `NULL`. Left empty (the default), empty
cells are imported as `NULL` and the text `NULL` is imported as the string `NULL`. Type `NULL`
(or `\N`, or any other marker your file uses) to import that text as SQL `NULL` instead; empty
cells are then imported as empty strings.

The file is read and the `INSERT` statements are built in the background; the dialog shows the
progress and stays responsive. On MySQL, PostgreSQL, SQLite and SQL Server the whole import runs
in one transaction: if any row is rejected, nothing is imported. MySQL tables on a
non-transactional engine (MyISAM) cannot be rolled back. Plugin engines have no transaction
path and still import statement by statement, so a failure can leave earlier rows in the table.

## Transfer Tables

Copies rows directly from one connection to another, without an intermediate file. Source and
target can be different engines (MySQL, PostgreSQL, SQLite, SQL Server).

- Select one or more tables. With a single table you can give the target table another name.
- **Create target table if it does not exist** builds the table from the source columns and
  primary key.
- **Delete all rows in the target table first** empties each target table before copying.
- **Row filter** is a `WHERE` condition applied to every selected source table; **Row limit**
  caps the rows copied per table.
- Columns are matched by name. Source columns that the target does not have are skipped and
  listed in the log.

Across engines the column types are approximated:

| Source type | PostgreSQL | MySQL | SQL Server | SQLite |
|---|---|---|---|---|
| boolean, `tinyint(1)`, `bit` | `BOOLEAN` | `TINYINT(1)` | `BIT` | `INTEGER` |
| integers | `SMALLINT` / `INTEGER` / `BIGINT` | `SMALLINT` / `INT` / `BIGINT` | same | `INTEGER` |
| `decimal(p,s)` | `NUMERIC(p,s)` | `DECIMAL(p,s)` | `DECIMAL(p,s)` | `NUMERIC` |
| `varchar(n)` | `VARCHAR(n)` | `VARCHAR(n)` | `NVARCHAR(n)` | `TEXT` |
| long text, enum, unknown types | `TEXT` | `LONGTEXT` | `NVARCHAR(MAX)` | `TEXT` |
| `date` | `DATE` | `DATE` | `DATE` | `TEXT` |
| timestamp, datetime | `TIMESTAMP` / `TIMESTAMPTZ` | `DATETIME(6)` | `DATETIME2` / `DATETIMEOFFSET` | `TEXT` |
| binary | `BYTEA` | `LONGBLOB` | `VARBINARY(MAX)` | `BLOB` |
| JSON | `JSONB` | `JSON` | `NVARCHAR(MAX)` | `TEXT` |
| UUID | `UUID` | `CHAR(36)` | `UNIQUEIDENTIFIER` | `TEXT` |

Limits to know about:

- Only columns and the primary key are created. Defaults, auto-increment, indexes, foreign keys
  and triggers are not copied; use **Export Objects as SQL** for a same-engine structure copy.
- A transfer is not one transaction, but each page is: if it stops halfway, the pages already
  written stay in the target and the failing page is rolled back completely.
- Rows are read in pages with keyset pagination (`WHERE key > last ORDER BY key`) when the
  source table has a primary key of integer, decimal, text or UUID columns. The cost of a page
  does not grow with its position, and existing rows are neither skipped nor read twice when
  the table changes during the transfer.
- A primary key of another type (float, timestamp, binary; on SQL Server also `varchar`) is read
  with `OFFSET` pages ordered by that key.
- A table without a primary key is read with `OFFSET` pages ordered by all sortable columns.
  The transfer log shows a `WARNING` for it: rows can be duplicated or missing if the table
  changes while it is being read. **Export Objects as SQL** reads table data the same way and
  writes the warning as a comment into the script.
- Values are copied as text literals. SQL `NULL` and a text value that is exactly `NULL` are
  kept apart: nullable text columns are read together with a `NULL` indicator.

## Export Objects as SQL

Builds one `.sql` script from the objects you tick in the tree: types, tables, views,
materialized views, functions, procedures, triggers, events and privileges. Which kinds are
listed depends on the engine.

- **Structure** and **Table data** can be exported together or separately.
- **Write indexes and foreign keys after the data** moves secondary indexes and foreign keys
  behind the `INSERT` statements, so a reload does not maintain indexes row by row. With the
  option off they follow the `CREATE TABLE` statements, after all tables exist.
- **Max INSERT size** and **Rows per table** work as in the result export.
- The script can be encrypted with a passphrase.

Engine notes:

- MySQL: table DDL comes from `SHOW CREATE TABLE`; routines are wrapped in `DELIMITER ;;`.
- PostgreSQL: table DDL is rebuilt from the catalog (columns, defaults, identity, generated
  columns, constraints, indexes). Sequences used by defaults are created with
  `CREATE SEQUENCE IF NOT EXISTS`; their current values are not exported.
- SQL Server: table DDL contains columns and the primary key only; defaults, identity and
  check constraints are missing. Routines end with `GO`.
- SQLite: DDL comes from `sqlite_master`.
- An object whose source cannot be read is skipped with a comment in the script.

## Compare Data

Compares two tables row by row and builds a script that makes the target match the source.

1. Choose the connection, database and table on both sides. Picking a table on one side selects
   the table with the same name on the other.
2. Rows are matched on the source primary key. Enter **Key columns** when the table has none or
   to match on something else.
3. Optional: a `WHERE` filter for both sides, a row limit per side (100,000 by default), and
   columns to ignore. Clearing the row limit reads both tables completely into memory.
4. **Compare** lists the differences: rows only in the source, rows only in the target, and
   changed rows with the differing cells marked.

### Comparing whole databases

Leave **Table** on **All tables** on both sides to compare every table of the two databases.
Tables are paired by name, ignoring case; a schema prefix is ignored when the name is otherwise
unambiguous, so `dbo.orders` pairs with `orders`. Each pair is compared with the same options,
on its own primary key unless **Key columns** is filled in.

The result lists every table with its counts. Tables that exist on one side only are reported
as missing and are not compared; a table that cannot be compared (for example no primary key)
shows its error and does not stop the others. Click a table to see its rows and its sync
script, or **All tables** for one script covering every table. That script follows table name
order, so foreign keys may require reordering it.

With **Ignore number/boolean/timestamp formatting** on, `1.0` equals `1`, `t` equals `1`, and
`2026-01-02T03:04:05.000` equals `2026-01-02 03:04:05`, so two engines that print the same value
differently do not show up as changed.

The sync script can insert missing rows, update changed rows and, when ticked, delete rows that
exist only in the target. Open it in the editor to review it, copy it, or apply it directly; the
comparison runs again after applying.

Warnings shown with the result:

- **Row limit reached**: only part of the data was read, so rows may be reported as missing.
- **Rows share a key**: the key columns do not identify rows uniquely; those rows were skipped.

SQL `NULL` and the text `NULL` are different values in a comparison and in the sync script.

**Saved comparisons** store both endpoints and the options under a name, in the local
`connections.db`. They refer to connections by id and are not synced.

### Comparing structure

Switch the mode picker at the top of the dialog to **Structure** to compare column definitions
instead of table data:

1. Columns that exist only in the source, only in the target, or have differing types, nullability,
   or primary key status are reported in a color-coded grid.
2. When comparing across different database engines, types are normalized logically (e.g. `int4` in
   PostgreSQL and `INT` in MySQL match), avoiding false positives.
3. Options in Structure mode: **Ignore columns**, **Ignore column name case**, and **Compare nullability**.
4. In whole database comparison (**All tables**), the summary shows column difference counts per table,
   and tables missing on either side.
5. The generated DDL sync script can add missing columns, alter changed columns, and (when ticked)
   drop columns existing only in the target. For whole-database comparisons, it also generates
   `CREATE TABLE` statements for tables missing in the target.
6. Engine-specific limitations (such as SQLite lacking support for altering column types) are emitted
   as `-- manual:` comment hints and skipped during execution.

## MongoDB and SQL Server backup

The backup and restore dialogs also drive the vendor tools:

| Engine | Tool | Formats |
|---|---|---|
| MongoDB | `mongodump`, `mongorestore` (MongoDB Database Tools) | `.archive.gz`, `.archive` |
| SQL Server | `sqlpackage` (`dotnet tool install -g microsoft.sqlpackage`) | `.bacpac` (schema and data), `.dacpac` (schema only) |

- The tools are searched in `PATH`, common install locations and `~/.dotnet/tools`; a custom
  path can be set in the dialog.
- The MongoDB password is passed through a temporary config file readable only by you and
  removed afterwards. `sqlpackage` only accepts the password as a command line argument, so it
  is visible to other processes of the same user while the backup runs.
- Restoring a MongoDB archive into a database with another name remaps all namespaces to it.
- A `.bacpac` must be imported into a database that does not exist yet or is empty.
- The tools connect to the host and port of the connection; SSH tunnels are not used.

## Encrypted exports

Encrypted exports use AES-256-GCM with a key derived from the passphrase by Argon2id. The file
gets an `.enc` suffix and this layout:

```text
"TABENC01" (8 bytes) | salt (16) | nonce (12) | ciphertext + GCM tag (16)
```

The header is authenticated, so a modified file fails to decrypt. There is no recovery without
the passphrase. Tabular opens `.enc` files directly in Data Files and in the import wizard, and
**Decrypt Exported File** writes the original file back.

## Export plugins (Wasm)

Export and code-generation plugins run in the sandboxed Wasm runtime (`src/plugin_runtime/`,
see `docs/PLUGIN_DEVELOPMENT.md`). Each run has a budget:

- CPU: 10,000,000,000 instructions ("fuel"), refilled before the module is instantiated and
  before its entry point is called. A module that loops forever stops with an "out of fuel"
  error.
- Memory: 256 MiB of linear memory. `memory.grow` beyond that returns `-1`; a module that
  declares more initial memory is rejected.
- One instance, one memory and at most four tables per run.

Plugins run on a worker thread. The dialog shows a busy indicator and stays usable while a
plugin runs; the result or the error appears when it finishes.

## For contributors

Everything headless lives in `src/data_transfer/` and takes plain data (`ConnectionConfig`,
pools, `TableData`); the GUI is `src/window_egui/transfer_ui.rs`, `transfer_dialogs.rs` and
`transfer_compare_ui.rs`.

| Module | Purpose |
|---|---|
| `formats.rs`, `encoding.rs`, `encrypt.rs` | Export formats, text encodings, encrypted files |
| `readers.rs` | File readers (delimited, JSON, spreadsheets, Parquet, compression) |
| `values.rs`, `types.rs` | SQL literals, `INSERT` batching, type inference and cross-engine type mapping |
| `catalog.rs` | `Endpoint` (connection + database), catalog queries, `TablePager` (keyset / `OFFSET` paging), `execute_atomic` |
| `transfer.rs`, `object_export.rs` | Table transfer and SQL object export |
| `compare.rs`, `saved.rs` | Data compare, sync script, saved comparisons |
| `data_files.rs` | The Data Files workspace |

Nullness: file readers and `TablePager` produce cells as `Option<String>`
(`TableData::from_cells`, `TableData::value`). Rows that come straight from the query executor
or the result grid are plain strings where SQL `NULL` and the text `NULL` look the same; that
one conversion is `data_transfer::cell_from_executor`, and exporting a result grid still goes
through it.

```bash
cargo test --lib data_transfer::
cargo test --lib plugin_runtime::
cargo test --test sql_parser_robustness
```
