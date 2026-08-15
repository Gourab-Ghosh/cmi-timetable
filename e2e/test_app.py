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
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import urllib.parse

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
# Where the offline test serves its own copy of dist (see t74): a separate
# port means a separate origin, so its worker registration and caches can
# never leak into the rest of the suite.
SW_PORT = int(os.environ.get("SW_PORT", "8979"))
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


def cache_from_before_cmi_moved_toc(gone_code="QCOM", also_move_iss=False):
    """A cached snapshot that disagrees with CMI's live pages, plus the
    override anchored to it.

    The app has no way to be handed a *different* CMI — the fixtures are what
    the fake CMI serves. So a test that needs "CMI moved a class you had
    customised" arranges it from the other side: the cache remembers TOC's
    first class on Friday 14:00, the student moved that class to Wednesday
    17:00, and the live pages put it back on Tuesday 09:10. From the merge's
    point of view that is exactly an upstream move of a meeting the student
    had customised, which is the situation under test.

    `also_move_iss` plays the same trick on ISS's Tue 09:10, so a sync
    raises TWO conflicts — for tests that answer one row and leave one.

    `gone_code` is renamed in the cache only, so that course looks removed
    upstream on the next sync. Returns (snapshot_json, overrides, gone_code).
    """
    snap = json.loads(SEED_SNAPSHOT_JSON)
    moved_from = None
    iss_from = None
    for course in snap["courses"]:
        if course["code"] == "TOC":
            for m in course["meetings"]:
                if m["day"] == "Tue" and m["slot"]["start_min"] == 550:
                    m["day"] = "Fri"
                    m["slot"] = {"start_min": 840, "end_min": 915}
                    moved_from = json.loads(json.dumps(m))
        if also_move_iss and course["code"] == "ISS":
            for m in course["meetings"]:
                if m["day"] == "Tue" and m["slot"]["start_min"] == 550:
                    m["day"] = "Fri"
                    m["slot"] = {"start_min": 840, "end_min": 915}
                    iss_from = json.loads(json.dumps(m))
    assert moved_from is not None, "the fixture must still have TOC on Tue 09:10"
    assert not also_move_iss or iss_from is not None, \
        "the fixture must still have ISS on Tue 09:10"

    renamed = f"{gone_code}X"
    assert not any(c["code"] == renamed for c in snap["courses"]), \
        f"{renamed} must not already exist upstream, or nothing looks removed"
    for course in snap["courses"]:
        if course["code"] == gone_code:
            course["code"] = renamed

    items = [{
        "id": 0, "course": "TOC",
        "base": {"day": "Fri",
                 "slot": {"start_min": 840, "end_min": 915},
                 "hall": moved_from.get("hall"), "temp_booking": False},
        "to": {"day": "Wed", "slot": {"start_min": 1020, "end_min": 1095},
               "hall": moved_from.get("hall"), "temp_booking": False},
        "created_at": 1754000000000.0}]
    if also_move_iss:
        items.append({
            "id": 1, "course": "ISS",
            "base": {"day": "Fri",
                     "slot": {"start_min": 840, "end_min": 915},
                     "hall": iss_from.get("hall"), "temp_booking": False},
            "to": {"day": "Thu", "slot": {"start_min": 1020, "end_min": 1095},
                   "hall": iss_from.get("hall"), "temp_booking": False},
            "created_at": 1754000000000.0})
    overrides = {
        "next_id": len(items),
        "items": items,
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

_cmi = {"up": False, "proxy": False, "bodies": {}}


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

    def _relayed_path(self):
        """The CMI path a relay was asked to fetch, or None if this request
        isn't a relayed one. Both public relays take the target as a `url`
        query parameter (`/raw?url=…` for allorigins, `/?url=…` for
        corsproxy), and the app hangs a cache-buster on that target — so the
        path is what identifies the page, never the whole string."""
        query = self.path.split("?", 1)[1] if "?" in self.path else ""
        target = urllib.parse.parse_qs(query).get("url", [None])[0]
        if target is None:
            return None
        return urllib.parse.urlsplit(target).path

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        # A relayed request answers with the same page the direct one would,
        # but only while the relays are switched on: the default is every
        # public route dead, which is what most tests want.
        relayed = self._relayed_path()
        if relayed is not None:
            path = relayed if _cmi["proxy"] else "/nowhere"
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


def serve_relays():
    """Make the public CORS relays answer too, for the tests that care which
    route the app takes. Off by default: with the relays dead, a sync falls
    through to the direct route, which is what most tests exercise."""
    _cmi["proxy"] = True


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
    """Back to an unreachable CMI — the state every test starts from, relays
    included."""
    _cmi["up"] = False
    _cmi["proxy"] = False
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


def serve_dist(port=PORT):
    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", port),
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
    # network, so a sync fails instantly and deterministically. The
    # exceptions are CMI itself and the two public relays the app tries
    # first, all pointed at our own TLS stand-in — which answers 503 unless a
    # test called serve_cmi() / serve_relays(), so the default is still
    # "nothing out there answers". The relays get a mapping rather than a
    # DNS failure on purpose: they are the first route now, and a test
    # environment where they fail differently from CMI would be testing the
    # resolver.
    opts.add_argument(
        f"--host-resolver-rules=MAP www.cmi.ac.in 127.0.0.1:{CMI_PORT}, "
        f"MAP api.allorigins.win 127.0.0.1:{CMI_PORT}, "
        f"MAP corsproxy.io 127.0.0.1:{CMI_PORT}, "
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
            # Service workers cache whole builds; no test may ever be served
            # yesterday's dist or start controlled. Every fresh boot begins
            # with no registrations and no caches on this origin.
            self.d.execute_async_script("""
                const done = arguments[arguments.length - 1];
                (async () => {
                    if ('serviceWorker' in navigator) {
                        const regs = await navigator.serviceWorker.getRegistrations();
                        await Promise.all(regs.map((r) => r.unregister()));
                    }
                    if (window.caches) {
                        const names = await caches.keys();
                        await Promise.all(names.map((n) => caches.delete(n)));
                    }
                })().then(() => done(null), (e) => done(String(e)));
            """)
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

    def dismiss_toasts(self):
        """Clear the toast stack. Toasts sit above the page, so a tall one
        can cover a dialog's sticky footer and swallow a click meant for a
        button. Only the ✕ is pressed — never the Undo beside it."""
        # Dismissing one re-renders the whole stack, so every handle taken
        # before the click goes stale: re-find the first one each time.
        for _ in range(12):
            found = self.d.find_elements(
                By.CSS_SELECTOR, ".toast button[aria-label='Dismiss']"
            )
            if not found:
                return
            try:
                found[0].click()
            except Exception:
                time.sleep(0.1)
        raise AssertionError("toasts did not clear")

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
                  "Storage inspector", "Raw HTML viewer", "Simulators"):
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
    """Unknown codes warn without breaking the known selection: a headline
    that says what happened, the codes set as codes, and the explanation
    under them — not one paragraph with the codes buried inside it."""
    app.boot("/?c=TOC,XYZQ")
    banner = app.wait_css(".banner")
    title = banner.find_element(By.CSS_SELECTOR, ".banner-title").text
    assert title == "One course in that link isn't in CMI's timetable, so it was left out", title
    # The code is set as a code of its own, not as a word in the sentence.
    codes = [c.text for c in banner.find_elements(By.CSS_SELECTOR, ".unknown-code")]
    assert codes == ["XYZQ"], codes
    note = banner.find_element(By.CSS_SELECTOR, ".banner-note").text
    assert "earlier semester" in note and "opened as usual" in note, note
    app.chip("TOC")  # TOC still selected and rendered
    banner.find_element(By.XPATH, ".//button[normalize-space()='Dismiss']").click()
    app.wait_gone(".banner")

    # Two of them, and the headline counts rather than pluralising blindly.
    app.boot("/?c=TOC,XYZQ,NOPE1")
    banner = app.wait_css(".banner")
    title = banner.find_element(By.CSS_SELECTOR, ".banner-title").text
    assert title == "2 courses in that link aren't in CMI's timetable, so they were left out", title
    codes = [c.text for c in banner.find_elements(By.CSS_SELECTOR, ".unknown-code")]
    assert codes == ["XYZQ", "NOPE1"], codes


def t05_credits_default_four(app):
    """Unstated credits count as 4; stated ones stay verbatim."""
    app.boot("/?c=TOC,RDBM")
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    total = app.css("section[aria-label='My courses'] .credit-summary .cs-num").text
    assert total == "6", total  # 4 (assumed) + 2
    assert "credits in total" in section.text, section.text
    assert "CMI doesn't list credits for one of your courses" in section.text, section.text
    assert "counts it as 4, the usual figure" in section.text, section.text
    assert "that part of the total above is a guess" in section.text, section.text
    assert "set it with Edit this course" in section.text, section.text
    # Details dialog explains the assumption in a full sentence.
    app.chip("TOC").click()
    dialog = app.wait_css(".dialog")
    assert "CMI doesn't list credits for this course, so the app counts the usual 4" \
        in dialog.text, dialog.text
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
    # The ⓘ answers the ⚠ it sent you from: for a course you haven't picked,
    # the details name WHICH of your courses it would run into, and when —
    # otherwise the warning is a dead end and you compare times by hand.
    nlp.find_element(
        By.XPATH, "./following-sibling::button[1]").click()
    dialog = app.wait_css(".dialog")
    assert "Would clash with 1 of your course" in dialog.text, dialog.text
    clashes = dialog.find_element(By.CSS_SELECTOR, ".clash-list")
    assert "TOC" in clashes.text, clashes.text
    assert "Thursday" in clashes.text, clashes.text
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()
    app.wait_gone(".dialog")
    # …and a course that fits shows no such section at all.
    mfd.find_element(By.XPATH, "./following-sibling::button[1]").click()
    dialog = app.wait_css(".dialog")
    assert "clash" not in dialog.text.lower(), dialog.text
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()


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
        By.XPATH, ".//li[contains(.,'TOC')]//button[normalize-space()=\"Back to CMI's time\"]"
    ).click()
    WebDriverWait(app.d, 10).until(
        lambda d: "Nothing yet. When you add or delete a course" in app.css(".dialog").text
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
    """A course CMI hasn't scheduled opens the same editor as any other, and
    a meeting added there places it."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    app.chip("SVA").click()  # unscheduled course → details dialog
    dialog = app.wait_css(".dialog")
    assert "hasn't put it on the timetable" in dialog.text
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    form = app.wait_css(".dialog .course-form")
    # It opens on what the course has: no times, and no row half-filled on
    # its behalf.
    assert not app.css_all(".course-form .meeting-draft"), \
        "the editor must not add a meeting the user did not ask for"
    assert "No meetings yet" in form.text
    form.find_element(
        By.XPATH, ".//button[contains(normalize-space(),'Add a weekly meeting')]").click()
    app.wait_css(".dialog .course-form #ce-day-0")
    # SVA isn't on the timetable, so the footer asks — a ticked box, visible
    # BEFORE the save, instead of a "Save changes" that quietly adds (R48,
    # §8.10). Ticked is the default, so saving still adds it.
    add_box = app.xpath(
        "//div[@class='dialog']//div[contains(@class,'actions')]"
        "//label[contains(.,'Also add SVA to my timetable')]//input")
    assert add_box.is_selected(), "the add box must start ticked"
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
    assert "set by you" in dialog.text, dialog.text
    assert "without your number the app would count 4" in dialog.text, dialog.text
    app.xpath("//div[@class='dialog']//button[normalize-space()='Close']").click()
    app.wait_gone(".dialog")
    section = app.css("section[aria-label='My courses']")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "5", section.text
    assert "You set the credits on one course yourself" in section.text, section.text
    # The 'Your changes' panel shows official → yours; removing it restores.
    app.open_tab("My timetable")
    panel = app.wait_css("[data-testid='your-changes']")
    # Case-insensitive: the group heading is small caps via CSS, and
    # .text returns painted text. The wording is the assertion, not the
    # styling.
    assert "credits you set" in panel.text.lower(), panel.text
    assert "4 (the app's guess) → 3" in panel.text, panel.text
    app.xpath("//button[contains(.,'1 change')]")  # toolbar pill
    panel.find_element(
        By.XPATH,
        ".//li[contains(.,'TOC')]//button[normalize-space()=\"Back to CMI's credits\"]"
    ).click()
    app.wait_toast("Removed your credit change to TOC")
    app.wait_gone("[data-testid='your-changes']")
    app.open_tab("My courses")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "6"


def t18_overwrites_panel_and_remove_all(app):
    """Meeting moves and credit changes appear together with provenance;
    'Undo my changes to CMI's courses' restores CMI's data in one step."""
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
    assert "4 (the app's guess) → 2" in panel.text, panel.text
    app.xpath("//button[contains(.,'2 changes')]")
    panel.find_element(
        By.XPATH, ".//button[normalize-space()=\"Undo my changes to CMI's courses\"]"
    ).click()
    app.wait_toast("Your changes to CMI's courses are removed")
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
    app.xpath(f"{card}//button[normalize-space()='Edit this course']").click()
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
    assert "You added this meeting. It isn't on CMI's timetable." in section.text
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
        By.XPATH, ".//div[@role='radiogroup'][@aria-label='Day']//button[normalize-space()='Tue']"
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
        By.XPATH, ".//div[@role='radiogroup'][@aria-label='Day']//button[normalize-space()='Tue']"
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
        By.XPATH, ".//div[@role='radiogroup'][@aria-label='Day']//button[normalize-space()='Tue']"
    ).click()
    section.find_element(By.XPATH, ".//button[contains(.,'Edit layout')]").click()
    app.drag(app.chip("TOC", dst_cell), app.css(src_cell))
    app.wait_toast("Moved TOC back to CMI's time")
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
    pill's tooltip names the route (the pill text itself stays quiet about
    live routes — "proxy"/"direct" is plumbing, not news). The relays are
    dead here, so the route is the direct one — the fallback doing its job
    (see t72/t73 for the order)."""
    serve_cmi()
    try:
        app.boot("/", seed=False)
        app.wait_css(".tabs .tab", timeout=30)
        app.wait_gone(".welcome-card")
        title = app.css(".sync-pill").get_attribute("title")
        assert "directly from cmi.ac.in" in title, title
        # The success toast names the route it came through, so "where did
        # this timetable come from?" is answerable without opening My data.
        assert "Timetable updated (directly from cmi.ac.in)." in app.toasts_text(), \
            app.toasts_text()
        assert "direct" not in app.css(".sync-pill").text, \
            "a live route word must not clutter the pill itself"
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
    assert matches.startswith(f"{visible} course"), matches
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
        "input[aria-label='Share link with courses and your changes']"
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
        # NOTHING is answered for you: no radio pre-checked, and Apply
        # (which would have nothing to do) is disabled until you answer.
        assert not [r for r in dialog.find_elements(
            By.CSS_SELECTOR, ".conflict-item input[type='radio']")
            if r.is_selected()], "no conflict row may come pre-answered"
        apply_btn = dialog.find_element(
            By.XPATH, ".//button[normalize-space()='Apply']")
        assert apply_btn.get_attribute("disabled") is not None, \
            "Apply must be disabled while nothing is answered"
        # Answer every row: keep the user's time.
        dialog.find_element(
            By.XPATH, ".//button[normalize-space()='Keep mine for all']"
        ).click()
        dialog.find_element(By.XPATH, ".//button[normalize-space()='Apply']").click()
        app.wait_toast("Your timetable now uses the times you picked.")
        app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
        app.wait_toast(f"CMI dropped {gone} from its timetable")
        # The banner leads with the reader's own week: their course moved
        # and another of theirs was dropped, so it names both before any
        # campus-wide count.
        banner = app.xpath(
            "//div[contains(@class,'banner')][contains(.,'See what changed')]")
        assert "of your courses" in banner.text, banner.text
        assert gone in banner.text and "TOC" in banner.text, banner.text
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
    """Export to calendar downloads a calendar whose events reflect the moved
    meeting, not the overridden official one."""
    for f in os.listdir(DOWNLOADS):
        os.remove(os.path.join(DOWNLOADS, f))
    app.boot("/", selection=["TOC"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='Export to calendar']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Download calendar file']").click()
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
    app.xpath("//div[@class='dialog']//button[normalize-space()='Put it back']").click()
    app.wait_toast("TOC's meeting is back")
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
        app.xpath(
            "//div[@class='dialog']//button[normalize-space()=\"Back to CMI's time\"]"
        ).click()
        app.wait_toast("Moved TOC back to CMI's time")
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
    app.wait_toast("Your timetable is empty now")
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
    assert "CMI doesn't list credits for 2 of your courses" in section.text, section.text
    assert "so the app fills the numbers in" in section.text, section.text
    # One sentence per reason that actually fired.
    assert "one credit per month" in section.text, section.text
    assert "Anything else counts as 4" in section.text, section.text

    # The MATH card's credits badge says 2 and explains why.
    badge = app.xpath(
        "//section[@aria-label='My courses']//div[contains(@class,'card')]"
        "[.//button[starts-with(@aria-label,'MATH,')]]"
        "//span[contains(@class,'badge')][contains(normalize-space(),'cr')]"
    )
    # The * is the same mark the printed sheet uses for the app's guesses.
    assert badge.text.strip() == "2 cr*", badge.text
    # The explanation is VISIBLE on the card (a tooltip is invisible on a
    # phone and unreachable by keyboard — R48, §8.13).
    note = app.xpath(
        "//section[@aria-label='My courses']//div[contains(@class,'card')]"
        "[.//button[starts-with(@aria-label,'MATH,')]]"
        "//p[contains(@class,'cr-note')]"
    )
    assert "It runs Oct-Nov, so the app counts one credit per month" \
        in note.text, note.text

    # The details dialog spells the same assumption out.
    chip = app.chip("MATH", "section[aria-label='My courses']")
    app.d.execute_script("arguments[0].scrollIntoView({block:'center'});", chip)
    chip.click()
    dialog = app.wait_css(".dialog")
    assert "It runs Oct-Nov, so the app counts one credit per month" in dialog.text, \
        dialog.text[:400]
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

    # A code that would break share links is refused with the reason: a
    # comma is the links' separator between codes, % starts an escape.
    code_box = app.css("#ce-code")
    code_box.send_keys(Keys.CONTROL, "a")
    code_box.send_keys("A,B")
    app.xpath("//button[normalize-space()='Add to my timetable']").click()
    err = app.css(".course-form .form-error")
    assert "can't contain a comma or a % sign" in err.text, err.text
    code_box.send_keys(Keys.CONTROL, "a")
    code_box.send_keys("GERMAN")

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
    assert badge.text == "Added by you", badge.text
    # The badge is a button now: its explanation lives in the details
    # dialog, where touch and keyboard can actually reach it (R48, §8.13).
    badge.click()
    dlg = app.wait_css(".dialog")
    assert "You made this course. It isn't on CMI's pages." in dlg.text, dlg.text
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    app.wait_gone(".dialog")
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
    # card offers "Edit this course" now, so it has to be GYM's own.)
    app.xpath(f"{GYM_CARD}//button[normalize-space()='Edit this course']").click()
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
    app.xpath(f"{GYM_CARD}//button[normalize-space()='Edit this course']").click()
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
    app.xpath(f"{GYM_CARD}//button[normalize-space()='Edit this course']").click()
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
        "input[aria-label='Share link with courses and your changes']"
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
    assert "CMI now lists this code too" in section.text, section.text
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
        By.XPATH, ".//button[normalize-space()=\"Delete my version and use CMI's\"]"
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
        "//section[@aria-label='Lecture halls']//div[@role='radiogroup' and @aria-label='Day']"
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
    # The tinted column explains itself in visible words under the grid —
    # not in a tooltip a phone never shows (R48, §8.13).
    assert "outside CMI's regular grid" in app.css(grid).text, \
        "the extra column needs its visible explanation"
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
    # Each button names what pressing it leaves behind: a struck-out
    # meeting comes back, a moved one goes back to CMI's time.
    assert groups[2].find_element(By.CSS_SELECTOR, "li .btn").text == "Put it back"
    assert groups[0].find_element(
        By.CSS_SELECTOR, "li .btn").text == "Back to CMI's time"
    # A room move says room, not time.
    assert groups[1].find_element(
        By.CSS_SELECTOR, "li .btn").text == "Back to CMI's room"


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

    # Back in the catalog — AND back on the timetable: the deletion took the
    # selection with it (TOC was selected when deleted), so Restore gives
    # back everything the deletion took.
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    WebDriverWait(app.d, 5).until(
        lambda d: bool(app.chips("TOC", "section[aria-label='Catalog']")),
        message="Restore must bring the course back to the catalog",
    )
    assert not app.css_all(".deleted-note")
    assert sorted(app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));"
    )) == ["ISS", "TOC"], \
        "restore must return the course to the timetable it was deleted from"
    app.open_tab("My timetable")
    app.wait_css("section[aria-label='My timetable'] table.tt")
    WebDriverWait(app.d, 5).until(
        lambda d: bool(app.chips("TOC", "section[aria-label='My timetable'] table.tt")),
        message="the restored course must be back on the timetable grid",
    )


def t54_editor_saves_everything_in_one_step(app):
    """The whole point of one editor: a move, an addition, a removal and a
    credit change are made together and saved as ONE change — so a single
    Undo puts all four back."""
    app.boot("/?c=TOC")  # Tue + Thu 09:10 in the fixture
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    app.xpath("//button[normalize-space()='Edit this course']").click()
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
    # The entry remembers the deletion took the selection too, so a later
    # Restore can give both back (R48, §8.11).
    assert app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.overrides'))"
        ".hidden[0].was_selected;"
    ) is True, "a deletion of a selected course must record was_selected"

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
    app.xpath("//button[normalize-space()='Edit this course']").click()
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
        app.xpath("//button[normalize-space()='Edit this course']").click()
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
    app.xpath("//button[normalize-space()='Edit this course']").click()
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
    """Scroll over credits, a meeting time or an export date and it moves
    one step — hovering is enough, no click first (R46: focus-gating read
    as "scrolling is broken"). The box swallows the scroll while the wheel
    is over it, so the dialog behind it stays put."""
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
    app.xpath("//button[normalize-space()='Edit this course']").click()
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
    app.xpath("//button[starts-with(normalize-space(),\"Use the app's\") or starts-with(normalize-space(),\"Use CMI's\")]")

    # The box's own min/max do the clamping, not us.
    for _ in range(25):
        wheel(box, -50)
    assert box.get_attribute("value") == "20", box.get_attribute("value")
    for _ in range(25):
        wheel(box, 50)
    assert box.get_attribute("value") == "0", box.get_attribute("value")

    # Hover is enough — deliberately unfocused, the wheel still steps.
    dialog = app.css(".dialog")
    app.d.execute_script("document.activeElement.blur();")
    wheel(box, -50)
    assert box.get_attribute("value") == "1", \
        "hovering must be enough — no click-first (value: %s)" \
        % box.get_attribute("value")

    # And while the wheel is over the box, the dialog behind it stays put —
    # the scroll belongs to the box.
    app.d.execute_script("arguments[0].scrollTop = 0;", dialog)
    wheel(box, 200)
    assert app.d.execute_script("return arguments[0].scrollTop;", dialog) == 0, \
        "a hovered box must swallow the scroll, not scroll the dialog too"

    # A meeting time steps by a minute — again without a click.
    app.d.execute_script("arguments[0].scrollTop = 0; document.activeElement.blur();", dialog)
    start_time = app.css("input[type='time'][aria-label='Start time']")
    before = start_time.get_attribute("value")
    wheel(start_time, -50)
    assert start_time.get_attribute("value") != before, \
        f"the start time must step: {before}"

    # An EMPTY box is left alone: the browser's own stepUp would fill it
    # with a time nobody chose (00:00 / today), so a wheel passing over a
    # blank box must not write into it.
    app.d.execute_script(
        "arguments[0].value = '';"
        "arguments[0].dispatchEvent(new Event('input', {bubbles: true}));",
        start_time)
    wheel(start_time, -50)
    assert start_time.get_attribute("value") == "", \
        "the wheel must not invent a value for an empty box"

    # A dropdown is a box with a step too — its steps are named, not
    # numbered — and the Day control sits right beside the time boxes.
    day = app.css_all(".course-form .meeting-draft select[aria-label='Day']")[0]
    picked = Select(day).first_selected_option.text
    wheel(day, 50)
    stepped = Select(day).first_selected_option.text
    assert stepped != picked, f"the Day dropdown must step on hover: {picked}"
    wheel(day, -50)
    assert Select(day).first_selected_option.text == picked, \
        "and step back the other way"

    # The reminder lead decouples its two steppers: the arrows jump by
    # fives (step=5 from min=5, so 5-10-15, never 1-6-11), the wheel
    # nudges by single minutes (data-wheel-step=1).
    app.boot("/", selection=["TOC"])
    app.xpath("//button[normalize-space()='Export to calendar']").click()
    app.wait_css(".dialog")
    app.xpath("//label[contains(.,'reminder')]//input").click()
    lead = app.wait_css("label.alarm-lead input")
    assert lead.get_attribute("step") == "5", lead.get_attribute("step")
    assert lead.get_attribute("min") == "5", lead.get_attribute("min")
    assert lead.get_attribute("value") == "10"
    wheel(lead, -50)
    assert lead.get_attribute("value") == "11", lead.get_attribute("value")
    wheel(lead, 50)
    wheel(lead, 50)
    assert lead.get_attribute("value") == "9", lead.get_attribute("value")

    # The clamp never overrules the hand on the wheel: a typed 2 is legal
    # (export clamps only at download), and scrolling DOWN over it must not
    # "clamp" the value UP to the min of 5. Scrolling UP from 2 may — the
    # wheel and the clamp then agree on the direction.
    app.d.execute_script(
        "arguments[0].value = '2';"
        "arguments[0].dispatchEvent(new Event('input', {bubbles: true}));",
        lead)
    wheel(lead, 50)
    assert lead.get_attribute("value") == "2", \
        f"wheel-down must never raise the value: {lead.get_attribute('value')}"
    wheel(lead, -50)
    assert lead.get_attribute("value") == "5", lead.get_attribute("value")

    # A trackpad doesn't step per event: deltas under a notch (~50px)
    # gather on the box and step once per accumulated notch, so one flick
    # is a step or two, not ten.
    for _ in range(3):
        app.d.execute_script(
            "arguments[0].dispatchEvent(new WheelEvent('wheel',"
            " {deltaY: -20, deltaMode: 0, bubbles: true, cancelable: true}));",
            lead)
    assert lead.get_attribute("value") == "6", \
        f"three 20px deltas are ONE notch, one step: {lead.get_attribute('value')}"

    # And an export date by a day.
    app.xpath("//div[@class='dialog']//button[normalize-space()='Cancel']").click()
    app.wait_gone(".dialog")
    app.open_tab("My timetable")
    app.xpath("//button[contains(.,'Export to calendar')]").click()
    frm = app.wait_css("#ex-from")
    frm.click()
    before = frm.get_attribute("value")
    wheel(frm, -50)
    assert frm.get_attribute("value") != before, f"the From date must step: {before}"


def t63_editing_a_course_with_no_time_never_invents_one(app):
    """A course CMI hasn't scheduled has the same door as every other course
    — "Edit this course" — and going through it to change the credits does
    not quietly give the course a time.

    It used to: the only button read "Give it a time" and opened the form
    with a row already filled in with Monday and the first slot, so Save
    scheduled a class nobody had asked to schedule."""
    app.boot("/", selection=["SVA"])  # CMI lists SVA but gives it no time
    app.open_tab("My timetable")
    tray = app.wait_css(".tray")
    assert "No fixed slot yet" in tray.text, tray.text
    tray.find_element(
        By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    form = app.wait_css(".dialog .course-form")
    assert not app.css_all(".course-form .meeting-draft"), \
        "the editor must open on what the course has: no times, no row"

    # Change only the credits, the thing that was unreachable before.
    form.find_element(
        By.XPATH, ".//div[@class='seg']//button[normalize-space()='2']").click()
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_gone(".dialog")

    # The credits took, and the course is still waiting for a time — not
    # sitting on a Monday morning it never asked for.
    tray = app.wait_css(".tray")
    assert app.chips("SVA", ".tray"), "SVA must still be waiting for a time"
    assert not app.chips("SVA", "table.tt"), \
        "saving credits must not put the course on the grid"
    # And the credits really took (the summary lives on My courses).
    app.open_tab("My courses")
    pills = [p.text for p in app.css_all(".credit-summary .cs-pill")]
    assert any("2 credits" in p for p in pills), pills


def t64_a_half_written_form_is_not_thrown_away_by_a_stray_key(app):
    """Escape and a click on the dark area are the two accidental ways out of
    a dialog. The course editor commits nothing until Save, so a slip there
    is the one loss in this app that Undo cannot reach — it asks first, and
    only when there is something to lose."""
    app.boot("/", selection=["TOC"])
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")

    # Nothing typed yet: Escape closes it, as it always did.
    app.xpath("//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    app.wait_gone(".dialog")

    # Now change something, and say no to the question.
    app.xpath("//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    app.xpath("//div[@class='seg']//button[normalize-space()='2']").click()
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    alert = WebDriverWait(app.d, 5).until(EC.alert_is_present())
    alert.dismiss()
    app.css(".dialog .course-form")  # still open, still holding the edit

    # A click on the dark area asks the same question.
    app.d.execute_script(
        "document.querySelector('.overlay').click();")
    WebDriverWait(app.d, 5).until(EC.alert_is_present()).accept()
    app.wait_gone(".dialog")


def t65_my_courses_has_the_same_filters(app):
    """My courses filters the courses you picked, with the same bar the
    catalog and the master grid use — and says so when the credit total
    above it counts more than the list below it."""
    app.boot("/", selection=["TOC", "RDBM", "SVA"])
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert len(app.css_all("section[aria-label='My courses'] .card")) == 3

    # The bar is the shared one, so the search box narrows the cards.
    box = section.find_element(By.CSS_SELECTOR, ".filterbar input[type='search']")
    box.send_keys("TOC")
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all("section[aria-label='My courses'] .card")) == 1)
    assert "1 course matches" in section.text, section.text
    # The credit total counts everything, so the difference is stated.
    assert "hiding 2 of your courses" in section.text, section.text

    # Filtering to nothing says the courses are still on the timetable, and
    # offers the fix that actually applies.
    box.send_keys("ZZZZ")
    WebDriverWait(app.d, 10).until(
        lambda d: not app.css_all("section[aria-label='My courses'] .card"))
    assert "None of your courses match these filters" in section.text, section.text
    section.find_element(
        By.XPATH, ".//button[normalize-space()='Clear the filters']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all("section[aria-label='My courses'] .card")) == 3)

    # "Fits my schedule" is NOT offered here: it hides whatever overlaps your
    # selection, and everything on this page IS your selection, so the box
    # could never hide a card. It stays where it can act.
    section = app.css("section[aria-label='My courses']")
    assert "Fits my schedule" not in section.text, section.text
    app.open_tab("Catalog")
    cat = app.wait_css("section[aria-label='Catalog']")
    assert "Fits my schedule" in cat.text, "the catalog keeps it"
    app.open_tab("Master grid")
    grid = app.wait_css("section[aria-label='Master grid']")
    assert "Fits my schedule" in grid.text, "the master grid keeps it"
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses'] .filterbar")

    # And no facet offers a value that could only ever match nothing: the
    # menus here list what YOUR courses have, not the whole catalog's.
    def opts(name):
        app.xpath("//section[@aria-label='My courses']//details[contains(@class,'facet')]"
                  f"/summary[starts-with(normalize-space(),'{name}')]").click()
        app.wait_css("details.facet[open] .menu")
        out = [o.text for o in app.css_all("details.facet[open] .menu label.opt")]
        app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
        time.sleep(0.2)
        return out

    course_opts = opts("Course")
    assert len(course_opts) == 3, course_opts
    assert all(any(c in o for c in ("TOC", "RDBM", "SVA")) for o in course_opts), course_opts
    instructors = opts("Instructor")
    assert 0 < len(instructors) <= 6, instructors

    # SEPARATE state (R43): filtering your own courses must not quietly
    # narrow the catalog you look at next — and vice versa.
    box = app.css("section[aria-label='My courses'] .filterbar input[type='search']")
    box.send_keys("RDBM")
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all("section[aria-label='My courses'] .card")) == 1)
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    assert app.css("section[aria-label='Catalog'] .filterbar input[type='search']"
                   ).get_attribute("value") == "", \
        "a My-courses filter must not leak into the catalog"


def t66_controls_that_cannot_act_are_not_offered(app):
    """A control shown where it cannot do anything is worse than no control:
    it invites a click and answers with nothing. This pins the sweep."""
    # Print is disabled on an empty timetable, like the Export beside it.
    app.boot("/")
    app.open_tab("My timetable")
    app.wait_css("section[aria-label='My timetable']")
    printer = app.xpath("//button[normalize-space()='Print']")
    assert printer.get_attribute("disabled") is not None, \
        "Print must be disabled with nothing to print, like Export to calendar"

    # A course CMI hasn't scheduled has nothing to export, and the dialog
    # already says so two lines above where the button used to be.
    app.boot("/", selection=["SVA"])
    app.open_tab("My timetable")
    app.chip("SVA", ".tray").click()
    dialog = app.wait_css(".dialog")
    assert "hasn't put it on the timetable" in dialog.text
    assert not dialog.find_elements(By.XPATH, ".//button[normalize-space()='Export to calendar']"), \
        "a course with no times must not offer a calendar export"
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    app.wait_gone(".dialog")

    # "Has custom time" describes a course of your own — it is custom, times
    # and all — and used to hide exactly those, because they carry no
    # override for the flag to find.
    app.boot("/", selection=["TOC", "GERMAN"], customs=HALL_CUSTOM)
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses'] .filterbar")
    assert len(app.css_all("section[aria-label='My courses'] .card")) == 2
    app.xpath("//section[@aria-label='My courses']//details[contains(@class,'facet')]"
              "/summary[starts-with(normalize-space(),'Status')]").click()
    app.wait_css("details.facet[open] .menu")
    app.xpath("//details[contains(@class,'facet') and @open]"
              "//label[contains(.,'Has custom time')]/input").click()
    time.sleep(0.5)
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    cards = [c.text for c in app.css_all("section[aria-label='My courses'] .card")]
    assert any("GERMAN" in c for c in cards), \
        f"your own course IS a custom time — the flag must match it: {cards}"

    # A value ticked where it was in scope stays visible where it is not —
    # WITHIN the pair of bars that share state (Catalog + Master grid; since
    # R43 My courses has its own set, so nothing leaks there at all).
    # M K Srivas teaches only SVA, which CMI hasn't timetabled: tickable on
    # the Catalog (it lists rows), out of scope on the Master grid (it draws
    # cells). Without with_picked the grid's menu would show no row while
    # its badge counted one, and "None" could not clear it.
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .filterbar")
    app.xpath("//details[contains(@class,'facet')]/summary"
              "[starts-with(normalize-space(),'Instructor')]").click()
    app.wait_css("details.facet[open] .menu")
    picked = "M K Srivas"
    app.xpath("//details[contains(@class,'facet') and @open]"
              f"//label[contains(normalize-space(),'{picked}')]/input").click()
    time.sleep(0.4)
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] .filterbar")
    app.xpath("//section[@aria-label='Master grid']//details[contains(@class,'facet')]"
              "/summary[starts-with(normalize-space(),'Instructor')]").click()
    app.wait_css("details.facet[open] .menu")
    rows = [r.text.strip() for r in app.css_all("details.facet[open] .menu label.opt")]
    assert picked in rows, f"a ticked value out of scope must still show: {rows}"
    assert app.css_all("details.facet[open] .menu input:checked"), \
        "and it must still read as ticked, so it can be taken off"


def t67_the_master_grid_counts_what_it_can_draw(app):
    """The master grid draws courses through cells, so one CMI lists without
    a time draws nothing at all. Counting it anyway left "1 match" standing
    over an empty grid — and Status → Unscheduled asked for exactly those."""
    app.boot("/")
    app.open_tab("Master grid")
    grid = app.wait_css("section[aria-label='Master grid']")
    box = grid.find_element(By.CSS_SELECTOR, ".filterbar input[type='search']")
    box.send_keys("SVA")  # listed by CMI, never given a time
    WebDriverWait(app.d, 10).until(
        lambda d: "0 courses match" in app.css("section[aria-label='Master grid']").text)
    grid = app.css("section[aria-label='Master grid']")
    assert not app.chips("SVA", "section[aria-label='Master grid']"), \
        "the grid has no cell to draw it in"
    note = grid.find_element(By.CSS_SELECTOR, ".unplaced-note").text
    assert "1 more course matches" in note, note
    assert "hasn't given it a time" in note and "no slot to put" in note, note
    assert "Open the catalog" in note, "the note must offer to go there: " + note

    # The catalog counts it, because a list of rows can show it.
    app.open_tab("Catalog")
    WebDriverWait(app.d, 10).until(
        lambda d: "1 course matches" in app.css("section[aria-label='Catalog']").text)

    # And the flag that selects for exactly those courses is not offered on
    # the grid that can never draw one — it still is where it can act.
    def flags(section):
        facets = app.d.find_elements(
            By.XPATH, f"//section[@aria-label='{section}']"
            "//details[contains(@class,'facet')]"
            "/summary[starts-with(normalize-space(),'Status')]")
        if not facets:
            return []
        facets[0].click()
        app.wait_css("details.facet[open] .menu")
        out = [o.text for o in app.css_all("details.facet[open] .menu label.opt")]
        app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
        time.sleep(0.2)
        return out

    app.css("section[aria-label='Catalog'] .filterbar input[type='search']").clear()
    assert any("Unscheduled" in f for f in flags("Catalog")), "the catalog can show them"
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] .filterbar")
    assert not any("Unscheduled" in f for f in flags("Master grid")), \
        "a filter that can only ever empty the grid must not be offered on it"


def t68_a_keyboard_move_on_a_phone_shows_where_it_is(app):
    """The per-day list takes drops like the desktop cells, so it has to
    take the keyboard the same way: with a cursor you can see, on the day
    you are looking at."""
    app.d.set_window_size(430, 900)
    try:
        app.boot("/", selection=["TOC"])
        app.open_tab("My timetable")
        app.wait_css("section[aria-label='My timetable']")
        app.xpath("//button[contains(.,'Edit layout')]").click()
        app.xpath("//div[@aria-label='Day view']//button[normalize-space()='Tue']").click()
        row = app.wait_css(".day-list .slotrow[data-day='1'][data-slot='550']")
        chip = row.find_element(By.CSS_SELECTOR, "button.chip")
        app.d.execute_script("arguments[0].focus();", chip)
        chip.send_keys("m")
        cursor = app.wait_css(".day-list .slotrow.kbd-cursor")
        assert cursor.get_attribute("data-day") == "1", "it starts under the chip"
        assert cursor.get_attribute("data-slot") == "550"
        assert cursor.is_displayed(), "a cursor nobody can see is no cursor"

        # Arrowing to another day brings that day with it: one day is on
        # screen, and a cursor on a row nobody is looking at is where Enter
        # used to drop a course out of sight.
        body = app.d.find_element(By.TAG_NAME, "body")
        body.send_keys(Keys.ARROW_DOWN)
        WebDriverWait(app.d, 10).until(
            lambda d: d.find_elements(
                By.CSS_SELECTOR, ".day-list .slotrow.kbd-cursor[data-day='2']"))
        assert app.xpath("//div[@aria-label='Day view']"
                         "//button[normalize-space()='Wed']"
                         ).get_attribute("aria-checked") == "true", \
            "the day strip has to say where the cursor went"
        assert app.css(".day-list .slotrow.kbd-cursor").is_displayed()
        body.send_keys(Keys.ENTER)
        app.wait_toast("Moved TOC")
        assert app.chips("TOC", ".day-list .slotrow[data-day='2'][data-slot='550']"), \
            "and it lands where the cursor was"
    finally:
        app.d.set_window_size(1500, 1000)


def snapshot_with_a_room_and_no_class(hall="Lecture Hall 5", day="Mon", start=550,
                                      code="SVA"):
    """CMI books a room for a course no branch grid schedules there — the
    halls page keeps the booking (join.rs warns about it) and draws it as a
    plain reference, since there is no meeting behind it to move."""
    snap = json.loads(SEED_SNAPSHOT_JSON)
    snap["hall_bookings"].append({
        "hall": hall, "day": day,
        "slot": {"start_min": start, "end_min": start + 75},
        "codes": [code], "temp": False,
    })
    return json.dumps(snap)


def t69_halls_marks_your_courses_even_without_a_meeting(app):
    """Halls says "✓ marks the courses on your timetable". A booking with no
    meeting behind it is still a course you may be taking, and it could
    never show the mark."""
    app.boot("/", selection=["SVA"], raw_snapshot=snapshot_with_a_room_and_no_class())
    app.open_tab("Halls")
    app.wait_css("section[aria-label='Lecture halls']")
    app.xpath("//div[@aria-label='Day']//button[normalize-space()='Mon']").click()
    chip = app.wait_css("section[aria-label='Lecture halls'] "
                        "button.chip[aria-label^='SVA,']")
    assert chip.find_elements(By.CSS_SELECTOR, ".sel-mark"), \
        "the ✓ the page promises must appear on a course you are taking"
    assert "in your timetable" in chip.get_attribute("aria-label")

    # Unselect it and the mark goes, so the mark still means what it says.
    app.boot("/", raw_snapshot=snapshot_with_a_room_and_no_class())
    app.open_tab("Halls")
    app.wait_css("section[aria-label='Lecture halls']")
    app.xpath("//div[@aria-label='Day']//button[normalize-space()='Mon']").click()
    chip = app.wait_css("section[aria-label='Lecture halls'] "
                        "button.chip[aria-label^='SVA,']")
    assert not chip.find_elements(By.CSS_SELECTOR, ".sel-mark"), \
        "and only for the courses on your timetable"


def t70_one_course_is_not_a_choice(app):
    """The export dialog asked which courses to put in the file when there
    was only ever one answer."""
    app.boot("/", selection=["TOC"])
    app.open_tab("My timetable")
    app.xpath("//button[normalize-space()='Export to calendar']").click()
    dialog = app.wait_css(".dialog")
    assert not dialog.find_elements(By.CSS_SELECTOR, "#ex-scope"), \
        "'All selected (1)' and that one course are the same file"
    row = dialog.find_element(By.CSS_SELECTOR, ".fieldrow.ro")
    assert "Courses" in row.text and "TOC" in row.text, row.text
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    app.wait_gone(".dialog")

    # Two courses, and the choice is a real one again.
    app.boot("/", selection=["TOC", "ISS"])
    app.open_tab("My timetable")
    app.xpath("//button[normalize-space()='Export to calendar']").click()
    dialog = app.wait_css(".dialog")
    opts = [o.text for o in Select(dialog.find_element(
        By.CSS_SELECTOR, "#ex-scope")).options]
    assert opts == ["All selected (2)", "TOC", "ISS"], opts


def t71_what_changed_never_opens_with_nothing_to_say(app):
    """The dialog exists to describe a difference. Its only way in is the
    banner, and the banner only exists while there is one — which is why it
    carried a paragraph for an empty diff that nobody could ever reach."""
    serve_cmi()
    try:
        # A sync that finds the pages exactly as they were cached: no diff,
        # so no banner, so no way into the dialog.
        app.boot("/", selection=["TOC"])
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        time.sleep(0.5)
        body = app.css("body").text
        assert "CMI updated the timetable" not in body, body
        assert not app.d.find_elements(
            By.XPATH, "//button[normalize-space()='See what changed']")

        # And when there IS a difference, the dialog says what it is. The
        # cached snapshot alone, without the override that goes with it:
        # this is about the diff, and a conflict would put its own dialog
        # in front of the banner.
        snap, _overrides, _gone = cache_from_before_cmi_moved_toc()
        app.boot("/", selection=["TOC"], raw_snapshot=snap)
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        app.xpath("//button[normalize-space()='See what changed']").click()
        dialog = app.wait_css(".dialog")
        assert "Nothing differs" not in dialog.text, dialog.text
        assert "TOC" in dialog.text, dialog.text
    finally:
        stop_serving_cmi()


def fetch_log_tiers(app):
    """The tiers this session's fetches used, oldest first. The developer
    fetch log renders newest-first, so this reverses it back."""
    app.d.get(f"{BASE}/#/developer")
    app.wait_css("section[aria-label='Developer mode']")
    # Scoped to the Fetch log panel: developer mode has more than one
    # `.devlog` table, and the others have nothing in their second column.
    rows = app.d.find_elements(
        By.XPATH,
        "//div[contains(@class,'panel')][h3[normalize-space()='Fetch log']]"
        "//table[contains(@class,'devlog')]/tbody/tr")
    return [r.find_elements(By.TAG_NAME, "td")[1].text for r in reversed(rows)]


def t72_a_relay_is_asked_before_cmi_itself(app):
    """Most people using this app are on CMI's own network, where
    cmi.ac.in is a LOCAL address — so a direct fetch makes the browser ask
    whether this page may reach devices on the local network, which reads
    like an attack. The relays are public hosts and can never raise it, so
    they go first and the direct route is never touched when one answers."""
    serve_cmi()
    serve_relays()
    try:
        app.boot("/", seed=False)
        app.wait_css(".tabs .tab", timeout=30)
        app.wait_gone(".welcome-card")
        title = app.css(".sync-pill").get_attribute("title")
        assert "through the helper site" in title, title
        tiers = fetch_log_tiers(app)
        assert tiers, "the sync must have been logged"
        assert all(t.startswith("proxy:") for t in tiers), \
            f"nothing may reach cmi.ac.in itself while a relay answers: {tiers}"
    finally:
        stop_serving_cmi()


def t73_cmi_itself_is_the_fallback_and_says_so(app):
    """The direct route is kept — it is CMI's own bytes, and the only route
    that works when every relay is down. It runs last, and it explains the
    prompt it may raise before raising it."""
    serve_cmi()  # relays stay dead
    try:
        app.boot("/", seed=False)
        app.wait_css(".tabs .tab", timeout=30)
        title = app.css(".sync-pill").get_attribute("title")
        assert "directly from cmi.ac.in" in title, title
        tiers = fetch_log_tiers(app)
        assert [t for t in tiers if t.startswith("proxy:")], \
            f"the relays must be tried before CMI itself: {tiers}"
        assert tiers[-1] == "direct", f"and CMI itself must be last: {tiers}"
        first_direct = next(i for i, t in enumerate(tiers) if t == "direct")
        assert all(t.startswith("proxy:") for t in tiers[:first_direct]), tiers

        # The prompt is explained by the app that causes it, before it
        # appears — not looked up afterwards by a worried student.
        app.d.get(f"{BASE}/")
        app.wait_css(".tabs .tab", timeout=30)
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Your browser may now ask whether this page can reach devices")
    finally:
        stop_serving_cmi()


def t74_offline_reload_boots_from_cache(app):
    """The offline copy is real: visit once with the network up so the
    worker installs, kill the server, reload — the app must boot entirely
    from the worker's cache and say, in a toast, that you're offline and
    everything still works. A server that is UP but BROKEN (answering 503,
    as GitHub Pages does during an outage) must lose to the cached copy the
    same way — an error page arriving fast is not "the network working".
    Runs on its own port/origin so its worker never touches the origin the
    rest of the suite uses."""
    base = f"http://127.0.0.1:{SW_PORT}"
    server = serve_dist(SW_PORT)
    d = app.d
    try:
        # First visit, network up: the app boots and the worker installs.
        d.get(f"{base}/")
        WebDriverWait(d, 15).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, ".header h1"))
        )
        # ready = a worker finished installing — and install waits on the
        # whole precache (cache.addAll runs inside waitUntil), so ready
        # means every file is cached. controller = it claimed this page.
        assert d.execute_async_script("""
            const done = arguments[arguments.length - 1];
            navigator.serviceWorker.ready.then(() => done(true), () => done(false));
        """), "the service worker must install on a normal online visit"
        WebDriverWait(d, 20).until(
            lambda d: d.execute_script(
                "return !!(navigator.serviceWorker"
                " && navigator.serviceWorker.controller);"
            ),
            message="the worker must claim the page it installed from",
        )

        # GitHub can be up but broken: during a Pages outage the origin
        # answers FAST with a 5xx error page. Fast garbage must not beat
        # the working offline copy — swap the server for one that only
        # says 503 and reload: the worker must serve the cached app.
        server.shutdown()
        server.server_close()

        class Outage(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                body = b"<h1>503 Service Unavailable</h1>"
                self.send_response(503)
                self.send_header("Content-Type", "text/html")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        # Chrome PRECONNECTS: it opens speculative sockets it may never
        # send a request on. server_close() only closes the listener, so
        # such a socket would survive "shutdown" with a live handler thread
        # behind it — and the app's is-my-origin-reachable probe in the
        # offline phase would ride it and get a 503 ("a response") instead
        # of a refused connection ("nothing listens"), hiding the offline
        # note this test asserts. Track every accepted socket so teardown
        # can really sever them.
        class OutageServer(http.server.ThreadingHTTPServer):
            def __init__(self, *a, **kw):
                super().__init__(*a, **kw)
                self.accepted = []

            def get_request(self):
                sock, addr = super().get_request()
                self.accepted.append(sock)
                return sock, addr

        server = OutageServer(("127.0.0.1", SW_PORT), Outage)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        d.get(f"{base}/")
        WebDriverWait(d, 15).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, ".header h1")),
            message="a 503 from the server must lose to the cached app",
        )

        # The network goes away entirely: nothing listens on the port, and
        # every socket the outage server ever accepted is severed too.
        server.shutdown()
        server.server_close()
        for sock in server.accepted:
            try:
                sock.shutdown(socket.SHUT_RDWR)
                sock.close()
            except OSError:
                pass

        # Reload. Only the worker's cache can answer now.
        d.get(f"{base}/")
        WebDriverWait(d, 15).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, ".header h1")),
            message="the app must boot from the worker's cache with no server",
        )
        # The note says so, in the app's own toast rail.
        WebDriverWait(d, 15).until(
            lambda d: "offline" in app.toasts_text().lower(),
            message=f"expected the offline note; toasts: {app.toasts_text()!r}",
        )
        # Not a dead shell: nothing was seeded on this origin, so the app
        # must be alive enough to show its first-run screen.
        assert app.css_all(".welcome-card"), \
            "the app must render its UI, not a blank page"
    finally:
        try:
            server.shutdown()
            server.server_close()
        except Exception:
            pass


def t75_my_courses_filters_are_its_own(app):
    """The Catalog and the Master grid share one filter state (they ask the
    same question); My courses has its own. Neither may overwrite the
    other, and undo restores the right one."""
    app.boot("/", selection=["TOC", "RDBM", "SVA"])
    # Set a catalog filter…
    app.open_tab("Catalog")
    cat = app.wait_css("section[aria-label='Catalog']")
    box = cat.find_element(By.CSS_SELECTOR, ".filterbar input[type='search']")
    box.send_keys("Theory")
    time.sleep(0.4)
    # …it shows on the master grid (shared)…
    app.open_tab("Master grid")
    assert app.css("section[aria-label='Master grid'] .filterbar input[type='search']"
                   ).get_attribute("value") == "Theory", "catalog and grid share state"
    # …but NOT on My courses, whose three cards are untouched.
    app.open_tab("My courses")
    assert app.css("section[aria-label='My courses'] .filterbar input[type='search']"
                   ).get_attribute("value") == ""
    assert len(app.css_all("section[aria-label='My courses'] .card")) == 3, \
        "a catalog filter must not hide the user's own courses"
    # A My-courses filter stays here…
    my_box = app.css("section[aria-label='My courses'] .filterbar input[type='search']")
    my_box.send_keys("TOC")
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all("section[aria-label='My courses'] .card")) == 1)
    app.open_tab("Catalog")
    assert app.css("section[aria-label='Catalog'] .filterbar input[type='search']"
                   ).get_attribute("value") == "Theory", "the catalog keeps its own"
    # …and undoing (one step) takes back the My-courses edit, not the
    # catalog's: history entries carry both sets.
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.CONTROL, "z")
    time.sleep(0.4)
    assert app.css("section[aria-label='Catalog'] .filterbar input[type='search']"
                   ).get_attribute("value") == "Theory"
    app.open_tab("My courses")
    assert app.css("section[aria-label='My courses'] .filterbar input[type='search']"
                   ).get_attribute("value") == ""


def t76_no_false_conflict_and_decide_later_survives_reload(app):
    """Two halves of the same trust story. (1) A share link with moved AND
    added meetings, opened in a browser that has never synced: the first
    sync must raise NO conflict — there is no history to compare, so
    nothing 'changed'. (2) A REAL conflict deferred with 'Decide later'
    must survive a reload: a question the app asked cannot evaporate
    because the page was refreshed."""
    # Closest honest repro of the report: overrides present (as a share
    # link leaves them) with NO snapshot, then the first sync runs against
    # the fake CMI.
    serve_cmi()
    try:
        app.d.get(f"{BASE}/e2e-blank")
        app.d.execute_script("""
            localStorage.clear();
            localStorage.setItem('cmitt.v1.selection', arguments[0]);
            localStorage.setItem('cmitt.v1.overrides', arguments[1]);
        """, json.dumps(["TOC", "RFLR"]), json.dumps({
            "next_id": 2,
            "items": [
                {"id": 0, "course": "TOC",
                 "base": {"day": "Tue", "slot": {"start_min": 550, "end_min": 625},
                          "hall": "Lecture Hall 803", "temp_booking": False},
                 "to": {"day": "Wed", "slot": {"start_min": 1020, "end_min": 1095},
                        "hall": "Lecture Hall 803", "temp_booking": False},
                 "created_at": 1754000000000.0},
                {"id": 1, "course": "RFLR", "base": None,
                 "to": {"day": "Mon", "slot": {"start_min": 710, "end_min": 785},
                        "hall": "NKN AV Hall", "temp_booking": False},
                 "created_at": 1754000000000.0},
            ],
            "credits": [],
        }))
        app.d.get(f"{BASE}/")
        app.wait_css(".tabs .tab", timeout=30)
        time.sleep(1.0)
        assert not app.css_all(".dialog"), \
            "a first sync has no history and must not claim CMI changed anything"
        assert "conflict" not in app.css("body").text.lower(), app.toasts_text()
        # Both changes are alive on the timetable.
        app.open_tab("My timetable")
        app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")

        # (2) Now a REAL conflict: the cache remembers a different TOC than
        # the live pages show. Defer it, reload, and it must still be there.
        snap, overrides, _gone = cache_from_before_cmi_moved_toc()
        app.boot("/", selection=["TOC"], raw_snapshot=snap, overrides=overrides)
        app.xpath("//button[normalize-space()='Sync now']").click()
        dialog = app.wait_css(".dialog")
        assert "conflict" in dialog.text.lower() or "CMI changed" in dialog.text, dialog.text
        dialog.find_element(By.XPATH, ".//button[normalize-space()='Decide later']").click()
        app.wait_gone(".dialog")
        banner = app.wait_css(".banner.warn")
        assert "Review" in banner.text
        app.d.refresh()
        app.wait_css(".tabs .tab", timeout=30)
        banner = app.wait_css(".banner.warn", timeout=10)
        assert "Review" in banner.text, \
            "a deferred question must survive a reload — refreshing is not an answer"
        # And Review still opens a working dialog after the reload.
        banner.find_element(By.XPATH, ".//button[normalize-space()='Review']").click()
        dialog = app.wait_css(".dialog")
        assert "TOC" in dialog.text
    finally:
        stop_serving_cmi()


def t77_what_changed_shows_what_a_dropped_course_was(app):
    """A dropped course is exactly the one the fresh snapshot can't
    describe — so the digest itself must carry what it WAS: name, instructor,
    and when it met. Shown in the dialog, and nowhere else in the app."""
    serve_cmi()
    try:
        snap, _overrides, gone = cache_from_before_cmi_moved_toc()
        app.boot("/", selection=["TOC"], raw_snapshot=snap)
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        app.xpath("//button[normalize-space()='See what changed']").click()
        dialog = app.wait_css(".dialog")
        assert gone in dialog.text, dialog.text
        # The digest lists a dropped course as ONE line — code, name, badge.
        # The record (instructor and times) waits behind the code, or a
        # many-course digest drowns in detail nobody asked to read yet.
        item = next(i for i in dialog.find_elements(By.CSS_SELECTOR, ".diff-item")
                    if gone in i.text)
        assert not item.find_elements(By.CSS_SELECTOR, "ul.meetings li"), \
            "the digest row must not carry meeting rows inline"
        assert not item.find_elements(By.CSS_SELECTOR, "span.muted"), \
            "the digest row must not carry the instructor inline"
        # Clicking the code opens the record as its own popup — instructor in a
        # kv row, meetings as the same aligned when/where rows cards use.
        item.find_element(By.CSS_SELECTOR, "button.chip").click()
        WebDriverWait(app.d, 10).until(
            lambda d: "No longer on CMI's timetable" in app.css(".dialog").text)
        popup = app.css(".dialog")
        assert gone in popup.text, popup.text
        assert "everything the app still knows about it" in popup.text, popup.text
        assert popup.find_element(By.CSS_SELECTOR, ".kv dd").text.strip(), \
            "the popup must name the instructor"
        rows = popup.find_elements(By.CSS_SELECTOR, "ul.meetings li")
        assert rows, "the popup must carry the meeting rows"
        assert "–" in rows[0].find_element(By.CSS_SELECTOR, ".when .t").text, \
            f"a meeting row must show the time span: {rows[0].text!r}"
        assert rows[0].find_element(By.CSS_SELECTOR, ".where .hall").text.strip(), \
            f"a meeting row must say where the class met: {rows[0].text!r}"
        # Back is a return trip, not a dead end: the digest reopens.
        popup.find_element(
            By.XPATH, ".//button[normalize-space()='Back to What changed']").click()
        WebDriverWait(app.d, 10).until(
            lambda d: "What changed since last sync" in app.css(".dialog").text)
        # …and nowhere else: closing the dialog, the code appears in no grid
        # or list (the fresh snapshot never heard of it).
        app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
        app.wait_gone(".dialog")
        app.open_tab("Catalog")
        cat = app.wait_css("section[aria-label='Catalog']")
        assert gone not in cat.text, "a dropped course must not haunt the catalog"
    finally:
        stop_serving_cmi()


def t87_a_dropped_course_can_be_kept_as_your_own(app):
    """The record in "What changed" is the last copy of a dropped course in
    existence, and it dies with the message. Keeping it writes that record
    into the user's own courses — permanently, in one undoable step, with
    CMI's own credits rather than an invented number."""
    serve_cmi()
    try:
        snap, _overrides, gone = cache_from_before_cmi_moved_toc()
        # The dropped course is ON the timetable: the ghost case, where it
        # renders from a stub with no name and no times of its own.
        app.boot("/", selection=["TOC", gone], raw_snapshot=snap)
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        app.xpath("//button[normalize-space()='See what changed']").click()
        dialog = app.wait_css(".dialog")
        item = next(i for i in dialog.find_elements(By.CSS_SELECTOR, ".diff-item")
                    if gone in i.text)
        item.find_element(By.CSS_SELECTOR, "button.chip").click()
        popup = app.wait_css(".dialog")
        WebDriverWait(app.d, 10).until(
            lambda d: "No longer on CMI's timetable" in app.css(".dialog").text)
        app.dismiss_toasts()   # a sync's toasts cover the dialog's footer
        popup.find_element(
            By.XPATH, ".//button[normalize-space()='Keep this as my own course']").click()
        app.wait_toast(f"{gone} is your own course now")
        # It goes back to the digest — not a dead end — and the row it came
        # from is still there, now describing a course that is yours.
        WebDriverWait(app.d, 10).until(
            lambda d: "What changed since last sync" in app.css(".dialog").text)
        app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
        app.wait_gone(".dialog")
        # The ghost is a real course now: badge flipped, name and instructor
        # back, and the times CMI last published on the week.
        app.open_tab("My courses")
        section = app.wait_css("section[aria-label='My courses']")
        assert "Quantum Computing" in section.text, section.text
        assert "Bijita Sarma" in section.text, section.text
        assert "No longer on CMI's timetable" not in section.text, section.text
        card = next(c for c in app.css_all("section[aria-label='My courses'] .card")
                    if gone in c.text)
        assert card.find_element(By.CSS_SELECTOR, ".badge.custom").text == "Added by you"
        # CMI never stated this course's credits, so the app must go on
        # calling its number a guess — keeping must not promote 4 to a fact.
        assert "4 cr*" in card.text, card.text
        # Listed as one of your own changes, like any course you added.
        app.open_tab("My timetable")
        app.xpath("//button[contains(.,'1 change')]").click()  # the ✎ pill
        changes = app.wait_css(".dialog")
        # The group heading renders uppercase, so compare lowercased (t41).
        assert "course you added" in changes.text.lower(), changes.text
        assert gone in changes.text, changes.text
        app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
        app.wait_gone(".dialog")
        # ONE undoable step, both ways. (Before the reload: the undo history
        # lives in memory, so a refresh is where undoing stops being an
        # option — which is exactly why keeping had to write to storage.)
        app.d.execute_script("window.scrollTo(0, 0);")
        app.dismiss_toasts()
        app.xpath("//button[@aria-label='Undo']").click()
        app.wait_toast("Undid")
        app.open_tab("My courses")
        section = app.wait_css("section[aria-label='My courses']")
        assert "No longer on CMI's timetable" in section.text, \
            "undo must put the ghost back exactly as it was"
        app.open_tab("My timetable")
        app.d.execute_script("window.scrollTo(0, 0);")
        app.dismiss_toasts()
        app.xpath("//button[@aria-label='Redo']").click()
        app.wait_toast("Redid")
        # It survives the update message being dismissed AND a reload: this
        # is the whole point — the record was memory-only until now.
        app.d.refresh()
        app.open_tab("My courses")
        section = app.wait_css("section[aria-label='My courses']")
        assert "Quantum Computing" in section.text, section.text
        assert "No longer on CMI's timetable" not in section.text, section.text
        assert app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.custom'))"
            ".courses.map(c => c.code);").count(gone) == 1
    finally:
        stop_serving_cmi()


def t88_keeping_a_dropped_course_keeps_your_own_times(app):
    """A dropped course holds its place on the timetable through the user's
    own overrides — and saving a course of theirs purges those. So keeping
    one must fold what is on the week INTO the definition: the class the
    student moved themselves must not snap back to CMI's old time."""
    serve_cmi()
    try:
        snap, _overrides, gone = cache_from_before_cmi_moved_toc()
        # A meeting the student placed on the ghost themselves: Wed 17:00.
        mine = {"next_id": 1, "credits": [], "items": [{
            "id": 0, "course": gone,
            "base": None,
            "to": {"day": "Wed", "slot": {"start_min": 1020, "end_min": 1095},
                   "hall": "Seminar Hall", "temp_booking": False},
            "created_at": 1754000000000.0}]}
        app.boot("/", selection=["TOC", gone], raw_snapshot=snap, overrides=mine)
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        app.xpath("//button[normalize-space()='See what changed']").click()
        dialog = app.wait_css(".dialog")
        item = next(i for i in dialog.find_elements(By.CSS_SELECTOR, ".diff-item")
                    if gone in i.text)
        item.find_element(By.CSS_SELECTOR, "button.chip").click()
        popup = app.wait_css(".dialog")
        WebDriverWait(app.d, 10).until(
            lambda d: "No longer on CMI's timetable" in app.css(".dialog").text)
        app.dismiss_toasts()   # a sync's toasts cover the dialog's footer
        popup.find_element(
            By.XPATH, ".//button[normalize-space()='Keep this as my own course']").click()
        app.wait_toast(f"{gone} is your own course now")
        app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
        app.wait_gone(".dialog")
        # The student's Wednesday class is still on Wednesday…
        app.open_tab("My timetable")
        app.wait_css(f"td[data-day='2'][data-slot='1020'] button.chip[aria-label^='{gone},']")
        # …and CMI's old Tuesday time was NOT added back beside it.
        assert not app.chips(gone, "td[data-day='1'][data-slot='930']"), \
            "keeping must not put CMI's old time back on the week as a second class"
        saved = app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.custom')).courses"
            ".find(c => c.code === arguments[0]).meetings;", gone)
        assert len(saved) == 1, saved
        assert saved[0]["day"] == "Wed" and saved[0]["hall"] == "Seminar Hall", saved
        # The override that carried it is gone — a course of your own keeps
        # its times in its own definition, never as a change layered on top.
        assert app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.overrides')).items"
            ".filter(o => o.course === arguments[0]).length;", gone) == 0, \
            "keeping must leave no override behind"
    finally:
        stop_serving_cmi()


def t78_many_filter_chips_collapse_behind_more(app):
    """Selecting every course in the catalog is legitimate; seventy chips
    drowning the page is not the UI for it. Past a line's worth the chips
    collapse behind '+N more' — and every one stays removable once
    expanded."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .filterbar")
    app.xpath("//section[@aria-label='Catalog']//details[contains(@class,'facet')]"
              "/summary[starts-with(normalize-space(),'Course')]").click()
    app.wait_css("details.facet[open] .menu")
    app.xpath("//details[contains(@class,'facet') and @open]"
              "//button[normalize-space()='All']").click()
    time.sleep(0.6)
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.ESCAPE)
    chips = app.css_all("section[aria-label='Catalog'] .chipline .filterchip")
    assert len(chips) == 8, f"collapsed to one line's worth, got {len(chips)}"
    more = app.css("section[aria-label='Catalog'] .chipline-more")
    n_hidden = int(more.text.strip().lstrip("+").split()[0])
    assert n_hidden > 20, more.text
    more.click()
    time.sleep(0.3)
    chips = app.css_all("section[aria-label='Catalog'] .chipline .filterchip")
    assert len(chips) == 8 + n_hidden, "expanded shows every chip"
    assert "Show fewer" in app.css("section[aria-label='Catalog'] .chipline-more").text
    # Removing one specific chip still works while expanded.
    chips[10].find_element(By.TAG_NAME, "button").click()
    time.sleep(0.3)
    assert len(app.css_all("section[aria-label='Catalog'] .chipline .filterchip")) \
        == 7 + n_hidden


def t79_json_exports_parse_and_the_backup_restores_everything(app):
    """Export the timetable as JSON (machine-first: stable keys, effective
    meetings, credit provenance), export the whole planner as one backup
    file, wipe the browser, import the backup — the selection, the custom
    move AND the catalog are all back, and the pill honestly says
    'imported' with the ORIGINAL fetch date's age."""
    app.boot("/", selection=["TOC", "RDBM"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='My data']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    time.sleep(1.0)
    tt_files = [f for f in os.listdir(DOWNLOADS) if f.startswith("cmi-timetable-")
                and f.endswith(".json")]
    assert tt_files, os.listdir(DOWNLOADS)
    with open(os.path.join(DOWNLOADS, sorted(tt_files)[-1]), encoding="utf-8") as f:
        tt = json.load(f)
    assert tt["format"] == "cmi-timetable-export"
    assert tt["format_version"].startswith("1.")
    codes = [c["code"] for c in tt["courses"]]
    assert codes == sorted(codes, key=str.lower) and set(codes) == {"TOC", "RDBM"}
    toc = next(c for c in tt["courses"] if c["code"] == "TOC")
    moved = [m for m in toc["meetings"] if m["origin"] == "moved"]
    assert moved and moved[0]["cmi_original"]["day"] == "Tue", moved
    assert moved[0]["day"] == "Wed" and moved[0]["start"]["hhmm"] == "17:00"
    assert toc["credits"]["source"] in ("assumed", "user", "cmi")

    # The section lives low in the dialog, beneath the sticky footer at this
    # scroll position — bring it to the middle first, like a reader would.
    everything = dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Export everything']")
    app.d.execute_script(
        "arguments[0].scrollIntoView({block: 'center'});", everything)
    everything.click()
    time.sleep(1.0)
    bak_files = [f for f in os.listdir(DOWNLOADS) if f.startswith("cmi-planner-")]
    assert bak_files, os.listdir(DOWNLOADS)
    bak_path = os.path.join(DOWNLOADS, sorted(bak_files)[-1])
    with open(bak_path, encoding="utf-8") as f:
        envelope = json.load(f)
    assert envelope["format"] == "cmi-planner-backup"
    assert envelope["snapshot"]["courses"], "the whole catalog rides in the file"
    assert set(envelope["selection"]) == {"TOC", "RDBM"}, envelope["selection"]
    assert envelope["overrides"]["items"], "the custom move rides in the file"
    assert "raw_html_gz" not in envelope["snapshot"] \
        or envelope["snapshot"]["raw_html_gz"] is None

    # Wipe everything, import the file, and the WHOLE planner is back —
    # labelled honestly as imported, at the DATA's age.
    app.boot("/", seed=False)
    app.wait_css(".welcome-card")
    # The first-run auto-sync fails against the stopped CMI and toasts about
    # it; the toast rail floats over the welcome note and would intercept
    # the click. Toasts auto-dismiss — wait them out.
    WebDriverWait(app.d, 20).until(
        lambda d: not app.css_all(".toasts .toast"),
        message="toasts must clear before the note is clickable")
    # The button appends a hidden file input; give the handler a beat and
    # retry the click once — a first-run background sync can be repainting
    # the welcome card at the same moment.
    app.xpath("//button[normalize-space()='Import it']").click()
    try:
        file_input = WebDriverWait(app.d, 5).until(
            lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input"))
    except Exception:
        app.xpath("//button[normalize-space()='Import it']").click()
        file_input = WebDriverWait(app.d, 10).until(
            lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input"))
    file_input.send_keys(bak_path)
    # A fresh browser has nothing to lose, so the import asks nothing and
    # reloads into the imported state.
    app.wait_css(".tabs .tab", timeout=20)
    WebDriverWait(app.d, 10).until(
        lambda d: "imported" in app.css(".sync-pill").text,
        message=f"pill: {app.css('.sync-pill').text!r}")
    # The selection came back...
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    # ...with the custom move intact (TOC on Wed 17:00, not CMI's Tue), and
    # the whole catalog behind it.
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    app.chip("QCOM")  # a course that was never selected — the catalog is whole


def t81_importing_courses_asks_replace_or_add(app):
    """'Import my courses' on Course selection reads the codes out of a
    timetable export and asks — in whole sentences — whether they replace
    the current courses or join them. Codes the catalog doesn't know are
    named and left out; an empty timetable skips the question (nothing to
    replace); and either answer is one undoable step."""
    crafted = os.path.join(DOWNLOADS, "crafted-import.json")
    with open(crafted, "w", encoding="utf-8") as f:
        # " BOGUS9" (leading space) after "BOGUS9" pins the parser's dedup:
        # trim happens BEFORE the duplicate check, so a whitespace variant
        # can't get the same code named twice in "Left out".
        json.dump({"format": "cmi-timetable-export", "format_version": "1.0.0",
                   "courses": [{"code": "MFD"}, {"code": "TOC"},
                               {"code": "BOGUS9"}, {"code": " BOGUS9"}]}, f)

    app.boot("/", selection=["TOC", "QCOM"])
    app.xpath("//button[normalize-space()='My data']").click()
    dialog = app.wait_css(".dialog")

    def send_import():
        dialog.find_element(
            By.XPATH, ".//button[normalize-space()='Import my courses…']").click()
        file_input = WebDriverWait(app.d, 10).until(
            lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input"))
        file_input.send_keys(crafted)

    # Keep both: MFD joins, TOC is recognised as already there, BOGUS9 is
    # named and left out.
    send_import()
    ask = WebDriverWait(app.d, 10).until(
        lambda d: app.css(".dialog") if "Courses from a file"
        in app.css(".dialog").text else None)
    assert "2 courses from this semester" in ask.text, ask.text
    assert "Left out: BOGUS9" in ask.text, ask.text
    assert ask.text.count("BOGUS9") == 1, \
        f"a whitespace-variant duplicate must be deduped, not named twice: {ask.text}"
    ask.find_element(
        By.XPATH, ".//button[contains(.,'Keep my courses and add')]").click()
    app.wait_toast("Added 1 course from the file")
    for code in ("TOC", "QCOM", "MFD"):
        app.wait_css(f"button.chip[aria-label^='{code},']")

    # Replace: the selection becomes exactly the file's two.
    app.xpath("//button[normalize-space()='My data']").click()
    dialog = app.wait_css(".dialog")
    send_import()
    ask = WebDriverWait(app.d, 10).until(
        lambda d: app.css(".dialog") if "Courses from a file"
        in app.css(".dialog").text else None)
    ask.find_element(
        By.XPATH, ".//button[contains(.,'Replace my courses')]").click()
    app.wait_toast("Your timetable now has exactly the 2 courses from that file.")
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    WebDriverWait(app.d, 10).until(
        lambda d: not app.css_all("button.chip[aria-label^='QCOM,']"),
        message="QCOM must be gone after Replace")

    # An empty timetable skips the question — there is nothing to replace.
    app.xpath("//button[normalize-space()='My data']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Clear selection']").click()
    app.wait_toast("Your timetable is empty now")
    send_import()
    app.wait_toast("Added 2 courses from the file.")
    assert "Courses from a file" not in app.css(".dialog").text, \
        "an empty selection must not be asked what to replace"

    # Importing the same file AGAIN changes nothing — and must say so
    # without spending an undo step: Ctrl+Z afterwards undoes the real add
    # above (the chips leave), not a phantom "nothing" step that would have
    # eaten the redo history.
    send_import()
    ask = WebDriverWait(app.d, 10).until(
        lambda d: app.css(".dialog") if "Courses from a file"
        in app.css(".dialog").text else None)
    ask.find_element(
        By.XPATH, ".//button[contains(.,'Keep my courses and add')]").click()
    app.wait_toast("nothing changed")
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    app.wait_gone(".dialog")
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.CONTROL, "z")
    WebDriverWait(app.d, 10).until(
        lambda d: not app.css_all("button.chip[aria-label^='MFD,']"),
        message="Ctrl+Z after a no-op import must undo the real add before it")


def t80_a_seminar_is_assumed_zero_credits(app):
    """CMI lists seminars without credits; assuming the campus default of 4
    for them inflated every total. A seminar with no stated credits counts
    0, the note says why in plain words, and a stated value always wins."""
    app.boot("/", selection=["CSEM"])  # "CS Seminar", creditless in the fixture
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert app.css("section[aria-label='My courses'] .credit-summary .cs-num").text == "0"
    assert "so the app counts it as 0" in section.text, section.text
    assert "seminars don't usually carry credit" in section.text, section.text
    badge = app.xpath(
        "//section[@aria-label='My courses']//div[contains(@class,'card')]"
        "[.//button[starts-with(@aria-label,'CSEM,')]]"
        "//span[contains(@class,'badge')][contains(normalize-space(),'cr')]"
    )
    assert badge.text.strip() == "0 cr*", badge.text
    note = app.xpath(
        "//section[@aria-label='My courses']//div[contains(@class,'card')]"
        "[.//button[starts-with(@aria-label,'CSEM,')]]"
        "//p[contains(@class,'cr-note')]"
    )
    assert "seminar" in note.text, note.text


def t82_conflicts_apply_answers_only_what_you_answered(app):
    """Opening the conflicts dialog to look costs nothing: no row comes
    pre-answered, Apply acts only on the rows you answered, the rest stay
    queued (surviving Dismiss and a reload), and the banner's Dismiss hides
    the banner without answering anything."""
    cached, overrides, gone = cache_from_before_cmi_moved_toc(also_move_iss=True)
    serve_cmi()
    try:
        app.boot("/", selection=["TOC", "ISS"], overrides=overrides,
                 raw_snapshot=cached)
        app.xpath("//button[normalize-space()='Sync now']").click()
        dialog = app.wait_css(".dialog", timeout=30)
        items = dialog.find_elements(By.CSS_SELECTOR, ".conflict-item")
        assert len(items) == 2, f"two customised meetings moved: {len(items)}"

        # Answer ONE row — keep the user's Wednesday for TOC — and leave ISS.
        toc_item = next(i for i in items if "TOC" in i.text)
        toc_item.find_element(
            By.XPATH, ".//label[contains(.,'your time')]//input").click()
        dialog.find_element(By.XPATH, ".//button[normalize-space()='Apply']").click()
        app.wait_toast("still waiting")

        # The answered row is applied…
        app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
        # …and the unanswered one is exactly as it was: still waiting, banner
        # counting one.
        banner = app.xpath("//div[contains(@class,'banner')][contains(.,'conflict')]")
        assert "1 timetable change" in banner.text, banner.text
        stored = app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.conflicts'));")
        assert len(stored) == 1 and stored[0]["course"] == "ISS", stored

        # Dismiss hides the banner but answers nothing…
        banner.find_element(By.XPATH, ".//button[normalize-space()='Dismiss']").click()
        WebDriverWait(app.d, 5).until(
            lambda d: not app.css_all(".banner.warn"),
            message="Dismiss must hide the conflicts banner",
        )
        assert app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.conflicts')).length;"
        ) == 1, "Dismiss must not touch the queue — hiding is not answering"
        # …and the question comes back with the next visit.
        app.d.get(f"{BASE}/")
        app.wait_css(".header h1")
        WebDriverWait(app.d, 10).until(
            lambda d: any("conflict" in b.text for b in app.css_all(".banner.warn")),
            message="the banner must return after a reload — the question stands",
        )
    finally:
        stop_serving_cmi()


def t83_saving_an_edit_asks_before_adding(app):
    """Editing a course that isn't on your timetable shows a ticked 'Also
    add … to my timetable' box in the footer. Untick it and Save stores the
    changes WITHOUT quietly changing the clash picture and the credit total;
    a course already on the timetable is never asked."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    app.chip("TOC").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")

    # The box is in the sticky footer, ticked — the add is asked, not assumed.
    box = app.xpath(
        "//div[@class='dialog']//div[contains(@class,'actions')]"
        "//label[contains(.,'Also add TOC to my timetable')]//input")
    assert box.is_selected(), "the add box must start ticked"
    box.click()

    # A real edit: move Tuesday's meeting to Wednesday 17:00.
    Select(app.css("#ce-day-0")).select_by_visible_text("Wednesday")
    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to TOC")
    assert app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection') || '[]');"
    ) == [], "unticked: saving must not add the course"
    assert app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.overrides')).items.length;"
    ) == 1, "the change itself must be stored"

    # A course already on the timetable has no box to offer — there is
    # nothing to ask.
    app.boot("/?c=ISS")
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    app.xpath("//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    assert not app.d.find_elements(
        By.XPATH, "//div[@class='dialog']//label[contains(.,'Also add')]"), \
        "a selected course's editor must not offer an add box"


def t84_editing_a_dropped_course_invents_no_credit_change(app):
    """A course CMI has dropped has no official credit value to differ from,
    so its editor shows a sentence instead of a credits picker, and an
    untouched Save writes no 'Credits you set: ? → 4' the student never
    made."""
    app.boot("/", selection=["TOC", "GONE"])
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert "No longer on CMI's timetable" in section.text, section.text
    card = app.xpath(
        "//section[@aria-label='My courses']//div[contains(@class,'card')]"
        "[.//button[starts-with(@aria-label,'GONE,')]]")
    card.find_element(
        By.XPATH, ".//button[normalize-space()='Edit this course']").click()
    form = app.wait_css(".dialog .course-form")

    # No picker that cannot act — a sentence says why.
    assert "no official credit value to change" in form.text, form.text
    assert not app.d.find_elements(
        By.CSS_SELECTOR, ".dialog .course-form div[role='radiogroup']"), \
        "a dropped course must not offer a credits picker"

    app.xpath("//div[@class='dialog']//button[normalize-space()='Save changes']").click()
    app.wait_toast("Saved your changes to GONE")
    stored = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.overrides') "
        "|| '{\"credits\":[]}').credits.length;")
    assert stored == 0, \
        f"an untouched save must not invent a credits change: {stored}"


def t85_a_course_hidden_by_filters_is_not_offered_as_new(app):
    """When the search WOULD find a course but a facet set earlier hides it,
    the empty state names the course and offers to clear the filters —
    ahead of the create button, whose suggested code the duplicate guard
    can't recognise (it comes from the name)."""
    app.boot("/")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .filterbar")
    # A Day facet excludes SVA outright: CMI never gave it a time.
    app.xpath(
        "//details[contains(@class,'facet')]/summary[starts-with(normalize-space(),'Day')]"
    ).click()
    app.wait_css("details.facet[open] .menu")
    app.xpath(
        "//details[contains(@class,'facet') and @open]//label[contains(.,'Fri')]//input"
    ).click()
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    search = app.css("section[aria-label='Catalog'] .filterbar input[type='search']")
    search.send_keys("SVA")
    empty = app.wait_css("section[aria-label='Catalog'] .empty")
    assert "(SVA) is in the catalog — a filter above is hiding it" in empty.text, \
        empty.text
    empty.find_element(
        By.XPATH, ".//button[normalize-space()='Clear filters to show it']").click()
    # The facets lift, the search stays, and the named course is on screen.
    WebDriverWait(app.d, 10).until(
        lambda d: bool(app.chips("SVA", "section[aria-label='Catalog']")),
        message="clearing the filters must reveal the course the message named",
    )
    assert app.css(
        "section[aria-label='Catalog'] .filterbar input[type='search']"
    ).get_attribute("value") == "SVA", "the search text must survive the click"


def t86_seg_groups_are_radio_groups_with_arrow_keys(app):
    """The day pickers and the credits row are radio groups: one Tab stop
    (the chosen value), and an arrow key moves the focus AND the choice in
    the same stroke."""
    # Halls day picker.
    app.boot("/")
    app.open_tab("Halls")
    app.wait_css("section[aria-label='Lecture halls']")
    group = "//section[@aria-label='Lecture halls']" \
            "//div[@role='radiogroup' and @aria-label='Day']"
    tue = app.xpath(group + "//button[normalize-space()='Tue']")
    tue.click()
    stops = app.d.find_elements(By.XPATH, group + "//button[@tabindex='0']")
    assert len(stops) == 1 and stops[0].text == "Tue", \
        f"exactly one Tab stop, the chosen day: {[s.text for s in stops]}"
    tue.send_keys(Keys.ARROW_LEFT)
    mon = app.xpath(group + "//button[normalize-space()='Mon']")
    WebDriverWait(app.d, 5).until(
        lambda d: mon.get_attribute("aria-checked") == "true",
        message="one arrow stroke must move the choice",
    )
    assert app.d.execute_script("return document.activeElement.textContent;") == "Mon", \
        "the focus must travel with the choice"

    # The editor's credits row.
    app.boot("/?c=TOC")
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    app.xpath("//button[normalize-space()='Edit this course']").click()
    app.wait_css(".dialog .course-form")
    seg = "//div[@class='dialog']//div[@role='radiogroup']"
    chosen = app.d.find_elements(By.XPATH, seg + "//button[@tabindex='0']")
    assert len(chosen) == 1, "one Tab stop in the credits group"
    start = int(chosen[0].text)
    chosen[0].send_keys(Keys.ARROW_LEFT)
    prev = app.xpath(seg + f"//button[normalize-space()='{start - 1}']")
    WebDriverWait(app.d, 5).until(
        lambda d: prev.get_attribute("aria-checked") == "true",
        message="the arrow must choose the previous credit value",
    )
    app.xpath("//div[@class='dialog']//button[normalize-space()='Cancel']").click()
    app.wait_gone(".dialog")


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
    t63_editing_a_course_with_no_time_never_invents_one,
    t64_a_half_written_form_is_not_thrown_away_by_a_stray_key,
    t65_my_courses_has_the_same_filters,
    t66_controls_that_cannot_act_are_not_offered,
    t67_the_master_grid_counts_what_it_can_draw,
    t68_a_keyboard_move_on_a_phone_shows_where_it_is,
    t69_halls_marks_your_courses_even_without_a_meeting,
    t70_one_course_is_not_a_choice,
    t71_what_changed_never_opens_with_nothing_to_say,
    t72_a_relay_is_asked_before_cmi_itself,
    t73_cmi_itself_is_the_fallback_and_says_so,
    t74_offline_reload_boots_from_cache,
    t75_my_courses_filters_are_its_own,
    t76_no_false_conflict_and_decide_later_survives_reload,
    t77_what_changed_shows_what_a_dropped_course_was,
    t78_many_filter_chips_collapse_behind_more,
    t79_json_exports_parse_and_the_backup_restores_everything,
    t80_a_seminar_is_assumed_zero_credits,
    t81_importing_courses_asks_replace_or_add,
    t82_conflicts_apply_answers_only_what_you_answered,
    t83_saving_an_edit_asks_before_adding,
    t84_editing_a_dropped_course_invents_no_credit_change,
    t85_a_course_hidden_by_filters_is_not_offered_as_new,
    t86_seg_groups_are_radio_groups_with_arrow_keys,
    t87_a_dropped_course_can_be_kept_as_your_own,
    t88_keeping_a_dropped_course_keeps_your_own_times,
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
