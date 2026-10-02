# Diagram notes

A note is a Markdown sticky note attached to a table or a group in the
diagram. A table or group can have any number of notes.

## Adding a note

Right-click a table or a group title and choose **Add note…**. The editor has:

| Field | What it does |
|---|---|
| Title | Optional. Without a title, the first line of the note is used |
| Color | Six sticky-note colors |
| Pin | Always show the note on the canvas, for everyone who opens the diagram |
| Preview | Live Markdown preview next to the editor |

Press **Save** or Cmd/Ctrl+Enter. A new note opens on the canvas next to its
table or group.

## Links to other tables

Notes use the Obsidian link format:

| You write | Links to |
|---|---|
| `[[orders]]` | table `orders` |
| `[[orders#user_id]]` | column `user_id` of `orders` |
| `[[orders\|the orders table]]` | table `orders`, shown as "the orders table" |
| `[[shop.orders]]` | table `orders` in database `shop` (linked databases) |

Names are matched without regard to case, and `[[users]]` also finds
`public.users` when only one table has that name. Links inside code
(`` `[[x]]` `` or a fenced block) are ignored.

While you type `[[`, the editor lists matching tables; click one to complete
the link. Links to tables that are not in the diagram are shown struck through,
and the editor lists them under the text.

Each link of a note that is on the canvas is drawn as a dashed line, in the
note's color, from the card to the table (or to the column row). Hover a line
to see which note and table it connects. Click a link inside a card to move
the view to that table.

## Showing notes

- A table or group with notes shows a badge with the number of notes. Click
  it to show all of them at once; click again to hide them. Right-click the
  badge to open the list.
- The notes of one table or group are placed together in a column next to it,
  on the side (right, left or below) where they cover the fewest other tables
  and notes.
- Drag a card to move it, drag its lower-right corner to resize it,
  double-click it to edit it. The card position is relative to its table or
  group, so it moves with them.
- Below 45% zoom a card shows only its title.
- **Show notes** in the canvas right-click menu, or **On canvas** in the list,
  hides every card and dashed line.

## The notes list

The **Notes** button in the toolbar opens the list of every note, grouped by
table and group. From a table or group menu, **Notes (n)…** opens the list for
that table or group only.

| Control | What it does |
|---|---|
| Note title | Shows the note on the canvas and moves the view to it |
| Eye | Shows or hides the note on the canvas |
| Pin | Pins or unpins the note for everyone |
| Tidy up (next to a table or group) | Places its shown notes again, next to each other |
| Table or group name | Moves the view to it |

**Unattached notes** lists notes whose table is no longer in the diagram, for
example after the table was dropped or renamed. They are kept until you remove
them.

## Who can read a note

Notes are part of the diagram, like group repository URLs. They are saved with
the diagram in the `diagram_by_tabular` table of the database (and in the
Obsidian vault when enabled). Everyone who can open the diagram from that
database can read them. Opening or closing a card is personal; pinning is
shared.

When two people change the same note, the diagram shows a merge window the
next time it is opened or saved; see [Diagram storage](DIAGRAM_STORAGE.md).

## Limits

- Notes cannot be attached to tables of a linked database; add them in that
  database's own diagram.
- **Delete Group** also removes the notes of that group.
- A tab opened with **Open in new tab** (focus or group subset) shows the notes
  of its tables but is not saved, so notes cannot be edited there.
- Mermaid export does not include notes.
