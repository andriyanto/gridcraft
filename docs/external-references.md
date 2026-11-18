# References to other workbooks

> **Last reviewed:** 2026-11-18 · **Last updated:** 2026-11-18 · **Change:** minor (stage C built: Update Values and Open Source read a linked workbook's file when asked; earlier: stage B; steps A1 to A4; the design) · **Target:** Microsoft Excel (Microsoft 365)

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

A formula at a place where a reference can go stays open when another window gets the
keyboard, and goes there with it. A click on a cell there puts `[Name]Sheet!A1` into the draft,
and the click that brought the window forward already does. Enter and Esc there act on the
draft and bring its own window back, as they now do from another sheet of the same workbook.
The sheet pointed at does not draw its own selection meanwhile.

- The draft is one window's at a time (`ui-egui/src/window_ui.rs`, `carry_draft`): the window
  that has the keyboard shows it in its formula bar, which takes its keys. The window it was
  typed in shows the text in its cell meanwhile, without an editor.
- It is entered in the workbook, sheet and cell it was typed in, whichever window Enter is
  pressed in. A command that enters drafts on its way (Save, New, a close) enters it there
  too, and then goes on in the window it was asked in.
- A draft that takes no reference (text, a finished formula) is entered when its window is
  left, as before. So is a formula when the window gone to has a draft, a dialog or the File
  view of its own, or shows a workbook whose name a formula can't hold.
- Another window on the formula's own workbook is pointed at the same way, with references of
  its own workbook (`Sheet2!A1`).

### 7. Nothing is read because a file says so

A workbook is hostile input, and so is the path inside it. Opening a workbook, calculating it,
saving it and listing its links never open, list or test for a file named by a link: the path
is text. A file a link names is read in two cases only, both asked for by a person in that
moment: Update Values and Open Source (section 10). Change Source reads the file the person
names, not one the workbook names.

### 8. Limits

Counts and sizes come from files and formulas, so they are bounded: links per workbook, sheet
names per link, cells kept per link, and the size of a range read from a link (a whole column of
another workbook reads its used part, as a whole column of this one does). Past a limit the
formula gives an error value; nothing is cut short silently.

### 9. Edit Links: the list, Change Source, Break Link

Data › Edit Links lists the workbooks the active one's formulas read (`data.editLinks`). For
each: whether it is open here (its values follow it), not open with values from the file, or
not open with nothing known of it; how many places formulas read; and where its path says it
is. The path is shown as text and is not looked for.

- **Change Source** (`data.changeLinkSource`) takes a file's path or the name of an open
  workbook. The formulas and names that read the old workbook read the new one; the link gets
  the new name and a path relative to this workbook's folder. Stage B reads no file, so when
  the new workbook is not open, nothing was known of it and its formulas were `#REF!` until it
  was open; since stage C a file that is named is read at once (section 10). A workbook already linked takes the formulas over: the two
  links become one. A name a formula can't hold is refused.
- **Break Link** (`data.breakLink`) replaces every cell formula that reads the workbook with
  its value (a formula that fills several cells with all of them) and takes the link out. It is
  one Undo step, where Excel's can't be undone. A defined name is not a value: one that reads
  the workbook stays, and the link with it; the command says which.
- A link the file brought leaves the file when broken only if it is the last of the file's
  links: the others are known to formulas by their place in the list, and conditional formats,
  validation rules and charts keep those numbers as text. One that is not the last keeps its
  part, and is listed again, read by nothing, when the file is opened again.

### 10. Update Values and Open Source: reading a linked workbook's file

**Update Values** (`data.updateLinks`, one link or all) reads, for each linked workbook that is
not open, the file its path names, and copies the values this workbook's formulas read. The
file is not opened as a document: it is read, calculated as opening it would calculate it (so
what its formulas spill is there), the values are taken, and the rest is dropped. It is no Undo
step and leaves the workbook changed, as following an open source does. **Open Source**
(`data.openLinkSource`) opens the file as File › Open would, or goes to the workbook when it is
open already. **Change Source** to a file reads it the same way at once.

What keeps this safe when the path came from someone else's file:

- **Only when asked.** No workbook, formula, macro or open makes it happen. A script's steps
  can't: they read no file (`io::ScriptSandbox`). When a workbook with sources that are not
  open is opened, a bar under the ribbon says its values are the ones the file saved and leads
  to Edit Links; that is where Excel asks whether to update, and here nothing is read on the
  way.
- **Only a workbook.** The bytes must be a workbook (XLSX, XLSB, ODS, XLS). A text file is not
  taken in as cells, whatever it is called: a link to some other file of the computer can't
  bring its content into the workbook. A password-protected workbook is not read: open it.
- **Only this computer.** On Windows a path on a server (`\\server\share\…`) is not read:
  reading it would make this computer call a machine the file chose. Open that workbook from
  File › Open instead, and its link is live. A web address is never read.
- **As any file is read.** The same size limits, no macros run, and the workbooks the source
  reads itself are not opened: its formulas into them use what its own file saved.
- The path is worked out from the text and the folder of the workbook that has the link
  (`xlsx::link_path`), and shown in Edit Links before anything is read.

## Steps

Each is one pull request with its tests, and leaves files from Excel reading and saving as before.

| Step | Content | Seen by a person |
|---|---|---|
| **A1** (done) | Formula text: lexer, parser, printer, `SheetSel::External`; edits to this workbook leave such a reference alone, a copied formula shifts it. Model: `Workbook::links`. Calculation from the copy. | A typed `=[Book2]Sheet1!A1` is accepted and stays blank: nothing fills the copy yet. A file's formulas into other workbooks now parse, and keep their saved values |
| **A2** (done) | Session: copies kept current between open workbooks after every command (`engine/src/book_links.rs`); a source saved under another name is renamed in the formulas that read it | Typed references between open workbooks are live. A source that is not open is `#REF!` until it is |
| **A3** (done) | File: links read into the model and written from it (`xlsx/src/passthrough/links.rs`), relative targets, one numbering with the kept links | They survive save and open. A file from elsewhere calculates from its saved values and is live once its source is open. Not opened in Excel: see below |
| **A4** (done) | Interface: pointing across windows (`ui-egui`: `carry_draft`, `EditState::refer`) | Click in the other window, as in Excel: the reference goes into the formula, Enter brings its window back |
| **B** (done) | Edit Links: the list with what is known of each source, Change Source, Break Link (`engine/src/cmd/data/links.rs`, `ui-egui/src/dialogs/links.rs`). The saved values with a source closed came with A3; the notice on open comes with C, which gives it something to offer | Data › Edit Links |
| **C** (done) | Update Values and Open Source (`engine/src/book_links.rs`, `update_link`); Change Source to a file reads it at once; the bar on open | Edit Links › Update Values |

A1 and A3 are the large ones. A2 can't be told from A1 without two workbooks open; A4 is the
smallest and comes last because it makes the feature reachable.

## Not checked

- **Pointing was run in real windows on Linux only** (X11, no window manager), and through
  the window tests. Windows and macOS deliver the keyboard and the first click of a window in
  their own order: to be tried there.
- **Update Values calculates the source** as opening it does, where Excel takes the values the
  source's file saved. The two differ where GridCraft calculates a formula differently from the
  program that saved the file. Not compared against Excel.
- **A path on a server** is refused on Windows by its form (`\\server\…`); that was not run
  on Windows. On other systems a mounted share is a local path and is read.
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
- **What Excel shows** for a source open in another instance, or with the same name twice: not
  observed (stage B was built without it; nothing in it depends on the answer).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-18 | minor | C: Update Values reads a linked workbook's file when asked (a workbook, on this computer, never from a script); Open Source; Change Source to a file reads it at once; a bar says so when a workbook with closed sources opens |
| 2026-11-18 | minor | B: Edit Links lists the workbooks read and what is known of each; Change Source points the formulas at another workbook; Break Link leaves their values, as one Undo step. Also: the values a source's formulas spilled are read (a reference into a spilled range was blank) |
| 2026-11-18 | minor | A4: a formula that can take a reference goes with the keyboard to another window; a click there writes `[Name]Sheet!A1`, Enter and Esc bring its own window back |
| 2026-11-18 | minor | A3: links in files. Read into the model under the source's file name, written back with one numbering; kept parts unchanged unless their values or target changed; paths relative to the saved file, kept after a save; of two open workbooks with one name the link's path says which |
| 2026-11-18 | minor | A2: the session keeps the values current between open workbooks, follows a source saved under another name, and gives `#REF!` for a source that is not open |
| 2026-11-18 | minor | A1: such references parse, print and calculate from the values the workbook keeps; a formula into a workbook there is no link to keeps its value |
| 2026-11-18 | minor | The design, before any of it is built |
