# Data mirror (tier 3)

`./deploy.sh --sync` (which runs the `/sync` binary) writes three files here:

- `latest.json` — the parsed, validation-gated snapshot plus metadata
- `timetable.php.html` — raw copy of the CMI timetable page
- `lecturehalls.php.html` — raw copy of the CMI lecture-hall page

The app fetches these same-origin as its `mirror` source tier when the CMI
site can't be reached directly or through a CORS proxy.

The files are deliberately NOT written by hand: the parser and the
validation gate are the only judges of what lands here, and the app ships no
timetable data at all (no bundled snapshot). Before the first sync the mirror
tier simply 404s and the sync chain reports honestly on the other tiers.
