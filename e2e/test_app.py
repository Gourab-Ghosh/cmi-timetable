#!/usr/bin/env python3
"""End-to-end browser tests for the CMI Timetable Planner.

Serves the built app (app/dist) on a local port and drives it with Selenium
(headless Chromium). Run after `trunk build --release`:

    python e2e/test_app.py

Environment:
    DIST_DIR    directory to serve   (default: ../app/dist relative to this file)
    CHROME_BIN  browser binary       (default: /usr/bin/chromium)
    PORT        local port           (default: 8977)
    CARGO_TARGET_DIR  target dir for the seed generator build
                      (default: ~/.rust-target-e2e, so a running
                      `trunk serve` can never race it)

The app ships no timetable data, so the suite derives a snapshot from the
committed fixtures at startup (core's `snapshot_json` example) and seeds it
into localStorage before each test — every test still runs offline and
deterministically. The browser is started with all non-localhost DNS
blackholed, so the direct/proxy sync tiers fail instantly and nothing ever
touches the real network; the first-run tests serve that same seed as a fake
same-origin mirror instead.
"""

import http.server
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import traceback

from selenium import webdriver
from selenium.webdriver.chrome.options import Options
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import Select, WebDriverWait

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, ".."))
DIST = os.environ.get("DIST_DIR", os.path.join(HERE, "..", "app", "dist"))
PORT = int(os.environ.get("PORT", "8977"))
BASE = f"http://127.0.0.1:{PORT}"
CHROME_BIN = os.environ.get("CHROME_BIN", "/usr/bin/chromium")
FIXTURES = os.path.join(REPO, "core", "fixtures")
DOWNLOADS = tempfile.mkdtemp(prefix="cmitt-e2e-dl-")

# TOC's official Tue meeting moved by the user to Wed 17:00, plus a credit
# change — the canonical "user customised things" seed.
TOC_OVR = {
    "next_id": 1,
    "items": [{
        "id": 0, "course": "TOC",
        "base": {"day": "Tue", "slot": {"start_min": 550, "end_min": 625},
                 "hall": "Lecture Hall 803", "temp_booking": False},
        "to": {"day": "Wed", "slot": {"start_min": 1020, "end_min": 1095},
               "hall": "Lecture Hall 803", "temp_booking": False},
        "created_at": 1754000000000.0}],
    "credits": [{"course": "TOC", "credits": 3, "created_at": 1754000000000.0}],
}

# Filled by build_seed() at startup: the parsed mirror-format dict and the
# ready-to-store snapshot JSON string.
SEED_LATEST = None
SEED_SNAPSHOT_JSON = None


def build_seed():
    """Derive the seed snapshot from the committed fixtures with the exact
    same parser the app uses (core's `snapshot_json` example)."""
    global SEED_LATEST, SEED_SNAPSHOT_JSON
    env = dict(os.environ)
    env.setdefault(
        "CARGO_TARGET_DIR", os.path.expanduser("~/.rust-target-e2e")
    )
    result = subprocess.run(
        [
            "cargo", "run", "-q", "-p", "cmi-timetable-core",
            "--example", "snapshot_json", "--features", "html", "--",
            os.path.join(FIXTURES, "timetable.php.html"),
            os.path.join(FIXTURES, "lecturehalls.php.html"),
        ],
        capture_output=True, text=True, cwd=REPO, env=env,
    )
    if result.returncode != 0:
        sys.exit(f"seed generation failed:\n{result.stderr}")
    SEED_LATEST = json.loads(result.stdout)
    snapshot = SEED_LATEST["snapshot"]
    snapshot["fetched_at"] = time.time() * 1000.0
    snapshot["source"] = "Mirror"
    SEED_SNAPSHOT_JSON = json.dumps(snapshot)


def write_fake_mirror(mutate=None):
    """Publish the seed as a same-origin mirror (DIST/data/) so a real sync
    can succeed through the mirror tier — external hosts are blackholed.
    With `mutate`, the snapshot inside latest.json is edited (simulating an
    upstream CMI change) and the raw HTML copies are omitted so the client
    adopts the CI-validated snapshot instead of re-parsing raw pages."""
    data_dir = os.path.join(DIST, "data")
    os.makedirs(data_dir, exist_ok=True)
    latest = dict(SEED_LATEST)
    latest["generated_at"] = time.time() * 1000.0
    if mutate is not None:
        snap = json.loads(json.dumps(SEED_LATEST["snapshot"]))
        mutate(snap)
        latest = dict(latest, snapshot=snap)
    with open(os.path.join(data_dir, "latest.json"), "w") as f:
        json.dump(latest, f)
    for name in ("timetable.php.html", "lecturehalls.php.html"):
        target = os.path.join(data_dir, name)
        if mutate is None:
            shutil.copy(os.path.join(FIXTURES, name), target)
        elif os.path.exists(target):
            os.remove(target)


def remove_fake_mirror():
    shutil.rmtree(os.path.join(DIST, "data"), ignore_errors=True)


def serve_dist():
    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", PORT),
        lambda *a, **kw: Quiet(*a, directory=DIST, **kw),
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def make_driver():
    opts = Options()
    opts.binary_location = CHROME_BIN
    opts.add_argument("--headless=new")
    opts.add_argument("--no-sandbox")
    opts.add_argument("--disable-gpu")
    opts.add_argument("--window-size=1500,1000")
    # The stylesheet honors prefers-reduced-motion; forcing it here disables
    # entry animations so dialogs are fully visible the moment they mount.
    opts.add_argument("--force-prefers-reduced-motion")
    # Blackhole every non-localhost hostname: sync's direct/proxy tiers fail
    # instantly and deterministically, and no test ever touches the network.
    opts.add_argument(
        "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1"
    )
    opts.add_experimental_option("prefs", {
        "download.default_directory": DOWNLOADS,
        "download.prompt_for_download": False,
    })
    return webdriver.Chrome(options=opts)


class App:
    def __init__(self, driver):
        self.d = driver
        self.wait = WebDriverWait(driver, 15)

    # -- lifecycle ---------------------------------------------------------

    def boot(self, path="/", fresh=True, seed=True, selection=None,
             overrides=None, raw_snapshot=None, customs=None):
        """Load the app. fresh=True wipes storage; seed=True (the default)
        pre-loads the fixture-derived snapshot and suppresses the background
        sync, so tests run on deterministic data. seed=False boots the app
        the way a first-time visitor sees it: empty. selection/overrides/
        customs pre-seed those stores; raw_snapshot stores an arbitrary (e.g.
        corrupt) blob in the snapshot slot."""
        if fresh:
            self.d.get(f"{BASE}/e2e-blank")  # same-origin 404 page
            if seed:
                script = (
                    "localStorage.clear();"
                    "localStorage.setItem('cmitt.v1.prefs', arguments[0]);"
                    "localStorage.setItem('cmitt.v1.snapshot', arguments[1]);"
                )
                args = [
                    json.dumps({"last_update_attempt": time.time() * 1000.0}),
                    raw_snapshot if raw_snapshot is not None else SEED_SNAPSHOT_JSON,
                ]
                if selection is not None:
                    script += f"localStorage.setItem('cmitt.v1.selection', arguments[{len(args)}]);"
                    args.append(json.dumps(selection))
                if overrides is not None:
                    script += f"localStorage.setItem('cmitt.v1.overrides', arguments[{len(args)}]);"
                    args.append(json.dumps(overrides))
                if customs is not None:
                    script += f"localStorage.setItem('cmitt.v1.custom', arguments[{len(args)}]);"
                    args.append(json.dumps(customs))
                self.d.execute_script(script, *args)
            else:
                self.d.execute_script("localStorage.clear();")
        self.d.get(f"{BASE}{path}")
        self.wait.until(EC.presence_of_element_located((By.CSS_SELECTOR, ".header h1")))

    # -- helpers -----------------------------------------------------------

    def css(self, sel):
        return self.d.find_element(By.CSS_SELECTOR, sel)

    def css_all(self, sel):
        return self.d.find_elements(By.CSS_SELECTOR, sel)

    def xpath(self, expr):
        return self.d.find_element(By.XPATH, expr)

    def wait_css(self, sel, timeout=15):
        return WebDriverWait(self.d, timeout).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, sel))
        )

    def wait_gone(self, sel, timeout=15):
        WebDriverWait(self.d, timeout).until(
            EC.invisibility_of_element_located((By.CSS_SELECTOR, sel))
        )

    def open_tab(self, label):
        self.d.execute_script("window.scrollTo(0, 0);")
        self.xpath(
            f"//button[@role='tab' and normalize-space()='{label}']"
        ).click()

    def toasts_text(self):
        return " | ".join(t.text for t in self.css_all(".toasts .toast"))

    def wait_toast(self, fragment, timeout=10):
        WebDriverWait(self.d, timeout).until(
            lambda d: fragment in self.toasts_text(),
            message=f"expected a toast containing {fragment!r}; got: {self.toasts_text()!r}",
        )

    def chip(self, code, container="body"):
        return self.d.find_element(
            By.CSS_SELECTOR, f"{container} button.chip[aria-label^='{code},']"
        )

    def chips(self, code, container="body"):
        return self.d.find_elements(
            By.CSS_SELECTOR, f"{container} button.chip[aria-label^='{code},']"
        )

    def cell(self, day, slot_start):
        return self.css(f"td[data-day='{day}'][data-slot='{slot_start}']")

    def drag(self, elem, target):
        (
            ActionChains(self.d)
            .click_and_hold(elem)
            .move_by_offset(12, 0)  # pass the drag threshold
            .move_to_element(target)
            .pause(0.15)
            .release()
            .perform()
        )
        time.sleep(0.5)  # let the click-suppression window lapse


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def t01_header_sync_button_and_hidden_dev(app):
    """'Sync now' button present; no Developer tab or link anywhere."""
    app.boot("/")
    app.xpath("//button[normalize-space()='Sync now']")
    app.xpath("//button[normalize-space()='My data']")
    tabs = [t.text for t in app.css_all(".tabs .tab")]
    assert "Developer" not in tabs, f"Developer tab should be hidden, got {tabs}"
    assert tabs == [
        "My timetable", "My courses", "Master grid", "Catalog", "Halls",
    ], tabs


def t02_developer_endpoint_only(app):
    """Developer mode still works when its endpoint is opened directly."""
    app.boot("/#/developer")
    section = app.wait_css("section[aria-label='Developer mode']")
    assert "Developer mode" in section.text
    for panel in ("Build info", "Fetch log", "Parse reports",
                  "Cache inspector", "Raw HTML viewer", "Simulators"):
        assert panel in section.text, f"missing panel {panel}"


def t03_url_selection_and_clash(app):
    """?c= reproduces a selection; overlapping courses get clash warnings."""
    app.boot("/?c=TOC,ISS")  # both meet Tue+Thu 09:10-10:25
    app.wait_css("button.chip.clash")
    clash_chips = app.css_all("button.chip.clash")
    labels = " ".join(c.get_attribute("aria-label") for c in clash_chips)
    assert "TOC" in labels and "ISS" in labels, labels
    assert "clashes with" in labels
    panel = app.xpath("//div[contains(@class,'panel')][.//h3[contains(.,'Clashes')]]")
    assert "TOC" in panel.text and "ISS" in panel.text


def t04_unknown_code_warning(app):
    """Unknown codes warn without breaking the known selection."""
    app.boot("/?c=TOC,XYZQ")
    banner = app.wait_css(".banner")
    assert "Unknown course code" in banner.text, banner.text
    # The code is a chip of its own, not buried in the sentence.
    assert banner.find_element(By.CSS_SELECTOR, ".chip").text == "XYZQ"
    app.chip("TOC")  # TOC still selected and rendered
    banner.find_element(By.XPATH, ".//button[normalize-space()='Dismiss']").click()
    app.wait_gone(".banner")


def t05_credits_default_four(app):
    """Unstated credits count as 4; stated ones stay verbatim."""
    app.boot("/?c=TOC,RDBM")
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    total = app.css("section[aria-label='My courses'] .credit-summary .cs-num").text
    assert total == "6", total  # 4 (assumed) + 2
    assert "credits in total" in section.text, section.text
    assert "CMI doesn't list credits for 1 course" in section.text, section.text
    assert "counted as 4 here" in section.text, section.text
    # Details dialog marks the assumption.
    app.chip("TOC").click()
    dialog = app.wait_css(".dialog")
    assert "4 (assumed" in dialog.text, dialog.text
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()


def t06_master_grid_wont_fit_warning(app):
    """Unselected clashing courses carry the ⚠ marker in the master grid,
    with 'Fits my schedule' OFF."""
    app.boot("/?c=TOC")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    # NLP meets Thu 09:10 (clashes with TOC) and is not selected.
    nlp = app.chip("NLP")
    assert nlp.find_elements(By.CSS_SELECTOR, ".wontfit"), \
        "NLP chip should carry the ⚠ would-clash marker"
    assert "would clash" in nlp.get_attribute("aria-label")
    # A non-clashing unselected course carries no marker (MFD: Mon/Wed/Fri
    # afternoons in the bundled snapshot).
    mfd = app.chip("MFD")
    assert not mfd.find_elements(By.CSS_SELECTOR, ".wontfit"), \
        "MFD should not be marked as clashing"


def t07_clash_toast_on_add(app):
    """Adding a clashing course warns immediately, naming the partner."""
    app.boot("/?c=TOC")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    app.chip("NLP").click()
    app.wait_toast("Added NLP")
    assert "clashes with TOC" in app.toasts_text(), app.toasts_text()


def t08_master_grid_info_button(app):
    """The ⓘ button opens full course details from the master grid."""
    app.boot("/")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    app.css("button.chip-info[aria-label='Details for TOC']").click()
    dialog = app.wait_css(".dialog")
    assert "Theory of Computation" in dialog.text
    assert "C Aiswarya" in dialog.text
    assert "BM2" in dialog.text and "MC1" in dialog.text


def t09_drag_requires_edit_mode(app):
    """Dragging does nothing until 'Edit layout' is turned on; then a drop
    creates an override (dashed chip + toast)."""
    app.boot("/?c=TOC")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")

    origin = app.cell(1, 550)   # Tue 09:10 — TOC's official slot
    target = app.cell(2, 1020)  # Wed 17:00 — empty

    # Without edit mode the drag is inert.
    app.drag(app.chip("TOC", "td[data-day='1'][data-slot='550']"), target)
    assert not app.chips("TOC", "td[data-day='2'][data-slot='1020']"), \
        "drag must be inert outside edit mode"
    assert "Moved" not in app.toasts_text()

    # Turn on edit mode and drag for real.
    app.xpath("//button[contains(.,'Edit layout')]").click()
    app.drag(app.chip("TOC", "td[data-day='1'][data-slot='550']"), target)
    app.wait_toast("Moved TOC")
    moved = app.chip("TOC", "td[data-day='2'][data-slot='1020']")
    assert "overridden" in moved.get_attribute("class"), \
        "moved chip should render as overridden"
    assert not app.chips("TOC", "td[data-day='1'][data-slot='550']"), \
        "chip should have left its official Tuesday slot"
    assert origin is not None


def t10_deselect_keeps_custom_time(app):
    """THE bug fix: deselecting after a drag must not snap the course back."""
    t09_drag_requires_edit_mode(app)  # leaves TOC selected + moved to Wed 17:00
    time.sleep(0.4)
    # Deselect via click (still in the master grid).
    app.chip("TOC", "td[data-day='2'][data-slot='1020']").click()
    app.wait_toast("Removed TOC")
    assert app.chips("TOC", "td[data-day='2'][data-slot='1020']"), \
        "deselected course must stay at its custom slot"
    assert not app.chips("TOC", "td[data-day='1'][data-slot='550']"), \
        "deselected course must NOT snap back to its official slot"
    # Re-select: custom time still in effect.
    time.sleep(0.2)
    app.chip("TOC", "td[data-day='2'][data-slot='1020']").click()
    app.wait_toast("Added TOC")
    assert app.chips("TOC", "td[data-day='2'][data-slot='1020']")


def t11_my_data_lists_and_removes_overrides(app):
    """'My data' shows exactly what the custom time overwrites and removes it."""
    t09_drag_requires_edit_mode(app)
    time.sleep(0.4)
    app.xpath("//button[normalize-space()='My data']").click()
    dialog = app.wait_css(".dialog")
    assert "Your changes" in dialog.text
    assert "TOC" in dialog.text and "→" in dialog.text, dialog.text
    assert "Tue 09:10–10:25" in dialog.text and "Wed 17:00–18:15" in dialog.text, \
        f"override line should show official → custom: {dialog.text!r}"
    dialog.find_element(
        By.XPATH, ".//li[contains(.,'TOC')]//button[normalize-space()='Remove']"
    ).click()
    WebDriverWait(app.d, 10).until(
        lambda d: "None. Meetings you move" in app.css(".dialog").text
    )
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()
    # Back on CMI's official Tuesday slot.
    app.wait_css("td[data-day='1'][data-slot='550'] button.chip[aria-label^='TOC,']")


def t12_undo_redo(app):
    """Undo restores the custom time removed in My data."""
    t11_my_data_lists_and_removes_overrides(app)
    app.xpath("//button[@aria-label='Undo']").click()
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    app.xpath("//button[@aria-label='Redo']").click()
    app.wait_css("td[data-day='1'][data-slot='550'] button.chip[aria-label^='TOC,']")


def t13_reload_persists_state(app):
    """Selection and custom times survive a reload (saved in the browser)."""
    t09_drag_requires_edit_mode(app)
    time.sleep(0.4)
    app.boot("/", fresh=False)  # plain reload, keep storage
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    assert app.chips("TOC", "td[data-day='2'][data-slot='1020']"), \
        "custom time must survive a reload"


def t14_edit_dialog_and_unscheduled(app):
    """'Give it a time' from details selects the course and places it."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    app.chip("SVA").click()  # unscheduled course → details dialog
    dialog = app.wait_css(".dialog")
    assert "hasn't put it on the timetable" in dialog.text
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Give it a time']").click()
    app.wait_css("#em-day")
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save']").click()
    app.wait_toast("Added SVA")
    app.open_tab("My timetable")
    app.wait_css("td[data-day='0'][data-slot='550'] button.chip[aria-label^='SVA,']")


def t15_halls_free_finder(app):
    """Free-hall finder needs BOTH day and slot, then lists only free halls."""
    app.boot("/")
    app.open_tab("Halls")
    section = app.wait_css("section[aria-label='Lecture halls']")
    day_sel = section.find_element(By.CSS_SELECTOR, "select[aria-label='Day']")
    slot_sel = section.find_element(By.CSS_SELECTOR, "select[aria-label='Time slot']")
    # Slot picked but no day: no answer yet.
    slot_sel.find_element(By.CSS_SELECTOR, "option[value='840']").click()
    assert not app.css_all(".finder-result")
    day_sel.find_element(By.CSS_SELECTOR, "option[value='1']").click()  # Tuesday
    app.wait_css(".finder-result")
    # Tue 14:00: Seminar Hall is free, Lecture Hall 6 is not (LIEA). The
    # answer is a list of rooms, one chip each, led by the count.
    free = [li.text for li in app.css_all(".hall-list li")]
    assert "Seminar Hall" in free, free
    assert "Lecture Hall 6" not in free, free
    assert app.css(".finder-count").text == str(len(free)), free
    assert "Tuesday" in app.css(".finder-when").text


def t16_facet_menus_close_each_other(app):
    """Opening one filter dropdown closes the previous one; outside clicks
    and Esc close them; clicks INSIDE a menu keep it open."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .filterbar")

    def facet(i):
        return app.css_all(".filterbar details.facet")[i]

    def summary(i):
        return app.css_all(".filterbar details.facet > summary")[i]

    summary(0).click()  # Branch
    assert facet(0).get_attribute("open") is not None
    # A click inside the open menu must NOT close it.
    facet(0).find_element(By.CSS_SELECTOR, ".menu label.opt input").click()
    time.sleep(0.2)
    assert facet(0).get_attribute("open") is not None, \
        "clicking a checkbox inside the menu must not close it"
    summary(1).click()  # Instructor — must close Branch
    time.sleep(0.2)
    assert facet(0).get_attribute("open") is None, \
        "opening the second menu must close the first"
    assert facet(1).get_attribute("open") is not None
    # Clicking anywhere outside closes the open menu.
    app.css("section[aria-label='Catalog'] .toolbar h2").click()
    time.sleep(0.2)
    assert all(
        f.get_attribute("open") is None
        for f in app.css_all(".filterbar details.facet")
    ), "outside click must close every open menu"
    # Esc closes too.
    summary(2).click()
    assert facet(2).get_attribute("open") is not None
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    time.sleep(0.2)
    assert all(
        f.get_attribute("open") is None
        for f in app.css_all(".filterbar details.facet")
    ), "Esc must close every open menu"


def t17_credit_override(app):
    """Credits can be overwritten per course, feed the total, and are listed
    with their official value and removable."""
    app.boot("/?c=TOC,RDBM")
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "6"  # 4 assumed + 2 stated
    app.chip("TOC").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(By.XPATH, ".//dd//button[normalize-space()='Edit']").click()
    inp = dialog.find_element(By.CSS_SELECTOR, "input[aria-label='Credits']")
    inp.clear()
    inp.send_keys("3")
    dialog.find_element(By.XPATH, ".//dd//button[normalize-space()='Save']").click()
    app.wait_toast("TOC now counts as 3 credits")
    WebDriverWait(app.d, 10).until(
        lambda d: "3 (set by you — CMI: 4 assumed)" in app.css(".dialog").text
    )
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()
    section = app.css("section[aria-label='My courses']")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "5", section.text
    assert "1 credit value set by you." in section.text, section.text
    # The 'Your changes' panel shows official → yours; removing it restores.
    app.open_tab("My timetable")
    panel = app.wait_css("[data-testid='your-changes']")
    assert "Credits you set" in panel.text, panel.text
    assert "4 (assumed) → 3" in panel.text, panel.text
    app.xpath("//button[contains(.,'1 change')]")  # toolbar pill
    panel.find_element(
        By.XPATH, ".//li[contains(.,'TOC')]//button[normalize-space()='Remove']"
    ).click()
    app.wait_toast("TOC back on official credits")
    app.wait_gone("[data-testid='your-changes']")
    app.open_tab("My courses")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "6"


def t18_overwrites_panel_and_remove_all(app):
    """Meeting moves and credit changes appear together with provenance;
    'Remove all changes' restores CMI's data in one step."""
    t09_drag_requires_edit_mode(app)  # TOC moved Tue 09:10 → Wed 17:00
    time.sleep(0.4)
    app.css("button.chip-info[aria-label='Details for TOC']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(By.XPATH, ".//dd//button[normalize-space()='Edit']").click()
    inp = dialog.find_element(By.CSS_SELECTOR, "input[aria-label='Credits']")
    inp.clear()
    inp.send_keys("2")
    dialog.find_element(By.XPATH, ".//dd//button[normalize-space()='Save']").click()
    app.wait_toast("TOC now counts as 2 credits")
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()
    # Inline provenance on the course card's meeting row.
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert "CMI: Tue 09:10–10:25" in section.text, section.text
    # The panel lists both overwrites; the pill counts them.
    app.open_tab("My timetable")
    panel = app.wait_css("[data-testid='your-changes']")
    assert "→ Wed 17:00–18:15" in panel.text, panel.text
    assert "4 (assumed) → 2" in panel.text, panel.text
    app.xpath("//button[contains(.,'2 changes')]")
    panel.find_element(
        By.XPATH, ".//button[normalize-space()='Remove all changes']"
    ).click()
    app.wait_toast("All custom changes removed")
    app.wait_gone("[data-testid='your-changes']")
    # Back on CMI's data: official Tuesday slot again.
    app.wait_css("td[data-day='1'][data-slot='550'] button.chip[aria-label^='TOC,']")


def t19_add_extra_meetings(app):
    """Any course can gain extra weekly time slots; adding twice creates two
    independent meetings (nothing gets overwritten)."""
    app.boot("/?c=TOC")
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    card = "//div[contains(@class,'card')][.//strong[contains(.,'Theory of Computation')]]"

    def add_meeting(day_idx, slot_start, toast):
        app.xpath(f"{card}//button[normalize-space()='Add a meeting']").click()
        dialog = app.wait_css(".dialog")
        assert "Add a meeting — TOC" in dialog.text, dialog.text
        dialog.find_element(
            By.CSS_SELECTOR, f"#em-day option[value='{day_idx}']"
        ).click()
        dialog.find_element(
            By.CSS_SELECTOR, f"#em-slot option[value='{slot_start}']"
        ).click()
        dialog.find_element(By.XPATH, ".//button[normalize-space()='Save']").click()
        app.wait_toast(toast)

    add_meeting(2, 1020, "Added a Wed 17:00–18:15 meeting to TOC")
    add_meeting(4, 1020, "Added a Fri 17:00–18:15 meeting to TOC")
    section = app.css("section[aria-label='My courses']")
    assert "Wed 17:00–18:15" in section.text and "Fri 17:00–18:15" in section.text, \
        "both added meetings must exist — the second must not overwrite the first"
    assert "not on CMI's timetable — created by you" in section.text
    # Official meetings untouched, both extras on the grid.
    app.open_tab("My timetable")
    assert app.chips("TOC", "td[data-day='1'][data-slot='550']"), "official Tue stays"
    assert app.chips("TOC", "td[data-day='2'][data-slot='1020']")
    assert app.chips("TOC", "td[data-day='4'][data-slot='1020']")
    app.xpath("//button[contains(.,'2 changes')]")


def t20_url_codes_any_case(app):
    """Share URLs typed by hand work regardless of casing: codes resolve
    against the catalog case-insensitively and canonicalize."""
    app.boot("/?c=toc,rdbm")
    app.chip("TOC")  # canonical casing rendered
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert "Theory of Computation" in section.text, section.text
    assert "RDBM" in section.text
    # No 'unknown code' warning for a merely lowercase code.
    assert "Unknown course code" not in app.d.find_element(By.TAG_NAME, "body").text


def t21_halls_drag_moves_hall_and_slot(app):
    """In the Halls view, dragging a course to another row/column moves it
    into that hall AND slot (edit mode required)."""
    app.boot("/?c=TOC")
    app.open_tab("Halls")
    section = app.wait_css("section[aria-label='Lecture halls']")
    section.find_element(
        By.XPATH, ".//div[@role='group'][@aria-label='Day']//button[normalize-space()='Tue']"
    ).click()
    src_cell = "td[data-hall='Lecture Hall 803'][data-slot='550']"
    dst_cell = "td[data-hall='Seminar Hall'][data-slot='840']"
    app.wait_css(f"{src_cell} button.chip[aria-label^='TOC,']")

    # Inert without edit mode.
    app.drag(app.chip("TOC", src_cell), app.css(dst_cell))
    assert "Moved" not in app.toasts_text()
    assert not app.chips("TOC", dst_cell)

    section.find_element(By.XPATH, ".//button[contains(.,'Edit layout')]").click()
    app.drag(app.chip("TOC", src_cell), app.css(dst_cell))
    app.wait_toast("Moved TOC to Tue 14:00–15:15 · Seminar Hall")
    # THE regression: the halls grid itself must update — the chip renders in
    # its new cell (dashed = customised) and leaves the official one.
    landed = app.wait_css(f"{dst_cell} button.chip[aria-label^='TOC,']")
    assert "overridden" in landed.get_attribute("class"), \
        "chip in the new cell should render as customised"
    assert not app.chips("TOC", src_cell), \
        "the moved chip must leave its official cell"
    # …and it survives a reload.
    app.boot("/", fresh=False)
    app.open_tab("Halls")
    section = app.wait_css("section[aria-label='Lecture halls']")
    section.find_element(
        By.XPATH, ".//div[@role='group'][@aria-label='Day']//button[normalize-space()='Tue']"
    ).click()
    app.wait_css(f"{dst_cell} button.chip[aria-label^='TOC,']")
    assert not app.chips("TOC", src_cell)
    # The timetable reflects the new hall and time.
    app.open_tab("My timetable")
    moved = app.wait_css(
        "td[data-day='1'][data-slot='840'] button.chip[aria-label^='TOC,']"
    )
    assert "Seminar Hall" in moved.get_attribute("aria-label")
    app.xpath("//button[contains(.,'1 change')]")
    # Dragging the landed chip back onto its official cell resets the
    # override (reuses it — no stacking).
    app.open_tab("Halls")
    section = app.wait_css("section[aria-label='Lecture halls']")
    section.find_element(
        By.XPATH, ".//div[@role='group'][@aria-label='Day']//button[normalize-space()='Tue']"
    ).click()
    section.find_element(By.XPATH, ".//button[contains(.,'Edit layout')]").click()
    app.drag(app.chip("TOC", dst_cell), app.css(src_cell))
    app.wait_toast("TOC back on CMI's time")
    back = app.wait_css(f"{src_cell} button.chip[aria-label^='TOC,']")
    assert "overridden" not in back.get_attribute("class")
    assert not app.chips("TOC", dst_cell)


def t22_filter_menu_keeps_focus_and_scroll(app):
    """Ticking a filter checkbox must not rebuild the menu: focus stays on
    the input, the menu keeps its scroll position, the page doesn't move."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .filterbar")
    app.css_all(".filterbar details.facet > summary")[1].click()  # Instructor
    app.wait_css("details.facet[open] .menu")
    app.d.execute_script(
        "document.querySelector('details.facet[open] .menu').scrollTop = 150;"
    )
    box = app.css_all("details.facet[open] .menu label.opt input")[10]
    app.d.execute_script("arguments[0].scrollIntoView({block:'nearest'});", box)
    scroll_before = app.d.execute_script(
        "return document.querySelector('details.facet[open] .menu').scrollTop;"
    )
    box.click()
    time.sleep(0.5)
    state = app.d.execute_script("""
        const menu = document.querySelector('details.facet[open] .menu');
        return {
            open: !!menu,
            menuScroll: menu ? menu.scrollTop : null,
            pageY: window.scrollY,
            focusedIsInput: document.activeElement
                && document.activeElement.tagName === 'INPUT',
            checked: document.querySelectorAll(
                'details.facet[open] .menu input:checked').length,
        };
    """)
    assert state["open"], "menu must stay open"
    assert state["menuScroll"] == scroll_before, state
    assert state["pageY"] == 0, state
    assert state["focusedIsInput"], "focus must stay on the clicked checkbox"
    assert state["checked"] == 1, state


def t23_master_grid_marks_selected(app):
    """Selected courses are unmistakable in the master grid: ✓ mark, accent
    ring, and an aria hint."""
    app.boot("/?c=TOC")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    toc = app.chip("TOC")
    assert toc.find_elements(By.CSS_SELECTOR, ".sel-mark"), "TOC needs the ✓ mark"
    assert "selected" in toc.get_attribute("class")
    assert "in your timetable" in toc.get_attribute("aria-label")
    nlp = app.chip("NLP")
    assert not nlp.find_elements(By.CSS_SELECTOR, ".sel-mark")
    assert "in your timetable" not in nlp.get_attribute("aria-label")


def t24_toast_pauses_while_hovered(app):
    """Toasts don't vanish mid-read: hovering pauses auto-dismiss."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    app.xpath("//div[contains(@class,'card')][1]//button[normalize-space()='Add']").click()
    toast = app.wait_css(".toasts .toast")
    ActionChains(app.d).move_to_element(toast).perform()
    time.sleep(7.5)  # well past the 6 s auto-dismiss
    assert app.css_all(".toasts .toast"), "hovered toast must not auto-dismiss"
    ActionChains(app.d).move_by_offset(-300, -200).perform()  # leave the toast
    app.wait_gone(".toasts .toast", timeout=10)


def t25_first_run_prompt_when_empty(app):
    """A first-time visitor sees the welcome prompt, no tabs, and an honest
    failure banner when the sync can't get through (all hosts blackholed)."""
    remove_fake_mirror()  # ensure the mirror tier 404s
    app.boot("/", seed=False)
    welcome = app.wait_css(".welcome-card")
    assert "Plan your semester" in welcome.text, welcome.text
    assert "sync every few days" in welcome.text, welcome.text
    assert not app.css_all(".tabs .tab"), "no tabs before the first sync"
    assert "Not synced yet" in app.css(".sync-pill").text or \
        app.css_all(".sync-pill .spinner"), "pill must show the unsynced state"
    # The automatic first sync fails (no reachable route) → banner + prompt stays.
    banner = app.wait_css(".banner", timeout=30)
    assert "couldn't be fetched" in banner.text, banner.text
    assert app.css_all(".welcome-card"), "prompt must survive a failed sync"
    app.xpath("//button[contains(.,'Fetch the timetable')]")


def t26_first_sync_populates_from_mirror(app):
    """With a reachable (same-origin) mirror, the automatic first sync fills
    the app: welcome disappears, tabs appear, data renders."""
    write_fake_mirror()
    try:
        app.boot("/", seed=False)
        # The auto-sync walks direct (dead) → proxies (dead) → mirror (live).
        app.wait_css(".tabs .tab", timeout=30)
        app.wait_gone(".welcome-card")
        assert "mirror" in app.css(".sync-pill").text, app.css(".sync-pill").text
        app.open_tab("Master grid")
        app.wait_css("section[aria-label='Master grid'] table.tt")
        app.chip("TOC")
    finally:
        remove_fake_mirror()


def t27_filters_undo_redo(app):
    """Filter changes are part of the undo history: a ticked facet can be
    undone/redone, and a burst of typing in the search box is ONE undo step."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .filterbar")
    app.css_all(".filterbar details.facet > summary")[0].click()  # Branch
    app.wait_css("details.facet[open] .menu")
    app.css("details.facet[open] .menu label.opt input").click()
    app.wait_css(".filterchip")
    app.xpath("//button[@aria-label='Undo']").click()
    app.wait_toast("Undid: the")
    app.wait_gone(".filterchip")
    assert not app.css_all("details.facet .menu input:checked")
    app.xpath("//button[@aria-label='Redo']").click()
    app.wait_css(".filterchip")
    # Search coalescing: several keystrokes, one undo.
    search = app.css(".filterbar input[type='search']")
    search.send_keys("toc")
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all(".filterchip")) == 2
    )
    app.xpath("//button[@aria-label='Undo']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: search.get_attribute("value") == ""
        and len(app.css_all(".filterchip")) == 1,
        message="one undo must revert the whole typed burst",
    )


def t28_facet_menu_search_and_select_all(app):
    """Every dropdown has its own search box + All/None shortcuts, and a
    Course facet filters to specific courses."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .filterbar")
    app.xpath(
        "//details[contains(@class,'facet')]/summary[starts-with(normalize-space(),'Course')]"
    ).click()
    app.wait_css("details.facet[open] .menu")
    total = len(app.css_all("details.facet[open] .menu label.opt"))
    assert total > 50, f"the Course menu should list the whole catalog, got {total}"
    search = app.css("details.facet[open] .menu input[type='search']")
    search.send_keys("theory")
    WebDriverWait(app.d, 10).until(
        lambda d: 0 < len(app.css_all("details.facet[open] .menu label.opt")) < total,
        message="the menu search must narrow the option list",
    )
    visible = len(app.css_all("details.facet[open] .menu label.opt"))
    # "All" ticks exactly the visible options → same number of filter chips.
    app.xpath(
        "//details[contains(@class,'facet') and @open]//button[normalize-space()='All']"
    ).click()
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all(".filterchip")) == visible,
        message="All must select every option the search shows",
    )
    # The catalog now shows exactly those courses.
    matches = app.css("section[aria-label='Catalog'] .filterbar .muted").text
    assert matches.startswith(f"{visible} match"), matches
    # "None" clears them again (menu search still narrowing).
    app.xpath(
        "//details[contains(@class,'facet') and @open]//button[normalize-space()='None']"
    ).click()
    app.wait_gone(".filterchip")
    # One undo brings the whole "All" pick back... after the None is undone.
    app.xpath("//button[@aria-label='Undo']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all(".filterchip")) == visible,
        message="undoing 'clear all in Course' must restore the picks",
    )


def t29_share_link_carries_custom_changes(app):
    """The 'incl. my custom changes' share URL reproduces the selection,
    the moved meeting AND the credit change on a fresh browser."""
    app.boot("/", selection=["TOC"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='Share']").click()
    app.wait_css(".dialog")
    url = app.css(
        "input[aria-label='Share link including custom times']"
    ).get_attribute("value")
    assert "?c=" in url and "s=" in url, url
    app.boot("/?" + url.split("?", 1)[1])  # fresh storage + shared link
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    assert not app.chips("TOC", "td[data-day='1'][data-slot='550']"), \
        "the shared override must apply — official Tue slot stays empty"
    app.xpath("//button[contains(.,'2 changes')]")
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "3", section.text


def t30_sync_merge_conflict_flow(app):
    """Upstream moves a customised meeting → conflict dialog; keep-mine
    rebases (no re-conflict on the next sync); a removed course gets its
    badge; the What-changed digest is structured."""
    def mutate(snap):
        for c in snap["courses"]:
            if c["code"] == "TOC":
                for m in c["meetings"]:
                    if m["day"] == "Tue":
                        m["day"] = "Fri"
                        m["slot"] = {"start_min": 840, "end_min": 915}
        snap["courses"] = [c for c in snap["courses"] if c["code"] != "QCOM"]
    write_fake_mirror(mutate)
    try:
        app.boot("/", selection=["TOC", "QCOM"], overrides=TOC_OVR)
        app.xpath("//button[normalize-space()='Sync now']").click()
        dialog = app.wait_css(".dialog", timeout=30)
        assert "your time" in dialog.text and "Fri 14:00" in dialog.text, dialog.text
        # Default is "Use CMI's" — actively keep the user's time instead.
        dialog.find_element(
            By.XPATH, ".//button[normalize-space()='Keep mine for all']"
        ).click()
        dialog.find_element(By.XPATH, ".//button[normalize-space()='Apply']").click()
        app.wait_toast("Conflicts resolved")
        app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
        app.wait_toast("QCOM is no longer on CMI's timetable")
        banner = app.xpath("//div[contains(@class,'banner')][contains(.,'CMI updated')]")
        banner.find_element(
            By.XPATH, ".//button[normalize-space()='See what changed']"
        ).click()
        dlg = app.wait_css(".dialog")
        assert "No longer listed" in dlg.text and "QCOM" in dlg.text, dlg.text
        app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
        app.open_tab("My courses")
        section = app.wait_css("section[aria-label='My courses']")
        assert "No longer on CMI's timetable" in section.text, section.text
        # The rebased override must NOT re-raise the conflict.
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        time.sleep(1.0)
        assert not app.css_all(".dialog"), \
            "keep-mine must rebase the override — no repeat conflict"
    finally:
        remove_fake_mirror()


def t31_keyboard_move_mode(app):
    """Accessibility path: focus a chip → M → arrows → Enter moves the
    meeting; Esc cancels a move in progress."""
    app.boot("/", selection=["TOC"])
    app.xpath("//button[contains(.,'Edit layout')]").click()
    chip = app.css("td[data-day='1'][data-slot='550'] button.chip")
    app.d.execute_script("arguments[0].focus();", chip)
    chip.send_keys("m")
    body = app.d.find_element(By.TAG_NAME, "body")
    body.send_keys(Keys.ARROW_DOWN)
    body.send_keys(Keys.ENTER)
    app.wait_toast("Moved TOC")
    app.wait_css("td[data-day='2'][data-slot='550'] button.chip[aria-label^='TOC,']")
    announce = app.css(".sr-only[aria-live='polite']").get_attribute("textContent")
    assert "Dropped TOC" in announce, announce
    chip2 = app.css("td[data-day='2'][data-slot='550'] button.chip")
    app.d.execute_script("arguments[0].focus();", chip2)
    chip2.send_keys("m")
    body.send_keys(Keys.ARROW_DOWN)
    body.send_keys(Keys.ESCAPE)
    time.sleep(0.3)
    assert app.chips("TOC", "td[data-day='2'][data-slot='550']"), \
        "Esc must cancel the move without dropping"


def t32_corrupt_storage_recovery(app):
    """An unreadable snapshot blob is backed up (never deleted), the sticky
    explanation banner survives the automatic sync attempt, and the app
    falls back to the first-run screen."""
    app.boot("/", raw_snapshot="not-json{{{")
    app.wait_css(".welcome-card")
    banner = app.wait_css(".banner")
    assert "couldn't be read" in banner.text, banner.text
    assert "Nothing was deleted" in banner.text, banner.text
    keys = app.d.execute_script(
        "return Object.keys(localStorage).filter(k => k.startsWith('cmitt.corrupt.'))"
    )
    assert keys, "corrupt blob must be backed up under cmitt.corrupt.*"


def t33_export_ics_honors_overrides(app):
    """Export .ics downloads a calendar whose events reflect the moved
    meeting, not the overridden official one."""
    for f in os.listdir(DOWNLOADS):
        os.remove(os.path.join(DOWNLOADS, f))
    app.boot("/", selection=["TOC"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='Export .ics']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Download .ics']").click()
    path = None
    deadline = time.time() + 10
    while time.time() < deadline:
        files = [f for f in os.listdir(DOWNLOADS) if f.endswith(".ics")]
        if files:
            path = os.path.join(DOWNLOADS, files[0])
            break
        time.sleep(0.3)
    assert path, "no .ics file downloaded"
    with open(path) as f:
        ics = f.read()
    assert "Theory of Computation" in ics
    # TOC officially meets Tue+Thu 09:10; the override moved Tue → Wed 17:00.
    assert ics.count("BEGIN:VEVENT") == 2, ics
    assert "T170000" in ics, "custom 17:00 meeting missing from the export"
    assert ics.count("T091000") == 1, "exactly one 09:10 DTSTART (Thu) may remain"
    assert "RRULE:FREQ=WEEKLY" in ics


def t34_mobile_longpress_drag(app):
    """Mobile drag & drop: a touch long-press must suppress the native
    context menu, a browser-cancelled drag must not deselect the course via
    the synthesized click, an actual touch drag must move the chip, and a
    plain tap must still toggle."""
    app.boot("/?c=TOC")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    app.xpath("//button[contains(.,'Edit layout')]").click()

    P = 7  # pointerId shared by the whole gesture

    def pointerdown_touch(chip):
        app.d.execute_script(
            """
            const el = arguments[0], id = arguments[1];
            const r = el.getBoundingClientRect();
            el.dispatchEvent(new PointerEvent('pointerdown', {
                pointerId: id, pointerType: 'touch', button: 0,
                bubbles: true, cancelable: true,
                clientX: r.left + r.width / 2, clientY: r.top + r.height / 2,
            }));
            """,
            chip, P,
        )

    # With no drag in progress, context menus stay available (desktop
    # right-click must keep working).
    assert app.d.execute_script(
        "return document.body.dispatchEvent(new MouseEvent('contextmenu',"
        " {bubbles: true, cancelable: true}));"
    ), "contextmenu must NOT be suppressed outside a drag gesture"

    # -- The reported bug: long-press → native context menu → pointercancel
    #    → synthesized click used to deselect the course.
    chip = app.chip("TOC", "td[data-day='1'][data-slot='550']")
    assert "selected" in chip.get_attribute("class")
    pointerdown_touch(chip)
    time.sleep(0.5)  # past the 350 ms long-press lift-off
    allowed = app.d.execute_script(
        "return arguments[0].dispatchEvent(new MouseEvent('contextmenu',"
        " {bubbles: true, cancelable: true}));",
        chip,
    )
    assert not allowed, "contextmenu must be suppressed during a touch drag"
    # Even if the browser DOES cancel the drag, the follow-up click must not
    # toggle the chip (pointercancel and click land within the same beat).
    app.d.execute_script(
        """
        const el = arguments[0], id = arguments[1];
        document.dispatchEvent(new PointerEvent('pointercancel',
            {pointerId: id, pointerType: 'touch', bubbles: true}));
        el.dispatchEvent(new MouseEvent('click', {bubbles: true, cancelable: true}));
        """,
        chip, P,
    )
    time.sleep(0.4)
    chip = app.chip("TOC", "td[data-day='1'][data-slot='550']")
    assert "selected" in chip.get_attribute("class"), \
        "cancelled long-press must not deselect the course"
    assert "Removed TOC" not in app.toasts_text()

    # -- A full touch drag (long-press, move, lift) must move the meeting.
    pointerdown_touch(chip)
    time.sleep(0.5)
    app.d.execute_script(
        """
        const cell = arguments[0], id = arguments[1];
        const r = cell.getBoundingClientRect();
        const x = r.left + r.width / 2, y = r.top + r.height / 2;
        for (const type of ['pointermove', 'pointermove', 'pointerup']) {
            document.dispatchEvent(new PointerEvent(type, {
                pointerId: id, pointerType: 'touch', bubbles: true,
                cancelable: true, clientX: x, clientY: y,
            }));
        }
        """,
        app.cell(2, 1020), P,
    )
    app.wait_toast("Moved TOC")
    assert app.chips("TOC", "td[data-day='2'][data-slot='1020']"), \
        "touch drag must land the chip on Wed 17:00"

    # -- A plain tap (no long-press) must still toggle the selection.
    time.sleep(0.4)  # let the click-suppression window lapse
    app.chip("TOC", "td[data-day='2'][data-slot='1020']").click()
    app.wait_toast("Removed TOC")


def t35_remove_meeting(app):
    """A meeting can be removed from the timetable (the counterpart to 'Add
    a meeting'): the chip leaves the grid, the removal is listed as a change
    with a Restore action, and it survives reloads."""
    app.boot("/?c=TOC")  # Tue + Thu 09:10-10:25 in the fixture
    assert app.chips("TOC", "td[data-day='1'][data-slot='550']"), "sanity: Tue chip"
    assert app.chips("TOC", "td[data-day='3'][data-slot='550']"), "sanity: Thu chip"

    # Details dialog -> first meeting row (Tue) -> Remove this meeting.
    app.chip("TOC", "td[data-day='1'][data-slot='550']").click()
    dialog = app.wait_css(".dialog")
    rows = dialog.find_elements(By.CSS_SELECTOR, "ul.meetings li")
    assert len(rows) == 2, f"expected 2 meeting rows, got {len(rows)}"
    tue = next(r for r in rows if "Tue" in r.text)
    tue.find_element(By.XPATH, ".//button[normalize-space()='Remove this meeting']").click()
    app.wait_toast("Removed a TOC meeting")
    # The open dialog re-renders reactively: one meeting row left.
    WebDriverWait(app.d, 5).until(
        lambda d: len(d.find_elements(By.CSS_SELECTOR, ".dialog ul.meetings li")) == 1,
        message="dialog should drop to one meeting row",
    )
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)

    assert not app.chips("TOC", "td[data-day='1'][data-slot='550']"), \
        "removed meeting must leave the Tuesday cell"
    assert app.chips("TOC", "td[data-day='3'][data-slot='550']"), \
        "the Thursday meeting must be untouched"

    # Listed as a change, and restorable.
    pill = app.xpath("//button[contains(.,'change')]")
    assert "1 change" in pill.text, pill.text
    app.d.get(f"{BASE}/")  # reload: persists
    app.wait_css(".header h1")
    time.sleep(0.5)
    assert not app.chips("TOC", "td[data-day='1'][data-slot='550']"), \
        "removal must survive a reload"
    app.xpath("//button[contains(.,'change')]").click()
    dialog_text = app.wait_css(".dialog").text
    assert "Meeting you removed" in dialog_text, dialog_text[:300]
    assert "Tue 09:10" in dialog_text, dialog_text[:300]
    app.xpath("//div[@class='dialog']//button[normalize-space()='Restore']").click()
    app.wait_toast("TOC back on CMI's time")
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    assert app.chips("TOC", "td[data-day='1'][data-slot='550']"), \
        "Restore must bring the meeting back"


def t36_out_of_grid_meeting_gets_its_own_column(app):
    """A meeting outside CMI's hours (e.g. 19:30-20:30) renders in its own
    clearly-marked column with its real times — never squeezed into the last
    official slot."""
    evening = {
        "next_id": 1,
        "items": [{
            "id": 0, "course": "TOC",
            "base": {"day": "Tue", "slot": {"start_min": 550, "end_min": 625},
                     "hall": "Lecture Hall 803", "temp_booking": False},
            "to": {"day": "Tue", "slot": {"start_min": 1170, "end_min": 1230},
                   "hall": None, "temp_booking": False},
            "created_at": 1754000000000.0}],
        "credits": [],
    }
    app.boot("/", selection=["TOC"], overrides=evening)

    header = app.css("section[aria-label='My timetable'] table.tt thead")
    assert "19:30–20:30" in header.text, header.text
    extra_th = app.css("section[aria-label='My timetable'] th.extra")
    assert "19:30" in extra_th.text

    # The chip sits in the synthetic 19:30 column on Tuesday…
    assert app.chips("TOC", "td[data-day='1'][data-slot='1170']"), \
        "chip must render in the synthetic column"
    # …not clamped into the last official slot (17:00), and not on its old time.
    assert not app.chips("TOC", "td[data-day='1'][data-slot='1020']"), \
        "chip must NOT be squeezed into the last official column"
    assert not app.chips("TOC", "td[data-day='1'][data-slot='550']")
    # Thursday's official 09:10 meeting is untouched, in an official column.
    assert app.chips("TOC", "td[data-day='3'][data-slot='550']")

    # Synthetic columns are REAL drop targets: drag the Thursday meeting
    # into the 19:30 column and it must land there (not silently no-op).
    app.xpath("//button[contains(.,'Edit layout')]").click()
    app.drag(
        app.chip("TOC", "td[data-day='3'][data-slot='550']"),
        app.cell(3, 1170),
    )
    app.wait_toast("Moved TOC to Thu 19:30")
    assert app.chips("TOC", "td[data-day='3'][data-slot='1170']"), \
        "drop onto a synthetic column must apply"

    # Restoring CMI's times makes the synthetic column disappear entirely.
    app.xpath("//button[contains(.,'change')]").click()
    for _ in range(2):
        app.xpath("//div[@class='dialog']//button[normalize-space()='Remove']").click()
        app.wait_toast("TOC back on CMI's time")
        time.sleep(0.3)
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    time.sleep(0.3)
    assert not app.css_all("section[aria-label='My timetable'] th.extra"), \
        "the synthetic column must vanish with its meetings"


def t37_catalog_updates_live(app):
    """Catalog rows update in place — no reload, no tab switch: clash marks
    appear/disappear as courses are added and removed, a changed meeting
    time updates the row's times, and 'Clear selection' in My data clears
    every mark at once. (Rows live in a keyed <For>, so they are never
    remounted by these changes — the state must be reactive inside them.)"""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .filterbar")
    CAT = "section[aria-label='Catalog']"

    def row_button(code, label):
        el = app.xpath(
            f"//section[@aria-label='Catalog']"
            f"//button[contains(@class,'chip') and starts-with(@aria-label,'{code},')]"
            f"/ancestor::div[contains(@class,'card')]"
            f"//button[normalize-space()='{label}']"
        )
        # Selenium's auto-scroll puts the element flush under the sticky
        # header; center it so the click isn't intercepted.
        app.d.execute_script("arguments[0].scrollIntoView({block:'center'});", el)
        return el

    def chip_classes(code):
        return app.chip(code, CAT).get_attribute("class")

    def row_times(code):
        return app.xpath(
            f"//section[@aria-label='Catalog']"
            f"//button[contains(@class,'chip') and starts-with(@aria-label,'{code},')]"
            f"/ancestor::div[contains(@class,'card')]//span[contains(@class,'mono')]"
        ).text

    # TOC and ISS both meet Tue+Thu 09:10 in the fixture. Neither is
    # selected: no clash marks anywhere in the catalog.
    assert "clash" not in chip_classes("TOC")
    assert "Tue 09:10" in row_times("TOC"), row_times("TOC")

    # Add both from the catalog itself: the moment the second lands, BOTH
    # rows must show the clash — same page, no refresh.
    row_button("TOC", "Add").click()
    app.wait_toast("Added TOC")
    assert "clash" not in chip_classes("TOC"), "one course alone cannot clash"
    row_button("ISS", "Add").click()
    for code in ("TOC", "ISS"):
        WebDriverWait(app.d, 5).until(
            lambda d, c=code: "clash" in chip_classes(c),
            message=f"{code}'s catalog chip must turn clashing live",
        )
    aria = app.chip("TOC", CAT).get_attribute("aria-label")
    assert "in your timetable" in aria and "clashes with ISS" in aria, aria

    # Change a time while the catalog stays mounted: removing TOC's Tuesday
    # meeting (details dialog opens over the catalog) must update the row's
    # printed times in place.
    toc_chip = app.chip("TOC", CAT)
    app.d.execute_script("arguments[0].scrollIntoView({block:'center'});", toc_chip)
    toc_chip.click()
    dialog = app.wait_css(".dialog")
    tue = next(r for r in dialog.find_elements(By.CSS_SELECTOR, "ul.meetings li")
               if "Tue" in r.text)
    tue.find_element(By.XPATH, ".//button[normalize-space()='Remove this meeting']").click()
    app.wait_toast("Removed a TOC meeting")
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    # NB: keep the message static — an f-string here would capture the
    # PRE-wait value and mislead on timeout.
    WebDriverWait(app.d, 5).until(
        lambda d: row_times("TOC") == "Thu 09:10",
        message="row times must drop Tuesday live",
    )
    assert "clash" in chip_classes("TOC"), "Thu 09:10 still clashes with ISS"

    # 'Clear selection' in My data (dialog over the same catalog): every
    # clash mark and selection marker must vanish at once.
    app.xpath("//button[normalize-space()='My data']").click()
    app.wait_css(".dialog")
    app.xpath("//div[@class='dialog']//button[normalize-space()='Clear selection']").click()
    app.wait_toast("Selection cleared")
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    for code in ("TOC", "ISS"):
        WebDriverWait(app.d, 5).until(
            lambda d, c=code: "clash" not in chip_classes(c),
            message=f"{code}'s clash mark must clear live",
        )
    assert "in your timetable" not in app.chip("TOC", CAT).get_attribute("aria-label")
    # The removal override survives a selection clear by design — the row
    # keeps showing Thursday only.
    assert row_times("TOC") == "Thu 09:10", row_times("TOC")


def t38_duration_based_credits(app):
    """A course annotated '(Oct-Nov)' runs 2 months, so its unstated credits
    are assumed at 2 (one per month) instead of the campus default 4;
    stated credits are never second-guessed. My courses breaks the
    selection down by credit value."""
    app.boot("/?c=MATH,TOC,RDBM")  # MATH (Oct-Nov, unstated) TOC (unstated) RDBM (2 credits)
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    # TOC assumed 4 + MATH assumed 2 (Oct-Nov) + RDBM stated 2 = 8.
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "8", section.text
    assert "credits in total" in section.text, section.text
    # One readable pill per credit value, heaviest first.
    pills = [p.text for p in app.css_all("section[aria-label='My courses'] .credit-summary .cs-pill")]
    assert pills == ["1 course at 4 credits", "2 courses at 2 credits"], pills
    # Two courses carry assumptions (at different values), one is stated.
    assert "CMI doesn't list credits for 2 courses" in section.text, section.text

    # The MATH card's credits badge says 2 and explains why.
    badge = app.xpath(
        "//section[@aria-label='My courses']//div[contains(@class,'card')]"
        "[.//button[starts-with(@aria-label,'MATH,')]]"
        "//span[contains(@class,'badge')][contains(normalize-space(),'cr')]"
    )
    assert badge.text.strip() == "2 cr", badge.text
    assert "Oct-Nov duration" in badge.get_attribute("title"), \
        badge.get_attribute("title")

    # The details dialog spells the same assumption out.
    chip = app.chip("MATH", "section[aria-label='My courses']")
    app.d.execute_script("arguments[0].scrollIntoView({block:'center'});", chip)
    chip.click()
    dialog = app.wait_css(".dialog")
    assert "assumed from its Oct-Nov duration" in dialog.text, dialog.text[:400]
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)


def t39_sync_pill_ticks_live(app):
    """The header's 'Synced … ago' text and its stale tint keep up with the
    wall clock on their own — no reload, no new sync. The header re-renders
    on a 30 s interval and instantly on visibilitychange, so a throttled
    background tab catches up the moment it comes back."""
    snap = json.loads(SEED_SNAPSHOT_JSON)
    snap["fetched_at"] = time.time() * 1000.0
    app.boot("/", raw_snapshot=json.dumps(snap))
    pill = app.css(".sync-pill")
    assert "Synced just now" in pill.text, pill.text
    assert "stale" not in pill.get_attribute("class"), pill.get_attribute("class")

    # Jump the page's wall clock forward, then poke the visibility hook the
    # header listens on — the deterministic stand-in for waiting 30 s.
    def jump(minutes):
        app.d.execute_script(
            "window.__realNow = window.__realNow || Date.now.bind(Date);"
            f"Date.now = () => window.__realNow() + {minutes} * 60000;"
            "document.dispatchEvent(new Event('visibilitychange'));"
        )

    jump(7)
    WebDriverWait(app.d, 5).until(
        lambda d: "7 min ago" in app.css(".sync-pill").text,
        message=f"pill should tick to '7 min ago'; got {app.css('.sync-pill').text!r}",
    )

    # 49 h out it crosses the 48 h staleness line — text and tint together.
    jump(49 * 60)
    WebDriverWait(app.d, 5).until(
        lambda d: "2 days ago" in app.css(".sync-pill").text
        and "stale" in app.css(".sync-pill").get_attribute("class"),
        message=f"pill should go stale at 49 h; got {app.css('.sync-pill').text!r}",
    )
    app.d.execute_script("Date.now = window.__realNow; delete window.__realNow;")


def _js_set(app, el, value):
    """Set an input's value the way a user would (fires the input event
    Leptos listens on) — used for <input type=time>, whose send_keys
    behavior is locale-dependent."""
    app.d.execute_script(
        "arguments[0].value = arguments[1];"
        "arguments[0].dispatchEvent(new Event('input', {bubbles: true}));",
        el, value,
    )


def t40_custom_course_create(app):
    """'Add your own course': name-first form with an auto-suggested code,
    segmented credits, official-slot and custom-time meetings, a live
    clash line, grid chips (incl. a synthetic out-of-grid column), the
    violet Custom badge, credit-summary integration and persistence."""
    app.boot("/", selection=["TOC"])
    app.open_tab("My courses")
    app.wait_css(".add-own-card").click()
    app.wait_css(".dialog .custom-form")

    # Name first; the code follows until touched.
    app.css("#cc-name").send_keys("German A1")
    assert app.css("#cc-code").get_attribute("value") == "GERMAN"
    app.xpath("//div[contains(@class,'seg')]/button[normalize-space()='2']").click()

    # Meeting 1: Tuesday, first official slot (09:10) — clashes with TOC.
    app.css("#cc-add-meeting").click()
    app.css("#cc-day-0 option[value='1']").click()
    note = app.css(".custom-form .clash-note")
    assert "clashes with TOC" in note.text and "you can still add it" in note.text, note.text

    # Meeting 2: Monday evening, custom time — 18:30 starts after CMI's
    # last official slot (17:00–18:15) ends, so it's outside the grid.
    app.css("#cc-add-meeting").click()
    row2 = app.css_all(".custom-form .meeting-draft")[1]
    row2.find_element(By.CSS_SELECTOR, "select[aria-label='Time'] option[value='custom']").click()
    _js_set(app, row2.find_element(By.CSS_SELECTOR, "input[aria-label='Start time']"), "18:30")
    _js_set(app, row2.find_element(By.CSS_SELECTOR, "input[aria-label='End time']"), "19:45")
    # A place CMI never lists: the hall dropdown's "Other place…" row opens
    # a free-text box for it.
    Select(
        row2.find_element(By.CSS_SELECTOR, "select[aria-label='Hall or place']")
    ).select_by_visible_text("Other place…")
    app.wait_css("#cc-hall-1-other").send_keys("Sports annexe")

    app.xpath("//button[normalize-space()='Add to my timetable']").click()
    app.wait_gone(".dialog")
    app.wait_toast("Added GERMAN")

    # Card: violet badge, clash flag, credit summary counts 2 credits.
    section = app.wait_css("section[aria-label='My courses']")
    badge = app.css("section[aria-label='My courses'] .badge.custom")
    assert badge.text == "Custom", badge.text
    assert "⚠ clash" in section.text, section.text
    pills = [p.text for p in app.css_all(".credit-summary .cs-pill")]
    assert "1 course at 2 credits" in pills, pills

    # Grid: Tuesday chip in the official slot, evening chip in its own
    # clearly-marked column.
    app.open_tab("My timetable")
    app.wait_css("td[data-day='1'][data-slot='550'] button.chip[aria-label^='GERMAN,']")
    app.wait_css("th.extra")
    app.wait_css("td[data-day='0'][data-slot='1110'] button.chip[aria-label^='GERMAN,']")

    # Survives a reload.
    app.d.refresh()
    app.wait_css("td[data-day='0'][data-slot='1110'] button.chip[aria-label^='GERMAN,']")


def t41_custom_course_edit_park_share_delete(app):
    """Editing moves the definition itself (no override bookkeeping), a
    removed custom parks under 'off the timetable' instead of dying, the
    full share link carries the definition to a fresh browser, and delete
    is one undoable step."""
    app.boot("/", selection=["TOC"])
    app.open_tab("My courses")
    app.wait_css(".add-own-card").click()
    app.wait_css(".dialog .custom-form")
    app.css("#cc-name").send_keys("Gym")
    app.xpath("//div[contains(@class,'seg')]/button[normalize-space()='0']").click()
    app.css("#cc-add-meeting").click()
    app.css("#cc-day-0 option[value='2']").click()  # Wednesday, first slot
    app.xpath("//button[normalize-space()='Add to my timetable']").click()
    app.wait_gone(".dialog")
    pills = [p.text for p in app.css_all(".credit-summary .cs-pill")]
    assert "1 course at 0 credits" in pills, pills

    # Edit the course itself: Wednesday → Friday. The chip follows.
    app.xpath("//button[normalize-space()='Edit course']").click()
    form = app.wait_css(".dialog .custom-form")
    # Deleting is not offered from inside the edit form — it belongs to the
    # course's own dialog, beside Edit.
    assert not form.find_elements(
        By.XPATH, ".//button[normalize-space()='Delete this course']"
    ), "the edit form must not offer to delete the course"
    app.css("#cc-day-0 option[value='4']").click()
    app.xpath("//button[normalize-space()='Save changes']").click()
    app.wait_gone(".dialog")
    app.open_tab("My timetable")
    app.wait_css("td[data-day='4'][data-slot='550'] button.chip[aria-label^='GYM,']")

    # Now move the meeting the OTHER way — the per-meeting edit dialog,
    # which goes through apply_override. For a custom course that must
    # rewrite the definition itself, so no "✎ N changes" appears. (Editing
    # via the course form can't create an override by construction; this
    # path can, which is what makes the assertion mean something.)
    app.open_tab("My courses")
    gym_row = app.xpath(
        "//section[@aria-label='My courses']//div[contains(@class,'card')]"
        "[.//button[starts-with(@aria-label,'GYM,')]]//ul[@class='meetings']/li"
    )
    gym_row.find_element(By.XPATH, ".//button[normalize-space()='Edit']").click()
    app.wait_css("#em-day")
    app.css("#em-day option[value='3']").click()   # Friday → Thursday
    app.xpath("//button[normalize-space()='Save']").click()
    app.wait_gone(".dialog")
    app.open_tab("My timetable")
    app.wait_css("td[data-day='3'][data-slot='550'] button.chip[aria-label^='GYM,']")
    assert not app.chips("GYM", "td[data-day='4'][data-slot='550']"), \
        "the old cell must be empty — the definition moved, not a copy"
    assert not app.d.find_elements(
        By.XPATH, "//button[contains(., '✎') and contains(., 'change')]"
    ), "moving a custom course's meeting must not create an override"
    # And the course form now shows the moved time (one source of truth).
    app.open_tab("My courses")
    app.xpath("//button[normalize-space()='Edit course']").click()
    app.wait_css(".dialog .custom-form")
    assert app.css("#cc-day-0").get_attribute("value") == "3", \
        app.css("#cc-day-0").get_attribute("value")
    app.xpath("//button[normalize-space()='Cancel']").click()
    app.wait_gone(".dialog")

    # The full share link reproduces the course on a fresh browser.
    app.xpath("//button[normalize-space()='Share']").click()
    dialog = app.wait_css(".dialog")
    assert "GYM" in dialog.text, dialog.text  # the travels-only-with hint
    url = app.css(
        "input[aria-label='Share link including custom times']"
    ).get_attribute("value")
    app.boot("/?" + url.split("?", 1)[1])
    # Thursday: the definition the per-meeting edit wrote, not the form's.
    app.wait_css("td[data-day='3'][data-slot='550'] button.chip[aria-label^='GYM,']")
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses'] .badge.custom")

    # Park: Remove keeps the definition under "off the timetable".
    gym_card = app.xpath(
        "//section[@aria-label='My courses']//div[contains(@class,'card')]"
        "[.//button[starts-with(@aria-label,'GYM,')]]"
    )
    gym_card.find_element(By.XPATH, ".//button[normalize-space()='Remove']").click()
    parked = app.wait_css(".parked")
    assert "Gym" in parked.text, parked.text
    parked.find_element(By.XPATH, ".//button[normalize-space()='Add back']").click()
    app.wait_css("section[aria-label='My courses'] .badge.custom")
    app.wait_gone(".parked")

    # Delete straight from the course's own dialog — no detour through the
    # edit form — and one Undo brings it all back.
    chip = app.chip("GYM", "section[aria-label='My courses']")
    app.d.execute_script("arguments[0].scrollIntoView({block:'center'});", chip)
    chip.click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Delete this course']"
    ).click()
    app.wait_gone(".dialog")
    app.wait_toast("Deleted GYM")
    assert not app.css_all("section[aria-label='My courses'] .badge.custom")
    app.d.execute_script("window.scrollTo(0, 0);")
    app.xpath("//button[@aria-label='Undo']").click()
    app.wait_css("section[aria-label='My courses'] .badge.custom")


def t42_custom_course_shadowed_by_cmi(app):
    """A custom course created before CMI listed that code keeps winning
    after the sync introduces it, says so, and can be switched to CMI's
    version in one undoable step — with the catalog chip updating live
    (no reload) once the custom is gone."""
    custom_toc = {"courses": [{
        "code": "TOC", "name": "My own TOC notes", "instructors": [],
        "branches": [], "credits": 1, "starts": None,
        "part_of_semester": None, "optional_flag": False,
        "status": "Scheduled",
        "meetings": [{"day": "Fri", "slot": {"start_min": 630, "end_min": 705},
                      "hall": None, "temp_booking": False}],
    }]}
    app.boot("/", selection=["TOC"])
    app.d.execute_script(
        "localStorage.setItem('cmitt.v1.custom', arguments[0]);", json.dumps(custom_toc)
    )
    app.d.refresh()
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    # The user's own definition wins over CMI's course of the same code.
    assert "My own TOC notes" in section.text, section.text
    assert "also on CMI now" in section.text, section.text
    pills = [p.text for p in app.css_all(".credit-summary .cs-pill")]
    assert pills == ["1 course at 1 credit"], pills

    # The catalog row for the shadowed code shows the custom's name — and
    # must update in place when the custom goes away.
    app.open_tab("Catalog")
    app.wait_css(".filterbar input[type='search']").send_keys("TOC")
    chip = app.wait_css("section[aria-label='Catalog'] button.chip[aria-label^='TOC,']")
    assert "My own TOC notes" in chip.get_attribute("aria-label"), \
        chip.get_attribute("aria-label")

    app.d.execute_script("arguments[0].scrollIntoView({block:'center'});", chip)
    chip.click()
    dialog = app.wait_css(".dialog")
    assert "You're seeing your own version" in dialog.text, dialog.text[:400]
    dialog.find_element(
        By.XPATH, ".//button[normalize-space()=\"Use CMI's version instead\"]"
    ).click()
    app.wait_gone(".dialog")
    app.wait_toast("TOC now uses CMI's version")

    # Live, without a reload: the catalog chip now carries CMI's name, and
    # the course is still selected — resolving to the official course, not
    # a "no longer on CMI's timetable" stub.
    WebDriverWait(app.d, 5).until(
        lambda d: "Theory of Computation" in app.chip(
            "TOC", "section[aria-label='Catalog']"
        ).get_attribute("aria-label"),
        message="the catalog chip must refresh when the custom is deleted",
    )
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert "Theory of Computation" in section.text, section.text
    assert "No longer on CMI's timetable" not in section.text, section.text
    assert not app.css_all("section[aria-label='My courses'] .badge.custom")


def t43_custom_form_survives_a_sync(app):
    """A sync landing while the create/edit form is open must not rebuild
    it: the dialog is constructed inside DialogHost's reactive closure, so
    every read in its builder is untracked and typed-but-unsaved input
    stays put. The live shadow note is the one exception (own closure)."""
    write_fake_mirror()
    try:
        app.boot("/", selection=["TOC"])
        app.open_tab("My courses")
        app.wait_css(".add-own-card").click()
        app.wait_css(".dialog .custom-form")
        app.css("#cc-name").send_keys("Half-typed seminar")
        app.css("#cc-add-meeting").click()
        app.wait_css(".custom-form .meeting-draft")

        # Sync from behind the modal overlay, the way the background
        # 12-hour re-check would land on its own.
        sync = app.xpath("//button[normalize-space()='Sync now']")
        app.d.execute_script("arguments[0].click();", sync)
        WebDriverWait(app.d, 30).until(
            lambda d: "Synced" in app.css(".sync-pill").text,
            message=f"sync should finish; pill: {app.css('.sync-pill').text!r}",
        )

        app.css(".dialog .custom-form")  # still open
        assert app.css("#cc-name").get_attribute("value") == "Half-typed seminar", \
            app.css("#cc-name").get_attribute("value")
        assert app.css("#cc-code").get_attribute("value") == "HALFTYPE", \
            app.css("#cc-code").get_attribute("value")
        assert len(app.css_all(".custom-form .meeting-draft")) == 1, \
            "the meeting row the user added must survive the sync"
    finally:
        remove_fake_mirror()


def _open_toc_tuesday_edit(app):
    """Details dialog for TOC -> the Tuesday meeting's Edit button."""
    app.chip("TOC", "td[data-day='1'][data-slot='550']").click()
    dialog = app.wait_css(".dialog")
    row = next(
        r for r in dialog.find_elements(By.CSS_SELECTOR, "ul.meetings li")
        if "Tue" in r.text
    )
    row.find_element(By.XPATH, ".//button[normalize-space()='Edit']").click()
    return app.wait_css("#em-hall")


def t44_hall_is_a_working_dropdown(app):
    """The hall field is a real dropdown: it lists every hall CMI publishes,
    opens already sitting on the meeting's current hall, and switches the
    stored hall when another is picked. 'Other place…' reveals a focused
    free-text box for rooms CMI never lists.

    Regression: this was an <input list=…> + <datalist>. Browsers filter
    datalist suggestions against the text already in the box, and the box
    starts pre-filled with the current hall — so the list collapsed to a
    single suggestion (the value already there) and the dropdown looked
    dead."""
    app.boot("/?c=TOC")
    halls = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.snapshot')).halls;"
    )
    assert len(halls) >= 3, halls

    sel = Select(_open_toc_tuesday_edit(app))
    assert sel.first_selected_option.get_attribute("value") in halls, \
        "the dropdown must open on the meeting's own hall, not on nothing"
    current = sel.first_selected_option.get_attribute("value")
    # Every hall, plus "Hall to be announced" and "Other place…".
    assert len(sel.options) == len(halls) + 2, \
        f"{len(sel.options)} options for {len(halls)} halls"

    # Pick a different hall — the whole point of the control.
    moved_to = next(h for h in halls if h != current)
    sel.select_by_value(moved_to)
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save']").click()
    app.wait_toast("Moved TOC")
    stored = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.overrides'))"
        ".items.map(o => o.to.hall);"
    )
    assert stored == [moved_to], stored

    # Re-opening shows the new hall, and "Other place…" opens a focused box.
    sel = Select(_open_toc_tuesday_edit(app))
    assert sel.first_selected_option.get_attribute("value") == moved_to
    sel.select_by_visible_text("Other place…")
    box = app.wait_css("#em-hall-other")
    assert app.d.switch_to.active_element == box, "the box should take focus"
    box.clear()
    box.send_keys("Seminar room")
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save']").click()
    app.wait_toast("Moved TOC")

    # A place CMI doesn't list survives — and comes back as an ordinary
    # choice under "Your own places", so it is typed once and picked after.
    sel = Select(_open_toc_tuesday_edit(app))
    assert sel.first_selected_option.get_attribute("value") == "Seminar room", \
        sel.first_selected_option.get_attribute("value")
    group = app.css("#em-hall optgroup[label='Your own places']")
    assert "Seminar room" in group.text, group.text
    assert not app.css_all("#em-hall-other"), \
        "a known place needs no free-text box"
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)

    # The same control, same behaviour, in the create-your-own-course form.
    app.open_tab("My courses")
    app.wait_css(".add-own-card").click()
    app.wait_css(".dialog .custom-form")
    app.css("#cc-name").send_keys("Reading group")
    app.css("#cc-add-meeting").click()
    row_hall = Select(app.wait_css("#cc-hall-0"))
    # CMI's halls, the "Seminar room" invented above, and the two standing
    # rows — a place typed once is offered everywhere afterwards.
    assert len(row_hall.options) == len(halls) + 3, len(row_hall.options)
    assert "Seminar room" in app.css(
        "#cc-hall-0 optgroup[label='Your own places']").text
    row_hall.select_by_value(halls[1])
    app.xpath("//button[normalize-space()='Add to my timetable']").click()
    app.wait_toast("Added READING")
    saved = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.custom'))"
        ".courses[0].meetings[0].hall;"
    )
    assert saved == halls[1], saved


def t45_edit_meeting_form_survives_a_sync(app):
    """Same rule as the create form (t43): the edit-meeting dialog is built
    inside DialogHost's reactive closure, so its builder reads untracked. A
    sync landing behind the modal must not rebuild the form and put the
    meeting's original day, time and hall back."""
    write_fake_mirror()
    try:
        app.boot("/?c=TOC")
        halls = app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.snapshot')).halls;"
        )
        sel = Select(_open_toc_tuesday_edit(app))
        current = sel.first_selected_option.get_attribute("value")
        moved_to = next(h for h in halls if h != current)
        Select(app.css("#em-day")).select_by_visible_text("Friday")
        sel.select_by_value(moved_to)

        sync = app.xpath("//button[normalize-space()='Sync now']")
        app.d.execute_script("arguments[0].click();", sync)
        WebDriverWait(app.d, 30).until(
            lambda d: "Synced" in app.css(".sync-pill").text,
            message=f"sync should finish; pill: {app.css('.sync-pill').text!r}",
        )

        assert Select(app.css("#em-day")).first_selected_option.text == "Friday", \
            "the day picked before the sync must still be picked"
        assert Select(app.css("#em-hall")).first_selected_option.get_attribute(
            "value"
        ) == moved_to, "the hall picked before the sync must still be picked"
        app.xpath("//div[@class='dialog']//button[normalize-space()='Save']").click()
        app.wait_toast("Moved TOC to Fri")
    finally:
        remove_fake_mirror()


def _halls_day(app, short):
    """Switch the Halls tab to a day by its short name."""
    app.xpath(
        "//section[@aria-label='Lecture halls']//div[@role='group' and @aria-label='Day']"
        f"//button[normalize-space()='{short}']"
    ).click()
    time.sleep(0.3)


def _halls_all(app):
    """Switch the Halls tab to every day at once."""
    _halls_day(app, "All")


# One custom course that exercises both halves of the problem: an official
# hall at a time CMI's grid doesn't have, and a place CMI never listed.
HALL_CUSTOM = {"courses": [{
    "code": "GERMAN", "name": "German A1", "instructors": [], "branches": [],
    "credits": 2, "starts": None, "part_of_semester": None,
    "optional_flag": False, "status": "Scheduled",
    "meetings": [
        {"day": "Mon", "slot": {"start_min": 1110, "end_min": 1185},
         "hall": "Lecture Hall 803", "temp_booking": False},
        {"day": "Mon", "slot": {"start_min": 550, "end_min": 625},
         "hall": "Room 1002", "temp_booking": False},
    ],
}]}


def t46_halls_show_your_own_places_and_times(app):
    """The Halls page has to show the user's own placements, not only CMI's
    allocation: a place CMI never listed gets its own row (marked "yours"),
    a time outside CMI's hours gets its own column, and the user's own
    courses appear at all — they have no override, so the old arrivals loop
    (snapshot courses with overrides) never saw them."""
    app.boot("/", selection=["TOC", "GERMAN"], customs=HALL_CUSTOM)
    app.open_tab("Halls")
    app.wait_css("section[aria-label='Lecture halls']")
    _halls_day(app, "Mon")

    # A row of the user's own, badged, after CMI's halls.
    own = app.wait_css("section[aria-label='Lecture halls'] tr.own-hall")
    head = own.find_element(By.CSS_SELECTOR, "th.rowhead")
    assert "Room 1002" in head.text and "your own" in head.text, head.text
    assert app.chips("GERMAN", "td[data-hall='Room 1002'][data-slot='550']"), \
        "the custom course must render in the place the user invented"

    # An out-of-grid time gets its own column here too, exactly like the
    # personal timetable — 18:30 starts after CMI's last slot ends.
    extra = app.css("section[aria-label='Lecture halls'] thead th.extra")
    assert "18:30" in extra.text, extra.text
    assert app.chips("GERMAN", "td[data-hall='Lecture Hall 803'][data-slot='1110']"), \
        "an evening meeting must land in the evening column, not vanish"

    # The finder speaks about the same world: it never calls a hall free
    # when one of your own meetings is sitting in it, and it says plainly
    # that your own places are not CMI's to allocate.
    section = app.css("section[aria-label='Lecture halls']")
    section.find_element(
        By.CSS_SELECTOR, "select[aria-label='Time slot'] option[value='550']"
    ).click()
    section.find_element(
        By.CSS_SELECTOR, "select[aria-label='Day'] option[value='0']"
    ).click()  # Monday
    app.wait_css(".finder-result")
    assert "Room 1002" not in [li.text for li in app.css_all(".hall-list li")], \
        "a place CMI doesn't allocate must not be offered as a free hall"
    assert "Room 1002" in app.css(".finder-note").text, \
        "…but the page must say why it isn't there"


def t47_moved_out_of_grid_meeting_keeps_its_hall_row(app):
    """The user's own report: change a course to a time outside CMI's hours
    and the Halls table must grow a column for it, like My timetable does.
    Its official cell empties, and the free-hall finder agrees — the room it
    left really is free now."""
    evening = {
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
    app.boot("/", selection=["TOC"], overrides=evening)
    app.open_tab("Halls")
    app.wait_css("section[aria-label='Lecture halls']")
    _halls_day(app, "Tue")

    extra = app.css("section[aria-label='Lecture halls'] thead th.extra")
    assert "18:30" in extra.text, extra.text
    assert app.chips("TOC", "td[data-hall='Lecture Hall 803'][data-slot='1110']"), \
        "the moved meeting must render in its new column"
    assert not app.chips("TOC", "td[data-hall='Lecture Hall 803'][data-slot='550']"), \
        "and must leave the cell it came from"

    section = app.css("section[aria-label='Lecture halls']")
    section.find_element(
        By.CSS_SELECTOR, "select[aria-label='Time slot'] option[value='550']"
    ).click()
    section.find_element(
        By.CSS_SELECTOR, "select[aria-label='Day'] option[value='1']"
    ).click()  # Tuesday
    app.wait_css(".finder-result")
    free = [li.text for li in app.css_all(".hall-list li")]
    assert "Lecture Hall 803" in free, \
        f"the hall TOC moved out of is free now, and the grid already says so: {free}"


def t49_halls_day_selection(app):
    """The Halls tab opens on today (or on every day, when today isn't a
    teaching day), offers an "All" view that stacks one table per day, and
    remembers a chosen day across reloads."""
    app.boot("/")
    app.open_tab("Halls")
    app.wait_css("section[aria-label='Lecture halls']")
    tables = "section[aria-label='Lecture halls'] table.tt"

    # Fixture days are Mon–Fri, so a weekend visit opens on all of them and
    # a weekday visit opens on that day alone.
    weekday = app.d.execute_script("return new Date().getDay();")  # 0 = Sunday
    corners = [
        t.find_element(By.CSS_SELECTOR, "th.corner").text
        for t in app.css_all(tables)
    ]
    if weekday in (0, 6):
        # Every day at once, and the all-days view is merged by default —
        # so that is one table, headed for a hall and a day.
        assert corners == ["Hall · day"], corners
    else:
        assert len(corners) == 1, corners
        today = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday",
                 "Friday", "Saturday"][weekday]
        assert corners[0] == today, corners

    # "All" shows every day — as ONE table, headed for a hall and a day.
    _halls_all(app)
    corners = [
        t.find_element(By.CSS_SELECTOR, "th.corner").text
        for t in app.css_all(tables)
    ]
    assert corners == ["Hall · day"], corners

    # A chosen day survives a reload — the preference is stored, not guessed
    # afresh from the clock.
    _halls_day(app, "Thu")
    assert len(app.css_all(tables)) == 1
    app.d.refresh()
    app.wait_css("section[aria-label='Lecture halls']")
    corners = [
        t.find_element(By.CSS_SELECTOR, "th.corner").text
        for t in app.css_all(tables)
    ]
    assert corners == ["Thursday"], corners


def t48_master_grid_extra_column(app):
    """The master grid grows its own column for a time outside CMI's hours,
    like My timetable and the Halls tab. It used to clamp such a meeting into
    CMI's nearest slot, so the column header said something untrue."""
    evening = {
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
    app.boot("/", selection=["TOC"], overrides=evening)
    app.open_tab("Master grid")
    grid = "section[aria-label='Master grid']"
    app.wait_css(grid)
    extra = app.css(f"{grid} thead th.extra")
    assert "18:30" in extra.text, extra.text
    assert app.chips("TOC", f"{grid} td[data-day='1'][data-slot='1110']"), \
        "the moved meeting belongs in its own column"
    assert not app.chips("TOC", f"{grid} td[data-day='1'][data-slot='1020']"), \
        "and must not be clamped into CMI's last slot (17:00–18:15)"


def t50_halls_all_days_one_table(app):
    """The all-days view is ONE table carrying every hall's week. A row still
    stands for one hall on one day, so a drop into it means that day."""
    app.boot("/?c=TOC")
    app.open_tab("Halls")
    section = app.wait_css("section[aria-label='Lecture halls']")
    tables = "section[aria-label='Lecture halls'] table.tt"
    _halls_all(app)

    # Every day, and still a single table.
    assert len(app.css_all(tables)) == 1, "the all-days view is one table"
    table = app.css(tables)
    assert table.find_element(By.CSS_SELECTOR, "th.corner").text == "Hall · day"

    # Rows are hall × day, with each hall's days together and in order.
    days = [b.text for b in section.find_elements(
        By.XPATH, ".//div[@aria-label='Day']//button")][:-1]  # minus "All"
    rows = table.find_elements(By.CSS_SELECTOR, "tbody tr")
    assert len(rows) % len(days) == 0 and len(rows) >= len(days), len(rows)
    first = rows[:len(days)]
    assert [r.find_element(By.CSS_SELECTOR, "th .day-tag").text
            for r in first] == days
    assert len({r.find_element(By.CSS_SELECTOR, "th .hall-name").text
                for r in first}) == 1, "a hall's days must sit together"

    # A drop lands on the day of the ROW, not on some day the merge lost.
    src = "td[data-day='1'][data-hall='Lecture Hall 803'][data-slot='550']"
    dst = "td[data-day='3'][data-hall='Lecture Hall 803'][data-slot='840']"
    app.wait_css(f"{src} button.chip[aria-label^='TOC,']")
    section.find_element(
        By.XPATH, ".//button[contains(.,'Edit layout')]").click()
    app.drag(app.chip("TOC", src), app.css(dst))
    app.wait_toast("Moved TOC to Thu 14:00–15:15 · Lecture Hall 803")
    assert app.chips("TOC", dst) and not app.chips("TOC", src)

    # …and it is still one table after a reload, with the move in it.
    app.d.refresh()
    app.wait_css("section[aria-label='Lecture halls']")
    assert len(app.css_all(tables)) == 1
    app.wait_css(f"{dst} button.chip[aria-label^='TOC,']")


def _meeting(day, start, end, hall):
    return {"day": day, "slot": {"start_min": start, "end_min": end},
            "hall": hall, "temp_booking": False}


def t51_changes_are_grouped_by_what_they_did(app):
    """Your changes are grouped by WHAT KIND of change they are, each group
    headed by its kind and count, and a row shows only the part that
    actually changed — a room move prints two room names, not two nearly
    identical sentences."""
    hall, other = "Lecture Hall 803", "Seminar Hall"
    overrides = {
        "next_id": 3,
        "items": [
            # time only, room only, and a removal
            {"id": 0, "course": "TOC",
             "base": _meeting("Tue", 550, 625, hall),
             "to": _meeting("Wed", 1020, 1095, hall),
             "created_at": 1754000000000.0},
            {"id": 1, "course": "ISS",
             "base": _meeting("Tue", 550, 625, hall),
             "to": _meeting("Tue", 550, 625, other),
             "created_at": 1754000001000.0},
            {"id": 2, "course": "TOC",
             "base": _meeting("Thu", 550, 625, hall),
             "to": None,
             "created_at": 1754000002000.0},
        ],
        "credits": [{"course": "ISS", "credits": 2,
                     "created_at": 1754000003000.0}],
    }
    app.boot("/", selection=["TOC", "ISS"], overrides=overrides)
    panel = app.wait_css("[data-testid='your-changes']")

    # One group per kind, in a fixed order, each headed by kind and count.
    heads = [h.text for h in panel.find_elements(
        By.CSS_SELECTOR, ".change-group h4 .ck")]
    assert heads == ["Moved to another time", "Moved to another room",
                     "Meeting you removed", "Credits you set"], heads
    counts = [c.text for c in panel.find_elements(
        By.CSS_SELECTOR, ".change-group .cg-count")]
    assert counts == ["1", "1", "1", "1"], counts

    groups = panel.find_elements(By.CSS_SELECTOR, ".change-group")
    # A room move says the rooms, and keeps the unchanged time as context.
    room = groups[1].find_element(By.CSS_SELECTOR, "li")
    assert room.find_element(By.CSS_SELECTOR, ".was").text == hall
    assert room.find_element(By.CSS_SELECTOR, ".now").text == other
    assert "Tue 09:10" in room.find_element(By.CSS_SELECTOR, ".ctx").text
    # A time move says the times, and keeps the unchanged room as context.
    moved = groups[0].find_element(By.CSS_SELECTOR, "li")
    assert "Tue 09:10" in moved.find_element(By.CSS_SELECTOR, ".was").text
    assert "Wed 17:00" in moved.find_element(By.CSS_SELECTOR, ".now").text
    assert hall in moved.find_element(By.CSS_SELECTOR, ".ctx").text
    # A removal has nothing on the right and is struck through.
    gone = groups[2].find_element(By.CSS_SELECTOR, "li .was.gone")
    assert "Thu 09:10" in gone.text, gone.text
    assert not groups[2].find_elements(By.CSS_SELECTOR, "li .now")
    # Restoring is what the removal's button offers; the others remove.
    assert groups[2].find_element(By.CSS_SELECTOR, "li .btn").text == "Restore"
    assert groups[0].find_element(By.CSS_SELECTOR, "li .btn").text == "Remove"


TESTS = [
    t01_header_sync_button_and_hidden_dev,
    t02_developer_endpoint_only,
    t03_url_selection_and_clash,
    t04_unknown_code_warning,
    t05_credits_default_four,
    t06_master_grid_wont_fit_warning,
    t07_clash_toast_on_add,
    t08_master_grid_info_button,
    t09_drag_requires_edit_mode,
    t10_deselect_keeps_custom_time,
    t11_my_data_lists_and_removes_overrides,
    t12_undo_redo,
    t13_reload_persists_state,
    t14_edit_dialog_and_unscheduled,
    t15_halls_free_finder,
    t16_facet_menus_close_each_other,
    t17_credit_override,
    t18_overwrites_panel_and_remove_all,
    t19_add_extra_meetings,
    t20_url_codes_any_case,
    t21_halls_drag_moves_hall_and_slot,
    t22_filter_menu_keeps_focus_and_scroll,
    t23_master_grid_marks_selected,
    t24_toast_pauses_while_hovered,
    t25_first_run_prompt_when_empty,
    t26_first_sync_populates_from_mirror,
    t27_filters_undo_redo,
    t28_facet_menu_search_and_select_all,
    t29_share_link_carries_custom_changes,
    t30_sync_merge_conflict_flow,
    t31_keyboard_move_mode,
    t32_corrupt_storage_recovery,
    t33_export_ics_honors_overrides,
    t34_mobile_longpress_drag,
    t35_remove_meeting,
    t36_out_of_grid_meeting_gets_its_own_column,
    t37_catalog_updates_live,
    t38_duration_based_credits,
    t39_sync_pill_ticks_live,
    t40_custom_course_create,
    t41_custom_course_edit_park_share_delete,
    t42_custom_course_shadowed_by_cmi,
    t43_custom_form_survives_a_sync,
    t44_hall_is_a_working_dropdown,
    t45_edit_meeting_form_survives_a_sync,
    t46_halls_show_your_own_places_and_times,
    t47_moved_out_of_grid_meeting_keeps_its_hall_row,
    t48_master_grid_extra_column,
    t49_halls_day_selection,
    t50_halls_all_days_one_table,
    t51_changes_are_grouped_by_what_they_did,
]


def main():
    if not os.path.isdir(DIST):
        sys.exit(f"dist directory not found: {DIST} — run `trunk build --release` first")
    build_seed()
    server = serve_dist()
    driver = make_driver()
    app = App(driver)
    failures = []
    # Optional argv filter, so one failing case can be re-run on its own:
    #     python test_app.py t44 t45
    only = sys.argv[1:]
    tests = [t for t in TESTS if not only or any(f in t.__name__ for f in only)]
    try:
        for test in tests:
            name = test.__name__
            try:
                test(app)
                print(f"PASS  {name}")
            except Exception:
                failures.append(name)
                print(f"FAIL  {name}")
                traceback.print_exc()
    finally:
        driver.quit()
        server.shutdown()
    print(f"\n{len(tests) - len(failures)}/{len(tests)} passed")
    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
