# Queries (Get & Transform)

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** minor (the Query Editor and the Queries & Connections pane) · **Target:** Microsoft Excel (Microsoft 365)

A query is a named formula in the Power Query M language that produces a table. GridCraft keeps
queries in the workbook, runs them, and loads their results into tables on sheets. This page says
what is there, how it behaves and what is not there yet. The gap entry is
[gaps.md §12](gaps.md#12-get--transform-external-data-data-model-f).

## What is there

| Part | Where | State |
|---|---|---|
| The M language: lexer, parser, evaluator | `crates/mquery` | Complete to the public language specification, except metadata (`meta` keeps the value and drops the metadata) and query folding (everything is evaluated here, eagerly) |
| The standard library | `crates/mquery/src/stdlib` | 420 functions and 92 constants: `Table.*` (78) and `#table`, `List.*` (67), `Text.*`, `Number.*`, `Date.*`/`Time.*`/`DateTime.*`/`Duration.*`, `Record.*`, `Splitter.*`, `Combiner.*`, `Comparer.*`, `Replacer.*`, `Value.*`, `Type.*`, `Csv.Document`, `Json.Document`, `Excel.CurrentWorkbook`, `Excel.Workbook`, `File.Contents`, text encodings and binary values |
| Queries in the model | `Workbook::queries` (`crates/model`) | Name, formula, description, the table it loads to |
| Commands | `crates/engine/src/cmd/query.rs` | `data.query.add`, `.set`, `.delete`, `.load`, `.refresh`, `.list`, `.preview`, `.parse`, `.functions`; `data.refreshAll` refreshes queries, then PivotTables |
| Interface | `crates/ui-egui/src/query_editor.rs`, `panes.rs`, `ribbon.rs` | Data tab: From Table/Range, New Query, Refresh All, Queries & Connections. The Query Editor: name, description, applied steps, the formula, a preview of the result or of one step. The Queries & Connections pane: each query with where it loads, Edit, Refresh or Load, Delete |
| In the file | `crates/xlsx/src/mashup.rs` | Read and written as the Data Mashup custom XML part ([MS-QDEFF]), the part Excel keeps its queries in |

In the interface, Data › From Table/Range opens the Query Editor on a query that reads the table
at the selection, New Query on an example, and the Queries & Connections pane lists the queries.
The editor writes M: there are no transform commands that add steps yet. Get Data (Text/CSV)
still imports once, without making a query. Everything the interface does is also a command
(CLI, MCP, control channel, scripts).

## How it behaves

- **Loading.** `data.query.add` runs the formula and writes the result as a table: a header row,
  then the rows. By default on a new sheet named after the query; with `at`, at a cell of an
  existing sheet. A list loads as one column, a record as names and values. `load: false` keeps a
  connection only, for other queries to use.
- **Refreshing** runs the query again and replaces the table in place. The table grows or
  shrinks; cells it no longer covers are cleared; a totals row moves with it. If the cells a
  larger result needs are not empty, the refresh fails and nothing changes. Nothing refreshes on
  its own, including when a workbook is opened.
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
| `Web.Contents` | Nothing: refused | no |

A query runs only when a command asks for it, so opening a workbook reads no files.

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

- In the Query Editor: commands that add steps (filter, remove columns, change type, group…)
  by writing M, column headers to act on, renaming or deleting steps, IntelliSense.
- Get Data and From Text/CSV making queries instead of importing once.
- A preview or refresh runs on the interface's thread: a slow query holds the window until it
  ends.
- `Web.Contents`, databases, folders (`Folder.Files`), XML and HTML tables, PDF, Parquet.
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
| 2026-10-14 | minor | The Query Editor (steps, formula, preview), the Queries & Connections pane, and From Table/Range, New Query, Refresh All on the Data tab; `data.query.parse` |
| 2026-10-12 | major | Created: M language engine (`crates/mquery`), query commands, load and refresh to tables, Data Mashup part read and written |
