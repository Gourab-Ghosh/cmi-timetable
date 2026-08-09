#!/usr/bin/env python3
"""Screenshot + print-PDF harness for design review.

Serves the built app and captures every view (light/dark, desktop/mobile)
plus three print PDFs — including the dense 12-course sheet and a heavy
multi-clash sheet — into e2e/shots/ (gitignored).

    (cd ../app && trunk build --release --dist dist-e2e)
    .venv/bin/python shoot.py

Environment knobs: DIST_DIR, CHROME_BIN, PORT, CARGO_TARGET_DIR (for the
seed generator; defaults to ~/.rust-target-e2e). Like the e2e tests, the
browser blackholes all non-localhost DNS — nothing touches the network.
"""
import base64
import http.server
import json
import os
import subprocess
import threading
import time

from selenium import webdriver
from selenium.webdriver.chrome.options import Options
from selenium.webdriver.common.by import By
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import Select, WebDriverWait

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, ".."))
DIST = os.environ.get("DIST_DIR", os.path.join(REPO, "app", "dist-e2e"))
OUT = os.path.join(HERE, "shots")
PORT = int(os.environ.get("PORT", "8973"))
CHROME_BIN = os.environ.get("CHROME_BIN", "/usr/bin/chromium")
os.makedirs(OUT, exist_ok=True)

# Seed snapshot from the fixtures (the app ships no data).
env = dict(os.environ)
env.setdefault("CARGO_TARGET_DIR", os.path.expanduser("~/.rust-target-e2e"))
gen = subprocess.run(
    [
        "cargo", "run", "-q", "-p", "cmi-timetable-core",
        "--example", "snapshot_json", "--features", "html", "--",
        os.path.join(REPO, "core", "fixtures", "timetable.php.html"),
        os.path.join(REPO, "core", "fixtures", "lecturehalls.php.html"),
    ],
    capture_output=True, text=True, cwd=REPO, env=env, check=True,
)
SNAPSHOT = json.loads(gen.stdout)["snapshot"]
SNAPSHOT["fetched_at"] = time.time() * 1000.0
SNAPSHOT["source"] = "Mirror"
SNAPSHOT_JSON = json.dumps(SNAPSHOT)


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


srv = http.server.ThreadingHTTPServer(
    ("127.0.0.1", PORT), lambda *a, **k: Quiet(*a, directory=DIST, **k)
)
threading.Thread(target=srv.serve_forever, daemon=True).start()


class NoData(Quiet):
    """404 the mirror files so true first-run states are capturable —
    same-origin /data always succeeds otherwise (DNS blackhole excludes
    127.0.0.1)."""
    def send_head(self):
        if self.path.startswith("/data/"):
            self.send_error(404)
            return None
        return super().send_head()


NODATA_PORT = PORT + 1
srv2 = http.server.ThreadingHTTPServer(
    ("127.0.0.1", NODATA_PORT), lambda *a, **k: NoData(*a, directory=DIST, **k)
)
threading.Thread(target=srv2.serve_forever, daemon=True).start()

opts = Options()
opts.binary_location = CHROME_BIN
opts.add_argument("--headless=new")
opts.add_argument("--no-sandbox")
opts.add_argument("--window-size=1440,900")
opts.add_argument("--hide-scrollbars")
opts.add_argument("--force-prefers-reduced-motion")
opts.add_argument("--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1")
d = webdriver.Chrome(options=opts)
wait = WebDriverWait(d, 15)

OVERRIDES = {
    "next_id": 1,
    "items": [
        {
            "id": 0,
            "course": "SVA",
            "base": None,
            "to": {
                "day": "Wed",
                "slot": {"start_min": 1020, "end_min": 1095},
                "hall": "Seminar Hall",
                "temp_booking": False,
            },
            "created_at": 1754000000000.0,
        }
    ],
    "credits": [{"course": "TOC", "credits": 3, "created_at": 1754000000000.0}],
}


def boot(theme, tab, query="", seed=True, prefs_extra=None, selection=None,
         port=PORT, customs=None, overrides=None):
    d.get(f"http://127.0.0.1:{port}/e2e-blank")
    if seed:
        prefs = {
            "last_update_attempt": time.time() * 1000.0,
            "theme": theme,
            "tab": tab,
        }
        prefs.update(prefs_extra or {})
        script = (
            "localStorage.clear();"
            "localStorage.setItem('cmitt.v1.prefs', arguments[0]);"
            "localStorage.setItem('cmitt.v1.selection', arguments[1]);"
            "localStorage.setItem('cmitt.v1.overrides', arguments[2]);"
            "localStorage.setItem('cmitt.v1.snapshot', arguments[3]);"
        )
        args = [
            json.dumps(prefs),
            json.dumps(selection or ["TOC", "QCOM", "MFD", "RFLR", "SVA"]),
            json.dumps(overrides if overrides is not None else OVERRIDES),
            SNAPSHOT_JSON,
        ]
        if customs is not None:
            script += "localStorage.setItem('cmitt.v1.custom', arguments[4]);"
            args.append(json.dumps(customs))
        d.execute_script(script, *args)
    else:
        d.execute_script(
            "localStorage.clear();"
            "localStorage.setItem('cmitt.v1.prefs', arguments[0]);",
            json.dumps({"theme": theme, "last_update_attempt": 0}),
        )
    d.get(f"http://127.0.0.1:{port}/{query}")
    wait.until(EC.presence_of_element_located((By.CSS_SELECTOR, ".header h1")))
    time.sleep(0.9)  # fonts/layout settle


def shot(name):
    d.save_screenshot(os.path.join(OUT, f"{name}.png"))
    print("shot", name)


def print_pdf(name):
    pdf = d.execute_cdp_cmd(
        "Page.printToPDF",
        {
            "landscape": True,
            "printBackground": True,
            "preferCSSPageSize": True,
            "paperWidth": 11.69,
            "paperHeight": 8.27,
        },
    )
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(base64.b64decode(pdf["data"]))
    print(name, "written")


boot("Light", "MyTimetable")
shot("01-light-my-timetable")

boot("Dark", "MyTimetable")
shot("02-dark-my-timetable")

boot("Light", "MasterGrid")
shot("03-light-master-grid")

boot("Dark", "MasterGrid")
shot("04-dark-master-grid")

boot("Light", "Catalog")
shot("05-light-catalog")
d.find_elements(By.CSS_SELECTOR, ".filterbar details.facet > summary")[0].click()
time.sleep(0.4)
shot("06-light-catalog-facet-open")

# Active filters → chipline spacing
boot(
    "Light", "Catalog",
    prefs_extra={"filters": {"branches": ["BM2"], "text": "theory"}},
)
shot("06b-light-catalog-filterchips")

boot("Dark", "MyCourses")
shot("07-dark-my-courses")

# Halls on Wednesday: the SVA override lands in Seminar Hall 17:00 (arrival).
boot("Light", "Halls", prefs_extra={"halls_view": {"Day": "Wed"}})
shot("08-light-halls-with-arrival")

# Every day at once: one table, each hall named once with its days under it.
boot("Light", "Halls", prefs_extra={"halls_view": "All"})
shot("32-light-halls-all-days")

boot("Dark", "Halls", prefs_extra={"halls_view": "All"})
shot("35-dark-halls-all-days")


def _m(day, start, end, hall):
    return {"day": day, "slot": {"start_min": start, "end_min": end},
            "hall": hall, "temp_booking": False}


# Your changes, one group per kind — the point being that a particular
# change is findable in a list of many.
MANY_CHANGES = {
    "next_id": 4,
    "items": [
        {"id": 0, "course": "TOC", "base": _m("Tue", 550, 625, "Lecture Hall 803"),
         "to": _m("Wed", 1020, 1095, "Lecture Hall 803"),
         "created_at": 1754000000000.0},
        {"id": 1, "course": "ISS", "base": _m("Tue", 550, 625, "Lecture Hall 803"),
         "to": _m("Tue", 550, 625, "Seminar Hall"), "created_at": 1754000001000.0},
        {"id": 2, "course": "TOC", "base": _m("Thu", 550, 625, "Lecture Hall 803"),
         "to": None, "created_at": 1754000002000.0},
        {"id": 3, "course": "SVA", "base": None,
         "to": _m("Wed", 1020, 1095, "Seminar Hall"),
         "created_at": 1754000003000.0},
    ],
    "credits": [{"course": "QCOM", "credits": 2, "created_at": 1754000004000.0}],
}
boot("Light", "MyTimetable", selection=["TOC", "ISS", "QCOM", "SVA"],
     overrides=MANY_CHANGES)
d.execute_script(
    "document.querySelector(\"[data-testid='your-changes']\")"
    ".scrollIntoView({block: 'center'});"
)
time.sleep(0.4)
shot("33-light-your-changes")

# A course dialog carrying both lists at once: several meetings, and more
# than one clash (TOC and ISS collide on Tuesday AND Thursday).
boot("Light", "MyTimetable", selection=["TOC", "ISS", "QCOM", "SVA"])
d.find_element(By.CSS_SELECTOR, "button.chip[aria-label^='TOC,']").click()
time.sleep(0.5)
shot("34-light-details-meetings-and-clashes")

boot("Light", "MyTimetable")
d.find_element(By.CSS_SELECTOR, "button.chip[aria-label^='TOC,']").click()
time.sleep(0.5)
shot("09-light-details-dialog")

boot("Dark", "MyTimetable")
d.find_element(By.XPATH, "//button[normalize-space()='My data']").click()
time.sleep(0.5)
shot("10-dark-my-data-dialog")

boot("Light", "MyTimetable")
d.find_element(By.XPATH, "//button[normalize-space()='My data']").click()
time.sleep(0.5)
shot("11-light-my-data-dialog")

boot("Light", "MyCourses")
shot("07b-light-my-courses")

# First-run welcome: served WITHOUT /data so the mirror tier fails too and
# the real hero card renders (all other hosts are blackholed already).
boot("Light", "MyTimetable", seed=False, port=NODATA_PORT)
time.sleep(2.5)
shot("12-light-welcome")
boot("Dark", "MyTimetable", seed=False, port=NODATA_PORT)
time.sleep(2.5)
shot("13-dark-welcome")

# Compact density lives on the Master grid (that's where the toggle is).
boot("Light", "MasterGrid", prefs_extra={"density": "Compact"})
wait.until(EC.presence_of_element_located((By.CSS_SELECTOR, ".density-compact")))
shot("14-light-compact")

# Custom courses: a scheduled one (GERMAN, on the grid) and a parked one
# (RG, kept but off the timetable).
CUSTOMS = {
    "courses": [
        {
            "code": "GERMAN",
            "name": "German A1",
            "instructors": ["Goethe-Institut"],
            "branches": [],
            "credits": 2,
            "starts": None,
            "part_of_semester": None,
            "optional_flag": False,
            "status": "Scheduled",
            "meetings": [
                {"day": "Tue", "slot": {"start_min": 550, "end_min": 625},
                 "hall": None, "temp_booking": False},
                {"day": "Mon", "slot": {"start_min": 1110, "end_min": 1185},
                 "hall": "Sports annexe", "temp_booking": False},
            ],
        },
        {
            "code": "RG",
            "name": "Algebra reading group",
            "instructors": [],
            "branches": [],
            "credits": 0,
            "starts": None,
            "part_of_semester": None,
            "optional_flag": False,
            "status": "UnscheduledListed",
            "meetings": [],
        },
    ]
}
CUSTOM_SEL = ["TOC", "QCOM", "MFD", "GERMAN"]

boot("Light", "MyCourses", selection=CUSTOM_SEL, customs=CUSTOMS)
shot("18-light-my-courses-custom")
boot("Dark", "MyCourses", selection=CUSTOM_SEL, customs=CUSTOMS)
shot("19-dark-my-courses-custom")

# The create form, with one official-slot row and one custom-time row.
boot("Light", "MyCourses", selection=CUSTOM_SEL, customs=CUSTOMS)
d.find_element(By.CSS_SELECTOR, ".add-own-card").click()
time.sleep(0.4)
d.find_element(By.CSS_SELECTOR, "#cc-name").send_keys("Music lesson")
d.find_element(By.CSS_SELECTOR, "#cc-add-meeting").click()
time.sleep(0.3)
d.find_element(
    By.CSS_SELECTOR, ".meeting-draft select[aria-label='Time'] option[value='custom']"
).click()
time.sleep(0.4)
shot("20-light-custom-form")
d.find_element(By.XPATH, "//button[normalize-space()='Cancel']").click()

boot("Dark", "MyCourses", selection=CUSTOM_SEL, customs=CUSTOMS)
d.find_element(By.CSS_SELECTOR, ".add-own-card").click()
time.sleep(0.4)
d.find_element(By.CSS_SELECTOR, "#cc-add-meeting").click()
time.sleep(0.4)
shot("21-dark-custom-form")
d.find_element(By.XPATH, "//button[normalize-space()='Cancel']").click()

# Edit mode: prefilled rows + the quiet-danger delete in the footer.
boot("Light", "MyCourses", selection=CUSTOM_SEL, customs=CUSTOMS)
d.find_element(By.XPATH, "//button[normalize-space()='Edit course']").click()
time.sleep(0.4)
shot("22-light-custom-form-edit")
d.find_element(By.XPATH, "//button[normalize-space()='Cancel']").click()

# My timetable with a custom evening column AND both under-grid panels
# (clashes + your changes) — the gap regression's home ground. GERMAN's
# Tue 09:10 intentionally clashes with TOC.
boot("Light", "MyTimetable", selection=CUSTOM_SEL, customs=CUSTOMS)
shot("23-light-tt-custom-panels")

# Halls with the user's own data in it: a row for a place CMI never listed
# ("Sports annexe", badged "yours") and a column for the evening meeting
# sitting in it.
boot("Light", "Halls", selection=CUSTOM_SEL, customs=CUSTOMS,
     prefs_extra={"halls_view": {"Day": "Mon"}})
# The own row sits below CMI's fifteen, so bring it into frame.
d.execute_script(
    "document.querySelector('tr.own-hall').scrollIntoView({block:'center'});"
)
time.sleep(0.4)
shot("29-light-halls-own-place")

# The foot of My courses: the create tile and the parked group (RG is in
# the custom store but not in the selection).
boot("Light", "MyCourses", selection=CUSTOM_SEL, customs=CUSTOMS)
d.execute_script("window.scrollTo(0, document.body.scrollHeight);")
time.sleep(0.5)
shot("25-light-parked-and-tile")

# A custom course whose code CMI later listed too: the shadow note and its
# one-click way out.
SHADOW = {"courses": [dict(CUSTOMS["courses"][0], code="TOC", name="My own TOC notes")]}
boot("Light", "MyCourses", selection=["TOC", "MFD"], customs=SHADOW)
d.find_element(By.CSS_SELECTOR, "button.chip[aria-label^='TOC,']").click()
time.sleep(0.5)
shot("26-light-shadow-note")
d.find_element(By.XPATH, "//button[normalize-space()='Close']").click()

# The free-hall answer, and the master grid with an evening column of its
# own (a course moved to 18:30 must not clamp into CMI's last slot).
EVENING = {
    "next_id": 1,
    "items": [{
        "id": 0, "course": "TOC",
        "base": {"day": "Tue", "slot": {"start_min": 550, "end_min": 625},
                 "hall": "Lecture Hall 803", "temp_booking": False},
        "to": {"day": "Tue", "slot": {"start_min": 1110, "end_min": 1185},
               "hall": "Lecture Hall 803", "temp_booking": False},
        "created_at": 1754000000000.0}],
    "credits": [],
}
boot("Light", "Halls", selection=CUSTOM_SEL, customs=CUSTOMS,
     prefs_extra={"halls_view": {"Day": "Tue"}})
Select(d.find_element(By.CSS_SELECTOR, "select[aria-label='Day']")).select_by_value("1")
Select(
    d.find_element(By.CSS_SELECTOR, "select[aria-label='Time slot']")
).select_by_value("840")
time.sleep(0.4)
d.execute_script(
    "document.querySelector('.finder-result').scrollIntoView({block:'center'});"
)
time.sleep(0.3)
shot("30-light-free-hall-answer")

boot("Light", "MasterGrid", selection=["TOC"])
d.execute_script(
    "localStorage.setItem('cmitt.v1.overrides', arguments[0]);", json.dumps(EVENING)
)
d.refresh()
time.sleep(1.2)
shot("31-light-master-grid-extra-column")

# The edit-meeting dialog: day, time and the hall dropdown — and the same
# dialog with "Other place…" chosen, which opens the free-text box.
boot("Light", "MyTimetable", selection=["TOC"])
d.find_element(
    By.CSS_SELECTOR, "td[data-day='1'][data-slot='550'] button.chip"
).click()
time.sleep(0.4)
next(
    r for r in d.find_elements(By.CSS_SELECTOR, ".dialog ul.meetings li")
    if "Tue" in r.text
).find_element(By.XPATH, ".//button[normalize-space()='Edit']").click()
time.sleep(0.4)
shot("27-light-edit-meeting")
Select(d.find_element(By.CSS_SELECTOR, "#em-hall")).select_by_visible_text(
    "Other place…"
)
time.sleep(0.3)
shot("28-light-edit-meeting-other-place")
d.find_element(By.XPATH, "//button[normalize-space()='Cancel']").click()

# Mobile width
d.set_window_size(390, 850)
boot("Light", "MyTimetable")
shot("15-mobile-my-timetable")
boot("Light", "Catalog")
shot("16-mobile-catalog")
# The custom-course form at phone width — sticky actions, wrapping rows.
boot("Light", "MyCourses", selection=CUSTOM_SEL, customs=CUSTOMS)
tile = d.find_element(By.CSS_SELECTOR, ".add-own-card")
d.execute_script("arguments[0].scrollIntoView({block:'center'});", tile)
time.sleep(0.2)
tile.click()
time.sleep(0.4)
add = d.find_element(By.CSS_SELECTOR, "#cc-add-meeting")
d.execute_script("arguments[0].scrollIntoView({block:'center'});", add)
time.sleep(0.2)
add.click()
time.sleep(0.4)
shot("24-mobile-custom-form")
d.find_element(By.XPATH, "//button[normalize-space()='Cancel']").click()
boot("Light", "MyTimetable", seed=False, port=NODATA_PORT)
time.sleep(2.5)
shot("17-mobile-welcome")
d.set_window_size(1440, 900)

# Print PDF (from light my timetable)
boot("Light", "MyTimetable")
print_pdf("print.pdf")

# A dense 12-course semester (one page, nothing clipped; 2 clashes).
boot("Light", "MyTimetable", selection=[
    "AML", "MAAT", "MFD", "MPML", "MVG", "QCOM",
    "RDBM", "SVA", "TAGT", "TSA", "VISU", "STPR",
])
print_pdf("print-12.pdf")

# Heavy clash stress: a triple-booked cell (TOC+ISS+NLP on Thu 09:10) plus
# the natural MAAT×MVG and TAGT×VISU overlaps — 6 clash lines.
boot("Light", "MyTimetable", selection=[
    "TOC", "ISS", "NLP", "MAAT", "MVG", "TAGT", "VISU", "MFD",
])
print_pdf("print-clash.pdf")

d.quit()
srv.shutdown()
srv2.shutdown()
