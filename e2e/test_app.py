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
committed test fixtures at startup (core's `snapshot_json` example) and seeds
it into localStorage before each test — every test still runs offline and
deterministically.

Nothing here ever touches the real network. The browser is started with every
non-localhost hostname blackholed, so a sync fails instantly by default,
which is what most tests want. The few tests that need a sync to SUCCEED turn
on a stand-in for cmi.ac.in: `serve_cmi()` starts a TLS server on localhost
holding the fixture pages, and Chromium is told to resolve www.cmi.ac.in to
it. That means those tests exercise the app's DIRECT tier — the same code
path a real student's browser takes first — rather than a special one that
only exists under test.
"""

import http.server
import json
import os
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import traceback

from selenium import webdriver
from selenium.webdriver.chrome.options import Options
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.common.actions.wheel_input import ScrollOrigin
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import Select, WebDriverWait

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, ".."))
DIST = os.environ.get("DIST_DIR", os.path.join(HERE, "..", "app", "dist"))
PORT = int(os.environ.get("PORT", "8977"))
# Where the stand-in for www.cmi.ac.in listens (see serve_cmi below).
CMI_PORT = int(os.environ.get("CMI_PORT", "8978"))
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

# Filled by build_seed() at startup: the snapshot as the app's own parser
# produces it from the fixtures, ready to drop into localStorage.
SEED_SNAPSHOT = None
SEED_SNAPSHOT_JSON = None


def build_seed():
    """Derive the seed snapshot from the committed fixtures with the exact
    same parser the app uses (core's `snapshot_json` example)."""
    global SEED_SNAPSHOT, SEED_SNAPSHOT_JSON
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
    SEED_SNAPSHOT = json.loads(result.stdout)
    SEED_SNAPSHOT["fetched_at"] = time.time() * 1000.0
    SEED_SNAPSHOT["source"] = "Direct"
    SEED_SNAPSHOT_JSON = json.dumps(SEED_SNAPSHOT)


def cache_from_before_cmi_moved_toc(gone_code="QCOM"):
    """A cached snapshot that disagrees with CMI's live pages, plus the
    override anchored to it.

    The app has no way to be handed a *different* CMI — the fixtures are what
    the fake CMI serves. So a test that needs "CMI moved a class you had
    customised" arranges it from the other side: the cache remembers TOC's
    first class on Friday 14:00, the student moved that class to Wednesday
    17:00, and the live pages put it back on Tuesday 09:10. From the merge's
    point of view that is exactly an upstream move of a meeting the student
    had customised, which is the situation under test.

    `gone_code` is renamed in the cache only, so that course looks removed
    upstream on the next sync. Returns (snapshot_json, overrides, gone_code).
    """
    snap = json.loads(SEED_SNAPSHOT_JSON)
    moved_from = None
    for course in snap["courses"]:
        if course["code"] != "TOC":
            continue
        for m in course["meetings"]:
            if m["day"] == "Tue" and m["slot"]["start_min"] == 550:
                m["day"] = "Fri"
                m["slot"] = {"start_min": 840, "end_min": 915}
                moved_from = json.loads(json.dumps(m))
    assert moved_from is not None, "the fixture must still have TOC on Tue 09:10"

    renamed = f"{gone_code}X"
    assert not any(c["code"] == renamed for c in snap["courses"]), \
        f"{renamed} must not already exist upstream, or nothing looks removed"
    for course in snap["courses"]:
        if course["code"] == gone_code:
            course["code"] = renamed

    overrides = {
        "next_id": 1,
        "items": [{
            "id": 0, "course": "TOC",
            "base": {"day": "Fri",
                     "slot": {"start_min": 840, "end_min": 915},
                     "hall": moved_from.get("hall"), "temp_booking": False},
            "to": {"day": "Wed", "slot": {"start_min": 1020, "end_min": 1095},
                   "hall": moved_from.get("hall"), "temp_booking": False},
            "created_at": 1754000000000.0}],
        "credits": [],
    }
    return json.dumps(snap), overrides, renamed


# ---------------------------------------------------------------------------
# A stand-in for www.cmi.ac.in
#
# The app has exactly one source of data: CMI's own two pages, fetched over
# https. So to test a sync that actually succeeds, we have to be CMI. This
# serves the fixture pages over TLS on localhost, and Chromium is started
# with www.cmi.ac.in resolving here and certificate errors ignored. The app
# is unmodified and unaware — it runs its normal DIRECT tier, the same code
# path a real student's browser takes first.
#
# Off by default: most tests want an unreachable CMI, which is what the
# blackholed resolver gives them, and leaving it off keeps those honest.
# ---------------------------------------------------------------------------

CMI_PAGES = {
    "/practical/timetable.php": "timetable.php.html",
    "/practical/lecturehalls.php": "lecturehalls.php.html",
}

_cmi = {"up": False, "bodies": {}}


def _make_cert(directory):
    """Self-signed cert for www.cmi.ac.in. Chromium is told to ignore
    certificate errors, so this only has to exist, not be trusted."""
    key = os.path.join(directory, "cmi-key.pem")
    crt = os.path.join(directory, "cmi-cert.pem")
    subprocess.run(
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
         "-keyout", key, "-out", crt, "-days", "1",
         "-subj", "/CN=www.cmi.ac.in",
         "-addext", "subjectAltName=DNS:www.cmi.ac.in"],
        check=True, capture_output=True,
    )
    return key, crt


class _CmiHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if not _cmi["up"] or path not in _cmi["bodies"]:
            self.send_response(503)
            self.send_header("Content-Length", "0")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
            return
        body = _cmi["bodies"][path].encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        # The real CMI sends no CORS header — that is the whole reason the
        # proxy tier exists. This one does, because a test that had to go
        # through a public relay to reach localhost would be testing the
        # relay, not the app.
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args, **kwargs):
        pass


def serve_cmi(timetable=None, lecturehalls=None):
    """Make CMI reachable for this test. Serves the fixture pages verbatim
    unless given something else."""
    given = {"timetable.php.html": timetable,
             "lecturehalls.php.html": lecturehalls}
    for path, name in CMI_PAGES.items():
        body = given[name]
        if body is None:
            with open(os.path.join(FIXTURES, name), encoding="utf-8") as f:
                body = f.read()
        _cmi["bodies"][path] = body
    _cmi["up"] = True


def stop_serving_cmi():
    """Back to an unreachable CMI — the state every test starts from."""
    _cmi["up"] = False
    _cmi["bodies"] = {}


def serve_fake_cmi():
    tmp = tempfile.mkdtemp(prefix="cmitt-e2e-cmi-")
    key, crt = _make_cert(tmp)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(crt, key)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", CMI_PORT), _CmiHandler)
    server.socket = ctx.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


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
    # Blackhole every non-localhost hostname: no test can touch the real
    # network, so a sync fails instantly and deterministically. The one
    # exception is CMI itself, pointed at our own TLS stand-in — which
    # answers 503 unless a test called serve_cmi(), so the default is still
    # "CMI is unreachable".
    opts.add_argument(
        f"--host-resolver-rules=MAP www.cmi.ac.in 127.0.0.1:{CMI_PORT}, "
        "MAP * ~NOTFOUND, EXCLUDE 127.0.0.1"
    )
    # The stand-in's certificate is self-signed; it is only there because
    # the app fetches over https.
    opts.add_argument("--ignore-certificate-errors")
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
        lambda d: "None. Courses you add or delete" in app.css(".dialog").text
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
    """'Give it a time' opens the course editor on a fresh meeting row, and
    saving selects the course and places it."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    app.chip("SVA").click()  # unscheduled course → details dialog
    dialog = app.wait_css(".dialog")
    assert "hasn't put it on the timetable" in dialog.text
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Give it a time']").click()
    app.wait_css(".dialog .course-form #ce-day-0")
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
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
    """Credits are overwritten in the course editor, feed the total, and are
    listed with their official value and removable."""
    app.boot("/?c=TOC,RDBM")
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "6"  # 4 assumed + 2 stated
    app.chip("TOC").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    app.xpath("//div[contains(@class,'seg')]/button[normalize-space()='3']").click()
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    app.wait_gone(".dialog")
    # Re-opening says whose number it is and exactly what it replaced.
    app.chip("TOC", "section[aria-label='My courses']").click()
    dialog = app.wait_css(".dialog")
    assert "set by you" in dialog.text and "CMI: 4 assumed" in dialog.text, dialog.text
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()
    app.wait_gone(".dialog")
    section = app.css("section[aria-label='My courses']")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "5", section.text
    assert "1 credit value set by you." in section.text, section.text
    # The 'Your changes' panel shows official → yours; removing it restores.
    app.open_tab("My timetable")
    panel = app.wait_css("[data-testid='your-changes']")
    # Case-insensitive: the group heading is small caps via CSS, and
    # .text returns painted text. The wording is the assertion, not the
    # styling.
    assert "credits you set" in panel.text.lower(), panel.text
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
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    app.xpath("//div[contains(@class,'seg')]/button[normalize-space()='2']").click()
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    app.wait_gone(".dialog")
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
    """Any course can gain extra weekly time slots, added in the editor. Two
    of them are two independent meetings — the second must not overwrite the
    first — and CMI's own meetings are untouched."""
    app.boot("/?c=TOC")
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    card = "//div[contains(@class,'card')][.//strong[contains(.,'Theory of Computation')]]"
    app.xpath(f"{card}//button[normalize-space()='Edit course']").click()
    app.wait_css(".dialog .course-form")
    assert "Edit TOC" in app.css(".dialog h2").text, app.css(".dialog h2").text

    def add_meeting(key, day_idx, slot_start):
        app.css("#ce-add-meeting").click()
        app.wait_css(f"#ce-day-{key}")
        app.css(f"#ce-day-{key} option[value='{day_idx}']").click()
        row = app.css_all(".course-form .meeting-draft")[-1]
        Select(
            row.find_element(By.CSS_SELECTOR, "select[aria-label='Time']")
        ).select_by_value(str(slot_start))

    # TOC's own two meetings hold keys 0 and 1; new rows carry on from there.
    add_meeting(2, 2, 1020)
    add_meeting(3, 4, 1020)
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    app.wait_gone(".dialog")
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
    failure banner when the sync can't get through (nothing reachable)."""
    stop_serving_cmi()  # make sure the previous test left CMI unreachable
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


def t26_first_sync_populates_from_cmi(app):
    """With CMI reachable, the automatic first sync fills the app straight
    from its pages: welcome disappears, tabs appear, data renders, and the
    pill says the data came directly from CMI."""
    serve_cmi()
    try:
        app.boot("/", seed=False)
        app.wait_css(".tabs .tab", timeout=30)
        app.wait_gone(".welcome-card")
        assert "direct" in app.css(".sync-pill").text, app.css(".sync-pill").text
        app.open_tab("Master grid")
        app.wait_css("section[aria-label='Master grid'] table.tt")
        app.chip("TOC")
    finally:
        stop_serving_cmi()


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
    cached, overrides, gone = cache_from_before_cmi_moved_toc()
    serve_cmi()
    try:
        app.boot("/", selection=["TOC", gone], overrides=overrides,
                 raw_snapshot=cached)
        app.xpath("//button[normalize-space()='Sync now']").click()
        dialog = app.wait_css(".dialog", timeout=30)
        assert "your time" in dialog.text and "Tue 09:10" in dialog.text, dialog.text
        # Default is "Use CMI's" — actively keep the user's time instead.
        dialog.find_element(
            By.XPATH, ".//button[normalize-space()='Keep mine for all']"
        ).click()
        dialog.find_element(By.XPATH, ".//button[normalize-space()='Apply']").click()
        app.wait_toast("Conflicts resolved")
        app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
        app.wait_toast(f"{gone} is no longer on CMI's timetable")
        banner = app.xpath("//div[contains(@class,'banner')][contains(.,'CMI updated')]")
        banner.find_element(
            By.XPATH, ".//button[normalize-space()='See what changed']"
        ).click()
        dlg = app.wait_css(".dialog")
        assert "No longer listed" in dlg.text and gone in dlg.text, dlg.text
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
        stop_serving_cmi()


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

    # Details dialog -> Edit this course -> strike the Tuesday row out.
    app.chip("TOC", "td[data-day='1'][data-slot='550']").click()
    dialog = app.wait_css(".dialog")
    rows = dialog.find_elements(By.CSS_SELECTOR, "ul.meetings li")
    assert len(rows) == 2, f"expected 2 meeting rows, got {len(rows)}"
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    _draft_for_day(app, "Tuesday").find_element(
        By.CSS_SELECTOR, "button.icon"
    ).click()
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    app.wait_gone(".dialog")
    # Re-opening: one meeting row left.
    app.chip("TOC", "td[data-day='3'][data-slot='550']").click()
    WebDriverWait(app.d, 5).until(
        lambda d: len(d.find_elements(By.CSS_SELECTOR, ".dialog ul.meetings li")) == 1,
        message="the details dialog should show one meeting row",
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
    assert "meeting you removed" in dialog_text.lower(), dialog_text[:300]
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
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    _draft_for_day(app, "Tuesday").find_element(
        By.CSS_SELECTOR, "button.icon"
    ).click()
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    app.wait_gone(".dialog")
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
    app.wait_css(".dialog .course-form")

    # Name first; the code follows until touched.
    app.css("#ce-name").send_keys("German A1")
    assert app.css("#ce-code").get_attribute("value") == "GERMAN"
    app.xpath("//div[contains(@class,'seg')]/button[normalize-space()='2']").click()

    # Meeting 1: Tuesday, first official slot (09:10) — clashes with TOC.
    app.css("#ce-add-meeting").click()
    app.css("#ce-day-0 option[value='1']").click()
    note = app.css(".course-form .clash-note")
    assert "clashes with TOC" in note.text and "you can still add it" in note.text, note.text

    # Meeting 2: Monday evening, custom time — 18:30 starts after CMI's
    # last official slot (17:00–18:15) ends, so it's outside the grid.
    app.css("#ce-add-meeting").click()
    row2 = app.css_all(".course-form .meeting-draft")[1]
    row2.find_element(By.CSS_SELECTOR, "select[aria-label='Time'] option[value='custom']").click()
    _js_set(app, row2.find_element(By.CSS_SELECTOR, "input[aria-label='Start time']"), "18:30")
    _js_set(app, row2.find_element(By.CSS_SELECTOR, "input[aria-label='End time']"), "19:45")
    # A place CMI never lists: the hall dropdown's "Other place…" row opens
    # a free-text box for it.
    Select(
        row2.find_element(By.CSS_SELECTOR, "select[aria-label='Hall or place']")
    ).select_by_visible_text("Other place…")
    app.wait_css("#ce-hall-1-other").send_keys("Sports annexe")

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


GYM_CARD = (
    "//section[@aria-label='My courses']//div[contains(@class,'card')]"
    "[.//button[starts-with(@aria-label,'GYM,')]]"
)


def t41_custom_course_edit_park_share_delete(app):
    """Editing moves the definition itself (no override bookkeeping), a
    removed custom parks under 'off the timetable' instead of dying, the
    full share link carries the definition to a fresh browser, and delete
    is one undoable step."""
    app.boot("/", selection=["TOC"])
    app.open_tab("My courses")
    app.wait_css(".add-own-card").click()
    app.wait_css(".dialog .course-form")
    app.css("#ce-name").send_keys("Gym")
    app.xpath("//div[contains(@class,'seg')]/button[normalize-space()='0']").click()
    app.css("#ce-add-meeting").click()
    app.css("#ce-day-0 option[value='2']").click()  # Wednesday, first slot
    app.xpath("//button[normalize-space()='Add to my timetable']").click()
    app.wait_gone(".dialog")
    pills = [p.text for p in app.css_all(".credit-summary .cs-pill")]
    assert "1 course at 0 credits" in pills, pills

    # Edit the course itself: Wednesday → Friday. The chip follows. (Every
    # card offers Edit course now, so it has to be GYM's own.)
    app.xpath(f"{GYM_CARD}//button[normalize-space()='Edit course']").click()
    form = app.wait_css(".dialog .course-form")
    # Deleting is not offered from inside the edit form — it belongs to the
    # course's own dialog, beside Edit.
    assert not form.find_elements(
        By.XPATH, ".//button[normalize-space()='Delete this course']"
    ), "the edit form must not offer to delete the course"
    app.css("#ce-day-0 option[value='4']").click()
    app.xpath("//button[normalize-space()='Save changes']").click()
    app.wait_gone(".dialog")
    app.open_tab("My timetable")
    app.wait_css("td[data-day='4'][data-slot='550'] button.chip[aria-label^='GYM,']")

    # Move it once more — Friday → Thursday. A course of your own has no CMI
    # version underneath, so editing it rewrites the definition itself and
    # must never leave an override behind.
    app.open_tab("My courses")
    app.xpath(f"{GYM_CARD}//button[normalize-space()='Edit course']").click()
    app.wait_css(".dialog .course-form")
    app.css("#ce-day-0 option[value='3']").click()   # Friday → Thursday
    app.xpath("//button[normalize-space()='Save changes']").click()
    app.wait_gone(".dialog")
    app.open_tab("My timetable")
    app.wait_css("td[data-day='3'][data-slot='550'] button.chip[aria-label^='GYM,']")
    assert not app.chips("GYM", "td[data-day='4'][data-slot='550']"), \
        "the old cell must be empty — the definition moved, not a copy"
    assert app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.overrides')).items.length;"
    ) == 0, "editing a course of your own must not create an override"
    # It IS one of your changes, though — an addition to CMI's data — so it
    # is listed with everything else you changed.
    app.xpath("//button[contains(.,'1 change')]").click()
    dialog = app.wait_css(".dialog")
    assert "course you added" in dialog.text.lower(), dialog.text
    assert "GYM" in dialog.text, dialog.text
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()
    app.wait_gone(".dialog")
    # And the form still shows the moved time (one source of truth).
    app.open_tab("My courses")
    app.xpath(f"{GYM_CARD}//button[normalize-space()='Edit course']").click()
    app.wait_css(".dialog .course-form")
    assert app.css("#ce-day-0").get_attribute("value") == "3", \
        app.css("#ce-day-0").get_attribute("value")
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
    serve_cmi()
    try:
        app.boot("/", selection=["TOC"])
        app.open_tab("My courses")
        app.wait_css(".add-own-card").click()
        app.wait_css(".dialog .course-form")
        app.css("#ce-name").send_keys("Half-typed seminar")
        app.css("#ce-add-meeting").click()
        app.wait_css(".course-form .meeting-draft")

        # Sync from behind the modal overlay, the way the background
        # 12-hour re-check would land on its own.
        sync = app.xpath("//button[normalize-space()='Sync now']")
        app.d.execute_script("arguments[0].click();", sync)
        WebDriverWait(app.d, 30).until(
            lambda d: "Synced" in app.css(".sync-pill").text,
            message=f"sync should finish; pill: {app.css('.sync-pill').text!r}",
        )

        app.css(".dialog .course-form")  # still open
        assert app.css("#ce-name").get_attribute("value") == "Half-typed seminar", \
            app.css("#ce-name").get_attribute("value")
        assert app.css("#ce-code").get_attribute("value") == "HALFTYPE", \
            app.css("#ce-code").get_attribute("value")
        assert len(app.css_all(".course-form .meeting-draft")) == 1, \
            "the meeting row the user added must survive the sync"
    finally:
        stop_serving_cmi()


def _open_toc_editor(app):
    """Details dialog for TOC -> Edit this course. Returns the hall control
    of the first row (TOC's Tuesday meeting — rows run in week order)."""
    app.chip("TOC", "td[data-day='1'][data-slot='550']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    return app.wait_css("#ce-hall-0")


def _draft_for_day(app, day):
    """The editor's meeting row currently sitting on `day` ("Tuesday")."""
    for row in app.css_all(".course-form .meeting-draft"):
        picked = Select(
            row.find_element(By.CSS_SELECTOR, "select[aria-label='Day']")
        ).first_selected_option.text
        if picked == day:
            return row
    raise AssertionError(f"no meeting row on {day}")


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

    sel = Select(_open_toc_editor(app))
    assert sel.first_selected_option.get_attribute("value") in halls, \
        "the dropdown must open on the meeting's own hall, not on nothing"
    current = sel.first_selected_option.get_attribute("value")
    # Every hall, plus "Hall to be announced" and "Other place…".
    assert len(sel.options) == len(halls) + 2, \
        f"{len(sel.options)} options for {len(halls)} halls"

    # Pick a different hall — the whole point of the control.
    moved_to = next(h for h in halls if h != current)
    sel.select_by_value(moved_to)
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    stored = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.overrides'))"
        ".items.map(o => o.to.hall);"
    )
    assert stored == [moved_to], stored

    # Re-opening shows the new hall, and "Other place…" opens a focused box.
    sel = Select(_open_toc_editor(app))
    assert sel.first_selected_option.get_attribute("value") == moved_to
    sel.select_by_visible_text("Other place…")
    box = app.wait_css("#ce-hall-0-other")
    assert app.d.switch_to.active_element == box, "the box should take focus"
    box.clear()
    box.send_keys("Seminar room")
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")

    # A place CMI doesn't list survives — and comes back as an ordinary
    # choice under "Your own places", so it is typed once and picked after.
    sel = Select(_open_toc_editor(app))
    assert sel.first_selected_option.get_attribute("value") == "Seminar room", \
        sel.first_selected_option.get_attribute("value")
    group = app.css("#ce-hall-0 optgroup[label='Your own places']")
    assert "Seminar room" in group.text, group.text
    assert not app.css_all("#ce-hall-0-other"), \
        "a known place needs no free-text box"
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)

    # The same control, same behaviour, in the create-your-own-course form.
    app.open_tab("My courses")
    app.wait_css(".add-own-card").click()
    app.wait_css(".dialog .course-form")
    app.css("#ce-name").send_keys("Reading group")
    app.css("#ce-add-meeting").click()
    row_hall = Select(app.wait_css("#ce-hall-0"))
    # CMI's halls, the "Seminar room" invented above, and the two standing
    # rows — a place typed once is offered everywhere afterwards.
    assert len(row_hall.options) == len(halls) + 3, len(row_hall.options)
    assert "Seminar room" in app.css(
        "#ce-hall-0 optgroup[label='Your own places']").text
    row_hall.select_by_value(halls[1])
    app.xpath("//button[normalize-space()='Add to my timetable']").click()
    app.wait_toast("Added READING")
    saved = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.custom'))"
        ".courses[0].meetings[0].hall;"
    )
    assert saved == halls[1], saved


def t45_editor_survives_a_sync(app):
    """Same rule as the create form (t43): the editor is built inside
    DialogHost's reactive closure, so its builder reads untracked. A sync
    landing behind the modal must not rebuild the form and put the meeting's
    original day, time and hall back."""
    serve_cmi()
    try:
        app.boot("/?c=TOC")
        halls = app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.snapshot')).halls;"
        )
        sel = Select(_open_toc_editor(app))
        current = sel.first_selected_option.get_attribute("value")
        moved_to = next(h for h in halls if h != current)
        Select(app.css("#ce-day-0")).select_by_visible_text("Friday")
        sel.select_by_value(moved_to)

        sync = app.xpath("//button[normalize-space()='Sync now']")
        app.d.execute_script("arguments[0].click();", sync)
        WebDriverWait(app.d, 30).until(
            lambda d: "Synced" in app.css(".sync-pill").text,
            message=f"sync should finish; pill: {app.css('.sync-pill').text!r}",
        )

        assert Select(app.css("#ce-day-0")).first_selected_option.text == "Friday", \
            "the day picked before the sync must still be picked"
        assert Select(app.css("#ce-hall-0")).first_selected_option.get_attribute(
            "value"
        ) == moved_to, "the hall picked before the sync must still be picked"
        app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
        app.wait_toast("Saved your changes to TOC")
    finally:
        stop_serving_cmi()


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
        # Every day at once — one table, whose first gutter is the hall.
        assert corners == ["Hall"], corners
    else:
        assert len(corners) == 1, corners
        today = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday",
                 "Friday", "Saturday"][weekday]
        assert corners[0] == today, corners

    # "All" shows every day — as ONE table, gutters for hall and day.
    _halls_all(app)
    corners = [
        t.find_element(By.CSS_SELECTOR, "th.corner").text
        for t in app.css_all(tables)
    ]
    assert corners == ["Hall"], corners

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

    # Every day, and still a single table, with a gutter each for the hall
    # and the day.
    assert len(app.css_all(tables)) == 1, "the all-days view is one table"
    table = app.css(tables)
    assert [c.text for c in table.find_elements(
        By.CSS_SELECTOR, "th.corner")] == ["Hall", "Day"]

    # A hall is NAMED ONCE, in a cell spanning its days; the days run down a
    # gutter of their own, in order.
    days = [b.text for b in section.find_elements(
        By.XPATH, ".//div[@aria-label='Day']//button")][1:]  # "All" comes first
    rows = table.find_elements(By.CSS_SELECTOR, "tbody tr")
    names = table.find_elements(By.CSS_SELECTOR, "tbody th.hallhead")
    assert len(rows) == len(names) * len(days), (len(rows), len(names))
    assert all(n.get_attribute("rowspan") == str(len(days)) for n in names), \
        "each hall's name must span its days"
    assert len({n.find_element(By.CSS_SELECTOR, ".hall-name").text
                for n in names}) == len(names), "one name per hall, no repeats"
    first = rows[:len(days)]
    assert [r.find_element(By.CSS_SELECTOR, "th.dayhead").text
            for r in first] == days
    assert len(first[0].find_elements(By.CSS_SELECTOR, "th.hallhead")) == 1
    assert not first[1].find_elements(By.CSS_SELECTOR, "th.hallhead"), \
        "only the first row of a hall's block carries its name"

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
    # textContent, not .text: the heading is small caps via text-transform,
    # and .text would return what is painted (upper case) rather than the
    # label the app actually wrote.
    heads = [h.get_attribute("textContent") for h in panel.find_elements(
        By.CSS_SELECTOR, ".change-group h4 .cg-title")]
    assert heads == ["Moved to another time", "Moved to another room",
                     "Meeting you removed", "Credits you set"], heads
    # Each heading is coloured by what the change does, so the three kinds
    # are told apart before the list is read.
    tones = [g.get_attribute("data-kind") for g in panel.find_elements(
        By.CSS_SELECTOR, ".change-group")]
    assert tones == ["changed", "changed", "gone", "changed"], tones
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


def t52_c_param_keeps_plain_commas(app):
    """The address bar separates codes with plain commas — %2C between every
    pair made it unreadable — while each CODE is still encoded. A link whose
    separators arrived percent-encoded opens exactly the same."""
    app.boot("/?c=TOC,ISS")
    app.wait_css("button.chip")
    assert "?c=TOC,ISS" in app.d.current_url, app.d.current_url

    # The same link with encoded separators: same selection, and the app
    # rewrites the address bar to the readable form.
    app.boot("/?c=TOC%2CISS")
    app.wait_css("button.chip")
    WebDriverWait(app.d, 5).until(
        lambda d: "?c=TOC,ISS" in d.current_url,
        message=f"expected plain commas, got {app.d.current_url}",
    )
    assert app.chips("TOC") and app.chips("ISS")

def t53_delete_a_cmi_course(app):
    """One of CMI's courses can be deleted too. That takes it out of YOUR
    planner — off the timetable, out of the catalog and the master grid —
    and records it in Your changes, where one click brings it back. The
    catalog says how many are hidden, so a short catalog explains itself."""
    app.boot("/?c=TOC,ISS")
    assert app.chips("TOC", "td[data-day='1'][data-slot='550']"), "sanity: on the grid"
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    chip = app.chip("TOC", "section[aria-label='Catalog']")
    app.d.execute_script("arguments[0].scrollIntoView({block:'center'});", chip)
    chip.click()
    dialog = app.wait_css(".dialog")
    delete = dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Delete this course']"
    )
    assert "danger" in delete.get_attribute("class"), delete.get_attribute("class")
    delete.click()
    app.wait_toast("Deleted TOC")

    # Out of the catalog, and the catalog owns up to it.
    WebDriverWait(app.d, 5).until(
        lambda d: not app.chips("TOC", "section[aria-label='Catalog']"),
        message="a deleted course must leave the catalog",
    )
    note = app.css(".deleted-note")
    assert "1 course you deleted is hidden here" in note.text, note.text
    # Out of the master grid and off the timetable.
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    assert not app.chips("TOC", "section[aria-label='Master grid']"), \
        "a deleted course must leave the master grid"
    app.open_tab("My timetable")
    grid = "section[aria-label='My timetable'] table.tt"
    app.wait_css(grid)
    assert not app.chips("TOC", grid), "a deleted course must leave the timetable"
    assert app.chips("ISS", grid), "ISS is untouched"

    # It survives a reload, and it is listed as a change of yours.
    app.d.get(f"{BASE}/")
    app.wait_css(".header h1")
    time.sleep(0.5)
    assert not app.chips("TOC", "section[aria-label='My timetable'] table.tt"), \
        "the deletion must survive a reload"
    app.xpath("//button[contains(.,'change')]").click()
    dialog = app.wait_css(".dialog")
    assert "course you deleted" in dialog.text.lower(), dialog.text
    dialog.find_element(
        By.XPATH, ".//li[contains(.,'TOC')]//button[normalize-space()='Restore']"
    ).click()
    app.wait_toast("TOC is back")
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)

    # Back in the catalog — but NOT back on the timetable: what was deleted
    # was the course, not your selection.
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    WebDriverWait(app.d, 5).until(
        lambda d: bool(app.chips("TOC", "section[aria-label='Catalog']")),
        message="Restore must bring the course back to the catalog",
    )
    assert not app.css_all(".deleted-note")
    assert app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));"
    ) == ["ISS"], "restoring must not re-add it to the timetable"


def t54_editor_saves_everything_in_one_step(app):
    """The whole point of one editor: a move, an addition, a removal and a
    credit change are made together and saved as ONE change — so a single
    Undo puts all four back."""
    app.boot("/?c=TOC")  # Tue + Thu 09:10 in the fixture
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    app.xpath("//button[normalize-space()='Edit course']").click()
    app.wait_css(".dialog .course-form")

    # Move Tuesday to Wednesday 17:00…
    row0 = app.css_all(".course-form .meeting-draft")[0]
    Select(app.css("#ce-day-0")).select_by_visible_text("Wednesday")
    Select(
        row0.find_element(By.CSS_SELECTOR, "select[aria-label='Time']")
    ).select_by_value("1020")
    # …and the row says what it replaced, with one click back to it.
    WebDriverWait(app.d, 5).until(
        lambda d: "Tue 09:10" in row0.find_element(By.CSS_SELECTOR, ".row-origin").text,
        message="a changed row must show the CMI meeting it replaces",
    )
    # …strike Thursday out…
    _draft_for_day(app, "Thursday").find_element(
        By.CSS_SELECTOR, "button.icon"
    ).click()
    # …add a Friday one…
    app.css("#ce-add-meeting").click()
    app.wait_css("#ce-day-2")
    app.css("#ce-day-2 option[value='4']").click()
    # …and set the credits.
    app.xpath("//div[contains(@class,'seg')]/button[normalize-space()='2']").click()
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    app.wait_gone(".dialog")

    app.open_tab("My timetable")
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    assert not app.chips("TOC", "td[data-day='1'][data-slot='550']"), "Tuesday moved"
    assert not app.chips("TOC", "td[data-day='3'][data-slot='550']"), "Thursday removed"
    assert app.chips("TOC", "td[data-day='4'][data-slot='550']"), "Friday added"
    app.xpath("//button[contains(.,'4 changes')]")

    # One action, so one Undo.
    app.d.execute_script("window.scrollTo(0, 0);")
    app.xpath("//button[@aria-label='Undo']").click()
    app.wait_css("td[data-day='1'][data-slot='550'] button.chip[aria-label^='TOC,']")
    app.wait_css("td[data-day='3'][data-slot='550'] button.chip[aria-label^='TOC,']")
    assert not app.chips("TOC", "td[data-day='4'][data-slot='550']")
    assert not app.d.find_elements(
        By.XPATH, "//button[contains(., '✎') and contains(., 'change')]"
    ), "one undo must take the whole edit back"


def t55_destructive_actions_are_red(app):
    """Anything that takes something away wears the same red as 'Delete all
    app data', at rest — not only on hover, and not only in the danger
    zone."""
    app.boot("/?c=TOC")

    def colour(el):
        return app.d.execute_script("return getComputedStyle(arguments[0]).color;", el)

    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    remove = app.xpath(
        "//section[@aria-label='My courses']//button[normalize-space()='Remove']"
    )
    app.d.execute_script("window.scrollTo(0, 0);")
    app.xpath("//button[normalize-space()='My data']").click()
    dialog = app.wait_css(".dialog")
    wipe = dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Delete all app data']"
    )
    close = dialog.find_element(By.XPATH, ".//button[normalize-space()='Close']")
    assert colour(remove) == colour(wipe), \
        f"card Remove {colour(remove)} vs Delete all app data {colour(wipe)}"
    assert colour(wipe) != colour(close), \
        "a destructive button must not look like an ordinary one"
    # The same red, in every list that offers to take something away.
    for label in ("Clear selection", "Reset"):
        btn = dialog.find_element(By.XPATH, f".//button[normalize-space()='{label}']")
        assert colour(btn) == colour(wipe), f"{label}: {colour(btn)}"

def t56_a_link_brings_a_deleted_course_back(app):
    """A course cannot be on your timetable AND deleted: a link that names
    one — an old bookmark, or a friend's share link — is you asking for it
    back, so opening it lifts the deletion instead of contradicting it."""
    app.boot("/?c=TOC,ISS")
    app.chip("TOC", "td[data-day='1'][data-slot='550']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Delete this course']"
    ).click()
    app.wait_toast("Deleted TOC")
    assert app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.overrides')).hidden.length;"
    ) == 1, "sanity: the deletion is stored"

    # The bookmark from before the deletion, on the same browser.
    app.boot("/?c=TOC,ISS", fresh=False)
    app.wait_css("td[data-day='1'][data-slot='550'] button.chip[aria-label^='TOC,']")
    assert app.d.execute_script(
        "return (JSON.parse(localStorage.getItem('cmitt.v1.overrides')).hidden || [])"
        ".length;"
    ) == 0, "the deletion must be lifted, not left contradicting the selection"
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    assert not app.css_all(".deleted-note"), "nothing is hidden any more"

def t57_editor_keeps_a_meeting_whose_cmi_original_moved(app):
    """A base CMI has since moved — an unresolved conflict, or a share link
    imported against fresher data — stands for nothing in today's timetable.
    Putting such a row back on its own base has to KEEP the meeting: it is
    the one case where saving could quietly lose what the form was showing."""
    stale = {
        "next_id": 1,
        "items": [{
            "id": 0, "course": "TOC",
            # CMI has no Wednesday meeting for TOC, so this base is stale.
            "base": _meeting("Wed", 1020, 1095, "Lecture Hall 803"),
            "to": _meeting("Fri", 1020, 1095, "Lecture Hall 803"),
            "created_at": 1754000000000.0}],
        "credits": [],
    }
    app.boot("/", selection=["TOC"], overrides=stale)
    app.wait_css("td[data-day='4'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    app.xpath("//button[normalize-space()='Edit course']").click()
    app.wait_css(".dialog .course-form")
    row = _draft_for_day(app, "Friday")
    row.find_element(By.XPATH, ".//button[normalize-space()='Put it back']").click()
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    app.wait_gone(".dialog")

    app.open_tab("My timetable")
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    assert app.chips("TOC", "td[data-day='1'][data-slot='550']"), "CMI's Tuesday stays"
    assert app.chips("TOC", "td[data-day='3'][data-slot='550']"), "CMI's Thursday stays"
    assert not app.chips("TOC", "td[data-day='4'][data-slot='1020']"), "it left Friday"


def t58_simulated_parse_failure_keeps_everything(app):
    """Developer mode's parse-failure simulator has to actually fail.

    It mangles the saved timetable page and runs it through the real
    pipeline to show the gate keeping the cached data. Since parser v3 a
    page that merely lost its vertical rules still reads perfectly, so the
    mangling has to break something the parser truly depends on — and if it
    ever stops failing, this button demonstrates nothing. The toast and the
    banner below only exist on the far side of that check, so this also
    catches the simulator taking the whole app down with it."""
    app.boot("/#/developer", selection=["TOC"])
    section = app.wait_css("section[aria-label='Developer mode']")
    section.find_element(
        By.XPATH, ".//button[normalize-space()='Simulate parse failure']"
    ).click()

    app.wait_toast("Simulated parse failure")
    banner = app.wait_css(".banner")
    assert "Simulated a parse failure" in banner.text, banner.text
    assert "Nothing was lost" in banner.text, banner.text

    # The report is filed under its own name and its gate really did fail —
    # and the panel re-rendering at all proves the app is still running.
    summary = WebDriverWait(app.d, 10).until(
        lambda d: next(
            (s for s in d.find_elements(By.CSS_SELECTOR,
                                        "section[aria-label='Developer mode'] summary")
             if "simulated-failure" in s.text),
            False,
        ),
        message="no simulated-failure parse report appeared",
    )
    assert "gate FAILED" in summary.text, summary.text

    # And the cached timetable is untouched: the course is still there.
    app.d.execute_script("window.location.hash = '';")
    app.wait_css("td button.chip[aria-label^='TOC,']")


def t59_a_booking_inside_a_slot_still_occupies_the_room(app):
    """CMI's two pages can disagree about the clock.

    A class published at 12:00 against an 11:50 column gets no column of its
    own — a time starting inside an official slot deliberately gets no extra
    column — so it has to be drawn in the slot that contains it, exactly as
    a meeting the user dragged there would be. Falling through the table
    instead would hide the class AND tell the free-hall finder the room is
    empty, which is the one wrong answer on this page that sends somebody to
    a room with a lecture in it."""
    seed = json.loads(SEED_SNAPSHOT_JSON)

    def pick():
        for b in seed["hall_bookings"]:
            if b["day"] != "Mon" or not b["codes"]:
                continue
            code = b["codes"][0]
            course = next((c for c in seed["courses"] if c["code"] == code), None)
            if course is None:
                continue
            for m in course["meetings"]:
                if (m["day"] == "Mon"
                        and m["slot"]["start_min"] == b["slot"]["start_min"]
                        and m.get("hall") == b["hall"]):
                    return b, code, m
        raise AssertionError("seed has no Monday booking backed by a meeting")

    booking, code, meeting = pick()
    hall, column = booking["hall"], booking["slot"]["start_min"]
    # Ten minutes late: still inside its official column, matching none.
    moved = {"start_min": column + 10, "end_min": booking["slot"]["end_min"]}
    booking["slot"] = moved
    meeting["slot"] = moved

    app.boot("/", raw_snapshot=json.dumps(seed), selection=[code])
    app.open_tab("Halls")
    _halls_day(app, "Mon")
    cell = f"td[data-day='0'][data-slot='{column}'][data-hall='{hall}']"
    app.wait_css(f"{cell} button.chip[aria-label^='{code},']")

    # And the finder must not offer a room that has a class standing in it.
    section = app.css("section[aria-label='Lecture halls']")
    section.find_element(
        By.CSS_SELECTOR,
        f"select[aria-label='Time slot'] option[value='{column}']").click()
    section.find_element(
        By.CSS_SELECTOR, "select[aria-label='Day'] option[value='0']").click()
    app.wait_css(".finder-result")
    free = [li.text for li in app.css_all(".hall-list li")]
    assert hall not in free, (hall, free)


def t60_a_conflicting_sync_does_not_steal_the_open_editor(app):
    """There is one dialog slot, and a sync can land at any moment.

    When CMI's update conflicts with something the user changed, the app
    opens the conflicts dialog — but if the course editor is open, taking
    the slot throws away the name they were typing, the rows they added,
    all of it. The conflicts banner is already on screen with Review, so
    the question can wait until they are finished."""
    cached, overrides, _gone = cache_from_before_cmi_moved_toc()
    serve_cmi()
    try:
        app.boot("/", selection=["TOC"], overrides=overrides,
                 raw_snapshot=cached)
        app.open_tab("My courses")
        app.wait_css("section[aria-label='My courses']")
        app.xpath("//button[normalize-space()='Edit course']").click()
        app.wait_css(".dialog .course-form")
        Select(app.css("#ce-day-0")).select_by_visible_text("Monday")

        sync = app.xpath("//button[normalize-space()='Sync now']")
        app.d.execute_script("arguments[0].click();", sync)
        WebDriverWait(app.d, 30).until(
            lambda d: "Synced" in app.css(".sync-pill").text,
            message=f"sync should finish; pill: {app.css('.sync-pill').text!r}",
        )

        # The editor is still there, still holding what they chose.
        assert app.css_all(".course-form"), "the sync must not close the editor"
        assert Select(app.css("#ce-day-0")).first_selected_option.text == "Monday", \
            "and must not reset what they had picked"

        # The conflict is not lost — it is waiting in the banner.
        banner = next(
            b for b in app.css_all(".banner") if "conflict" in b.text
        )
        assert "Review" in banner.text, banner.text

        # Finish the edit, then review the conflict.
        app.xpath(
            "//div[@class='dialog']//button[normalize-space()='Save changes']"
        ).click()
        app.wait_gone(".dialog")
        next(b for b in app.css_all(".banner") if "conflict" in b.text).find_element(
            By.XPATH, ".//button[normalize-space()='Review']"
        ).click()
        dialog = app.wait_css(".dialog")
        assert "TOC" in dialog.text, dialog.text
    finally:
        stop_serving_cmi()


def t61_adding_a_meeting_where_a_moved_one_used_to_be(app):
    """Two rows can want the same CMI meeting, and only one may have it.

    TOC's Tuesday 09:10 class has been moved to Wednesday 17:00. The student
    now adds a class of their own back at Tuesday 09:10 — the slot CMI's
    meeting vacated. The added row coincides with a CMI meeting that the
    moved row already speaks for, and rows are processed in day order, so
    the newcomer used to claim it first, store nothing, and disappear on
    save while the move stored itself against a base already spoken for."""
    app.boot("/", selection=["TOC"], overrides=TOC_OVR)  # Tue 09:10 -> Wed 17:00
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    app.xpath("//button[normalize-space()='Edit course']").click()
    form = app.wait_css(".dialog .course-form")

    # Add a row and put it exactly where CMI's Tuesday class used to be.
    n = len(app.css_all(".course-form .meeting-draft"))
    app.css("#ce-add-meeting").click()
    app.wait_css(f"#ce-day-{n}")
    app.css(f"#ce-day-{n} option[value='1']").click()  # Tuesday
    row = app.css_all(".course-form .meeting-draft")[n]
    Select(
        row.find_element(By.CSS_SELECTOR, "select[aria-label='Time']")
    ).select_by_value("550")
    assert form is not None
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    app.wait_gone(".dialog")

    # Both survive: the moved class where they put it, and the new one where
    # CMI's used to be.
    app.open_tab("My timetable")
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    assert app.chips("TOC", "td[data-day='1'][data-slot='550']"), \
        "the meeting added where CMI's used to be must not vanish on save"

    # …and they survive a reload, i.e. they were really stored.
    app.boot("/", fresh=False)
    app.open_tab("My timetable")
    app.wait_css("td[data-day='1'][data-slot='550'] button.chip[aria-label^='TOC,']")
    assert app.chips("TOC", "td[data-day='2'][data-slot='1020']")


def t62_the_wheel_steps_the_boxes_that_have_a_step(app):
    """Scroll over credits, a meeting time or an export date and it moves one
    step — but ONLY while that box has focus. All three live in dialogs that
    scroll, and a value that changed because someone scrolled past it is a
    change they never asked for."""
    def wheel(el, dy):
        ActionChains(app.d).scroll_from_origin(
            ScrollOrigin.from_element(el), 0, dy).perform()
        time.sleep(0.25)

    # An out-of-grid time, so the editor offers the time boxes at all.
    odd_hour = {
        "next_id": 1,
        "items": [{
            "id": 0, "course": "TOC",
            "base": {"day": "Tue", "slot": {"start_min": 550, "end_min": 625},
                     "hall": "Lecture Hall 803", "temp_booking": False},
            "to": {"day": "Wed", "slot": {"start_min": 1230, "end_min": 1305},
                   "hall": "Lecture Hall 803", "temp_booking": False},
            "created_at": 1754000000000.0}],
        "credits": [],
    }
    app.boot("/", selection=["TOC"], overrides=odd_hour)
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    app.xpath("//button[normalize-space()='Edit course']").click()
    app.wait_css(".dialog .course-form")
    app.xpath("//div[@class='seg']//button[normalize-space()='Other…']").click()
    box = app.wait_css("input[type='number'][aria-label='Credits']")

    # It takes focus by itself, so the wheel works without a second click.
    assert app.d.execute_script(
        "return document.activeElement === arguments[0];", box), \
        "the Other… box must take focus when it appears"

    start = int(box.get_attribute("value"))
    wheel(box, -50)
    assert int(box.get_attribute("value")) == start + 1, box.get_attribute("value")
    wheel(box, 50)
    assert int(box.get_attribute("value")) == start, box.get_attribute("value")

    # The app hears it, exactly as if it had been typed: "Use CMI's value"
    # only shows when the value differs from CMI's.
    wheel(box, -50)
    app.xpath("//button[normalize-space()=\"Use CMI's value\"]")

    # The box's own min/max do the clamping, not us.
    for _ in range(25):
        wheel(box, -50)
    assert box.get_attribute("value") == "20", box.get_attribute("value")
    for _ in range(25):
        wheel(box, 50)
    assert box.get_attribute("value") == "0", box.get_attribute("value")

    # Unfocused, the wheel leaves it alone and scrolls the dialog instead.
    dialog = app.css(".dialog")
    app.d.execute_script("document.activeElement.blur();")
    before = box.get_attribute("value")
    wheel(box, 200)
    assert box.get_attribute("value") == before, \
        "an unfocused box must not change when the dialog scrolls past it"

    # Focused, the dialog stays put — the scroll belongs to the box.
    box.click()
    app.d.execute_script("arguments[0].scrollTop = 0;", dialog)
    wheel(box, 200)
    assert app.d.execute_script("return arguments[0].scrollTop;", dialog) == 0, \
        "a focused box must swallow the scroll, not scroll the dialog too"

    # A meeting time steps by a minute.
    app.d.execute_script("arguments[0].scrollTop = 0;", dialog)
    start_time = app.css("input[type='time'][aria-label='Start time']")
    start_time.click()
    before = start_time.get_attribute("value")
    wheel(start_time, -50)
    assert start_time.get_attribute("value") != before, \
        f"the start time must step: {before}"

    # And an export date by a day.
    app.xpath("//div[@class='dialog']//button[normalize-space()='Cancel']").click()
    app.wait_gone(".dialog")
    app.open_tab("My timetable")
    app.xpath("//button[contains(.,'Export .ics')]").click()
    frm = app.wait_css("#ex-from")
    frm.click()
    before = frm.get_attribute("value")
    wheel(frm, -50)
    assert frm.get_attribute("value") != before, f"the From date must step: {before}"


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
    t26_first_sync_populates_from_cmi,
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
    t45_editor_survives_a_sync,
    t46_halls_show_your_own_places_and_times,
    t47_moved_out_of_grid_meeting_keeps_its_hall_row,
    t48_master_grid_extra_column,
    t49_halls_day_selection,
    t50_halls_all_days_one_table,
    t51_changes_are_grouped_by_what_they_did,
    t52_c_param_keeps_plain_commas,
    t53_delete_a_cmi_course,
    t54_editor_saves_everything_in_one_step,
    t55_destructive_actions_are_red,
    t56_a_link_brings_a_deleted_course_back,
    t57_editor_keeps_a_meeting_whose_cmi_original_moved,
    t58_simulated_parse_failure_keeps_everything,
    t59_a_booking_inside_a_slot_still_occupies_the_room,
    t60_a_conflicting_sync_does_not_steal_the_open_editor,
    t61_adding_a_meeting_where_a_moved_one_used_to_be,
    t62_the_wheel_steps_the_boxes_that_have_a_step,
]


def main():
    if not os.path.isdir(DIST):
        sys.exit(f"dist directory not found: {DIST} — run `trunk build --release` first")
    build_seed()
    server = serve_dist()
    # Always listening, but answering 503 until a test calls serve_cmi().
    cmi = serve_fake_cmi()
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
                # A test that failed before its own cleanup must not leave
                # CMI reachable for the next one.
                stop_serving_cmi()
    finally:
        driver.quit()
        server.shutdown()
        cmi.shutdown()
    print(f"\n{len(tests) - len(failures)}/{len(tests)} passed")
    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
