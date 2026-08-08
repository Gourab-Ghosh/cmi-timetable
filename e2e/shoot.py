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
from selenium.webdriver.support.ui import WebDriverWait

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
         port=PORT):
    d.get(f"http://127.0.0.1:{port}/e2e-blank")
    if seed:
        prefs = {
            "last_update_attempt": time.time() * 1000.0,
            "theme": theme,
            "tab": tab,
        }
        prefs.update(prefs_extra or {})
        d.execute_script(
            "localStorage.clear();"
            "localStorage.setItem('cmitt.v1.prefs', arguments[0]);"
            "localStorage.setItem('cmitt.v1.selection', arguments[1]);"
            "localStorage.setItem('cmitt.v1.overrides', arguments[2]);"
            "localStorage.setItem('cmitt.v1.snapshot', arguments[3]);",
            json.dumps(prefs),
            json.dumps(selection or ["TOC", "QCOM", "MFD", "RFLR", "SVA"]),
            json.dumps(OVERRIDES),
            SNAPSHOT_JSON,
        )
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
boot("Light", "Halls", prefs_extra={"halls_day": "Wed"})
shot("08-light-halls-with-arrival")

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

# Mobile width
d.set_window_size(390, 850)
boot("Light", "MyTimetable")
shot("15-mobile-my-timetable")
boot("Light", "Catalog")
shot("16-mobile-catalog")
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
