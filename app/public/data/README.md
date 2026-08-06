# Data mirror (tier 3)

The optional `sync.yml` GitHub Actions cron commits three files here:

- `latest.json` — the parsed, validation-gated snapshot plus metadata
- `timetable.php.html` — raw copy of the CMI timetable page
- `lecturehalls.php.html` — raw copy of the CMI lecture-hall page

The app fetches these same-origin as its `mirror` source tier when the CMI
site can't be reached directly or through a CORS proxy.

The files are deliberately NOT committed by hand: the app ships no timetable
data at all (no bundled snapshot, no checked-in mirror). Until the cron has
run — or when it is disabled — the mirror tier simply 404s and the sync
chain reports honestly on the other tiers.
