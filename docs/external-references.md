# References to other workbooks

> **Last reviewed:** 2026-11-18 · **Last updated:** 2026-11-18 · **Change:** minor (step A3 built: links in files; earlier: step A2; step A1; the design) · **Target:** Microsoft Excel (Microsoft 365)

A formula that reads a cell of another workbook: `=[Budget.xlsx]Sheet1!B4`. This page is the
design, written before the work; the table of steps says which are built. Statements about Excel come from the
public file format (ECMA-376, `externalLink` parts) and Microsoft's documentation of external
references; none was observed in Excel for this page.

## Where GridCraft was before this work

Checked in the code and by running it, 2026-11-18, before step A1. The table of steps says what
has changed since:

- **Typing one is refused.** `gridcraft-cli eval "=[Book2.xlsx]Sheet1!A1+1"` answers "There's a
  problem with this formula". The lexer reads `[Book2.xlsx]` as a structured reference and the
  parser has nothing to do with the sheet prefix that follows.
- **Pointing at another workbook's window ends the formula.** A window that loses the keyboard
  enters its draft (`doc_windows.rs`), so `=` followed by a click in another window gives the
  formula error.
- **A file from Excel that has them survives.** Their formulas (`[1]Sheet1!A1`) don't parse; such
  a cell keeps its formula text and the value the file saved, which nothing recalculates
  (`xlsx/src/read.rs`, `UnreadFormulas`, one warning a sheet). The `externalLink` parts and
  `<externalReferences>` are kept as bytes and written back (`xlsx/src/passthrough.rs`), and a
  relative path to a linked workbook is rewritten when the workbook is saved into another folder
  (`rebase_links`). Open, edit, save keeps the links. **This must stay true at every step.**
- **Calculation is per workbook.** A `Calc` holds one workbook's dependency graph and evaluates
  against that workbook alone; the session (`engine`) holds the open workbooks side by side.

## What is wanted

Three stages. This page designs the first and leaves room for the others.

| Stage | What works | Reads another file from disk |
|---|---|---|
| **A** | References between workbooks that are open: typed or pointed at, live values, saved in the file with a relative path | never |
| **B** | A source that is closed: the values the file saved, a question on open, Change Source | never |
| **C** | Update Values from a closed source file | yes, when the person asks |

## Decisions

### 1. The path is never made up

Every open workbook has the path the system gave when it was opened or saved (`DocState::path`),
in that system's own form. The formula does not hold it. What a formula holds is the source's
**name**; what the file holds is the path, once, apart from the formulas.

- In a formula: `[Budget.xlsx]Sheet1!B4`, quoted when the name or the sheet needs it:
  `'[My Budget.xlsx]Sheet 1'!B4`. A workbook not saved yet is `[Book2]`.
- In the file: the formula has a number, `[1]Sheet1!B4`; `<externalReferences>` lists the links
  in that order; each link is a part with the sheet names, the saved values, and a relationship
  whose target is the path.

### 2. One way to write the path, on every system

- **Relative to the folder of the workbook that refers**, with forward slashes, percent-encoded
  as a relationship target is: `Budget.xlsx`, `../data/Budget.xlsx`. The same text is right on
  Windows, macOS and Linux, and stays right when the folder moves or is synchronised as a whole.
- **Absolute, as a `file:///` address**, only when there is no relative path (another drive, a
  network share).
- Reading: a relative target is joined to the folder of the file being opened with the standard
  library's path type, which gives the system's own form. An absolute target from another
  system (`file:///C:/…` opened on Linux) names nothing here: the source counts as not found, the
  saved values are used, and nothing is guessed.
- `rebase_links` already rewrites relative targets on Save As. It keeps doing so, for the
  links in the model as for the kept bytes.
- A source without a path (not saved yet) is written by name alone. It is found again only while
  both are open.
- On every save the path of a source that is open and has a file is written from where the two
  files are then. After a save the workbook's paths are relative to the folder it was saved
  into, so the path outlives the source being closed, and a later Save As starts from there.

### 3. The values live in the workbook that refers

Each workbook keeps, per link, the values of the cells its formulas read from it: what Excel
stores in the link part, and what GridCraft keeps today as bytes. Evaluation reads that copy.

- `Calc` stays per workbook. No dependency edge crosses workbooks, no workbook is locked while
  another calculates, and a calculation running in the background (`ui-egui/src/background.rs`)
  never needs a second workbook.
- A formula that reads a link is noted as a reader of links (as readers of hidden rows are
  today). When the copy changes, those formulas and what depends on them are recalculated.
- A cell the copy does not have is blank, as an empty cell is. A sheet the copy does not have
  is `#REF!`.
- A formula that reads a workbook this one has **no link to** keeps the value it has. That is
  how a file's formulas into other workbooks keep the values the file saved, at every
  recalculation, until the links are read with the file (A3).

### 4. The session keeps the copies current

After a command changes workbook S, the session looks at the other open workbooks that have a
link named S, refreshes the cells their formulas read, and recalculates them when a value
differs. Also when S is opened, and when a formula that reads S is entered.

- It is not an Undo step of the reader, and it marks the reader changed (its saved values are no
  longer what the file has), as an update of links does in Excel.
- A chain (C reads B reads A) settles in order; a loop across workbooks stops after a fixed
  number of rounds and leaves the values of the last one.
- Matching is by name (the name the source's window shows: its file name, or its title while
  it has no file), as a formula names it. When two open workbooks have that name (the same file
  name in two folders), the one the link's path names is the source; the path is compared as
  text and nothing is tested for on disk. A workbook of that name from another folder than the
  link's path says is still the source when it is the only one open: a pair of files moved or
  sent together keeps working.
- What a workbook's formulas read is collected once per version of the workbook, and only for
  a workbook that may read another: a session in which none does pays nothing. A workbook that
  does is looked through whole after each of its own changes; that is the part to make
  incremental if it shows.
- A link's number as a file writes it (`[1]`) is never matched to an open workbook: its link is
  the file's to bring.
- Saving S under another name renames it in the formulas of the open workbooks that read it.

### 5. The file: links become part of the model

An `externalLink` part whose target is a workbook is read into the link list (name, target,
sheet names, saved values) as well as kept as bytes. A link GridCraft does not understand (DDE,
OLE) stays kept bytes only, and keeps its number.

- **Reading.** The link's name is the file name its target ends in. The formulas and defined
  names that use its number (`[1]Sheet1!A1`) are rewritten to the name
  (`[Budget.xlsx]Sheet1!A1`), so they calculate from the saved values and follow the source once
  it is open. A link stays a number, with its formulas keeping the values the file saved, when
  all of it is not understood: a value of an unknown kind, a name a formula can't hold or that
  would read as a number, two links with one file name, more sheets or values than the limits.
  A sheet the part marks as not read (`refreshError`) is not known to the link: a reference
  into it is `#REF!`. That reading of the mark comes from the format's description, not from
  Excel.
- **Writing.** Formulas get numbers again. A link the file brought keeps its number; links made
  here follow, in the order formulas first read them; a file with both kinds keeps one
  numbering. A kept part goes back byte for byte unless the link's values, sheets or target
  changed. Then its sheet names and values are rewritten in place and everything else in it
  stays (the other workbook's defined names, extensions). The sheet names keep their places,
  new ones are added after them, and a sheet nothing is known of is marked as not read. A part
  whose two elements can't be found as text (a prefix, a comment around them) is written as it
  was.
- A link made here is written as a new part when a formula reads it, with every sheet the
  formulas read listed; a link no formula reads is not written.
- Conditional formats, validation rules, table formulas and charts are not rewritten on read:
  there a link keeps its number in the text, which still names the right link when written.
  What they read of a link the file brought is kept: such a link is brought up to date where
  the cell formulas read it and otherwise left as the file had it.
- Round trip is tested against the files the pass-through tests use: the same links, the same
  targets, the same saved values, with and without an edit in between.

### 6. Pointing across windows

A formula at a place where a reference can go stays open when another workbook's window gets
the keyboard. A click on a cell there puts `[Name]Sheet!A1` into the draft. Enter and Esc there
act on the draft and bring its own window back, as they now do from another sheet of the same
workbook. The sheet pointed at does not draw its own selection meanwhile.

### 7. Nothing is read because a file says so

A workbook is hostile input, and so is the path inside it. Stages A and B never open, list or
test for a file named by a link: the path is text. Stage C reads one only when the person asks
for that update, and reads it as any file is read (size limits, no macros run, no further links
followed).

### 8. Limits

Counts and sizes come from files and formulas, so they are bounded: links per workbook, sheet
names per link, cells kept per link, and the size of a range read from a link (a whole column of
another workbook reads its used part, as a whole column of this one does). Past a limit the
formula gives an error value; nothing is cut short silently.

## Steps

Each is one pull request with its tests, and leaves files from Excel reading and saving as before.

| Step | Content | Seen by a person |
|---|---|---|
| **A1** (done) | Formula text: lexer, parser, printer, `SheetSel::External`; edits to this workbook leave such a reference alone, a copied formula shifts it. Model: `Workbook::links`. Calculation from the copy. | A typed `=[Book2]Sheet1!A1` is accepted and stays blank: nothing fills the copy yet. A file's formulas into other workbooks now parse, and keep their saved values |
| **A2** (done) | Session: copies kept current between open workbooks after every command (`engine/src/book_links.rs`); a source saved under another name is renamed in the formulas that read it | Typed references between open workbooks are live. A source that is not open is `#REF!` until it is |
| **A3** (done) | File: links read into the model and written from it (`xlsx/src/passthrough/links.rs`), relative targets, one numbering with the kept links | They survive save and open. A file from elsewhere calculates from its saved values and is live once its source is open. Not opened in Excel: see below |
| **A4** | Interface: pointing across windows | Click in the other window, as in Excel |
| **B** | Question on open, the saved values with a source closed, Edit Links (Change Source, Break Link) | |
| **C** | Update Values from a closed file | |

A1 and A3 are the large ones. A2 can't be told from A1 without two workbooks open; A4 is the
smallest and comes last because it makes the feature reachable.

## Not checked

- **Files GridCraft writes were not opened in Excel.** The parts follow the public format and
  read back here; whether Excel opens them without a repair prompt, and what it shows for a
  sheet marked as not read, is to be observed.
- **A link's values in Excel's own files** were read from parts built by hand after the format's
  description, not from files Excel wrote.

## Not decided here

- **Web addresses and cloud paths** as a link's target (`https://…`): kept as they are, never
  followed. Whether stage C ever reads one is its own question.
- **Names and tables of another workbook** (`[Book2]!Total`, `[Book2]Table1[Col]`): refused with
  the formula error in stage A; the file format has room for them.
- **What Excel shows** for a source open in another instance, or with the same name twice: to
  observe before stage B.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-18 | minor | A3: links in files. Read into the model under the source's file name, written back with one numbering; kept parts unchanged unless their values or target changed; paths relative to the saved file, kept after a save; of two open workbooks with one name the link's path says which |
| 2026-11-18 | minor | A2: the session keeps the values current between open workbooks, follows a source saved under another name, and gives `#REF!` for a source that is not open |
| 2026-11-18 | minor | A1: such references parse, print and calculate from the values the workbook keeps; a formula into a workbook there is no link to keeps its value |
| 2026-11-18 | minor | The design, before any of it is built |
