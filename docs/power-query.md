# Queries (Get & Transform)

> **Last reviewed:** 2026-10-17 · **Last updated:** 2026-10-17 · **Change:** minor (new sources and visual query transformations) · **Target:** Microsoft Excel (Microsoft 365)

A query is a named formula in the Power Query M language that produces a table. GridCraft keeps
queries in the workbook, runs them, and loads their results into tables on sheets. This page says
what is there, how it behaves and what is not there yet. The gap entry is
[gaps.md §12](gaps.md#12-get--transform-external-data-data-model-f).

## What is there

| Part | Where | State |
|---|---|---|
| The M language: lexer, parser, evaluator | `crates/mquery` | Complete to the public language specification, except metadata (`meta` keeps the value and drops the metadata) and query folding (everything is evaluated here, eagerly) |
| The standard library | `crates/mquery/src/stdlib` | 431 functions and 92 constants: `Table.*` (78) and `#table`, `List.*` (67), `Text.*`, `Number.*`, `Date.*`/`Time.*`/`DateTime.*`/`Duration.*`, `Record.*`, `Splitter.*`, `Combiner.*`, `Comparer.*`, `Replacer.*`, `Value.*`, `Type.*`, `Csv.Document`, `Json.Document`, `Excel.CurrentWorkbook`, `Excel.Workbook`, `File.Contents`, `Sql.Database`, `Sql.Databases`, `PostgreSQL.Database`, `MySQL.Database`, `SQLite.Database`, `Value.NativeQuery`, `Folder.Files`, `Web.Contents`, `Web.Page`, `Xml.Document`, `Xml.Tables`, `OData.Feed`, text encodings and binary values |
| Databases | `crates/db`, `crates/engine/src/datasource.rs`, `crates/engine/src/cmd/database.rs` | SQL Server, PostgreSQL, MySQL/MariaDB and SQLite, with a login per server kept for the session; see [Databases](#databases) |
| Queries in the model | `Workbook::queries` (`crates/model`) | Name, formula, description, the table it loads to |
| Commands | `crates/engine/src/cmd/query.rs`, `cmd/database.rs` | `data.query.add`, `.set`, `.delete`, `.load`, `.refresh`, `.list`, `.preview`, `.parse`, `.functions`, `.addStep`, `.removeStep`, `.renameStep`; `data.source.formula` builds source formulas and `data.web.approve` grants session access to a site; `data.refreshAll` refreshes queries, then PivotTables; `data.fromDatabase`, `data.database.connect`, `.preview`, `.approve`, `.needs`, `data.credentials.set`, `.clear`, `.list` |
| Interface | `crates/ui-egui/src/query_editor.rs`, `database_dialog.rs`, `panes.rs`, `ribbon.rs` | Data tab: From Database, From Table/Range, New Query, Refresh All, Queries & Connections. From Database: type, host, port, database, login, password, encryption; the tables to pick from with a preview, or SQL to run. The Query Editor: name, description, applied steps, the formula, a preview of the result or of one step. The Queries & Connections pane: each query with where it loads, Edit, Refresh or Load, Delete |
| In the file | `crates/xlsx/src/mashup.rs` | Read and written as the Data Mashup custom XML part ([MS-QDEFF]), the part Excel keeps its queries in |

In the interface, Data › From Database makes a query that reads a table of a database or runs
SQL on it, Data › From Table/Range opens the Query Editor on a query that reads the table at the
selection, New Query on an example, and the Queries & Connections pane lists the queries.
The editor adds transformations as M steps and lets the user rename or remove a step. Data ›
More Sources opens folders, XML, web pages, JSON from the web and OData feeds in the editor.
Nested tables have an Open Table link. Get Data (Text/CSV) still imports once, without making a query. Everything the interface does is also a command
(CLI, MCP, control channel, scripts).

## How it behaves

- **Loading.** `data.query.add` runs the formula and writes the result as a table: a header row,
  then the rows. By default on a new sheet named after the query; with `at`, at a cell of an
  existing sheet. A list loads as one column, a record as names and values. `load: false` keeps a
  connection only, for other queries to use.
- **Refreshing** runs the query again and replaces the table in place. The table grows or
  shrinks; cells it no longer covers are cleared; a totals row moves with it. A calculated column
  the user added to the table stays after the query's columns, its formula on every new row (as
  in Excel); other columns the query no longer gives are cleared. If the cells a larger result
  needs are not empty, the refresh fails and nothing changes. Nothing refreshes on its own,
  including when a workbook is opened.
- **One undo step** undoes a load or a refresh entirely (sheet, table, query).
- **Types.** Dates, times, date-times and durations load as numbers with a number format;
  `Percentage.Type` columns get a percent format. A cell the user already formatted as a date
  keeps its format. Lists, records and tables nested in a cell show as `[List]`, `[Record]`,
  `[Table]`. A cell whose value was an error in the query loads empty and is counted in the
  command's `errors`.
- **Reading the workbook.** `Excel.CurrentWorkbook()` lists the workbook's tables and the defined
  names that are plain ranges. A number in a cell formatted as a date arrives as a `date`, `time`
  or `datetime`.
- **Cultures.** Functions that take a culture (`Number.From`, `Date.From`,
  `Table.TransformColumnTypes`, `Date.ToText`…) know en-US, en-GB, en-CA, en-AU, pt-BR, pt-PT,
  es-ES, es-MX, fr-FR, de-DE, it-IT, ja-JP, zh-CN and sv-SE (number separators, date order, month
  and day names where the language is covered). Others fall back to their language, then to en-US.

## What a query can reach

The evaluator does no I/O. The engine's host (`crates/engine/src/query.rs`) grants:

| Function | Reaches | Web build |
|---|---|---|
| `Excel.CurrentWorkbook` | Tables and named ranges of the same workbook | yes |
| `File.Contents` | A local file, by path | no |
| `Excel.Workbook` | The sheets, tables and names of workbook bytes (XLSX, XLSB, ODS) | yes |
| `DateTime.LocalNow` and relatives | The system clock, in UTC (like `NOW()`) | yes |
| `Sql.Database` and the other database functions | A database server the user gave a login for in this session; a SQLite file, read-only | no |
| `Folder.Files` | Files recursively under a local folder; binary content is read only on access | no |
| `Web.Contents`, `OData.Feed` | HTTP(S) sites the user approved in this session | no |
| `Xml.Document`, `Xml.Tables`, `Web.Page` | XML or HTML already supplied as text or binary | yes |

A query runs only when a command asks for it, so opening a workbook reads no files and
connects to no database.

## Databases

The database browser opens as a larger, resizable window that adapts to the screen. Once
connected, the connection fields collapse into a summary with Edit Connection; reopening the
settings requires reconnecting before loading a source. The object list occupies the full-height
left sidebar, with Tables / Views tabs, counts and a search by name or schema. Filtering the list
preserves the selected source and SQL; reopening a saved view selects its Views tab.
The framed preview occupies the right side. SQL starts hidden, even for a saved SQL source;
Edit SQL reveals its textbox above the data, and Hide SQL reclaims the preview space without
clearing the query or changing the sidebar height. A saved or typed SQL query can still be
previewed and saved while its editor is hidden. Column headers stay on one line. Load/Save,
Query Editor and Cancel remain
at the bottom; Query Editor opens the selected source for further transformations, reusing an
existing query when editing one.

While typing SQL, the previous preview stays visible. A single simple SELECT is previewed
automatically after 800 ms without edits; each keystroke restarts that delay. A failed preview
shows its error while keeping the last successful table. Changing the table or reconnecting
cancels a pending SQL preview; Preview runs immediately and cancels its pending timer.
Scripts, functions, comments, parameters, SELECT INTO and modification statements require an
explicit action instead of typing-triggered execution (`automatic:true` on
`data.database.preview` returns `manualRequired:true` without connecting for these queries).
This conservative check is not a security boundary: source permissions still apply, and even
a SELECT of a view may invoke database code. SQL Server, PostgreSQL and MySQL do not currently
enforce read-only sessions; use a database login with only the required read permissions.

Edit in Queries & Connections opens this same browser for database queries and restores the
connection, table or SQL. Save updates the existing query and its loaded table, preserving its
name, description, destination and downstream M steps; Cancel discards the changes. SQL can be
edited directly without writing M. Literal sources using the four database connectors and
`Value.NativeQuery` are recognized, including table navigation. Dynamic, parameterized or
ambiguous sources remain in the advanced M editor. Source inspection and draft rewriting
(`data.database.source`, `data.database.formula`) do not connect or execute SQL. Opening an
existing SQL source lists tables but waits for editing, Preview or Save before running its SQL.

`Sql.Database(server, database)`, `PostgreSQL.Database`, `MySQL.Database` and `SQLite.Database(path)`
(GridCraft's own: Power Query reaches SQLite only through ODBC drivers) return a navigation table:
`Name` (`schema.table`), `Data`, `Schema`, `Item`, `Kind` (`Table` or `View`). A table's rows are
fetched only when its `Data` is read, so `Source{[Schema="dbo",Item="Orders"]}[Data]` reads that
table and nothing else. `[HierarchicalNavigation=true]` nests the tables under their schemas;
`Sql.Databases(server)` lists the databases of a server. SQL runs through the `Query` option or
`Value.NativeQuery(database, sql)`; parameters of native queries aren't supported yet. Servers are
written as Power Query writes them: `host,port` for SQL Server (`host:port` too), `host:port` for
the others.

- **Logins.** A query names the server and the database, never the login. The user gives a login
  (user name and password) per server in Data › From Database, or with `data.credentials.set`; it
  is kept by the session: never written to the workbook, the journal or any file of GridCraft's,
  and never shown in a command result. With **Remember** (`remember: true`) the desktop app keeps it
  in the system's credential store (Windows Credential Manager, the macOS keychain, the Secret
  Service on Linux and the BSDs: GNOME Keyring, KWallet) and the next session starts with it;
  `data.credentials.clear` forgets it there too. Without a store (the web, the CLI, Linux without a
  Secret Service) logins last the session. A workbook from elsewhere reaches only servers the user gave a login
  for. When a refresh or a preview finds no login (or the server refuses it), the interface asks
  for one and runs the command again; `data.database.needs` tells scripts what was missing.
- **Native queries.** SQL can change a database, so SQL written in a query runs only once the
  user approved that exact text for that database (`data.database.approve`; the interface asks,
  as Excel does). SQL typed in From Database is approved by the call that makes the query.
  Reading tables through the navigation table needs no approval. SQLite files are opened
  read-only: no query changes one, and a missing file isn't created.
- **Encryption.** Connections are encrypted (TLS) unless turned off; the server's certificate is
  checked unless the user chooses to trust it (servers with a certificate of their own). With
  encryption off, SQL Server still encrypts the login, as its own clients do.
- **Types.** Integers load as `Int64.Type`, `decimal`/`numeric` as `Decimal.Type`, `money` as
  `Currency.Type`, floating point as numbers, dates, times, date-times, date-times with offset and
  intervals as M's own types, binary columns as binary values. Text that a type can't read (a
  PostgreSQL array, an `infinity` date) stays text. MySQL `TINYINT(1)` reads as logical and `TIME`
  as a duration; SQLite columns go by their declared type, ISO text in a `DATE` column reading as a
  date.
- **Limits.** Reads stop with an error past the evaluator's caps (rows, cells, bytes); a statement
  may run ten minutes, a connection attempt fifteen seconds. Off the web only: the web build
  refuses every database.

Tested against SQL Server 2022, PostgreSQL 17 and MySQL 8.4 in containers (2026-10-15):
`crates/db/src/tests.rs` and `crates/engine/src/cmd/database.rs` read seeded servers when
`GRIDCRAFT_TEST_SQLSERVER`, `GRIDCRAFT_TEST_POSTGRESQL` and `GRIDCRAFT_TEST_MYSQL` name them
(`host=…;port=…;user=…;password=…;database=…`, plus `encrypt=0` or `trust=1`), seeded with
`crates/db/tests/seed/*.sql`; without them those tests pass untested. SQLite is always tested.

## Additional sources

`Folder.Files(path)` returns Content, Name, Extension, access/modification/creation dates,
Attributes and Folder Path. Paths use the platform separator (including `/` on Linux) and are
shown as people write them: on Windows without the `\\?\` prefix of a resolved path. Listing
is bounded by 100,000 files/directories and 64 directory levels; symlinks and special files are
skipped. A failure to list an entry is an error, rather than an incomplete successful result.

`Xml.Document` keeps element names, namespaces, attributes and nested content; `Xml.Tables`
groups repeated child elements into tables. XML nesting and text are bounded, and DOCTYPEs
are refused. `Web.Page` reads HTML tables, including row/column spans and common entities.
These are limited table readers, rather than a browser or a complete XML schema importer.

`Web.Contents(url, [RelativePath, Query, Headers, Timeout])` returns bytes. Timeout is a positive
M duration, defaults to 100 seconds, and is capped at one hour. Responses are limited to 256 MiB
(and the evaluator's own cap). A site typed in Get Data › From Web (or JSON / OData) is approved
by that choice, as SQL typed in From Database is; a site a query names on its own asks first.
Redirects are limited to ten and each target needs approval;
HTTPS cannot redirect to HTTP, and credentials are not forwarded to another origin. URLs with
embedded logins are refused. Only GET is supported: POST, caching, API-key credentials and
response metadata remain unsupported. The options follow the public
[Web.Contents reference](https://learn.microsoft.com/en-us/powerquery-m/web-contents).

`OData.Feed` reads JSON feeds (v4 `value`, v2 `d.results`), follows relative or absolute next-page
links, and returns service entity sets as lazy tables. Pagination is capped at 10,000 pages and
the evaluator's row/cell limits; repeated links fail. Entity and next-page links must stay on
the same origin to avoid forwarding supplied headers to another site. OData metadata/schema
negotiation and authenticated credential management are not implemented.

## Visual transformations

Select a preview column with a click, or several with Ctrl/Command-click. The toolbar can remove
or keep columns, change types, filter and sort rows, rename columns, group with count/sum/average/
minimum/maximum, remove duplicates, replace values, split a column into two, add a custom column,
keep the first N rows, promote headers, remove blank rows, fill down and unpivot other columns.
The controls and source dialogs have Brazilian Portuguese translations.

Every transformation appends to the formula's current result, even while an earlier step is
being previewed. `data.query.addStep {formula, name?, op, params}` returns `{formula, step}`;
`data.query.renameStep {formula, step, to}` and `data.query.removeStep {formula, step}` return
`{formula}`. The workbook changes only when the user saves. The commands validate columns
against the preceding result and parse the generated M. Renaming resolves lexical scopes and
preserves field names, literals and comments; capture of another variable is refused. Removing
a step reconnects consumers to its predecessor; a referenced source or a new self-reference is
refused. A formula without a simple top-level step result is wrapped as a source when appending.

Tests: `crates/engine/tests/query_steps.rs`, `crates/engine/tests/web_sources.rs`,
`crates/mquery/src/tests.rs` and `crates/ui-egui/tests/queries.rs`. HTTP tests use local servers
and require no internet or real credentials.

## Never crash

Queries are arbitrary programs, often from files that came from elsewhere.

- Syntax trees are bounded in nesting and height; long operator chains are flat.
- Evaluation depth is bounded. Off the web, queries run on their own thread with a large stack,
  where recursion ten thousand levels deep has room; a panic there becomes an error.
- Values of any nesting drop without recursion.
- Lists, tables and text have size caps (lower on 32-bit targets); passing one is an error that
  `try` cannot hide.
- Sorting uses its own merge sort: a comparer written in the query that gives an inconsistent
  order cannot bring it down.
- The evaluator polls its host for cancellation; the engine doesn't offer a way to cancel yet.

Tests: `crates/mquery/src/tests.rs` (`deep_nesting_is_an_error_not_a_crash`,
`deep_values_drop_without_recursion`, `sizes_are_bounded`, `scopes_are_released`),
`crates/engine/src/cmd/query.rs` (`hostile_queries_end_in_errors`).

## In the file

Queries are saved in the `.xlsx` as a custom XML part whose root is `DataMashup`: base64 of a
version, a zip holding the queries as an M section document (`Formulas/Section1.m`), a permissions
XML, a metadata block and permission bindings.

- **Reading** takes each query's name, formula and description from the section document, and the
  table it fills from the metadata (`FillEnabled`, `FillTarget`).
- **Writing** leaves the part exactly as it was read when the queries did not change. When they
  did, the part is written anew, without permission bindings (they are tied to the machine that
  wrote them; the specification has the reader fall back to default permissions).
- Excel's connections and query tables, which tie a table to its query for Excel's own refresh,
  are kept as they were read but are not written for new queries.

Observed in Excel (Microsoft 365, pt-BR, 2026-10-12, black-box with synthetic workbooks):

- Excel opens a workbook written here without a repair prompt and lists its queries, with their
  formulas and descriptions. Two details it insists on, both in the writer now: the package
  description (`Config/Package.xml`) is in no namespace, and the metadata block ends with an empty
  zip package, not with nothing.
- The part Excel holds for a workbook with a described connection-only query and a query loaded
  to a table (quoted name, comment and `;` inside the formula) is read here: names, descriptions,
  formulas and the loaded table. Refreshing that query here gives the rows Excel loaded.

Not verified: a file saved by Excel read end to end (the Excel at hand could not save), Excel
refreshing a query written here, and other Excel versions. In Excel, tables loaded here are
ordinary tables and the queries show as connection only, since no connection is written.

## Not there yet

- In the Query Editor: IntelliSense, insertion between existing steps, and multiple aggregation
  outputs in one Group By action.
- Get Data and From Text/CSV making queries instead of importing once.
- A preview or refresh runs on the interface's thread: a slow query holds the window until it
  ends.
- PDF, Parquet; Oracle, ODBC and other databases; Windows (integrated) logins and SQL Server
  named instances (give the port); parameters of native queries.
- On macOS the system TLS library may refuse SQL Server's encryption; not tried.
- Connections and query tables for new queries, so that Excel refreshes them itself.
- Relationships and the data model.
- Background refresh, refresh on open, privacy levels.
- Metadata on values, query folding, lazy tables (a query that takes the first rows of a large
  source still reads all of it).
- Text positions count characters, where Excel counts UTF-16 units; they differ for characters
  outside the Basic Multilingual Plane.
- The local time zone: the clock is UTC.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-17 | patch | Review: Folder Path without Windows' `\\?\` prefix; the site typed in Get Data needs no second approval; transformation options split into readable helpers |
| 2026-10-17 | minor | SQL preview waits 800 ms after typing, keeps previous rows on edits and failures; automatic previews limited to a single simple SELECT, with explicit execution for other SQL |
| 2026-10-17 | minor | Full-height object sidebar restored; right-hand preview uses the available space, SQL textbox appears only on explicit Edit SQL and can be hidden without losing its contents |
| 2026-10-17 | minor | Full-width framed database preview below the SQL editor; Tables / Views tabs, name/schema search and an explicit Query Editor button |
| 2026-10-17 | minor | Existing database queries use the same resizable browser; always-visible SQL, source editing preserves transformations and load destination, Cancel/undo and failed-save regressions covered; layout checked down to 800×600 |
| 2026-10-17 | minor | Folders, XML, HTML tables, HTTP and OData sources; source dialogs and navigation; 16 visual transformations, step rename/removal; per-redirect web permission, timeouts and relative OData links |
| 2026-10-16 | minor | Logins can be remembered in the system's credential store (Windows, macOS, Linux/BSD Secret Service) |
| 2026-10-16 | minor | Refresh keeps calculated columns the user added; `Table.Pivot` gives its rows in the order of the other columns' values and `Table.PromoteHeaders` numbers repeats with one counter, both as observed in Excel by the author of storytold/gridcraft#1532; compared with #1203, #1532 and #1430 on one workbook |
| 2026-10-15 | minor | Databases: `Sql.Database`, `Sql.Databases`, `PostgreSQL.Database`, `MySQL.Database`, `SQLite.Database`, `Value.NativeQuery` with navigation tables read when opened; logins per server for the session; native query approval; Data › From Database |
| 2026-10-14 | minor | The Query Editor (steps, formula, preview), the Queries & Connections pane, and From Table/Range, New Query, Refresh All on the Data tab; `data.query.parse` |
| 2026-10-12 | major | Created: M language engine (`crates/mquery`), query commands, load and refresh to tables, Data Mashup part read and written |
