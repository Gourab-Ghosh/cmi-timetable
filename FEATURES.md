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
| **Works offline** | The app itself opens with no connection after one normal visit, and your timetable is already in the browser — only syncing with CMI needs the internet |
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
- **On a phone, one day at a time.** A week grid needs sideways scrolling on
  a phone, so the timetable opens on today's classes instead — the question
  you ask your phone is "what do I have today?". A strip above it switches
  between **Week** and any single day.
  **What you pick there stays picked**: choose Week, or Thursday, and a
  refresh — tomorrow, next week — shows exactly that. Opening on today is
  what happens when you have never chosen, not something the app keeps
  deciding for you. A wider screen always shows the whole week, without
  forgetting what your phone was set to.
- **Your changes** lists everything you have altered (see below).
- **Print** produces a poster-style sheet — see *Taking it with you*.

### 📚 My courses

One card per course you are taking: who teaches it, which branches it belongs
to, its credits, every weekly meeting, and clash marks where they apply.

- A **credit summary** at the top: the total in large type, then one plain
  sentence per credit value ("3 courses at 4 credits"), plus footnotes about
  any credits that were assumed or set by you.
- The **same filter bar** the catalog uses, with **its own separate state** —
  so "which of mine meet on Thursday" is one click, and narrowing your own
  courses never quietly narrows the catalog you look at next (or the other
  way round; the Catalog and Master grid still share one state, because they
  ask the same question). Its dropdowns list only what *your* courses have:
  your instructors, your halls, your days. The credit total keeps counting
  your whole timetable, and says so when the list is showing fewer.
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
- **ⓘ** opens the full details of any course — and answers the ⚠ rather than
  just repeating it: for a course you haven't picked, it names which of *your*
  courses it would run into, and at what times.
- Click a course to add or remove it. With *Edit layout* on, you can drag a
  course you haven't picked straight onto a slot — that adds it and places it
  in one gesture.
- **"N courses match" counts what this grid can actually show.** A course CMI
  lists without a time has no cell to sit in, so it is not counted here — and
  if your filters match one, a line under the bar says how many, why, and
  offers an "Open the catalog" button. For the same reason the *Unscheduled* filter isn't
  offered on this page: it can only ever ask for the courses this grid has
  nowhere to draw.

### 🔎 Catalog

Every course this semester, searchable and filterable.

- Search by code, name or instructor — with the three switches every editor
  has, at the right-hand end of the box:
  **`Aa`** match case, **`ab`** whole word, **`.*`** regular expression.
  Whole-word means what `\b` means, so it means the same thing whether or not
  the pattern switch is on; a pattern can use the other two switches with it.
  A pattern you are still typing is **never read as an empty search**: the box
  says what is wrong with it so far (`Not a pattern yet — unclosed group`) and
  shows nothing, rather than quietly showing the whole catalog. The switches
  are filters like any other — they persist, they show in the chip line
  (`/^ana/ (case)`), and Ctrl+Z takes them back. A **✕** appears in the box the
  moment there is something to clear, and the box is wide enough that what it
  says about itself is never cut off by the switches beside it.
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
- Every row carries **Add/Remove** and, beside it, **Delete** — the stronger
  action that takes a course out of the catalog and the master grid as well
  as your timetable. It is recorded under *Your changes*, so it can be put
  back. (Not offered on the one row where a code of yours shadows CMI's: the
  full sentence for that case, *"Delete my version and use CMI's"*, lives in
  the course's own details.)
- Filters are **undoable** like everything else, and shown as removable chips
  so you always know why a list is short.
- Nothing found? The empty state offers to add it as a course of your own,
  and tells you if you had deleted it earlier. And when a filter set earlier
  is what's hiding a course your search would find, it names the course and
  offers **Clear filters to show it** — ahead of the create button, so you
  don't mint a duplicate by accident.

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
- Courses on your timetable carry the same **✓** here as everywhere else —
  including a booking CMI published without any class behind it, where the
  room is allocated but no branch's timetable puts that course in it. Those
  chips can't be dragged (there is no meeting to move), but they are still
  your courses and now say so.
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
- Editing a course that isn't on your timetable shows a ticked **Also add
  {code} to my timetable** box beside Save — the add happens only with your
  tick, so "Save changes" never quietly changes your clash picture or your
  credit total. Untick it and only the changes themselves are stored.
- A course CMI has dropped has no official credit value to differ from, so
  its editor says exactly that instead of offering a credits picker — and an
  untouched Save invents no "credits you set" change.
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
  to drop, **Esc** to cancel. The cell you are standing on is outlined the
  whole time — in the phone's single-day view too, where arrowing onto another
  day brings that day on screen with you rather than leaving the cursor
  somewhere you can't see.
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
  weekly meetings, on CMI's slots or at times of your own. The code stays
  within 12 characters and can't contain a comma or a % sign — those would
  break the links that share your timetable, and the form says so.
- They behave like real courses **everywhere** — clash detection, grids, drag
  & drop, credit totals, `.ics` export, share links.
- A violet **Added by you** badge marks them — and the badge is a button:
  tapping it opens the course's details, where the badge is explained in
  visible words (no hover needed, so it works on a phone).
- **Remove** parks one with its definition intact; only **Delete** destroys —
  and both are undoable.
- If a later CMI sync introduces the same code, **your version keeps winning**,
  with a one-click offer to switch to CMI's instead.

### Deleting a course

Any course can be deleted, CMI's included. CMI's pages are never edited, so
this is about *your copy* of them: the course leaves your timetable, the
catalog and the master grid.

- The deletion is recorded with everything else you changed, with a **Restore**
  that brings the course back along with the changes you had made to it — and
  its place on your timetable, if it was there when you deleted it. Deleting
  took both, so Restore returns both.
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
| No credits stated | **4**, the campus default | Marked with a `*` |
| No credits, and the name says **seminar** | **0** — seminars are attended, not credited | Marked with a `*`, the note says why |
| No credits, but a shorter month span is noted | **One credit per month** — "(Oct–Nov)" ⇒ 2, "(Sep)" ⇒ 1 | Marked with a `*`, the note says why |
| You disagree | Whatever you set, 0–20 | Marked with a `✎`, listed under *Your changes* |

The credit summary on My courses spells each assumption out in a full
sentence — which part of the total is a guess, why the app guessed what it
did, and that **Edit this course** takes the real number if you know it.
Where you set a number yourself, it names what that number stood in for:
CMI's figure where CMI publishes one, the app's own guess where it does not,
and both at once when your courses are a mix of the two.

When CMI writes the credit count into a course's name — "Visualization(2
credits)" — the app reads the number and shows the name without the note,
so cards never state the credits twice. Month notes like "(Oct–Nov)" stay:
they carry dates. Exports of raw data and the editor keep CMI's name
verbatim.

Totals, the per-value breakdown, the credit filter and the catalog all follow
whatever is true after your changes.

---

## Nothing gets lost

### Moving between sections

The five sections — My timetable, My courses, Master grid, Catalog, Halls —
are a real ARIA tab list, and every ordinary way of moving through a list
works on them:

- **Arrow keys**, both axes. The rail is a column on a desktop and a bar on a
  phone, so Up/Down and Left/Right both step it rather than one being dead at
  whichever width you happen to be at. **Home** and **End** jump to the ends,
  and the list wraps.
- **The wheel**, while the pointer is over the rail. The page underneath does
  not move while it is — not between notches, and not at the ends.
- **A swipe**, on a touchscreen. Drag along the bar and the sections follow.

The rail is **one Tab stop**, not five: Tab reaches it, the arrows move
within it, and one more Tab press leaves it for the page. Arrows pressed
anywhere else still scroll the page exactly as before, and while a class is
being moved by keyboard the arrows belong to the class, not the rail.

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
CMI's room*, and for credits whichever is true of that course — *Back to
CMI's credits*, *Back to the app's 4*, or *Remove this change* where CMI has
dropped the course and there is no number to go back to. Because those
targets differ row by row, the panel's opening line does not name one for all
of them — it points at the button beside each row instead. One button undoes all of your
changes to CMI's courses while keeping your own courses untouched.

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

### Making a link short

A link carrying your whole timetable is long. **Make this link short** — one
button in the share dialog, and everything about shortening lives behind it —
trades it for a short one through a free shortening service.

- **Three services, no account needed by any of them**: TinyURL (suggested —
  the best known, and the least likely to be stripped out of an email), da.gd
  (the shortest links) and clck.ru. Each says where your link is being sent
  before you send it, and all three answer in about the same time.
- **Nothing is sent until you press the button.** Opening the share dialog
  shortens nothing; opening this popup shortens nothing. This is the only
  action in the app that hands your timetable to anyone else, and it says so
  in as many words. One press asks one service — and if that service is slow
  to answer, the app quietly tries a helper site alongside it rather than
  leaving you waiting, then tells you exactly who ended up seeing the link.
- **Links you have made are kept**, for each service separately. Close the
  popup, close the browser, come back tomorrow: your short link is still
  there, and the service that made it is marked *link ready*. Nobody is
  asked again unless you ask them to be.
- **A link made before your timetable changed is shown as an earlier one**,
  never as the current answer. A short link is a permanent redirect to one
  address, so the one you made last week still opens last week's timetable —
  it is kept and clearly labelled, because you may already have sent it to
  someone, and a fresh one is one press away.
- **The full link is always one tap away**, under *The full link, as it is
  now*, so a service being down or slow costs you nothing.
- **All three are equally quick.** Most of the wait for a short link is not
  the service thinking, it is your browser meeting it for the first time —
  looking up the name, opening the connection, agreeing on encryption. While
  the popup is open the app gets that out of the way for the service you have
  picked: a handshake and nothing else, no link and no timetable. The popup
  says so. It is what takes da.gd from 629 ms to 244 ms, and it is why the
  three now finish within a few milliseconds of each other.
- If a service cannot be reached, the app says which one and why, and leaves
  no half-made link behind.

Bitly is deliberately **not** offered: its free tier needs a personal key,
and a key built into a web page anyone can read is not private. The popup
says this rather than showing a button that cannot work.

### Calendar export (`.ics`)

- The whole timetable, or a single course — the choice appears only when you
  have more than one course, since with one there is only one file to make.
- A date range, pre-filled from the semester CMI names.
- An optional **reminder** on every class — you pick how many minutes
  before it starts (10 unless you say otherwise).
- Honours your changes — moved classes, your own courses, your rooms — and
  courses annotated "starts …" or "runs … only" are exported with their own
  dates.
- It says plainly that CMI's holidays are **not** excluded, rather than
  pretending otherwise.

### JSON, for your own tools

Every way a timetable comes in or goes out lives in one place — the
**Share or import** button in the header — as three sections: *As a link*,
*As a timetable file* and *As a full backup*. (It was called "Share" while
sharing was all it did; the name now says both directions, because a door
labelled only with the way out is one nobody tries when carrying something
in.)

- **Export my courses** (Share or import → As a timetable file) writes your whole week as one
  machine-readable file — not a list of codes. It has two halves. `courses`
  is the readable one: every course you have picked, with its credits and
  where each number came from (CMI, the app's guess and why, or you), plus
  the meetings you actually attend, each marked as CMI's own, moved by you,
  or added by you (moved ones carry the CMI original alongside).
  `my_changes` is the exact one, and it is what lets the file be loaded back
  somewhere else: every class you moved, added or struck out (each labelled
  `moved`, `added` or `removed`, with the CMI class it replaces beside where
  you put it), every credit you corrected, and every course you wrote
  yourself, in full.
- Both halves are written for a program to read, not just for this app to
  re-read: stable keys, deterministic order, every list always present, and
  every value said twice where that helps — minutes to compute with beside
  "HH:MM" to read, an ISO weekday beside "Mon", an ISO 8601 timestamp beside
  epoch milliseconds. Reading is forgiving in the other direction, so a
  program can *write* one of these files with only the parts that carry
  meaning and this app will load it.
- **Import my courses** sits right beside it and takes such a file back in —
  one saved from another device, or one that was shared. A dialog first says
  what the file holds (how many courses, how many classes moved, how many
  credit corrections, how many of the courses were added by hand) and then
  asks, in whole sentences, whether it should **join** your timetable or
  **replace** it. Nothing changes until you pick, and either answer is one
  Ctrl+Z from undone. Focus lands on the dialog itself rather than on an
  answer, so the Space press that scrolls a long question cannot answer it.
- **A browser with nothing in it is never asked.** If you have picked no
  courses, changed nothing and written no course of your own, an import has
  nothing to replace — so the file simply loads, with no question in the
  way. The question comes back the moment there is something to lose. (The
  timetable downloaded from CMI does not count as something to lose: a sync
  fetches it again.) The whole backup skips its confirm on the same terms,
  plus one more: it also replaces your settings, so a chosen theme, row
  height or filter set is enough to bring the question back.
- **Merging keeps everything from both, apart from one kind of collision.**
  Everything already on your timetable stays; everything in the file that
  this browser can take arrives. A change made on both sides counts once.
  Where a change in the file meets a change of yours **on the same class** —
  a class cannot be in two places at once — yours stays and the app names
  the course it did that for. Classes invented on either side are additions,
  so both survive. A file is never treated as disagreeing with *itself*: if
  it holds two changes to one class, both arrive, and nothing is blamed on a
  timetable that never made a change at all.
- Nothing is ever dropped quietly. Codes this semester's catalog doesn't
  know are named and left out; a course of theirs whose code is already one
  of yours keeps yours; a course of theirs whose code CMI uses stays out, so
  a private course can't hide a real one. All of it is said before you
  choose, and again in the sentence afterwards. In the rare case where a
  course they wrote themselves arrives under a code you had saved changes
  for, those changes go — a course added by hand carries its own times —
  and the app names the course before you choose, not after. The same goes
  for a course you had **deleted** that the file brings back: it is named in
  the dialog, and the answer that would otherwise promise to take nothing
  away stops promising.
- Whole courses you **deleted** are the one thing the file does not carry: a
  deleted course is off your timetable by definition, and importing someone
  else's deletions would take courses out of your catalog. Use **Export
  everything** to move a whole browser, deletions included.
- **Export everything** (Share or import → As a full backup) writes the whole
  planner as one JSON backup: the timetable downloaded from CMI — the whole
  catalog, halls and slots — plus your selected courses, every change you
  made, your own courses, your settings and any conflicts you postponed.
  **Import everything** loads such a file back, from Share or import or from the
  welcome screen, and the planner then looks exactly like the one that made the
  file — even years later, even if CMI's site has changed or gone. It
  replaces what the browser has saved (it asks first when there is anything
  to lose), and the app stays honest about provenance: the sync pill says
  "imported", and the data keeps its original fetch date (old data does not
  become young by travelling in a file). A damaged or wrong-kind file is
  refused with a plain explanation naming what the file actually was,
  leaving what you had untouched.

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
twice a day, whenever you have it open. That first fetch is the only one it
ever asks you for — **Sync now** is there for when you'd rather not wait, not
as a chore to remember. The header says so in as many words.

The header always says **when it last synced** and by which route, counting up
on its own as time passes — "just now", "12 min ago", "2 days ago" — and
turning a warning colour once the data is two days old. CMI edits its
timetable all semester, and a planner that can't tell you how old it is isn't
worth much. Each successful sync says where it came from as it happens —
*"Timetable updated (through the helper site corsproxy.io)."* or
*"…(directly from cmi.ac.in)."* — so the route is never a mystery.

### Where the data comes from

Every route ends at **cmi.ac.in** itself:

1. **relays** — public CORS relays, all raced at once, first valid answer wins
2. **direct** — a short attempt at CMI's own URLs, only if no relay answered

**Why the relays go first.** On CMI's own network, `cmi.ac.in` is a *local*
address. A web page asking for a local address is exactly what your browser's
"this site wants to access devices on your local network" prompt is for — so
on campus, pressing Sync used to raise a warning that looks like an attack,
about the one thing this app exists to do. The relays are ordinary public
websites, so that route can never raise it.

The direct route is still here, because it is CMI's own bytes and the only
route left when the relays are down. It runs **last**, and it tells you it is
about to run: if the prompt ever does appear, the app has already said what it
is, and answering *no* costs you nothing but that one route.

The trade: the relays can see which CMI pages are being fetched — nothing else
leaves your browser, not your courses, not who you are. And since a relay's
own cache would otherwise decide how old your timetable is, the app asks it
for a fresh copy every time.

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
  which version to keep, per course, and told what each choice means. Nothing
  is answered for you: every row starts blank, Apply is disabled until you
  answer something, and it acts only on the rows you answered — the rest keep
  waiting. Answer **Decide later** and the question stays — through reloads
  too — until you answer it; the banner's **Dismiss** just hides the banner
  for this sitting (hiding a question is not answering it — it returns with
  the next sync or reload). And if CMI's change and yours turn out to say the
  same thing (same day, time and room), there is nothing to ask: your change
  is retired with a note, and CMI's own listing takes over.
- **Opening a share link in a fresh browser never invents a conflict.** A
  browser that has never synced has no history to compare, so the first sync
  asks nothing — the link's changes simply apply.
- **What changed leads with your own week.** The banner says what happened to
  *your* courses first, by name — "CMI changed 2 of your courses — TOC,
  QCOM" — and keeps the campus-wide count as a tail. When none of the change
  was yours it says exactly that, so you needn't open anything to find out.
- **Only my courses.** The digest lists everything CMI touched, on the whole
  campus — a rollover can be two hundred lines of courses you will never take.
  One box at the top of it, **Only my courses**, narrows the list to the
  courses you've picked, and the tally beside it ("3 of 30 changes") says how
  much you are choosing not to read. It stays how you leave it, so a reader
  who only cares about their own week ticks it once, ever. It is offered only
  when it can act: when none of the update touches your courses, the digest
  says that in a line instead — and when it is ticked, the per-line "in your
  timetable" badges retire, since by then every line is yours.
- **What changed keeps what a dropped course was** — name, instructor, and when
  it met — so the digest can tell you what you lost, even though the fresh
  timetable no longer knows it. The list stays one line per course; clicking
  the code opens the record as its own pop-up, laid out like a course's
  details page (instructor row, aligned day/time/hall meeting rows), with
  **Back to What changed** returning to the digest.
- **You can keep a dropped course.** That record is the last copy in
  existence — nothing about a dropped course is saved anywhere — so the
  pop-up offers **Keep this as my own course**: it becomes one of your own
  courses, with CMI's name, instructor, times and credits, and survives the
  message being dismissed, a reload and a share link. One undoable step. If
  the course was on your timetable and you had moved a class yourself, your
  time is what gets kept — CMI's old one is never put back beside it. The
  credits stay exactly what the app was already counting, guess and all, so
  keeping never quietly moves your total. When there is nothing to keep
  (it's already one of your own courses, or CMI has listed it again) the
  pop-up says so instead of offering a button that would do nothing.
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

## Staying current — when you say so

The app checks **once a day**, while it is open, whether a newer version of
itself has been published. If there is one, it **asks**.

- **A tab left open for a week finds out about the fixes.** Browsers only look
  for a new version when a page is navigated, so a tab nobody reloads can run
  last month's app indefinitely. This closes that.
- **When nothing has changed, it says nothing.** The daily check on an
  up-to-date app is completely silent — no banner, no toast, no "you're up to
  date". Only a version genuinely different from the one you loaded raises
  anything, and only the app's own files count towards that: a browser
  extension that adds a stylesheet to the page cannot make the app think it is
  out of date.
- **Nothing installs itself.** A banner says a newer version is ready and waits
  for one of two answers:
  - **Update now** — the page reloads and you are on the new version.
  - **Not now** — the banner goes. The new version is still one ordinary
    refresh away whenever you feel like it, and the app asks again tomorrow.
- **Never a reload you didn't ask for.** Not on an idle tab, not after a
  countdown, not while you're looking away. A page that reloads itself takes
  things with it that were never saved — an **Undo** you hadn't used yet, where
  you had scrolled to, a thought you were half way through.
- **Nothing you have done is lost by updating.** Your timetable, your changes
  and your preferences live in this browser, not in the page.
- **You can switch the asking off** — in the banner (**Stop checking**) or in
  My data → **App updates**. Off means the app never looks; you still get the
  newest version whenever you refresh. The same switch turns it back on, and
  **Check now** beside it asks the server there and then.
- **No internet, no problem.** If the check cannot reach the server it changes
  nothing at all, says nothing, and tries again in an hour — the app you are
  using is already downloaded, and it keeps working exactly as it did.
- **It works wherever the app is hosted.** The check asks the server the app
  was loaded from, so moving it to another repository, another domain, a
  sub-path, or off GitHub Pages entirely needs no change and no configuration.
- Developer mode shows the build id the check compares, and has the same
  **Check for an update now** button.

## Comfort and control

| | |
|:--|:--|
| **Theme** | Auto (follows your system), light, or dark — including the controls the browser draws for itself. Checkboxes, date and number fields and the scrollbars follow the theme too, so nothing on a dark page is left painted for a light one |
| **Row height** | Roomy or tight in the Master grid. Until you choose, it follows the screen you opened it on — tight on a phone, where roomy rows push most of the week off the bottom edge, and roomy on a computer. Press the button once and that is your answer everywhere, on every screen and every reload; Reset in My data hands the decision back to the device |
| **On a phone** | The timetable opens on today's classes until you pick otherwise — and then it stays picked (the whole week is one tap away), tap targets sized for fingers, a header that packs tight, and long-press to drag |
| **Motion** | Animations respect "reduce motion" |
| **The wheel** | Scroll over any box with a step — credits, a meeting's start or end time, an export date, the reminder lead, or any dropdown — and it moves one step. Hovering is enough, no click first; while the wheel is over a box, the box takes the scroll and the page behind it stays put. The reminder lead nudges by single minutes on the wheel while its arrows jump by fives. A trackpad flick counts by full notches — a step or two per gesture, not ten — an empty box is never filled by a passing wheel, and the wheel never moves a value opposite to the way you scrolled |
| **Enter** | Saves in the editor, downloads in Export, and dismisses the keyboard in a search box |
| **Escape** | Cancels a drag, then a keyboard move, then an open filter menu, then a dialog — in that order |

### Built to be used without a mouse, or without sight

- Every dialog traps Tab, opens focused on its first **field** (not a toggle),
  and hands focus back where it came from on close.
- Live regions announce validation errors, clash lines and move confirmations
  — so a screen reader hears them rather than nothing.
- Meaning is never carried by colour alone: a clash has a ⚠ and a word, a
  selected course has a ✓ and a ring.
- The day pickers and the editor's credits row are **radio groups**: one Tab
  stop each, arrow keys move the focus and the choice in the same stroke, and
  a screen reader hears one choice of six — not six separate toggles.
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
- **The app opens offline.** After one normal visit, a copy of the app
  itself is kept by your browser (a service worker), so with no connection
  the planner still opens and everything in it works — and a quiet note says
  you're offline. Only syncing with CMI's pages needs the internet. A new
  version of the app replaces the copy on your next online reload.
- **Every tab opens at once.** Switching to the Master grid or Halls used to
  take a visible beat on a slower laptop: both were working out the whole
  timetable again inside every cell of the table. They now work it out once
  and draw from it, and the filter menus build their long option lists when
  you first open one rather than for every tab you visit. On a machine four
  times slower than a desktop, the Master grid went from 114 ms to 67 ms and
  Halls from 116 ms to 86 ms — under the tenth of a second that reads as
  "instant".
- No accounts, no analytics, no cookies, no tracking. The only network
  requests it ever makes are for CMI's two timetable pages — through a public
  relay, or straight from cmi.ac.in if no relay answers. A relay learns which
  CMI page was asked for and nothing else: your courses, your changes and your
  own courses never leave your browser.

---

## For the curious

**Rust → WebAssembly**, built with [Leptos](https://leptos.dev) (client-side
rendering) and [Trunk](https://trunkrs.dev), served as static files from
GitHub Pages. The parsing, validation, merging, calendar generation and URL
codecs live in a separate crate with no browser dependencies, so they can be
tested on their own.

**Tested like it matters:** 114 native tests — including a synthetic CMI
website the tests generate themselves, with other semesters, other time
formats, renamed halls and ten different kinds of broken page — plus 86
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

- **Two tabs of the same browser share one timetable.** That is how browser
  storage works: every tab reads and writes the same saved data. Sync with
  CMI in one tab and the other picks the new timetable up on its own — the
  whole update, not just the clock on it, so what the "Synced …" pill says is
  always true of the timetable underneath it. If that other tab is in the
  middle of something — a course form with unsaved typing, a drag, a conflict
  you are answering — it waits until you have finished and then catches up in
  one step. Everything else still shows in the other tab after a refresh.
  A durable "separate
  timetable per tab" doesn't exist in the web platform — the only per-tab
  storage a browser offers is wiped when the tab closes, which would break
  the promise that your data stays until you delete it. To compare two
  plans side by side, use a second browser profile, a private window, or
  export a snapshot and a share link.
