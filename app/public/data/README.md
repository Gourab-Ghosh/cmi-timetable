# Data mirror (tier 3)

The optional `sync.yml` GitHub Actions cron commits three files here:

- `latest.json` — the parsed, validation-gated snapshot plus metadata
- `timetable.php.html` — raw copy of the CMI timetable page
- `lecturehalls.php.html` — raw copy of the CMI lecture-hall page

The app fetches these same-origin as its `mirror` source tier when the CMI
site can't be reached directly or through a CORS proxy. The app works fine
without them (it falls back to the snapshot bundled at build time).
