# What this app does

**CMI Timetable Planner** turns Chennai Mathematical Institute's two public
timetable pages into a week you can actually plan: pick your courses, move a
class when it really meets somewhere else, see your clashes, and take the
result to your calendar, your phone or a printer.

Everything happens **inside your browser**. There is no account to make, no
server to trust, and nothing to install.

**Live:** <https://gourab-ghosh.github.io/cmi-timetable/>

---

## At a glance

| | |
|:--|:--|
| **Your week, laid out** | Five views: your timetable, your courses, the whole catalog on one grid, a searchable course list, and every lecture hall |
| **Change anything** | Move a class, change its room, add a meeting, strike one out, set its credits — CMI's pages are never touched, your copy is |
| **Nothing is lost** | Every change is listed in one place, every change is undoable, and nothing is committed until you say so |
| **Clashes are shown, never blocked** | You are told immediately and clearly; you decide |
| **Take it with you** | Share links, `.ics` calendar export, and a print sheet that fits one page |
| **Works offline** | After the first sync, everything works with no connection |
| **Private by construction** | 100% client-side, no accounts, no analytics, no cookies, nothing sent anywhere |

---

## The five views

### 📅 My timetable

Your week as a grid — days down the left, CMI's time slots across the top.

- **Classes at unusual hours get their own column.** An evening seminar or a
  class in the lunch gap is never squeezed into the nearest official slot and
  never mislabelled. The grid grows a clearly marked column carrying the real
  time.
- **"No fixed slot yet"** holds the courses you have picked that have no time
  yet — CMI hasn't scheduled them, or they are yours and waiting for you.
  Drag one onto the grid, or open it and set a time.
- **Clashes** are listed under the grid: one row per pair of courses, with
  every colliding time beside it.
- **Your changes** lists everything you have altered (see below).
- **Print** produces a poster-style sheet — see *Taking it with you*.

### 📚 My courses

One card per course you are taking: who teaches it, which branches it belongs
to, its credits, every weekly meeting, and clash marks where they apply.

- A **credit summary** at the top: the total in large type, then one plain
  sentence per credit value ("3 courses at 4 credits"), plus footnotes about
  any credits that were assumed or set by you.
- The **same filter bar** the catalog uses, narrowed to your own courses — so
  "which of mine meet on Thursday" is one click. Its dropdowns list only what
  *your* courses have: your instructors, your halls, your days. The credit
  total keeps counting your whole timetable, and says so when the list is
  showing fewer.
- **"Fits my schedule" is not here**, because it could not do anything here:
  it hides whatever overlaps your selection, and every course on this page is
  your selection. It stays on the Catalog and the Master grid, where it has
  something to hide.
- **Your courses, off the timetable**: courses of your own you removed are
  parked here with their definition intact, ready to add back.

### 🗓 Master grid

CMI's entire catalog laid onto one grid, so you can see what is on offer at
any hour of the week.

- Courses already in your timetable carry a **✓ mark and an accent ring** —
  never colour alone.
- Courses that **would clash** with your current selection carry a **⚠**.
- **ⓘ** opens the full details of any course.
- Click a course to add or remove it. With *Edit layout* on, you can drag a
  course you haven't picked straight onto a slot — that adds it and places it
  in one gesture.

### 🔎 Catalog

Every course this semester, searchable and filterable.

- Search by code, name or instructor.
- Filter by **branch, instructor, day, time slot, hall, credits, course** and
  **flags** (optional courses, unscheduled ones, ones you have customised),
  plus a **"Fits my schedule"** switch that hides anything overlapping what
  you already have.
- Every filter dropdown has its own search box and **All** / **None**
  shortcuts that act on whatever the search is currently showing.
- **A dropdown only offers values that something in front of you actually
  has.** No Thursday when nothing meets on Thursday, no instructor who
  teaches none of these courses, no course you have deleted. An option that
  could only ever produce an empty list is not offered at all, and a
  dropdown with nothing left to offer does not appear. A filter you set on
  one page still shows there — ticked, and removable — even where its value
  is out of scope, so a short list always explains itself.
- Filters are **undoable** like everything else, and shown as removable chips
  so you always know why a list is short.
- Nothing found? The empty state offers to add it as a course of your own,
  and tells you if you had deleted it earlier.

### 🏛 Halls

Every room CMI publishes, as one table for the whole week — rows are hall ×
day, columns are time slots, so a room's entire week reads straight down.

- Each hall is **named once**, in a cell spanning its days, and both the hall
  and day gutters stay put when the week scrolls sideways.
- Each room says **how busy it is** all week — "free all week" being the
  answer this page exists to give.
- Empty days shrink to a line, alternate rooms carry a faint band, and today
  is marked.
- **Places of your own** (a room CMI doesn't list, typed by you) get their own
  rows, badged as yours.
- **Find a free hall**: pick a day *and* a slot — never a silently assumed
  Monday — and get a count and a scannable list of rooms that are free. It
  reads the same booking data the grid does, so the two can never disagree.

---

## Making it yours

### One editor per course

Clicking any course and pressing **"Edit this course"** opens the whole of it
in a single form: every weekly meeting's day, time and hall, its credits, and
— for a course of your own — its name and code.

- Saved in **one step you can undo in one go**.
- Each row you change says **which of CMI's meetings it stands in for**, with
  a **Put it back** beside it.
- Meetings you struck out are listed underneath, so those can come back too.
- A **live clash line** under each row tells you what it would collide with,
  while you are still deciding.
- **＋ Add a weekly meeting** appends another one — any course can have as
  many as it really has.
- A course CMI hasn't scheduled opens with **no meetings and no row filled in
  on your behalf**, so you can change its credits or its name without it
  quietly acquiring a Monday morning class.

### Drag and drop, on purpose

Drag & drop lives behind an explicit **✎ Edit layout** toggle, so scrolling on
a phone and ordinary clicking stay accident-free.

- **Mouse or pen**: drag a chip to a new slot.
- **Touch**: long-press to lift, then drag.
- **Keyboard**: focus a chip, press **M**, move with the arrow keys, **Enter**
  to drop, **Esc** to cancel.
- **In the Halls view** a drop targets a hall row *and* a time column, so one
  gesture moves a class into a different room and a different hour.
- **Drop a chip back where CMI put it** to undo the move.

### Rooms, including ones CMI never lists

The hall picker offers every hall CMI publishes, a row for "hall to be
announced", and **"Other place…"** for a room CMI never lists. Places you type
yourself come back as ordinary choices under **"Your own places"**, appear in
the Hall filter, and get their own rows on the Halls page.

### Courses of your own

**"Add your own course"** creates anything CMI's pages don't list: seminars,
reading groups, a class at another institute. You can also create one straight
from a search that found nothing.

- Name first, with a code suggested from the name; credits 0–20; any number of
  weekly meetings, on CMI's slots or at times of your own.
- They behave like real courses **everywhere** — clash detection, grids, drag
  & drop, credit totals, `.ics` export, share links.
- A violet **Custom** badge marks them.
- **Remove** parks one with its definition intact; only **Delete** destroys —
  and both are undoable.
- If a later CMI sync introduces the same code, **your version keeps winning**,
  with a one-click offer to switch to CMI's instead.

### Deleting a course

Any course can be deleted, CMI's included. CMI's pages are never edited, so
this is about *your copy* of them: the course leaves your timetable, the
catalog and the master grid.

- The deletion is recorded with everything else you changed, with a **Restore**
  that brings the course back along with the changes you had made to it.
- The catalog says **how many are hidden** — a list quietly shorter than CMI's
  is one nobody can trust.
- A share link naming a deleted course **lifts the deletion** rather than
  contradicting it.
- The Halls page keeps every booking either way: it answers "is this room
  free?", and that stays true whatever you want the course.

### Credits

CMI states credits only exceptionally, so the app is explicit about what it is
assuming:

| Situation | What it counts | How you know |
|---|---|---|
| CMI states the credits | Exactly that | Never second-guessed |
| No credits stated | **4**, the campus default | Marked "assumed" |
| No credits, but a shorter month span is noted | **One credit per month** — "(Oct–Nov)" ⇒ 2, "(Sep)" ⇒ 1 | Marked "assumed", tooltip explains |
| You disagree | Whatever you set, 0–20 | Listed under *Your changes*, with "Use CMI's value" to go back |

Totals, the per-value breakdown, the credit filter and the catalog all follow
whatever is true after your changes.

---

## Nothing gets lost

### Your changes, in one place

The **"Your changes"** panel (on My timetable, and the same list inside *My
data*) shows every alteration as **official → yours**, grouped by what kind of
change it is, with a count per group:

> Courses you added · Courses you deleted · Moved to other times · Moved to
> other rooms · Moved to another time and room · Meetings you added ·
> Meetings you removed · Credits you set

Each group is colour-railed by what it does — violet for what you added, red
for what you took away, blue for what you changed in place — so the kind of
change registers before a word of it is read.

Each row prints only what actually changed — a room move shows two room names
with the unchanged time beside it as context; a removal is struck through and
has nothing on its right. Every row has a one-click way back that **says what
pressing it leaves behind**: *Put it back*, *Back to CMI's time*, *Back to
CMI's room*, *Back to CMI's credits*. One button undoes all of your changes to
CMI's courses while keeping your own courses untouched.

A **✎ N changes** pill sits in the grid toolbars whenever custom data is in
play, counting exactly the rows in that list.

### Undo, for everything

- **100 steps deep**, with redo.
- **Ctrl+Z** / **Ctrl+Y** / **Ctrl+Shift+Z** (⌘ on a Mac).
- Filters ride the same history — one step per change, and one per burst of
  typing rather than one per keystroke.
- Notifications carry their own **Undo**, and pause their auto-dismiss while
  you hover, focus or tap them, so there is always time to read one.

### Two quiet safety nets

- **Deselecting a course keeps its custom times**, so adding it back doesn't
  silently revert a move you made weeks ago.
- **A half-written form isn't thrown away by a stray key.** Nothing in the
  course editor is committed until Save, so Escape and a click on the dark
  area ask first — but only once you have actually changed something.

---

## Taking it with you

### Share links

| Link | Carries |
|---|---|
| **Copy link** | Your selection (`?c=TOC,QCOM,MFD`) — readable, with plain commas |
| **Copy link with custom changes** | The selection *plus* your moved meetings, your credits and your own courses, compressed into the URL |

Codes are matched case-insensitively, so a hand-typed `?c=toc` works, and a
link that got re-encoded on its way through a chat app still opens correctly.
Codes the catalog doesn't know are **warned about, not silently dropped** —
with an explanation of why that happens and what still worked.

### Calendar export (`.ics`)

- The whole timetable, or a single course.
- A date range, pre-filled from the semester CMI names.
- An optional **10-minute reminder** on every class.
- Honours your changes — moved classes, your own courses, your rooms — and
  courses annotated "starts …" or "runs … only" are exported with their own
  dates.
- It says plainly that CMI's holidays are **not** excluded, rather than
  pretending otherwise.

### Print

A proper poster sheet, not a screenshot of a web page:

- Masthead with the semester and the date you last synced.
- Framed grid with a dark time band and chips in their branch colours, the
  code centred and the room beneath it.
- **Dashed border + ✎** marks a time you changed; **red border + ⚠** marks a
  clash, so the meaning survives a black-and-white printer.
- A red strip listing every clash on the sheet.
- A two-column course legend with names, instructors, credits and meetings.
- A dense 12-course semester fits on **one page**, with nothing clipped.

---

## Keeping up with CMI

The app ships **no timetable data at all**. On first load it asks for one
sync; after that everything works offline, and it re-checks on its own at most
twice a day.

The header always says **when it last synced** and by which route, counting up
on its own as time passes — "just now", "12 min ago", "2 days ago" — and
turning a warning colour once the data is two days old. CMI edits its
timetable all semester, and a planner that can't tell you how old it is isn't
worth much.

### Where the data comes from

Every route ends at **cmi.ac.in** itself:

1. **direct** — a short attempt at CMI's URLs
2. **proxy** — public CORS relays, all raced at once, first valid answer wins

There is deliberately **no copy of CMI's pages** inside the app or hosted
beside it. A fallback like that works by showing you something CMI published a
while ago without telling you how long ago, and a timetable you can't date is
worse than an honest "couldn't reach CMI".

### Fail closed, always

A freshly fetched timetable replaces the stored one **only after every
validation rule passes** — enough branch grids, enough courses, nearly every
legend entry resolved, a sane hall grid and slot list, semester labels that
agree, and cross-page consistency checks that catch a page truncated halfway.

If anything fails, **your saved timetable is kept untouched** and the problem
is explained in plain language. The parser and the gate are the only judges of
content, so a CMI redesign surfaces honestly as "this app needs an update" —
never as a fake "CMI is unreachable", and never as a plausible-looking wrong
timetable.

### When CMI changes something

- **What changed** — a readable digest: new courses, courses no longer listed,
  and per-course lines like *renamed: X → Y* or *credits: 2 → 4*.
- **Conflicts** — when CMI moves a class you had moved yourself, you are asked
  which version to keep, per course, and told what each choice means.
- **Courses CMI drops** stay visible with a badge, and anything you had placed
  for them stays on your week.
- **Changes that no longer apply** are announced rather than silently
  vanishing — if CMI drops a class you had struck out, you are told there is
  nothing left to remove.

### A parser that expects to be surprised

CMI's pages are plain text inside HTML, and they drift. The parser reads
reworded day labels, times written with dots or am/pm or "to", ragged rows
whose separators don't line up with their header, and — if a page loses its
separators entirely — falls back to column alignment. Nothing about CMI's
current pages is hard-coded anywhere.

When a parser improvement ships, the stored pages are **re-read locally**
without refetching, and the app is careful to announce its own re-reading as
its own, never as a change CMI made.

---

## Comfort and control

| | |
|:--|:--|
| **Theme** | Auto (follows your system), light, or dark |
| **Density** | Comfortable or compact |
| **On a phone** | A single-day view, tap targets sized for fingers, a header that packs tight, and long-press to drag |
| **Motion** | Animations respect "reduce motion" |
| **The wheel** | Scroll over a focused box with a step — credits, a meeting's start or end time, an export date, or any dropdown — and it moves one step. Only while the box has focus, so a value never changes because you scrolled past it |
| **Enter** | Saves in the editor, downloads in Export, and dismisses the keyboard in a search box |
| **Escape** | Cancels a drag, then a keyboard move, then an open filter menu, then a dialog — in that order |

### Built to be used without a mouse, or without sight

- Every dialog traps Tab, opens focused on its first **field** (not a toggle),
  and hands focus back where it came from on close.
- Live regions announce validation errors, clash lines and move confirmations
  — so a screen reader hears them rather than nothing.
- Meaning is never carried by colour alone: a clash has a ⚠ and a word, a
  selected course has a ✓ and a ring.
- `I` opens details on a focused chip; `M` starts a keyboard move.

---

## Your data is yours

- **Everything lives in your browser** (`localStorage`), under keys you can
  read: your selection, your changes, your own courses, your preferences, and
  the timetable CMI published.
- **My data** is a complete inventory with one-click removal for each piece —
  nothing is hidden from you, and nothing is sent anywhere.
- Only the stored timetable is a **cache**; it can be fetched again. Your
  selection, your changes and your own courses **exist nowhere else**, and the
  app treats them accordingly: a save that fails says so, and a browser short
  on space drops the re-parseable page copies first.
- If a stored value is ever unreadable, it is **backed up rather than
  deleted**, and you are told where the copy is.
- No accounts, no analytics, no cookies, no tracking. The only network
  requests it ever makes are for CMI's two timetable pages — directly, or
  through a public relay when the browser won't allow a direct fetch.

---

## For the curious

**Rust → WebAssembly**, built with [Leptos](https://leptos.dev) (client-side
rendering) and [Trunk](https://trunkrs.dev), served as static files from
GitHub Pages. The parsing, validation, merging, calendar generation and URL
codecs live in a separate crate with no browser dependencies, so they can be
tested on their own.

**Tested like it matters:** 100 native tests — including a synthetic CMI
website the tests generate themselves, with other semesters, other time
formats, renamed halls and ten different kinds of broken page — plus 65
end-to-end browser tests driving the real app in a real browser: drag & drop,
touch gestures, keyboard-only flows, storage corruption, and a stand-in CMI
that lets the true sync path be exercised end to end.

**Developer mode** is deliberately not linked anywhere in the interface.
Navigate to `#/developer` for build info, the fetch log, per-branch parse
reports, a storage inspector, a raw-HTML viewer, and simulators that prove the
fail-closed behaviour actually fails closed.

---

## Things this app deliberately does not do

Being clear about these is part of the design:

- **It never edits CMI's pages.** Everything you change is your copy, and it
  always says which of CMI's data it replaced.
- **It never blocks you.** A clash is shown immediately and loudly, and then it
  is your decision.
- **It never ships or hosts a copy of CMI's timetable.** What you see was
  fetched from cmi.ac.in by your own browser.
- **It never guesses quietly.** An assumed credit says "assumed"; an
  unreachable CMI says so; a page it cannot read says the app needs an update.
- **It doesn't exclude CMI's holidays** from calendar exports, and says so.
- **Keyboard move mode isn't available on the Halls page** — that table is
  organised by room rather than by day, so pressing `M` there explains where
  moving does work instead of starting a move you cannot see. That page's
  Edit-layout button doesn't mention the key either: the page that refuses a
  shortcut shouldn't be the page that teaches it.
- **A control that cannot act isn't shown.** Print is disabled with nothing
  to print; a course with no times is not offered a calendar export; a filter
  that could not change this page's list is not on this page. If a button is
  in front of you, pressing it does something.

---

*This file is part of the app, not a brochure: it is updated in the same
change that adds, renames or removes a feature, so what it describes is what
you will find. If you spot a difference, the app is right and this is a bug.*
