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
                      (default: ~/.rust-cache/timetable-e2e — inside
                      ~/.rust-cache because ALL build artifacts live there
                      (the user's cargo is a wrapper that enforces it), and
                      its own subfolder so a running `trunk serve`, which
                      the wrapper routes to ~/.rust-cache/timetable, can
                      never race it)

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

import datetime
import http.server
import json
import os
import socket
import ssl
import subprocess
import sys
import shutil
import tempfile
import threading
import time
import traceback
import urllib.parse

from selenium import webdriver
from selenium.common.exceptions import StaleElementReferenceException
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


def newest_download(prefix, suffix=".json"):
    """The file the browser has just written, by modification time.

    NOT by name: two exports on the same day want the same filename, and
    Chrome disambiguates the second as "… (1).json" — which sorts BEFORE
    "….json", because a space beats a dot. Sorting by name hands a test the
    stale file and every assertion after it becomes a lie about the wrong
    week."""
    files = [os.path.join(DOWNLOADS, f) for f in os.listdir(DOWNLOADS)
             if f.startswith(prefix) and f.endswith(suffix)]
    assert files, f"no {prefix}*{suffix} in {os.listdir(DOWNLOADS)}"
    return max(files, key=os.path.getmtime)

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

# A LONG meeting: TOC's Tue class stretched to 09:10–14:00. The start sits ON
# an official slot, so no synthetic column is minted (t36 pins that rule) and
# the covered columns are exactly 630 and 710 — 840 is NOT covered, because
# Slot::overlaps is half-open and the meeting ends exactly where that column
# starts (the clash panel's own arithmetic).
LONG_OVR = {
    "next_id": 1,
    "items": [{
        "id": 0, "course": "TOC",
        "base": {"day": "Tue", "slot": {"start_min": 550, "end_min": 625},
                 "hall": "Lecture Hall 803", "temp_booking": False},
        "to": {"day": "Tue", "slot": {"start_min": 550, "end_min": 840},
               "hall": "Lecture Hall 803", "temp_booking": False},
        "created_at": 1754000000000.0}],
    "credits": [],
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
        "CARGO_TARGET_DIR", os.path.expanduser("~/.rust-cache/timetable-e2e")
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

_cmi = {"up": False, "proxy": False, "relays": None, "bodies": {},
        "cors": True, "dead": False}


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
        isn't a relayed one.

        Relays disagree about how the target rides along, and the app now
        ships four of them (R84), so this recognises all three shapes rather
        than one: a `url=` query parameter (allorigins, cors.lol), a `quest=`
        one (codetabs), and the whole target appended to the relay's own path
        (cors.sh). The app hangs a cache-buster on the target either way, so
        what identifies the page is its PATH, never the whole string."""
        query = self.path.split("?", 1)[1] if "?" in self.path else ""
        params = urllib.parse.parse_qs(query)
        target = params.get("url", params.get("quest", [None]))[0]
        if target is None:
            # cors.sh style: https://proxy.cors.sh/https://www.cmi.ac.in/…
            head = self.path.split("?", 1)[0]
            for scheme in ("/https://", "/http://"):
                if scheme in head:
                    target = head[head.index(scheme) + 1:]
                    break
        if target is None:
            return None
        return urllib.parse.urlsplit(target).path

    def do_OPTIONS(self):
        """CORS preflight.

        A request carrying a header that is not CORS-safelisted — the app
        sends `x-return-format` to exactly one relay — is preceded by an
        OPTIONS the browser will not let the real request past. Without this
        the stand-in answered 501 and that relay looked dead, which is a
        property of this harness and not of the app (R84)."""
        if _cmi["dead"]:
            self.close_connection = True
            return
        self.send_response(204)
        if _cmi["cors"]:
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Access-Control-Allow-Headers", "*")
            self.send_header("Access-Control-Allow-Methods", "GET, OPTIONS")
            self.send_header("Access-Control-Max-Age", "600")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):
        # "Nothing at that address at all": answer nothing and hang up. The
        # ordinary "down" state of this stand-in is a 503, which is a real
        # failure but a DIFFERENT one — the app can read a status, which
        # proves the cross-origin rule was not what stopped it. Telling those
        # two apart is the whole point of the failure copy (R84), so the
        # harness has to be able to produce both.
        if _cmi["dead"]:
            self.close_connection = True
            return
        path = self.path.split("?", 1)[0]
        # A relayed request answers with the same page the direct one would,
        # but only while the relays are switched on: the default is every
        # public route dead, which is what most tests want.
        relayed = self._relayed_path()
        if relayed is not None:
            # Which relay was asked matters now that the app ships four of
            # them: a test can leave exactly one alive and prove the others
            # are not what carried the sync (R84).
            host = (self.headers.get("Host") or "").split(":")[0]
            allowed = _cmi["relays"] is None or host in _cmi["relays"]
            path = relayed if (_cmi["proxy"] and allowed) else "/nowhere"
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
        # proxy tier exists. This one does by default, because a test that
        # had to go through a public relay to reach localhost would be
        # testing the relay, not the app. `serve_cmi(cors=False)` turns it
        # off, which reproduces the real cmi.ac.in exactly: the page answers
        # 200, the browser refuses to let the app READ it, and the app has to
        # say so without blaming a network that is working (R84).
        if _cmi["cors"]:
            self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args, **kwargs):
        pass


def serve_relays(only=None):
    """Make the public CORS relays answer too, for the tests that care which
    route the app takes. Off by default: with the relays dead, a sync falls
    through to the direct route, which is what most tests exercise.

    `only` is a set of relay hostnames; everything else keeps answering as if
    it were down. The app ships four relays plus whatever the reader supplies
    (R84), so "one of them is enough" and "yours is asked first" are things a
    test can now state."""
    _cmi["proxy"] = True
    _cmi["relays"] = set(only) if only is not None else None


def serve_cmi(timetable=None, lecturehalls=None, cors=True):
    """Make CMI reachable for this test. Serves the fixture pages verbatim
    unless given something else.

    `cors=False` withholds `Access-Control-Allow-Origin`, which is what the
    real cmi.ac.in does — the pages answer and the browser will not let the
    app read them. Only one test wants that; everything else would be testing
    a relay instead of the app."""
    given = {"timetable.php.html": timetable,
             "lecturehalls.php.html": lecturehalls}
    for path, name in CMI_PAGES.items():
        body = given[name]
        if body is None:
            with open(os.path.join(FIXTURES, name), encoding="utf-8") as f:
                body = f.read()
        _cmi["bodies"][path] = body
    _cmi["up"] = True
    _cmi["cors"] = cors


def stop_serving_cmi():
    """Back to an unreachable CMI — the state every test starts from, relays
    included."""
    _cmi["up"] = False
    _cmi["proxy"] = False
    _cmi["relays"] = None
    _cmi["bodies"] = {}
    _cmi["cors"] = True
    _cmi["dead"] = False


class _QuietCmiServer(http.server.ThreadingHTTPServer):
    """Hanging up mid-request is a THING THIS SERVER DOES on purpose (see
    `_cmi["dead"]`), and socketserver prints a full traceback for every one of
    them. Swallow it: a stack trace that means "the test is working" trains
    the eye to skip stack traces."""

    def handle_error(self, request, client_address):
        pass


def serve_fake_cmi():
    tmp = tempfile.mkdtemp(prefix="cmitt-e2e-cmi-")
    key, crt = _make_cert(tmp)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(crt, key)
    server = _QuietCmiServer(("127.0.0.1", CMI_PORT), _CmiHandler)
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
        f"MAP proxy.cors.sh 127.0.0.1:{CMI_PORT}, "
        f"MAP api.cors.lol 127.0.0.1:{CMI_PORT}, "
        f"MAP api.allorigins.win 127.0.0.1:{CMI_PORT}, "
        f"MAP api.codetabs.com 127.0.0.1:{CMI_PORT}, "
        f"MAP cors-get-proxy.sirjosh.workers.dev 127.0.0.1:{CMI_PORT}, "
        f"MAP corsmirror.onrender.com 127.0.0.1:{CMI_PORT}, "
        f"MAP r.jina.ai 127.0.0.1:{CMI_PORT}, "
        # Somewhere for "a helper site the reader supplied" to point at, so
        # that route can be tested as what it is: a host the app has never
        # heard of (R84).
        f"MAP helper.example 127.0.0.1:{CMI_PORT}, "
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
             overrides=None, raw_snapshot=None, customs=None, prefs=None):
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
                stored_prefs = {"last_update_attempt": time.time() * 1000.0}
                # `prefs` merges rather than replaces, so a test that needs one
                # setting does not have to restate the sync throttle that keeps
                # every other test deterministic.
                stored_prefs.update(prefs or {})
                args = [
                    json.dumps(stored_prefs),
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
                if prefs:
                    self.d.execute_script(
                        "localStorage.setItem('cmitt.v1.prefs', arguments[0]);",
                        json.dumps(prefs))
        self.d.get(f"{BASE}{path}")
        self.wait.until(EC.presence_of_element_located((By.CSS_SELECTOR, ".header h1")))

    # -- helpers -----------------------------------------------------------

    def pin_weekday(self, weekday):
        """Make the browser believe today is a given weekday (Mon=0), for
        every document loaded from here on.

        Tests that assert "today is marked" used to read the HOST clock and
        skip their own subject on a Saturday or Sunday — two days in seven,
        including the day one deploy went out — passing by asserting that
        nothing was marked, which is also exactly what a completely missing
        feature looks like (R83). The app reads the date through the ordinary
        `Date` constructor, so replacing it before any script runs is enough,
        and it makes the positive assertions run every day of the week.

        Call `unpin_weekday()` afterwards; it does not survive a new driver.
        """
        offset_days = (weekday - datetime.datetime.now().weekday()) % 7
        return self.d.execute_cdp_cmd(
            "Page.addScriptToEvaluateOnNewDocument",
            {"source": """
                (() => {
                  const SHIFT = %d * 86400000;
                  const Real = Date;
                  const Shifted = function (...args) {
                    if (args.length === 0) return new Real(Real.now() + SHIFT);
                    return new Real(...args);
                  };
                  Shifted.prototype = Real.prototype;
                  Shifted.now = () => Real.now() + SHIFT;
                  Shifted.parse = Real.parse;
                  Shifted.UTC = Real.UTC;
                  window.Date = Shifted;
                })();
             """ % offset_days},
        )["identifier"]

    def unpin_weekday(self, identifier):
        """Undo `pin_weekday`. MUST be called in a `finally`: an injected
        script outlives the test that added it and would silently shift the
        clock for every test after it."""
        self.d.execute_cdp_cmd("Page.removeScriptToEvaluateOnNewDocument",
                               {"identifier": identifier})

    def wait_for_print_tab(self, app_handle, timeout=25):
        """Wait until the print tab exists AND has been filled with a sheet.

        A bare `sleep(3)` was the only wait in the newest print test, and 3s
        is less than the app's own budget for that tab — so on a loaded
        machine it failed CLOSED, which is a false red rather than a false
        green, but it is still the first thing to suspect (R83). Polling for
        the thing the test is actually waiting for removes the guess.

        Returns the print tab's handle and leaves the driver on the ORIGINAL
        window, so callers keep the control they had.
        """
        end = time.time() + timeout
        found = None
        while time.time() < end:
            others = [h for h in self.d.window_handles if h != app_handle]
            if others:
                found = others[0]
                try:
                    self.d.switch_to.window(found)
                    filled = self.d.execute_script(
                        "const s = document.getElementById('sheet');"
                        "return !!(s && s.children.length);")
                except Exception:
                    filled = False
                finally:
                    self.d.switch_to.window(app_handle)
                if filled:
                    return found
            time.sleep(0.15)
        return found

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

    def confirm_text(self, timeout=10):
        """The app's own confirmation, which replaced window.confirm. Waits
        for it, because several of them are raised from an async file read."""
        return WebDriverWait(self.d, timeout).until(
            lambda d: (
                el.text
                if (el := d.find_element(By.CSS_SELECTOR, ".dialog.confirm"))
                and el.is_displayed()
                else None
            )
        )

    def answer_confirm(self, yes):
        """Press one of its two buttons. Cancel is found by its marker
        attribute rather than its words, so the copy can change freely."""
        box = self.wait_css(".dialog.confirm")
        if yes:
            # The confirming button is the one that is NOT cancel.
            btn = box.find_element(
                By.CSS_SELECTOR, ".actions button:not([data-confirm-cancel])")
        else:
            btn = box.find_element(By.CSS_SELECTOR, "[data-confirm-cancel]")
        btn.click()

    def toasts_text(self):
        """Whatever is in the toast rail right now.

        Tolerant of a toast that vanishes MID-READ: toasts auto-dismiss on a
        timer, so between `css_all` and reading an element's `.text` one can
        stop existing and Selenium raises `StaleElementReferenceException`.
        That is the rail working, not a failure — but it was crashing whole
        tests, including inside a `WebDriverWait` predicate, where it killed
        the wait instead of retrying it (seen in t74, R84)."""
        out = []
        for t in self.css_all(".toasts .toast"):
            try:
                out.append(t.text)
            except StaleElementReferenceException:
                continue
        return " | ".join(out)

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

    def drag_hover(self, elem, target):
        """The first half of `drag`: hold the pointer over `target` without
        releasing, so what the page shows MID-drag can be asserted. Finish
        with `drop()` — an unreleased button leaks into the next action."""
        (
            ActionChains(self.d)
            .click_and_hold(elem)
            .move_by_offset(12, 0)  # pass the drag threshold
            .move_to_element(target)
            .pause(0.15)
            .perform()
        )

    def drop(self):
        ActionChains(self.d).release().perform()
        time.sleep(0.5)  # let the click-suppression window lapse

    def lit_cell(self):
        """The one cell highlighted as the drop target, or a failure naming
        how many there were. The highlight is derived state (`App::drop_target`)
        recomputed on every pointermove, so 'exactly one' is the assertion
        that matters — two would mean a stale cell never cleared."""
        lit = self.css_all("td.drop-ok")
        assert len(lit) == 1, f"exactly one cell should be lit, got {len(lit)}"
        return lit[0]


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def t01_header_sync_button_and_hidden_dev(app):
    """'Sync now' present; developer mode is out of the everyday view but
    findable: never a rail tab, always a quiet door inside My data (R87)."""
    app.boot("/")
    app.xpath("//button[normalize-space()='Sync now']")
    app.xpath("//button[normalize-space()='My data']")
    tabs = [t.text for t in app.css_all(".tabs .tab")]
    assert "Developer" not in tabs, f"Developer tab should be hidden, got {tabs}"
    assert tabs == [
        "My timetable", "My courses", "Master grid", "Catalog", "Halls",
    ], tabs
    # The one discoverable path: My data → "Under the hood".
    app.xpath("//button[normalize-space()='My data']").click()
    app.wait_css(".dialog")
    app.xpath("//div[contains(@class,'dialog')]"
              "//button[normalize-space()='Open developer mode']")
    assert "Under the hood" in app.css(".dialog").text
    app.css(".dialog").send_keys(Keys.ESCAPE)
    WebDriverWait(app.d, 5).until(lambda d: not app.css_all(".dialog"))


def t02_developer_endpoint_only(app):
    """The endpoint still works opened directly, and the R87 re-shelving
    renamed nothing load-bearing: every original panel heading still exists,
    each on its own category of the developer rail."""
    app.boot("/#/developer")
    section = app.wait_css("section[aria-label='Developer mode']")
    assert "Developer mode" in section.text
    # Bare #/developer lands on Overview, hash untouched.
    assert app.d.execute_script("return location.hash") == "#/developer"
    expected = {
        "Overview": ("Build info", "This browser"),
        "Sync": ("Simulators", "Fetch log", "Parse reports"),
        "Storage": ("Storage inspector", "Raw HTML viewer"),
        "Tweaks": ("Marks", "The week grid", "Colour and motion"),
    }
    rail = [t.text for t in app.css_all(".tabs .tab")]
    assert rail == ["Overview", "Tweaks", "Sync", "Storage"], rail
    for label, panels in expected.items():
        app.open_tab(label)
        # A category hop is a hash change, and hashchange is asynchronous —
        # the OLD category's section is still mounted for a beat after the
        # click, so waiting on the section alone reads yesterday's panels.
        # The tabpanel's per-category id is the proof the swap landed.
        app.wait_css(f"#panel-dev-{label.lower()}")
        section = app.wait_css("section[aria-label='Developer mode']")
        for panel in panels:
            assert panel in section.text, f"{label}: missing panel {panel}"
    # The way out is first in the rail, and it is a button, not a tab.
    exit_btn = app.css(".tabs .tab-exit")
    assert exit_btn.get_attribute("role") != "tab"
    exit_btn.click()
    app.wait_css("section[aria-label='My timetable']")


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
    assert title == "One course in that link isn't in CMI's timetable, so it was left out.", title
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
    assert title == "2 courses in that link aren't in CMI's timetable, so they were left out.", title
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
    with 'Fits my timetable' OFF."""
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
    assert "Would clash with one other course of yours" in dialog.text, dialog.text
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
    # Mid-drag, the cell under the pointer — and only that cell — lights up.
    # The whole grid carries a drop-ok binding, so this also proves the other
    # ~30 cells stayed dark.
    app.drag_hover(app.chip("TOC", "td[data-day='1'][data-slot='550']"), target)
    lit = app.lit_cell()
    assert (lit.get_attribute("data-day"), lit.get_attribute("data-slot")) == ("2", "1020"), \
        "the lit cell must be the one under the pointer"
    app.drop()
    app.wait_toast("Moved TOC")
    assert not app.css_all("td.drop-ok"), "the highlight must clear on drop"
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

    # A profile that never made a custom course must not grow the key from a
    # mere undo walk: an empty store persists as NO store, or every backup
    # carries a `{"courses":[]}` husk (final sweep, core-flows-1).
    assert app.d.execute_script(
        "return localStorage.getItem('cmitt.v1.custom');") is None, \
        "undo must not materialize an empty cmitt.v1.custom"


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

    # By NAME, not by index: R77 put the "Search in" dropdown first in the
    # bar, and any future insertion would silently retarget an index.
    def facet_named(name):
        for f in app.css_all(".filterbar details.facet"):
            if f.find_element(By.CSS_SELECTOR, "summary").text.strip().startswith(name):
                return f
        raise AssertionError(f"no facet named {name!r} in the bar")

    branch, instructor = facet_named("Branch"), facet_named("Instructor")
    branch.find_element(By.CSS_SELECTOR, "summary").click()
    assert branch.get_attribute("open") is not None
    # A click inside the open menu must NOT close it.
    branch.find_element(By.CSS_SELECTOR, ".menu label.opt input").click()
    time.sleep(0.2)
    assert branch.get_attribute("open") is not None, \
        "clicking a checkbox inside the menu must not close it"
    instructor.find_element(By.CSS_SELECTOR, "summary").click()  # closes Branch
    time.sleep(0.2)
    assert branch.get_attribute("open") is None, \
        "opening the second menu must close the first"
    assert instructor.get_attribute("open") is not None
    # Clicking anywhere outside closes the open menu.
    app.css("section[aria-label='Catalog'] .toolbar h2").click()
    time.sleep(0.2)
    assert all(
        f.get_attribute("open") is None
        for f in app.css_all(".filterbar details.facet")
    ), "outside click must close every open menu"
    # Esc closes too — including the Search in menu, which is a facet to
    # every global handler (that is why it wears the class).
    searchin = facet_named("Search in")
    searchin.find_element(By.CSS_SELECTOR, "summary").click()
    assert searchin.get_attribute("open") is not None
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
    # CMI publishes no credit figure for TOC, so the summary must not say the
    # total prefers your number over CMI's — the card right below it says CMI
    # has none. What your 3 stands in for is the app's guess of 4.
    assert "rather than the app's guess" in section.text, section.text
    assert "not CMI's" not in section.text, section.text
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
        # TOC's 4 is the app's own guess, not a figure CMI publishes, so
        # the button offers to go back to the app's number and says so.
        ".//li[contains(.,'TOC')]//button[normalize-space()=\"Back to the app's 4\"]"
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
        By.XPATH, ".//div[@role='radiogroup'][@aria-label='Day view']//button[normalize-space()='Tue']"
    ).click()
    src_cell = "td[data-hall='Lecture Hall 803'][data-slot='550']"
    dst_cell = "td[data-hall='Seminar Hall'][data-slot='840']"
    app.wait_css(f"{src_cell} button.chip[aria-label^='TOC,']")

    # Inert without edit mode.
    app.drag(app.chip("TOC", src_cell), app.css(dst_cell))
    assert "Moved" not in app.toasts_text()
    assert not app.chips("TOC", dst_cell)

    section.find_element(By.XPATH, ".//button[contains(.,'Edit layout')]").click()
    # Mid-drag: exactly one cell lights up, and here the hall is part of the
    # answer — several halls share this column, and the match on the hall name
    # is byte-exact. A hall row lighting up at the same time as another would
    # mean the drop target had stopped distinguishing rooms.
    app.drag_hover(app.chip("TOC", src_cell), app.css(dst_cell))
    lit = app.lit_cell()
    assert (lit.get_attribute("data-hall"), lit.get_attribute("data-slot")) \
        == ("Seminar Hall", "840"), "the lit cell must be the hall under the pointer"
    app.drop()
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
        By.XPATH, ".//div[@role='radiogroup'][@aria-label='Day view']//button[normalize-space()='Tue']"
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
        By.XPATH, ".//div[@role='radiogroup'][@aria-label='Day view']//button[normalize-space()='Tue']"
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
    # By name, not index — see t16 (the Search in menu sits first since R77).
    app.xpath("//div[contains(@class,'filterbar')]//details"
              "[starts-with(normalize-space(summary), 'Instructor')]/summary").click()
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
    # The welcome note promises the app will keep checking by itself — the
    # first fetch is the only one it ever asks the reader for.
    assert "twice a day" in welcome.text, welcome.text
    assert not app.css_all(".tabs .tab"), "no tabs before the first sync"
    assert "Not synced yet" in app.css(".sync-pill").text or \
        app.css_all(".sync-pill .spinner"), "pill must show the unsynced state"
    # The automatic first sync fails (no reachable route) → banner + prompt stays.
    banner = app.wait_css(".banner", timeout=30)
    # WHICH failure it names is t136's subject, and it depends on what the
    # stand-in does — here it answers 503 at every address, which is a real
    # failure but not the same one as a dead network. What this test is about
    # is that the app says so plainly, does not pretend a timetable arrived,
    # and leaves the reader somewhere to go.
    assert "still empty" in banner.text, banner.text
    assert any(reason in banner.text for reason in (
        "answered, but with an error", "Nothing answered",
        "isn't allowed to read it", "offline",
    )), banner.text
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
    # By name, not index — see t16 (the Search in menu sits first since R77).
    app.xpath("//div[contains(@class,'filterbar')]//details"
              "[starts-with(normalize-space(summary), 'Branch')]/summary").click()
    app.wait_css("details.facet[open] .menu")
    app.css("details.facet[open] .menu label.opt input").click()
    app.wait_css(".filterchip")
    app.xpath("//button[@aria-label='Undo']").click()
    app.wait_toast("Undid: the")
    app.wait_gone(".filterchip")
    # Not the Search in menu: its three boxes are CHECKED at rest (all parts
    # read is the default), so sweeping every facet checkbox would fail on
    # the one menu whose default is ticks.
    assert not app.css_all("details.facet:not(.searchin) .menu input:checked")
    app.xpath("//button[@aria-label='Redo']").click()
    app.wait_css(".filterchip")
    # Search coalescing: several keystrokes, one undo.
    search = app.css(".filterbar input[type='search']")
    search.send_keys("toc")
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all(".filterchip")) == 2
    )
    # Typed filters reach localStorage SYNCHRONOUSLY — read with no reload and
    # no sleep. A trailing-timer "optimisation" here would race every reload
    # in this suite and, worse, land on top of the three code paths that clear
    # this key and reload; this line is the fence that keeps it out.
    assert '"text":"toc"' in app.d.execute_script(
        "return localStorage.getItem('cmitt.v1.prefs');"
    ), "a keystroke must be persisted before the next statement runs"
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
    app.xpath("//button[normalize-space()='Share or import']").click()
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
    app.wait_toast("Put TOC's meeting back")
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
    on a schedule that follows the words (1 s under a minute, 15 s inside the
    hour, 15 min past it) and instantly on visibilitychange, so a throttled
    background tab catches up the moment it comes back.

    This test is also the WORDING contract: 'just now', 'N min ago',
    'N hours ago', 'N days ago' and nothing finer. A seconds counter would
    fail the first assertion below."""
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


def t91_a_sync_in_one_tab_reaches_the_other(app):
    """Two tabs, one browser. Sync in the first and the second must not go
    on saying the timetable is twenty minutes old — it takes the whole
    update, data and clock together, without being touched.

    The probe is the point. Reading the pill after switching back would pass
    for a fix that merely re-reads storage when a tab is shown; this captures
    what tab two displayed while it was still in the BACKGROUND, so only a
    real cross-tab signal satisfies it."""
    stale = json.loads(SEED_SNAPSHOT_JSON)
    stale["fetched_at"] = time.time() * 1000.0 - 20 * 60_000  # 20 min ago
    stale["courses"] = [c for c in stale["courses"] if c["code"] != "TOC"]
    app.boot("/", raw_snapshot=json.dumps(stale))
    first = app.d.current_window_handle
    assert "20 min ago" in app.css(".sync-pill").text, app.css(".sync-pill").text

    app.d.switch_to.new_window("tab")
    second = app.d.current_window_handle
    app.d.get(BASE + "/")
    app.wait_css(".header h1")
    assert "20 min ago" in app.css(".sync-pill").text, app.css(".sync-pill").text
    # Watch the pill from inside the background tab: every change to its text
    # is recorded, so nothing here depends on when we look.
    app.d.execute_script("""
        window.__pill = [];
        new MutationObserver(() => {
            const t = document.querySelector('.sync-pill');
            if (t) window.__pill.push(t.textContent.trim());
        }).observe(document.querySelector('.header'),
                   {subtree: true, childList: true, characterData: true});
    """)

    # Tab one syncs for real, against the stand-in CMI.
    app.d.switch_to.window(first)
    serve_cmi()
    try:
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        WebDriverWait(app.d, 20).until(
            lambda d: "just now" in app.css(".sync-pill").text,
            message="tab one should show its own fresh sync",
        )
    finally:
        stop_serving_cmi()

    # …and tab two catches up on its own, while it was never in front.
    app.d.switch_to.window(second)
    WebDriverWait(app.d, 20).until(
        lambda d: "just now" in app.css(".sync-pill").text,
        message=f"the second tab is still stale: {app.css('.sync-pill').text!r}",
    )
    seen = app.d.execute_script("return window.__pill || [];")
    assert any("just now" in s for s in seen), \
        f"the second tab must refresh in the background, not on focus; saw {seen}"
    # The DATA came too, not just the clock: TOC was cut from the seeded
    # snapshot and CMI's pages have it, so it can only be here via the sync.
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    assert app.chips("TOC"), "the second tab must adopt the timetable, not only its timestamp"
    app.d.close()
    app.d.switch_to.window(first)


def t92_the_pill_refreshes_at_the_pace_the_words_change(app):
    """The refresh schedule follows the text: about a second while it can
    still say 'just now', 15 s while it counts minutes, 15 min once it counts
    hours. Read off the delays the page actually asks the browser for —
    no other timer in the app uses any of these three numbers."""
    snap = json.loads(SEED_SNAPSHOT_JSON)
    snap["fetched_at"] = time.time() * 1000.0
    app.boot("/", raw_snapshot=json.dumps(snap))
    app.d.execute_script("""
        window.__delays = [];
        const real = window.setTimeout;
        window.setTimeout = function (fn, ms) {
            window.__delays.push(ms);
            return real.apply(this, arguments);
        };
    """)

    def armed():
        return [d for d in app.d.execute_script("return window.__delays;")
                if d in (1000, 15000, 900000)]

    WebDriverWait(app.d, 10).until(lambda d: 1000 in armed(),
                                   message=f"a fresh sync should tick every second; saw {armed()}")

    def jump(minutes):
        app.d.execute_script(
            "window.__realNow = window.__realNow || Date.now.bind(Date);"
            f"Date.now = () => window.__realNow() + {minutes} * 60000;"
            "window.__delays = [];"
        )

    jump(7)          # minutes bucket — the next re-arm should be 15 s
    WebDriverWait(app.d, 10).until(
        lambda d: 15000 in armed(),
        message=f"7 min old should re-arm at 15 s; saw {armed()}",
    )
    jump(3 * 60)     # hours bucket — 15 minutes, and never a 1 s spin again
    WebDriverWait(app.d, 30).until(
        lambda d: 900000 in armed(),
        message=f"3 h old should re-arm at 15 min; saw {armed()}",
    )
    assert 1000 not in armed(), \
        f"an hours-old sync must not keep waking every second; saw {armed()}"
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
    app.xpath("//button[normalize-space()='Share or import']").click()
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
        "//section[@aria-label='Lecture halls']//div[@role='radiogroup' and @aria-label='Day view']"
        f"//button[normalize-space()='{short}']"
    ).click()
    time.sleep(0.3)


def _halls_all(app):
    """Switch the Halls tab to every day at once."""
    _halls_day(app, "Week")


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
        By.XPATH, ".//div[@aria-label='Day view']//button")][1:]  # "All" comes first
    rows = table.find_elements(By.CSS_SELECTOR, "tbody tr")
    names = table.find_elements(By.CSS_SELECTOR, "tbody th.hallhead")
    assert len(rows) == len(names) * len(days), (len(rows), len(names))
    assert all(n.get_attribute("rowspan") == str(len(days)) for n in names), \
        "each hall's name must span its days"
    assert len({n.find_element(By.CSS_SELECTOR, ".hall-name").text
                for n in names}) == len(names), "one name per hall, no repeats"

    # The weekly load line reads in the UI face — its token used to be the
    # undefined --font-sans, and the whole line fell back to the table's
    # mono (final sweep, css-audit-1).
    load_face = app.d.execute_script(
        "const l = document.querySelector('th.hallhead .hall-load');"
        "return l ? getComputedStyle(l).fontFamily : '';")
    assert load_face.split(",")[0].strip().strip('"') == "Inter", \
        f"hall loads must use the UI face, got {load_face}"
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
    app.boot("/#/developer/sync", selection=["TOC"])
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
    # The app's own question now, not the browser's grey box — and it is
    # asked ON TOP of the editor, which must still be mounted behind it or
    # the answer "keep editing" would come back to an empty form.
    assert "Close this form?" in app.confirm_text(), app.confirm_text()
    assert app.css(".dialog .course-form"), "the editor was unmounted to ask"
    app.answer_confirm(False)
    app.wait_gone(".dialog.confirm")
    app.css(".dialog .course-form")  # still open, still holding the edit

    # A click on the dark area asks the same question.
    app.d.execute_script(
        "document.querySelector('.overlay').click();")
    assert "Close this form?" in app.confirm_text(), app.confirm_text()
    app.answer_confirm(True)
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
    # Scoped to the empty panel on purpose: the filter bar above it now carries
    # the same label (one action, one name), so an unscoped XPath would click
    # the bar's button instead of the one this test is about.
    section.find_element(
        By.XPATH,
        ".//div[contains(@class,'empty')]//button[normalize-space()='Clear all filters']",
    ).click()
    WebDriverWait(app.d, 10).until(
        lambda d: len(app.css_all("section[aria-label='My courses'] .card")) == 3)

    # "Fits my timetable" is NOT offered here: it hides whatever overlaps your
    # selection, and everything on this page IS your selection, so the box
    # could never hide a card. It stays where it can act.
    section = app.css("section[aria-label='My courses']")
    assert "Fits my timetable" not in section.text, section.text
    app.open_tab("Catalog")
    cat = app.wait_css("section[aria-label='Catalog']")
    assert "Fits my timetable" in cat.text, "the catalog keeps it"
    app.open_tab("Master grid")
    grid = app.wait_css("section[aria-label='Master grid']")
    assert "Fits my timetable" in grid.text, "the master grid keeps it"
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
    # A menu builds its rows when it first opens, not while it is closed
    # (that was three hundred rows per filter bar, on every tab switch).
    WebDriverWait(app.d, 10).until(
        lambda d: d.find_elements(
            By.CSS_SELECTOR, "details.facet[open] .menu label.opt"),
        message="the open menu never built its rows",
    )
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

        # A COMMITTED move takes the strip with it — the reader put the class
        # on Wednesday and wants to see it there.
        def checked():
            return app.xpath("//div[@aria-label='Day view']"
                             "//button[@aria-checked='true']").text
        assert checked() == "Wed", checked()
        stored = app.d.execute_script(
            "return JSON.stringify(JSON.parse("
            "localStorage.getItem('cmitt.v1.prefs')).plan_view);")

        # …but a move CANCELLED with Escape must leave that choice exactly as
        # it was. Walking days while moving is looking, not choosing: the
        # first version of this wrote a preference on every arrow key, so
        # pressing Escape left the reader on a day they never picked.
        chip = app.chip("TOC", ".day-list .slotrow[data-day='2'][data-slot='550']")
        app.d.execute_script("arguments[0].focus();", chip)
        chip.send_keys("m")
        app.wait_css(".day-list .slotrow.kbd-cursor")
        body.send_keys(Keys.ARROW_UP)
        WebDriverWait(app.d, 10).until(
            lambda d: d.find_elements(
                By.CSS_SELECTOR, ".day-list .slotrow.kbd-cursor[data-day='1']"))
        assert checked() == "Tue", "the strip follows the cursor while moving"
        body.send_keys(Keys.ESCAPE)
        WebDriverWait(app.d, 10).until(lambda d: checked() == "Wed")
        assert app.d.execute_script(
            "return JSON.stringify(JSON.parse("
            "localStorage.getItem('cmitt.v1.prefs')).plan_view);") == stored, \
            "a cancelled move must not have changed the stored day"
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
    app.xpath("//div[@aria-label='Day view']//button[normalize-space()='Mon']").click()
    chip = app.wait_css("section[aria-label='Lecture halls'] "
                        "button.chip[aria-label^='SVA,']")
    assert chip.find_elements(By.CSS_SELECTOR, ".sel-mark"), \
        "the ✓ the page promises must appear on a course you are taking"
    assert "in your timetable" in chip.get_attribute("aria-label")

    # Unselect it and the mark goes, so the mark still means what it says.
    app.boot("/", raw_snapshot=snapshot_with_a_room_and_no_class())
    app.open_tab("Halls")
    app.wait_css("section[aria-label='Lecture halls']")
    app.xpath("//div[@aria-label='Day view']//button[normalize-space()='Mon']").click()
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
    # A hash change on the same document — never a reload, or the session's
    # fetch log would empty and five tests would fail looking like fetch bugs.
    app.d.get(f"{BASE}/#/developer/sync")
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


def t89_the_digest_narrows_to_the_readers_own_courses(app):
    """A sync brings CMI's whole campus into the digest; one box narrows it
    to the reader's own week. The box is offered only when it can act — a
    sync that misses their courses entirely explains itself instead of
    handing over a control that could only empty the dialog — and the choice
    is a stored preference, so the next sync opens the way they left it."""
    serve_cmi()
    try:
        snap, _overrides, gone = cache_from_before_cmi_moved_toc()
        app.boot("/", selection=["TOC"], raw_snapshot=snap)
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        app.dismiss_toasts()
        app.xpath("//button[normalize-space()='See what changed']").click()
        dialog = app.wait_css(".dialog")
        # Wide open: CMI's new course, its dropped one, and the reader's TOC.
        assert "New courses" in dialog.text and gone in dialog.text, dialog.text
        everything = dialog.find_elements(By.CSS_SELECTOR, ".diff-item")
        assert len(everything) > 1, dialog.text
        box = dialog.find_element(
            By.CSS_SELECTOR, ".diff-filter input[type='checkbox']")
        assert not box.is_selected(), "the digest opens wide; the filter is opt-in"
        # A dialog you READ must not open with focus on the control that
        # decides what is in it — Space is how a tall list gets scrolled.
        assert app.d.switch_to.active_element != box, \
            "opening the digest must not land focus on the filter"
        box.click()
        WebDriverWait(app.d, 10).until(
            lambda d: len(d.find_elements(By.CSS_SELECTOR, ".diff-item"))
            < len(everything),
            message="ticking the box must drop what is not the reader's",
        )
        rows = app.css_all(".diff-item")
        assert rows and all("TOC" in r.text for r in rows), [r.text for r in rows]
        assert gone not in app.css(".dialog").text, \
            "a course nobody picked must go, dropped by CMI or not"
        # Every line left IS theirs, so the badge repeating that on each one
        # retires — the ticked box has said it once, at the top.
        assert "in your timetable" not in app.css(".dialog").text
        prefs = app.d.execute_script("return localStorage.getItem('cmitt.v1.prefs');")
        assert '"changes_mine_only":true' in prefs, prefs

        # Same update, a reader none of it touches: no box, and a line
        # saying why. The stored preference must NOT blank this digest —
        # the guard is in the dialog, not in what was saved.
        app.boot("/", selection=["MFD"], raw_snapshot=snap)
        app.d.execute_script(
            "const p = JSON.parse(localStorage.getItem('cmitt.v1.prefs'));"
            "p.changes_mine_only = true;"
            "localStorage.setItem('cmitt.v1.prefs', JSON.stringify(p));"
        )
        app.d.refresh()
        app.wait_css(".header h1")
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_toast("Timetable updated")
        app.dismiss_toasts()
        app.xpath("//button[normalize-space()='See what changed']").click()
        dialog = app.wait_css(".dialog")
        assert not dialog.find_elements(By.CSS_SELECTOR, ".diff-filter"), \
            "a box whose only result is an empty dialog must not be offered"
        assert "None of this touches the courses you've picked" in dialog.text, \
            dialog.text
        assert "TOC" in dialog.text, \
            "with nothing of theirs to narrow to, the digest stays whole"
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
    meetings, credit provenance, and a `my_changes` half a program can read
    without knowing this app), export the whole planner as one backup file,
    wipe the browser, import the backup — the selection, the custom move AND
    the catalog are all back, and the pill honestly says 'imported' with the
    ORIGINAL fetch date's age."""
    app.boot("/", selection=["TOC", "RDBM"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='Share or import']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    time.sleep(1.0)
    with open(newest_download("cmi-timetable-"), encoding="utf-8") as f:
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

    # The changes half, read the way another program would: every list
    # present, every change saying in a word what kind it is, every time
    # giving both the number to compute with and the string to read.
    changes = tt["my_changes"]
    for key in ("meeting_changes", "credit_changes", "my_own_courses"):
        assert isinstance(changes[key], list), f"{key} is always a list"
    move = next(c for c in changes["meeting_changes"] if c["course"] == "TOC")
    assert move["kind"] == "moved", move
    assert move["from"]["day"] == "Tue" and move["from"]["iso_weekday"] == 2
    assert move["to"]["start"]["minutes"] == 1020
    assert move["to"]["start"]["hhmm"] == "17:00"
    assert move["made_at"].endswith("Z") and isinstance(move["made_at_ms"], (int, float))
    assert changes["credit_changes"][0] == {
        "course": "TOC", "credits": 3,
        "made_at": "2025-07-31T22:13:20Z", "made_at_ms": 1754000000000.0,
    }, changes["credit_changes"]
    # A whole day's timetable, computed from the file alone — the shape has
    # to support arithmetic, not just display.
    wednesday = sorted(
        (m["start"]["minutes"], c["code"])
        for c in tt["courses"] for m in c["meetings"] if m["iso_weekday"] == 3)
    assert (1020, "TOC") in wednesday, wednesday

    # The whole-planner backup lives in the same dialog now — third section,
    # low enough to sit beneath the sticky footer at this scroll position,
    # so bring it to the middle first, like a reader would.
    everything = dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Export everything']")
    app.d.execute_script(
        "arguments[0].scrollIntoView({block: 'center'});", everything)
    everything.click()
    time.sleep(1.0)
    bak_path = newest_download("cmi-planner-")
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
    """'Import my courses…' under Share reads a timetable file and asks — in
    whole sentences — whether it joins the current timetable or replaces it.
    A file from before changes travelled (format 1.0.0, no `my_changes`)
    still imports as the course list it always was. Codes the catalog
    doesn't know are named and left out; an empty timetable skips the
    question; and either answer is one undoable step."""
    crafted = os.path.join(DOWNLOADS, "crafted-import.json")
    with open(crafted, "w", encoding="utf-8") as f:
        # " BOGUS9" (leading space) after "BOGUS9" pins the parser's dedup:
        # trim happens BEFORE the duplicate check, so a whitespace variant
        # can't get the same code named twice in "Left out".
        json.dump({"format": "cmi-timetable-export", "format_version": "1.0.0",
                   "courses": [{"code": "MFD"}, {"code": "TOC"},
                               {"code": "BOGUS9"}, {"code": " BOGUS9"}]}, f)

    app.boot("/", selection=["TOC", "QCOM"])

    def send_import():
        app.xpath("//button[normalize-space()='Share or import']").click()
        dialog = app.wait_css(".dialog")
        dialog.find_element(
            By.XPATH, ".//button[normalize-space()='Import my courses…']").click()
        file_input = WebDriverWait(app.d, 10).until(
            lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input"))
        file_input.send_keys(crafted)

    def wait_ask():
        return WebDriverWait(app.d, 10).until(
            lambda d: app.css(".dialog") if "A timetable from a file"
            in app.css(".dialog").text else None)

    # Add it: MFD joins, TOC is recognised as already there, BOGUS9 is named
    # and left out. A file with no changes says so by not mentioning any.
    send_import()
    ask = wait_ask()
    assert "2 courses on that timetable" in ask.text, ask.text
    assert "Left out: BOGUS9" in ask.text, ask.text
    assert ask.text.count("BOGUS9") == 1, \
        f"a whitespace-variant duplicate must be deduped, not named twice: {ask.text}"
    assert "moved, added or struck out" not in ask.text, \
        f"a course-list file must not claim changes it doesn't carry: {ask.text}"
    ask.find_element(
        By.XPATH, ".//button[contains(.,'Add it to my timetable')]").click()
    app.wait_toast("Added 1 course from the file")
    for code in ("TOC", "QCOM", "MFD"):
        app.wait_css(f"button.chip[aria-label^='{code},']")

    # Replace: the timetable becomes exactly the file's two.
    send_import()
    wait_ask().find_element(
        By.XPATH, ".//button[contains(.,'Replace my timetable with it')]").click()
    app.wait_toast("Your timetable now has exactly the 2 courses from that file.")
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    WebDriverWait(app.d, 10).until(
        lambda d: not app.css_all("button.chip[aria-label^='QCOM,']"),
        message="QCOM must be gone after Replace")

    # An empty timetable with nothing of its own skips the question — there
    # is nothing to replace, and joining an empty week is the same act.
    app.xpath("//button[normalize-space()='My data']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Clear selection']").click()
    app.wait_toast("Your timetable is empty now")
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    app.wait_gone(".dialog")
    send_import()
    app.wait_toast("Added 2 courses from the file.")
    assert "A timetable from a file" not in app.css("body").text, \
        "an empty selection must not be asked what to replace"

    # Importing the same file AGAIN changes nothing — and must say so
    # without spending an undo step: Ctrl+Z afterwards undoes the real add
    # above (the chips leave), not a phantom "nothing" step that would have
    # eaten the redo history.
    send_import()
    wait_ask().find_element(
        By.XPATH, ".//button[contains(.,'Add it to my timetable')]").click()
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
        assert "One timetable change" in banner.text, banner.text
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
            "//div[@role='radiogroup' and @aria-label='Day view']"
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


def t93_the_grid_picks_its_row_height_from_the_screen(app):
    """Nobody has chosen a row height, so the screen decides: tight on a
    phone, roomy on a computer. Same build, same storage, two window
    sizes."""
    app.d.set_window_size(430, 900)
    try:
        app.boot("/")
        app.open_tab("Master grid")
        app.wait_css("section[aria-label='Master grid'] table.tt")
        assert "Rows: tight" in app.xpath("//button[contains(.,'Rows:')]").text
        assert app.css_all("section[aria-label='Master grid'] .density-compact"), \
            "a phone should get the compact grid"
    finally:
        app.d.set_window_size(1500, 1000)

    app.boot("/")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    assert "Rows: roomy" in app.xpath("//button[contains(.,'Rows:')]").text
    assert not app.css_all("section[aria-label='Master grid'] .density-compact"), \
        "a computer should get the roomy grid"


def t94_a_chosen_row_height_is_never_overruled(app):
    """Once you press the button the app stops guessing — your choice holds
    on any screen and across reloads. Reset hands the decision back."""
    app.boot("/")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    app.xpath("//button[contains(.,'Rows:')]").click()   # roomy -> tight
    WebDriverWait(app.d, 5).until(
        lambda d: "Rows: tight" in app.xpath("//button[contains(.,'Rows:')]").text
    )
    # It survives a reload on the SAME desktop window, where the device
    # default would say roomy.
    app.boot("/", fresh=False)
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    assert "Rows: tight" in app.xpath("//button[contains(.,'Rows:')]").text, \
        "a chosen density must outlive a reload"

    # …and the reverse choice holds on a phone, where the device would say
    # tight. This is the half that proves the default never overrules.
    app.xpath("//button[contains(.,'Rows:')]").click()   # tight -> roomy
    WebDriverWait(app.d, 5).until(
        lambda d: "Rows: roomy" in app.xpath("//button[contains(.,'Rows:')]").text
    )
    app.d.set_window_size(430, 900)
    try:
        app.boot("/", fresh=False)
        app.open_tab("Master grid")
        app.wait_css("section[aria-label='Master grid'] table.tt")
        assert "Rows: roomy" in app.xpath("//button[contains(.,'Rows:')]").text, \
            "a phone must not overrule a density the user chose"
    finally:
        app.d.set_window_size(1500, 1000)

    # Reset gives the decision back to the device: on this desktop, roomy —
    # and, crucially, nothing stored, so a phone would say tight again.
    app.boot("/", fresh=False)
    app.xpath("//button[normalize-space()='My data']").click()
    app.wait_css(".dialog")
    reset = app.xpath("//div[@class='dialog']//button[normalize-space()='Reset']")
    # Scrolled to the MIDDLE of the dialog, then clicked for real. WebDriver's
    # own scroll-into-view stops at the nearest edge, which in this dialog can
    # be under the sticky actions footer — the click then hits the footer and
    # the failure reads as a missing button. (Whose Reset lands there depends on
    # how many sections the dialog has; R73 added one.)
    app.d.execute_script("arguments[0].scrollIntoView({block: 'center'});", reset)
    time.sleep(0.2)
    reset.click()
    app.dismiss_toasts()
    stored = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.prefs') || '{}').density;")
    assert stored is None, f"Reset should forget the choice, not pin one; got {stored!r}"


def t90_a_grid_chip_shows_the_tick_without_being_rebuilt(app):
    """Clicking a chip in the Master grid marks it — on the chip that is
    already there, not a replacement for it.

    The grid feeds its cells from a memo keyed on what it draws, so a click
    that adds no ⚠ anywhere leaves every chip mounted: the ✓, the ring and
    the aria hint can only come from the chip's own selection state. Filter
    the grid to a single course and there is nothing else to change, so
    holding the element handle across the click proves both halves — the
    mark appeared, and the node was never replaced. t23 cannot see this: it
    boots with the selection already in the URL."""
    app.boot("/")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    # The filter bar's own box, by its aria-label: every facet menu carries a
    # search input of its own, so `.filterbar input[type=search]` is eight
    # elements. And the needle is the NAME, not the code — "TOC" also matches
    # S-toc-hastic Processes.
    search = app.css("section[aria-label='Master grid'] input[aria-label='Search courses']")
    search.send_keys("Theory of Comp")

    def only_toc_is_drawn(_):
        chips = app.css_all("section[aria-label='Master grid'] button.chip")
        return bool(chips) and all(
            c.get_attribute("aria-label").startswith("TOC,") for c in chips
        )

    # One course, though it may hold several classes in the week — what
    # matters is that no OTHER course is on the grid to gain or lose a ⚠,
    # which is what would rebuild the table and hide the thing being tested.
    WebDriverWait(app.d, 10).until(
        only_toc_is_drawn, message="the search should leave only TOC on the grid"
    )
    chip = app.css("section[aria-label='Master grid'] button.chip")
    assert not chip.find_elements(By.CSS_SELECTOR, ".sel-mark")
    chip.click()
    app.wait_toast("Added TOC")
    # Same handle: a StaleElementReferenceException here would mean the cell
    # was torn down and rebuilt, and this assertion would be testing nothing.
    WebDriverWait(app.d, 5).until(
        lambda d: "selected" in chip.get_attribute("class"),
        message="the ring must appear on the chip that was clicked",
    )
    assert chip.find_elements(By.CSS_SELECTOR, ".sel-mark"), "the ✓ must appear too"
    assert "in your timetable" in chip.get_attribute("aria-label")


def t97_a_planner_with_nothing_in_it_is_never_asked_what_to_replace(app):
    """Importing asks "replace or merge?" only when there is something to
    replace. A browser that has synced but has no courses, no changes and
    no courses of its own has set nothing up — the question would have one
    answer — so the timetable file applies straight away and the whole
    backup loads without a confirm. The moment anything IS set up, both ask
    again."""
    def assert_no_confirm():
        assert not app.css_all(".dialog.confirm"), \
            f"a first-run import must not confirm: {app.css('.dialog.confirm').text}"

    def wait_for_confirm(seconds=10):
        """Reading the chosen file is asynchronous (FileReader), so the
        question arrives a few ms after the path is handed over. Say what DID
        happen when nothing comes, so a missing confirm is never a mystery."""
        deadline = time.time() + seconds
        while time.time() < deadline:
            found = app.css_all(".dialog.confirm")
            if found and found[0].is_displayed():
                return found[0]
            time.sleep(0.2)
        raise AssertionError(
            "a browser with courses in it must be asked before a backup "
            f"replaces them; toasts instead: {app.toasts_text()!r}")

    # --- make both kinds of file, from a browser that HAS set things up ---
    app.boot("/", selection=["TOC", "RDBM"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='Share or import']").click()
    dialog = app.wait_css(".dialog")
    dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    time.sleep(1.0)
    week_file = os.path.join(DOWNLOADS, "first-run-week.json")
    os.rename(newest_download("cmi-timetable-"), week_file)
    everything = dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Export everything']")
    app.d.execute_script("arguments[0].scrollIntoView({block: 'center'});", everything)
    everything.click()
    time.sleep(1.2)
    backup_file = os.path.join(DOWNLOADS, "first-run-backup.json")
    os.rename(newest_download("cmi-planner-"), backup_file)

    def open_share_and_import(label, path):
        app.xpath("//button[normalize-space()='Share or import']").click()
        app.wait_css(".dialog").find_element(
            By.XPATH, f".//button[normalize-space()='{label}']").click()
        WebDriverWait(app.d, 10).until(
            lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input")
        ).send_keys(path)

    # --- a synced but untouched browser: no question, either way ----------
    # It HAS the timetable downloaded from CMI, which the old rule counted
    # as something to lose. It isn't: a sync fetches that again.
    app.boot("/", selection=[])
    assert app.css_all(".welcome-card") == [], "seeded: the catalog is here"
    open_share_and_import("Import my courses…", week_file)
    app.wait_toast("Added 2 courses from the file")
    assert_no_confirm()
    assert "A timetable from a file" not in app.css("body").text, \
        "nothing was set up, so there was nothing to ask about"

    app.boot("/", selection=[])
    open_share_and_import("Import everything…", backup_file)
    # No confirm: the backup writes and reloads straight into its state.
    app.wait_css(".tabs .tab", timeout=20)

    def pill_says_imported(_d):
        # The import replaces storage and navigates, so anything found here
        # can go stale between the find and the read. `until` ignores only
        # NoSuchElementException, and a StaleElementReference thrown from
        # inside the predicate ends the wait instead of retrying it.
        try:
            return "imported" in app.css(".sync-pill").text
        except Exception:
            return False

    WebDriverWait(app.d, 20).until(
        pill_says_imported,
        message="after a backup import the pill must say the data came from a file")
    app.wait_css("button.chip[aria-label^='TOC,']")

    # --- and once something IS set up, both ask again ---------------------
    app.boot("/", selection=["QCOM"])
    open_share_and_import("Import my courses…", week_file)
    ask = WebDriverWait(app.d, 10).until(
        lambda d: app.css(".dialog") if "A timetable from a file"
        in app.css(".dialog").text else None)
    assert "Replace my timetable with it" in ask.text, ask.text
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    app.wait_gone(".dialog")

    open_share_and_import("Import everything…", backup_file)
    ask = wait_for_confirm()
    assert "Replace everything with this file?" in ask.text, ask.text
    # The question now counts both sides of the trade rather than saying
    # "your courses and settings" whatever the browser happens to hold.
    assert "2 courses" in ask.text, ask.text
    assert "1 course here is replaced" in ask.text, ask.text
    app.answer_confirm(False)
    app.wait_gone(".dialog.confirm")
    app.wait_css("button.chip[aria-label^='QCOM,']")


def t95_two_students_combine_their_timetables(app):
    """The point of the file: one student exports their week — courses, a
    moved class, a credit correction and a course CMI never listed — and
    another adds it to theirs in one step. Nothing of the reader's is lost,
    everything of the sender's that this browser can take arrives, and the
    sentence afterwards says how much came with it."""
    sender_file = os.path.join(DOWNLOADS, "sender-week.json")

    # --- the sender's browser -------------------------------------------
    app.boot("/", selection=["TOC", "RDBM", "GERMAN"],
             overrides=TOC_OVR, customs=HALL_CUSTOM)
    app.xpath("//button[normalize-space()='Share or import']").click()
    dialog = app.wait_css(".dialog")
    assert "the classes you moved" in dialog.text, \
        f"Share must say what the file carries: {dialog.text}"
    dialog.find_element(
        By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    app.wait_toast("your changes included")
    time.sleep(1.0)
    os.rename(newest_download("cmi-timetable-"), sender_file)

    # --- the reader's browser, with a week of its own -------------------
    app.boot("/", selection=["QCOM"])
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Import my courses…']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input")
    ).send_keys(sender_file)

    ask = WebDriverWait(app.d, 10).until(
        lambda d: app.css(".dialog") if "A timetable from a file"
        in app.css(".dialog").text else None)
    # The bill of contents is counted before anything is decided.
    assert "3 courses on that timetable" in ask.text, ask.text
    assert "1 class moved, added or struck out" in ask.text, ask.text
    assert "1 credit correction" in ask.text, ask.text
    assert "1 of them added by hand, not from CMI's catalog" in ask.text, ask.text
    ask.find_element(
        By.XPATH, ".//button[contains(.,'Add it to my timetable')]").click()
    app.wait_toast("2 changes came with them")

    # The reader keeps their own course, gains the sender's three, and the
    # sender's move arrives with them: TOC sits on Wednesday 17:00, where
    # the sender put it — not on CMI's Tuesday.
    for code in ("QCOM", "TOC", "RDBM", "GERMAN"):
        app.wait_css(f"button.chip[aria-label^='{code},']")
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    # A course CMI never listed came over whole — name and times, not a code.
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert "German A1" in section.text, section.text

    # And it is ONE undo step: Ctrl+Z puts the reader's week back exactly.
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.CONTROL, "z")
    WebDriverWait(app.d, 10).until(
        lambda d: not app.css_all("button.chip[aria-label^='TOC,']"),
        message="one Ctrl+Z must undo the whole import")
    app.wait_css("button.chip[aria-label^='QCOM,']")


def t96_a_disagreement_over_one_class_keeps_your_own(app):
    """Two people moved the SAME class to different times. A class cannot be
    in two places at once, so the reader's own stays and the file's loss is
    named. Replacing instead takes the file's version whole."""
    sender_file = os.path.join(DOWNLOADS, "contested-week.json")
    # The sender moved TOC's Tuesday class to Wednesday 17:00 (TOC_OVR).
    app.boot("/", selection=["TOC"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    time.sleep(1.0)
    os.rename(newest_download("cmi-timetable-"), sender_file)

    # The reader moved the same Tuesday class to Friday 15:30 instead. (Not
    # to a slot TOC already meets in — TOC officially meets Tue AND Thu, and
    # a target that lands on its own second class would prove nothing.)
    mine = json.loads(json.dumps(TOC_OVR))
    mine["items"][0]["to"] = {"day": "Fri",
                              "slot": {"start_min": 930, "end_min": 1005},
                              "hall": "Lecture Hall 803", "temp_booking": False}
    mine["credits"] = []
    app.boot("/", selection=["TOC"], overrides=mine)

    def import_file():
        app.xpath("//button[normalize-space()='Share or import']").click()
        app.wait_css(".dialog").find_element(
            By.XPATH, ".//button[normalize-space()='Import my courses…']").click()
        WebDriverWait(app.d, 10).until(
            lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input")
        ).send_keys(sender_file)
        return WebDriverWait(app.d, 10).until(
            lambda d: app.css(".dialog") if "A timetable from a file"
            in app.css(".dialog").text else None)

    # TOC's own weekly classes, as the reader's week already draws them —
    # the number that must not grow. A losing move kept "as well" would add
    # a class to the week, which is the failure this test exists for.
    app.wait_css("td[data-day='4'][data-slot='930'] button.chip[aria-label^='TOC,']")
    before = len(app.css_all(".week-grid button.chip[aria-label^='TOC,']"))

    import_file().find_element(
        By.XPATH, ".//button[contains(.,'Add it to my timetable')]").click()
    # The credit correction had nowhere to lose, so it lands; the move meets
    # the reader's own and is refused BY NAME.
    app.wait_toast("You had already changed TOC")
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    # Friday, the reader's choice — and not a class more than before.
    app.wait_css("td[data-day='4'][data-slot='930'] button.chip[aria-label^='TOC,']")
    assert not app.css_all(
        "td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']"), \
        "the file's losing move must not appear as a second class"
    assert len(app.css_all(".week-grid button.chip[aria-label^='TOC,']")) == before

    # Replace takes the file's week whole: TOC goes back to the sender's
    # Wednesday, and the reader's Friday is gone.
    import_file().find_element(
        By.XPATH, ".//button[contains(.,'Replace my timetable with it')]").click()
    app.wait_toast("Your timetable now has exactly the one course from that file.")
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    WebDriverWait(app.d, 10).until(
        lambda d: not app.css_all(
            "td[data-day='4'][data-slot='930'] button.chip[aria-label^='TOC,']"),
        message="Replace must drop the reader's competing move")


def t98_a_file_never_argues_with_itself(app):
    """The reported bug, end to end. A file can hold two changes to ONE class
    — data written before the master grid stopped making them still does —
    and reading it must not be read as the READER disagreeing with it. The
    reader here has an empty override store (a share link's selection and one
    course of its own, nothing moved), so any sentence claiming a change of
    theirs was kept is a claim about something that never happened."""
    sender_file = os.path.join(DOWNLOADS, "self-disagreeing-week.json")

    # The sender's browser: one class, moved twice, both changes still
    # standing against the same official meeting.
    twice = json.loads(json.dumps(TOC_OVR))
    twice["items"].append({
        "id": 1, "course": "TOC",
        "base": twice["items"][0]["base"],
        "to": {"day": "Sat", "slot": {"start_min": 550, "end_min": 625},
               "hall": "Lecture Hall 803", "temp_booking": False},
        "created_at": 1754000023000.0})
    twice["next_id"] = 2
    app.boot("/", selection=["TOC"], overrides=twice)
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    time.sleep(1.0)
    os.rename(newest_download("cmi-timetable-"), sender_file)

    # The reader's browser: courses and a course of its own, but nothing
    # moved, added, struck out or re-credited anywhere.
    app.boot("/", selection=["QCOM", "GERMAN"], customs=HALL_CUSTOM)
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Import my courses…']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input")
    ).send_keys(sender_file)
    ask = WebDriverWait(app.d, 10).until(
        lambda d: app.css(".dialog") if "A timetable from a file"
        in app.css(".dialog").text else None)
    ask.find_element(
        By.XPATH, ".//button[contains(.,'Add it to my timetable')]").click()

    app.wait_toast("Added 1 course from the file")
    toast = app.toasts_text()
    assert "already changed" not in toast, \
        f"nothing of the reader's was changed, so nothing of theirs was kept: {toast}"
    assert "3 changes came with it" in toast, \
        f"both moves and the credit correction travel: {toast}"
    # Both of the sender's placements arrive; neither is dropped as a
    # "disagreement" with the other.
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    app.wait_css("td[data-day='2'][data-slot='1020'] button.chip[aria-label^='TOC,']")
    app.wait_css("td[data-day='5'][data-slot='550'] button.chip[aria-label^='TOC,']")


def t99_the_same_file_twice_says_what_it_refused(app):
    """"Nothing changed" and "it was all already here" are different
    outcomes. A file whose courses are all present and whose only change
    loses to a change of the reader's own leaves the state untouched — and
    must not report that the changes were already here. They were refused."""
    sender_file = os.path.join(DOWNLOADS, "refused-week.json")
    app.boot("/", selection=["TOC"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    time.sleep(1.0)
    os.rename(newest_download("cmi-timetable-"), sender_file)

    # Same course, same credit correction, the class moved somewhere else.
    mine = json.loads(json.dumps(TOC_OVR))
    mine["items"][0]["to"] = {"day": "Fri",
                              "slot": {"start_min": 930, "end_min": 1005},
                              "hall": "Lecture Hall 803", "temp_booking": False}
    app.boot("/", selection=["TOC"], overrides=mine)
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Import my courses…']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input")
    ).send_keys(sender_file)
    ask = WebDriverWait(app.d, 10).until(
        lambda d: app.css(".dialog") if "A timetable from a file"
        in app.css(".dialog").text else None)
    ask.find_element(
        By.XPATH, ".//button[contains(.,'Add it to my timetable')]").click()

    app.wait_toast("nothing changed")
    toast = app.toasts_text()
    assert "the courses and the changes both" not in toast, \
        f"the file's change was refused, not already here: {toast}"
    assert "TOC" in toast and "yours stayed" in toast, toast


def t100_replacing_twice_with_one_file_is_one_change(app):
    """"Replace" clears the file's courses and takes the file's copies back,
    so the second run of the same file leaves an identical week under ids one
    higher. That is not a change: no undo step is spent on it, and the
    sentence must not count changes that were already here."""
    sender_file = os.path.join(DOWNLOADS, "replace-twice.json")
    app.boot("/", selection=["TOC"], overrides=TOC_OVR)
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    time.sleep(1.0)
    os.rename(newest_download("cmi-timetable-"), sender_file)

    app.boot("/", selection=["QCOM"])

    def replace_with_file():
        app.xpath("//button[normalize-space()='Share or import']").click()
        app.wait_css(".dialog").find_element(
            By.XPATH, ".//button[normalize-space()='Import my courses…']").click()
        WebDriverWait(app.d, 10).until(
            lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input")
        ).send_keys(sender_file)
        WebDriverWait(app.d, 10).until(
            lambda d: app.css(".dialog") if "A timetable from a file"
            in app.css(".dialog").text else None).find_element(
            By.XPATH, ".//button[contains(.,'Replace my timetable with it')]").click()

    replace_with_file()
    app.wait_toast("2 changes came with it")
    app.dismiss_toasts()
    undos_before = len(app.css_all("button[aria-label='Undo']"))

    replace_with_file()
    app.wait_toast("so nothing changed")
    toast = app.toasts_text()
    assert "came with it" not in toast, \
        f"the second run brought nothing, so it counts nothing: {toast}"
    # And it spent no undo step: one Ctrl+Z is still the pre-import week.
    app.dismiss_toasts()
    assert len(app.css_all("button[aria-label='Undo']")) == undos_before
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.CONTROL, "z")
    app.wait_css("button.chip[aria-label^='QCOM,']")


def t101_an_import_that_undeletes_a_course_says_so(app):
    """A course deleted from this planner cannot be on the timetable and
    deleted at once, so a file bringing it back undoes the deletion. That is
    the reader's own work being reversed: it is named before the question and
    again in the sentence after it, and the answer promising to take nothing
    away stops promising."""
    sender_file = os.path.join(DOWNLOADS, "brings-back-mfd.json")
    app.boot("/", selection=["MFD"])
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Export my courses']").click()
    time.sleep(1.0)
    os.rename(newest_download("cmi-timetable-"), sender_file)

    app.boot("/", selection=["QCOM"], overrides={
        "next_id": 0, "items": [], "credits": [],
        "hidden": [{"course": "MFD", "created_at": 1754000000000.0,
                    "was_selected": True}]})
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".dialog").find_element(
        By.XPATH, ".//button[normalize-space()='Import my courses…']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: d.find_element(By.CSS_SELECTOR, "#cmitt-import-input")
    ).send_keys(sender_file)
    ask = WebDriverWait(app.d, 10).until(
        lambda d: app.css(".dialog") if "A timetable from a file"
        in app.css(".dialog").text else None)
    assert "You deleted MFD" in ask.text, \
        f"the reversal is named before it happens: {ask.text}"
    join = ask.find_element(
        By.XPATH, ".//button[contains(.,'Add it to my timetable')]")
    assert "Nothing of yours is taken away" not in join.text, \
        f"it does take something: {join.text}"
    join.click()

    app.wait_toast("MFD was deleted here, and the file put it back")
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    app.wait_css("button.chip[aria-label^='MFD,']")


def t102_credit_note_names_what_your_own_number_replaced(app):
    """A number of your own does not always stand in for CMI's. Where CMI
    lists no credits it stands in for the app's guess — the course's own card
    says so — and the summary above it has to agree. t17 covers the
    guess-only case; this covers the CMI-listed case and the mixed one."""
    def set_credits(code, value):
        app.chip(code, "section[aria-label='My courses']").click()
        dialog = app.wait_css(".dialog")
        dialog.find_element(
            By.XPATH, ".//button[normalize-space()='Edit this course']").click()
        app.wait_css(".dialog .course-form")
        app.xpath(
            f"//div[contains(@class,'seg')]/button[normalize-space()='{value}']").click()
        app.xpath(
            "//div[@class='dialog']//button[normalize-space()='Save changes']").click()
        app.wait_toast(f"Saved your changes to {code}")
        app.wait_gone(".dialog")

    app.boot("/?c=TOC,RDBM")
    app.open_tab("My courses")
    summary = "section[aria-label='My courses'] .credit-summary"

    # RDBM is a course CMI does publish credits for (2), so overriding it is
    # the one case where "not CMI's" is the true sentence.
    set_credits("RDBM", "3")
    text = app.css(summary).text
    assert "You set the credits on one course yourself" in text, text
    assert "not CMI's" in text, text
    assert "the app's guess" not in text, text

    # Now one of each. The note has to cover both without claiming CMI
    # published a figure for the course it published nothing for.
    set_credits("TOC", "1")
    text = app.css(summary).text
    assert "You set the credits on 2 courses yourself" in text, text
    assert "in place of CMI's where CMI lists them" in text, text
    assert "of the app's guess where it doesn't" in text, text


def t103_putting_a_class_back_where_it_was_changes_nothing(app):
    """A drop that moves nothing must say nothing. The check used to compare
    the drop against CMI's OFFICIAL cell, so a class you had already moved
    failed it and every re-drop announced a move, spent an undo step and
    cleared the redo stack — for a chip that never left its cell."""
    app.boot("/?c=TOC")
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    app.xpath("//button[contains(.,'Edit layout')]").click()

    # Move it once, for real.
    app.drag(app.chip("TOC", "td[data-day='1'][data-slot='550']"),
             app.cell(2, 1020))
    app.wait_toast("Moved TOC")
    changes_after_real_move = app.xpath("//button[contains(.,'change')]").text
    app.dismiss_toasts()

    # Now pick it up and put it straight back down in the SAME cell.
    app.drag(app.chip("TOC", "td[data-day='2'][data-slot='1020']"),
             app.cell(2, 1020))
    time.sleep(0.6)
    assert "Moved" not in app.toasts_text(), \
        f"a drop that moved nothing must not report a move: {app.toasts_text()!r}"
    assert app.xpath("//button[contains(.,'change')]").text == changes_after_real_move, \
        "a no-op drop must not add a change"
    # And it is still exactly where it was.
    assert app.chips("TOC", "td[data-day='2'][data-slot='1020']"), \
        "the chip must stay in the cell it was dropped back into"

    # Redo survives too: `act` clears the redo stack, so a phantom change
    # after an undo used to make the redo unreachable.
    app.d.find_element(By.CSS_SELECTOR, "body").send_keys(Keys.CONTROL, "z")
    app.wait_css("button.chip[aria-label^='TOC,']")
    app.drag(app.chip("TOC", "td[data-day='1'][data-slot='550']"),
             app.cell(1, 550))
    time.sleep(0.5)
    redo = app.xpath("//button[@aria-label='Redo']")
    assert redo.is_enabled(), "a no-op drop must not clear the redo stack"


def t104_catalog_row_can_delete_a_course(app):
    """The catalog offers the stronger action beside Add/Remove: Delete takes
    the course out of the catalog, the master grid and the timetable, and it
    is recorded under Your changes so it can be put back."""
    app.boot("/?c=TOC")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .card")

    def rows():
        return app.css_all("section[aria-label='Catalog'] .card")

    def catalog_row_for(code):
        for r in rows():
            if code in r.text:
                return r
        raise AssertionError(f"no catalog row for {code}")

    qcom = catalog_row_for("QCOM")
    delete = qcom.find_element(By.XPATH, ".//button[normalize-space()='Delete']")
    assert "danger" in delete.get_attribute("class"), \
        "anything that takes something away wears red"
    # Its neighbour is untouched and still first.
    assert qcom.find_elements(By.XPATH, ".//button[normalize-space()='Add']"), \
        "Delete must sit beside Add, not replace it"
    delete.click()

    app.wait_toast("QCOM")
    # Gone from the catalog it was listed in.
    WebDriverWait(app.d, 10).until(
        lambda d: all("QCOM" not in r.text for r in rows()),
        message="a deleted course must leave the catalog")
    # And recorded, so it can come back.
    app.open_tab("My timetable")
    panel = app.wait_css("[data-testid='your-changes']")
    assert "QCOM" in panel.text, f"the deletion must be listed: {panel.text}"


def t106_the_wheel_over_the_rail_walks_the_sections(app):
    """Turning the wheel over the list of sections moves between them — and
    while the pointer is there the page itself must not move at all, not
    between notches and not at the ends of the list."""
    app.boot("/", selection=["TOC", "RDBM", "SVA", "QCOM", "MFD"])
    rail = app.wait_css("nav.tabs")

    def selected():
        return app.css("nav.tabs button[aria-selected='true']").text

    def wheel(dy, times=1):
        for _ in range(times):
            app.d.execute_script(
                """const el = arguments[0];
                   const r = el.getBoundingClientRect();
                   el.dispatchEvent(new WheelEvent('wheel', {
                     deltaY: arguments[1], bubbles: true, cancelable: true,
                     clientX: r.left + r.width / 2,
                     clientY: r.top + r.height / 2}));""",
                el_rail, dy)
            time.sleep(0.15)

    el_rail = rail
    assert selected() == "My timetable"
    wheel(120)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "My courses")
    wheel(120)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Master grid")
    wheel(-120)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "My courses")

    # Stops at the ends rather than wrapping — a wheel is a continuous
    # gesture, so reappearing at the far end reads as a slip.
    wheel(-120, times=4)
    assert selected() == "My timetable", selected()
    wheel(120, times=8)
    assert selected() == "Halls", selected()

    # And through all of that the page never moved. A tall tab so there IS
    # something to scroll.
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    app.d.execute_script("window.scrollTo(0, 0);")
    time.sleep(0.2)
    el_rail = app.css("nav.tabs")
    before = app.d.execute_script("return window.scrollY;")
    wheel(150, times=6)
    time.sleep(0.3)
    after = app.d.execute_script("return window.scrollY;")
    assert after == before, \
        f"the page must not scroll while the wheel is over the rail: {before} -> {after}"


def _open_shorten(app):
    """The share dialog, then the one button in it that leads to shortening.
    Returns the full share link the popup is holding — which is the key every
    remembered short link is filed under, so tests plant links against it
    rather than hard-coding a payload that the encoder is free to change."""
    app.dismiss_toasts()
    app.xpath("//button[normalize-space()='Share or import']").click()
    app.wait_css(".share-shorten button")
    buttons = app.css_all(".share-shorten button")
    assert len(buttons) == 1, \
        f"the share dialog must carry exactly ONE shortening control, found {len(buttons)}"
    buttons[0].click()
    app.wait_css(".shorten-dialog")
    return app.css(".shorten-long input").get_attribute("value")


def _plant_short_links(app, entries):
    """Write short links into storage the way a successful generate would.
    The e2e browser has every non-localhost name blackholed, so no test can
    (or should) depend on a shortener being up to check that the app
    remembers what it was told."""
    app.d.execute_script(
        "localStorage.setItem('cmitt.v1.shortlinks', arguments[0]);",
        json.dumps(entries))
    # Reloaded, not re-booted: `boot` navigates to a bare path, and the app
    # also keeps the selection in the address bar — so a fresh navigation
    # would build a DIFFERENT share link and none of the planted entries
    # would match.
    app.d.refresh()
    app.wait_css(".header h1")


def t107_a_short_link_is_remembered_for_each_service(app):
    """A link, once made, is still there when you come back — per service,
    and after a reload. Making one costs a request to a stranger and is a
    permanent redirect once made; the popup used to throw it away the moment
    it was closed, so the only way to see it again was to pay for it again."""
    app.boot("/", selection=["TOC", "RDBM"])
    long = _open_shorten(app)
    assert app.css(".shorten-empty"), "a browser that has shortened nothing shows nothing"

    _plant_short_links(app, [
        {"service": "dagd", "long": long, "short": "https://da.gd/zzz"},
        {"service": "tinyurl", "long": long, "short": "https://tinyurl.com/abcd"},
    ])
    assert _open_shorten(app) == long, "the same timetable must build the same link"

    # The default service's link, shown without anyone being asked anything.
    shown = app.wait_css(".shorten-have .shorten-short")
    assert shown.get_attribute("value") == "https://tinyurl.com/abcd", \
        shown.get_attribute("value")
    ready = [b.text for b in app.css_all(".shorten-opt .shorten-ready")]
    assert len(ready) == 2, f"both services have a link for this timetable: {ready}"
    # And the button offers to spend another request, rather than pretending
    # there is nothing there.
    assert "again" in app.css(".shorten-dialog .actions button:last-child").text.lower()

    # Each service keeps its own, and switching between them costs nothing.
    radios = app.css_all(".shorten-opt input")
    radios[1].click()
    WebDriverWait(app.d, 5).until(
        lambda d: app.css(".shorten-have .shorten-short").get_attribute("value")
        == "https://da.gd/zzz")
    radios[2].click()
    WebDriverWait(app.d, 5).until(lambda d: app.css_all(".shorten-empty"))
    assert app.css(".shorten-dialog .actions button:last-child").text == \
        "Make it short", "a service with no link yet offers to make one"
    radios[0].click()
    WebDriverWait(app.d, 5).until(
        lambda d: app.css(".shorten-have .shorten-short").get_attribute("value")
        == "https://tinyurl.com/abcd")

    # Leaving the popup and coming back is the case the user reported.
    app.xpath("//div[contains(@class,'dialog')]//button[normalize-space()='Back']").click()
    app.wait_css(".share-dialog")
    app.xpath("//div[contains(@class,'dialog')]//button[normalize-space()='Close']").click()
    assert _open_shorten(app) == long
    assert app.css(".shorten-have .shorten-short").get_attribute("value") == \
        "https://tinyurl.com/abcd"


def t108_a_link_made_before_the_timetable_changed_is_not_offered_as_current(app):
    """The safety property. A short link is a permanent redirect to ONE
    address, so the link made yesterday points at yesterday's timetable.
    Remembering it is right; handing it back as though it were the answer
    would silently share the wrong courses."""
    app.boot("/", selection=["TOC", "RDBM"])
    long = _open_shorten(app)
    _plant_short_links(app, [
        # Filed under a link this timetable no longer builds.
        {"service": "tinyurl", "long": long + "&fromanothertime=1",
         "short": "https://tinyurl.com/older"},
    ])
    _open_shorten(app)

    assert app.css_all(".shorten-stale"), "an earlier link is shown, and said to be earlier"
    assert not app.css_all(".shorten-have"), "but never as the link for this timetable"
    assert "earlier" in app.css(".shorten-out").text.lower()
    assert app.css(".shorten-dialog .actions button:last-child").text == \
        "Make it short", "and the button offers to make the current one"
    # No "link ready" badge either: the chooser must not claim a link this
    # timetable does not have.
    assert not app.css_all(".shorten-opt .shorten-ready")


def t109_nothing_is_sent_until_the_button_is_pressed(app):
    """Opening the popup — or the share dialog behind it — must not shorten
    anything. This is the one action in the app that hands a student's
    timetable to a stranger, and it happens once per press or not at all."""
    app.boot("/", selection=["TOC", "RDBM"])
    _open_shorten(app)
    assert "hasn't left this browser" in app.css(".shorten-empty").text
    time.sleep(1.5)
    assert not app.css_all(".shorten-short"), "a link appeared without anyone asking for one"
    assert app.d.execute_script(
        "return localStorage.getItem('cmitt.v1.shortlinks');") is None


def t110_a_shortener_that_cannot_be_reached_says_so_and_invents_nothing(app):
    """Every name but localhost is blackholed here, so pressing Generate is
    a real request that really fails — the state a student meets on a train.
    It has to end in a sentence, not a spinner, and it must never leave a
    made-up link behind.

    TWO sentences, because the two failures are not alike (R82: the app used to
    say "couldn't be reached" for both, which sends a reader to check their
    wifi over a 429 only the service can fix):

    * the service ANSWERED badly — which is what this harness actually produces,
      since every host resolves to the stand-in server and it replies 503 —
      must read "answered with an error (HTTP 503)";
    * nothing came back at all — a blocked request, the train tunnel — must
      read "couldn't be reached"."""
    app.boot("/", selection=["TOC", "RDBM"])
    _open_shorten(app)
    app.css(".shorten-dialog .actions button:last-child").click()

    failed = WebDriverWait(app.d, 40).until(
        lambda d: d.find_elements(By.CSS_SELECTOR, ".shorten-failed") or False)
    text = failed[0].text
    assert "TinyURL" in text and "answered with an error" in text, text
    assert "HTTP" in text, f"the status earns its place in the sentence: {text}"
    assert "copy the full link instead" in text, text
    assert not app.css_all(".shorten-short"), "a failure must not produce a link"
    assert app.d.execute_script(
        "return localStorage.getItem('cmitt.v1.shortlinks');") is None
    # The fallback is still on screen, which is the whole reason it is there.
    assert app.css(".shorten-long input").get_attribute("value").startswith("http")
    # And the failure belongs to the service that failed: picking another one
    # shows a clean slate rather than TinyURL's bad news.
    app.css_all(".shorten-opt input")[1].click()
    WebDriverWait(app.d, 5).until(lambda d: app.css_all(".shorten-empty"))

    # The OTHER sentence: a request that never lands at all. Blocking the URL
    # outright is the closest thing here to a tunnel — the fetch rejects with
    # the browser's own exception instead of a status, and then, and only then,
    # "couldn't be reached" is the true thing to say.
    app.d.execute_cdp_cmd("Network.enable", {})
    app.d.execute_cdp_cmd("Network.setBlockedURLs", {"urls": ["*da.gd*"]})
    try:
        app.css(".shorten-dialog .actions button:last-child").click()
        blocked = WebDriverWait(app.d, 40).until(
            lambda d: d.find_elements(By.CSS_SELECTOR, ".shorten-failed") or False)
        gone = blocked[0].text
        assert "couldn't be reached" in gone, (
            "a request that never landed must not be reported as an answer: "
            f"{gone}")
        assert "HTTP" not in gone, \
            f"there was no status to quote: {gone}"
        assert not app.css_all(".shorten-short"), \
            "a blocked request must not produce a link either"
    finally:
        app.d.execute_cdp_cmd("Network.setBlockedURLs", {"urls": []})


def t111_the_chosen_service_is_warmed_and_only_the_chosen_one(app):
    """A first request to a host pays DNS, TCP and TLS before a byte of it is
    sent, and on a shortener that is most of the wait — 629ms cold against
    244ms warmed, for da.gd. So the popup opens the connection while the
    reader is still reading. Two things have to stay true: it happens for the
    service that is CHOSEN, and it does not happen for the others — a
    handshake sends no data, but it is still a connection to a company the
    reader did not pick."""
    def warmed():
        return app.d.execute_script(
            "return [...document.querySelectorAll(\"link[rel='preconnect']\")]"
            ".map(l => l.href).join(' ');")

    app.boot("/", selection=["TOC", "RDBM"])
    _open_shorten(app)
    WebDriverWait(app.d, 5).until(lambda d: "tinyurl.com" in warmed())
    assert "da.gd" not in warmed(), warmed()
    assert "clck.ru" not in warmed(), warmed()

    app.css_all(".shorten-opt input")[1].click()
    WebDriverWait(app.d, 5).until(lambda d: "da.gd" in warmed())
    assert "clck.ru" not in warmed(), \
        f"a service nobody picked must not be contacted: {warmed()}"

    # And it stays a hint, not a request: warming must never put a link on
    # screen or in storage.
    assert not app.css_all(".shorten-short")
    assert app.d.execute_script(
        "return localStorage.getItem('cmitt.v1.shortlinks');") is None

    # Coming back has to warm again. A browser acts on this hint when the
    # element is INSERTED, and the connection it opened is long closed by the
    # next visit — so leaving the old element in place would make every visit
    # after the first one the slow one. The old element must be gone.
    old = app.css("link[rel='preconnect'][href*='da.gd']")
    app.xpath("//div[contains(@class,'dialog')]//button[normalize-space()='Back']").click()
    app.wait_css(".share-dialog")
    app.xpath("//div[contains(@class,'dialog')]//button[normalize-space()='Close']").click()
    _open_shorten(app)
    WebDriverWait(app.d, 5).until(EC.staleness_of(old))
    assert app.css_all("link[rel='preconnect'][href*='da.gd']"), \
        "the service still chosen must be warmed again on the way back in"


def t112_a_chosen_day_view_survives_a_refresh(app):
    """On a phone My timetable opens on today, because that is the question
    a student asks their phone. But "opens on today" is a default, not an
    instruction: once the reader taps Week — or taps another day — a refresh
    must show what they tapped. It used to work the choice out fresh on
    every mount, so every reload undid it and answered a question they had
    already answered."""
    app.d.set_window_size(420, 850)
    try:
        app.boot("/", selection=["TOC", "RDBM", "MFD"])
        strip = app.wait_css(".toolbar .seg[aria-label='Day view']")

        def checked():
            return app.css(
                ".toolbar .seg[aria-label='Day view'] button[aria-checked='true']").text

        def button(label):
            return strip.find_element(
                By.XPATH, f".//button[normalize-space()='{label}']")

        # Untouched, it still opens on today (or on the week, at a weekend or
        # on a day CMI does not teach) — the default is not being removed.
        opened_on = checked()
        assert app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.prefs')).plan_view;") is None, \
            "opening the tab must not write a choice nobody made"

        # Week is a choice like any other. This is the reported bug.
        button("Week").click()
        WebDriverWait(app.d, 5).until(lambda d: checked() == "Week")
        app.d.refresh()
        app.wait_css(".toolbar .seg[aria-label='Day view']")
        assert checked() == "Week", \
            f"a refresh replaced the reader's choice with {checked()!r}"

        # And so is a particular day — including one that is not today.
        strip = app.css(".toolbar .seg[aria-label='Day view']")
        others = [b for b in strip.find_elements(By.CSS_SELECTOR, "button")
                  if b.text not in ("Week", opened_on)]
        assert others, "the fixture week must have a day that is not today"
        picked = others[-1].text
        others[-1].click()
        WebDriverWait(app.d, 5).until(lambda d: checked() == picked)
        app.d.refresh()
        app.wait_css(".toolbar .seg[aria-label='Day view']")
        assert checked() == picked, f"expected {picked}, got {checked()}"

        # The day list on screen is the one the strip claims.
        assert app.css_all(".day-list"), "a day view shows the day's list"

        # A choice made on a phone must not follow the reader to a screen
        # that has no day strip to change it with: the week grid fits there,
        # and a hidden day list is work done for nobody. The choice is kept,
        # not cleared — going back to phone width still shows it.
        app.d.set_window_size(1500, 1000)
        app.d.refresh()
        app.wait_css(".week-grid")
        assert not app.css_all(".day-list"), \
            "a wide screen shows the week, whatever the phone had chosen"
        assert app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.prefs')).plan_view;") is not None, \
            "and it must not have thrown the phone's choice away"

        app.d.set_window_size(420, 850)
        app.d.refresh()
        app.wait_css(".toolbar .seg[aria-label='Day view']")
        assert checked() == picked, f"back on a phone: expected {picked}, got {checked()}"
    finally:
        app.d.set_window_size(1500, 1000)


def t113_the_shortening_service_you_picked_is_the_one_you_come_back_to(app):
    """The same bug in another place, found by sweeping for it: the chosen
    shortener lived only in memory. A reader who prefers da.gd was handed
    TinyURL again after every reload — and since each service remembers its
    own links, they were shown a different service's link than the one they
    had been using."""
    app.boot("/", selection=["TOC", "RDBM"])
    _open_shorten(app)

    def picked():
        return app.css(".shorten-opt:has(input:checked) .shorten-opt-name").text

    assert picked().startswith("TinyURL"), picked()
    app.css_all(".shorten-opt input")[1].click()
    WebDriverWait(app.d, 5).until(lambda d: picked().startswith("da.gd"))

    app.d.refresh()
    app.wait_css(".header h1")
    _open_shorten(app)
    assert picked().startswith("da.gd"), \
        f"a refresh put the service back to {picked()!r}"


def _serve_dir(directory, port):
    """A server for one directory on one port — the update tests need to swap
    what a single origin serves, which is the whole point of them.

    Tracks every accepted socket, for the same reason t74 does: Chrome holds
    keep-alive (and speculative) connections open, and `server_close()` only
    closes the LISTENER. Without severing them, the "no network" phase is
    still quietly being served — which is exactly how this test first passed
    the wrong way, reporting "this is the newest version" while pretending to
    be offline.
    """
    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def end_headers(self):
            # Chrome's own HTTP cache would otherwise hand the second build's
            # request the first build's index.html, and the test would be
            # measuring the cache rather than the app.
            self.send_header("Cache-Control", "no-store")
            super().end_headers()

    class Tracked(http.server.ThreadingHTTPServer):
        def __init__(self, *a, **kw):
            super().__init__(*a, **kw)
            self.accepted = []

        def get_request(self):
            sock, addr = super().get_request()
            self.accepted.append(sock)
            return sock, addr

    server = Tracked(
        ("127.0.0.1", port),
        lambda *a, **kw: Quiet(*a, directory=directory, **kw),
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def _really_stop(server):
    """Stop listening AND sever every connection already open."""
    server.shutdown()
    server.server_close()
    for sock in getattr(server, "accepted", []):
        try:
            sock.shutdown(socket.SHUT_RDWR)
            sock.close()
        except OSError:
            pass


def _next_build(src):
    """A copy of the built app that is a DIFFERENT build.

    The stylesheet is renamed — content untouched, so its SRI hash still
    matches — and every reference to it rewritten, including the service
    worker's precache list and its cache name. That is exactly what a real
    deploy looks like to the update check: the shell names different files.
    """
    dst = tempfile.mkdtemp(prefix="cmitt-next-build-")
    shutil.copytree(src, dst, dirs_exist_ok=True)
    css = next(f for f in os.listdir(dst) if f.startswith("styles-") and f.endswith(".css"))
    new_css = "styles-00ff00ff00ff00ff.css"
    os.rename(os.path.join(dst, css), os.path.join(dst, new_css))
    for name in ("index.html", "sw.js"):
        path = os.path.join(dst, name)
        if not os.path.exists(path):
            continue
        text = open(path, encoding="utf-8").read().replace(css, new_css)
        # A new build is a new cache, or the worker would keep serving the old
        # files out of the old one.
        text = text.replace("cmitt-sw-", "cmitt-sw-next-")
        open(path, "w", encoding="utf-8").write(text)
    return dst, css, new_css


def _shown_build(d):
    return d.execute_script(
        "return [...document.querySelectorAll('link[href]')]"
        ".map(l => l.href).join(' ');")


def _scheduled_check(d):
    """Run the DAILY check the way the app runs it, with no button pressed: the
    schedule is due and the network just came back. Every path that must obey
    "off" and "not now" is this one — the developer button is deliberately
    exempt from both, so testing with it would prove nothing."""
    d.execute_script("""
        const raw = localStorage.getItem('cmitt.v1.update');
        const state = raw ? JSON.parse(raw) : {};
        state.attempted_at = 0;
        state.next_check_at = 0;
        localStorage.setItem('cmitt.v1.update', JSON.stringify(state));
        window.dispatchEvent(new Event('online'));
    """)


def _update_state(d):
    return d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.update') || '{}');")


def _build_id_of(d):
    """The id the app would compute for the build the SERVER is offering — which
    is what the loop guard stores. Taken the same way `ttcore::update::build_id`
    takes it: the hashed asset names in the shell, deduplicated and sorted."""
    return d.execute_async_script("""
        const done = arguments[arguments.length - 1];
        fetch('./index.html?probe=' + Date.now())
          .then(r => r.text())
          .then(html => {
              const names = [...new Set(
                  (html.match(/[A-Za-z0-9_.-]+-[0-9a-f]{8,}(?:_bg)?\\.(?:js|css|wasm)/g) || []))];
              names.sort();
              done(names.join(' '));
          })
          .catch(() => done(''));
    """)


def _set_update_checks(app, on):
    """Turn the daily check off or on the way a reader does: My data → App
    updates. Not by editing localStorage — the running app holds prefs in a
    signal, so a stored value it never read would prove nothing."""
    app.xpath("//button[normalize-space()='My data']").click()
    box = app.wait_css("[data-update-switch]")
    if box.is_selected() != on:
        app.d.execute_script("arguments[0].click();", box)
    WebDriverWait(app.d, 5).until(
        lambda d: app.css("[data-update-switch]").is_selected() == on)
    app.d.execute_script("""
        const b = [...document.querySelectorAll('.dialog button')]
          .find(x => x.textContent.trim() === 'Close');
        if (b) b.click();
    """)
    WebDriverWait(app.d, 5).until(lambda d: not app.css_all(".dialog"))
    app.dismiss_toasts()


def t114_the_app_asks_before_it_updates_itself(app):
    """A tab left open for a week never learns that a new version was
    deployed: browsers only look for a new service worker when a page is
    navigated. So the app asks, itself — and then it asks the READER, because
    a page that reloads itself takes things with it that are not saved: an
    Undo offer, a scroll position, a half-finished thought.

    Six things have to hold: nothing new says so, no network says so and breaks
    nothing, a new build ASKS and never installs itself, "Not now" is honoured
    for the day, "Update now" installs it, and "stop checking" means no
    checking — reversibly.

    Runs on its own port so its service worker and its update marker never
    touch the origin the rest of the suite uses.
    """
    port = SW_PORT + 1
    base = f"http://127.0.0.1:{port}"
    d = app.d
    server = _serve_dir(DIST, port)
    nxt = None
    try:
        d.get(f"{base}/#/developer")
        WebDriverWait(d, 20).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, "[data-update-check]")))
        d.execute_script("localStorage.removeItem('cmitt.v1.update');"
                         "localStorage.removeItem('cmitt.v1.prefs');")
        first_build = _shown_build(d)

        # 1. Nothing new: it says so when ASKED, and says nothing at all when
        # it looked on its own.
        d.find_element(By.CSS_SELECTOR, "[data-update-check]").click()
        app.wait_toast("newest version")
        assert not app.css_all(".update-banner")
        assert _shown_build(d) == first_build
        app.dismiss_toasts()

        # The daily check on an up-to-date app must be INVISIBLE: no banner, no
        # toast, no word of any kind. A check that announced "you're up to
        # date" once a day would be a notification nobody asked for.
        _scheduled_check(d)
        time.sleep(3.0)
        assert not app.css_all(".update-banner"), "nothing new is not an update"
        assert app.toasts_text() == "", \
            f"the daily check must be silent when nothing changed; said {app.toasts_text()!r}"

        # …and something ELSE on the page carrying a hashed name must not be
        # mistaken for this app's own build. A theme extension injecting a
        # stylesheet would otherwise change the running app's id, and the app
        # would ask every day about an update that does not exist.
        d.execute_script("""
            const l = document.createElement('link');
            l.rel = 'stylesheet';
            l.href = 'https://example.invalid/injected-0badc0de1234beef.css';
            document.head.appendChild(l);
        """)
        _scheduled_check(d)
        time.sleep(3.0)
        assert not app.css_all(".update-banner"), \
            "a foreign hashed file is not a new version of this app"
        assert app.toasts_text() == "", app.toasts_text()
        d.execute_script(
            "document.querySelector('link[href^=\"https://example.invalid\"]').remove();")

        # 2. No network: the check fails, says so plainly, and the app keeps
        # working — everything it needs is already in this browser.
        _really_stop(server)
        d.find_element(By.CSS_SELECTOR, "[data-update-check]").click()
        app.wait_toast("Couldn't reach the server")
        assert not app.css_all(".update-banner"), "offline is not an update"
        assert _shown_build(d) == first_build, "and it must never reload on a failure"
        # Still a working app: a control still answers.
        d.find_element(By.CSS_SELECTOR, "[data-update-check]").click()
        app.wait_toast("Couldn't reach the server")
        app.dismiss_toasts()

        # 3. A new build appears on the same origin: the app ASKS. It must not
        # install anything — not now, not in a second and a half, not when the
        # tab is hidden.
        nxt, old_css, new_css = _next_build(DIST)
        server = _serve_dir(nxt, port)
        _scheduled_check(d)
        banner = WebDriverWait(d, 30).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, ".update-banner")))
        assert "Update now" in banner.text and "Not now" in banner.text, banner.text
        d.execute_script(
            "Object.defineProperty(document, 'hidden', {value: true, configurable: true});"
            "document.dispatchEvent(new Event('visibilitychange'));")
        time.sleep(3.0)
        assert old_css in _shown_build(d), \
            "nothing may reload on its own — not even a tab nobody is looking at"
        assert app.css_all(".update-banner"), "and the question stays until it is answered"

        # Asking is not downloading. The check used to ask the service worker to
        # update itself BEFORE the reader had agreed, which pulled ~2MB of a
        # version they might decline while My data promised "a few kilobytes".
        # Asserted here, at the first offer: after "Update now" the new build is
        # in a cache for the right reason. (R74)
        cached = d.execute_async_script("""
            const done = arguments[arguments.length - 1];
            caches.keys().then(ks => Promise.all(ks.map(k =>
                caches.open(k).then(c => c.keys()))))
              .then(all => done(all.flat().map(r => r.url)), () => done([]));
        """)
        assert not any(new_css in u for u in cached), \
            f"the new build was fetched before the reader agreed: {cached}"

        # 4. "Not now": the banner goes, the app says how to get it later, and
        # the scheduled check leaves it alone for the day.
        d.find_element(
            By.XPATH, "//div[contains(@class,'update-banner')]"
                      "//button[normalize-space()='Not now']").click()
        app.wait_toast("Refresh the page whenever")
        WebDriverWait(d, 5).until(lambda drv: not app.css_all(".update-banner"))
        assert _update_state(d)["declined"], "the answer has to survive a reload"
        app.dismiss_toasts()
        _scheduled_check(d)
        time.sleep(3.0)
        assert not app.css_all(".update-banner"), \
            "'not now' asked twice in one afternoon is not 'not now'"

        # …but asking for it yourself always answers, declination or not.
        d.find_element(By.CSS_SELECTOR, "[data-update-check]").click()
        WebDriverWait(d, 30).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, ".update-banner")))

        # 5. "Update now": that, and only that, installs it — and the page says
        # why it reappeared rather than leaving it eerie.
        d.find_element(
            By.XPATH, "//div[contains(@class,'update-banner')]"
                      "//button[normalize-space()='Update now']").click()
        WebDriverWait(d, 30).until(lambda drv: new_css in _shown_build(drv))
        assert old_css not in _shown_build(d)
        app.wait_toast("Updated to the newest version")
        # The loop guard is spent, not left armed.
        assert _update_state(d)["reload_target"] is None
        app.dismiss_toasts()

        # 6. "Stop checking" means it stops — and the same switch turns it back
        # on. Back to the OLD build first, so there is something to find again;
        # every reload from here on has to happen while the old build is being
        # served, or the reload itself does the updating.
        _really_stop(server)
        server = _serve_dir(DIST, port)
        # `d.get` with the URL the browser is already on — hash and all — is a
        # same-document navigation and reloads NOTHING, so the tab would have
        # stayed on the new build and there would have been nothing to find.
        d.refresh()
        WebDriverWait(d, 20).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, "[data-update-check]")))
        assert old_css in _shown_build(d), \
            "this phase needs the old build back, or it proves nothing"
        d.execute_script("localStorage.removeItem('cmitt.v1.update');")
        _set_update_checks(app, False)
        _really_stop(server)
        server = _serve_dir(nxt, port)

        _scheduled_check(d)
        time.sleep(3.0)
        assert not app.css_all(".update-banner"), \
            "checks are off: the app must not even look"

        # …but "Check now", pressed by the reader, answers even with checks off
        # — and answers VISIBLY, which is the part that was missing: the banner
        # it raises is behind the My data dialog it was pressed in, so without a
        # word the button looked broken. (R74)
        _set_update_checks(app, False)
        d.execute_script("localStorage.removeItem('cmitt.v1.update');")
        app.xpath("//button[normalize-space()='My data']").click()
        app.wait_css("[data-update-switch]")
        # Scoped to the dialog: the developer panel behind the modal carries the
        # same marker, and an unscoped find picks IT — then the click lands on
        # the overlay and the failure reads as a broken button.
        # Scrolled to the middle first, the way the suite presses anything far
        # down a long dialog: this one grew when the network lede became a
        # three-item list, so "Check now" now starts life under the sticky
        # action bar, and Chrome refuses a click it would land on the bar.
        check_now = d.find_element(By.CSS_SELECTOR, ".dialog [data-update-check]")
        d.execute_script("arguments[0].scrollIntoView({block: 'center'});", check_now)
        check_now.click()
        app.wait_toast("close this to see the message at the top", timeout=30)
        d.execute_script("""
            const b = [...document.querySelectorAll('.dialog button')]
              .find(x => x.textContent.trim() === 'Close');
            if (b) b.click();
        """)
        banner = WebDriverWait(d, 10).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, ".update-banner")))
        app.dismiss_toasts()

        # And with checks off, "Not now" must not promise a question that can
        # never come. (R74: the toast said "the app will ask again tomorrow"
        # unconditionally, while nothing was going to ask ever again.)
        d.find_element(
            By.XPATH, "//div[contains(@class,'update-banner')]"
                      "//button[normalize-space()='Not now']").click()
        app.wait_toast("won't ask again", timeout=15)
        assert "tomorrow" not in app.toasts_text(), app.toasts_text()
        app.dismiss_toasts()
        assert banner is not None

        # On again, through the app's own switch — the point of the setting is
        # that it is not a one-way door.
        _set_update_checks(app, True)
        d.execute_script("localStorage.removeItem('cmitt.v1.update');")
        _scheduled_check(d)
        WebDriverWait(d, 30).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, ".update-banner")),
            message="switched back on, the app has to look again")

        # 8. A loop guard that never expires would silence a real update for the
        # life of the browser profile. Plant a guard from two days ago and the
        # app must offer the build again. (R74)
        d.execute_script("""
            const s = JSON.parse(localStorage.getItem('cmitt.v1.update') || '{}');
            s.declined = null;
            s.reload_target = arguments[0];
            s.reload_target_at = Date.now() - 2 * 24 * 60 * 60 * 1000;
            localStorage.setItem('cmitt.v1.update', JSON.stringify(s));
        """, _build_id_of(d))
        d.find_element(
            By.XPATH, "//div[contains(@class,'update-banner')]"
                      "//button[normalize-space()='Not now']").click()
        WebDriverWait(d, 5).until(lambda drv: not app.css_all(".update-banner"))
        app.dismiss_toasts()
        d.execute_script("""
            const s = JSON.parse(localStorage.getItem('cmitt.v1.update') || '{}');
            s.declined = null;
            localStorage.setItem('cmitt.v1.update', JSON.stringify(s));
        """)
        _scheduled_check(d)
        WebDriverWait(d, 30).until(
            EC.presence_of_element_located((By.CSS_SELECTOR, ".update-banner")),
            message="a guard from two days ago must have lapsed")
    finally:
        try:
            _really_stop(server)
        except Exception:
            pass
        if nxt:
            shutil.rmtree(nxt, ignore_errors=True)
        # Leave no worker behind on that origin for whatever runs next.
        d.execute_async_script("""
            const done = arguments[arguments.length - 1];
            navigator.serviceWorker.getRegistrations()
              .then(rs => Promise.all(rs.map(r => r.unregister())))
              .then(() => done(null), () => done(null));
        """)


def t115_the_search_box_has_the_three_switches_every_editor_has(app):
    """Match case, whole word, regular expression — beside the box, the way a
    find bar does it. Each has to actually change the answer, an unfinished
    pattern must never be read as "no search at all" (which would show the
    whole catalog), and all three are filters like any other: they persist and
    Ctrl+Z reaches them."""
    app.boot("/", selection=["TOC"])
    app.open_tab("Catalog")
    section = app.wait_css("section[aria-label='Catalog']")

    def switch(label):
        return app.css(f"section[aria-label='Catalog'] "
                       f".search-switch[aria-label='{label}']")

    def on(label):
        return switch(label).get_attribute("aria-pressed") == "true"

    def type_search(text):
        box = app.css("section[aria-label='Catalog'] .searchbox input")
        box.clear()
        box.send_keys(text)
        time.sleep(0.35)

    def rows():
        return len(app.css_all("section[aria-label='Catalog'] .card"))

    # Plain search: case-insensitive, matching inside words.
    type_search("alg")
    loose = rows()
    assert loose > 0, "the fixture catalog has algebra courses"

    # Whole word: 'alg' stops matching 'Algebra'.
    switch("Whole word").click()
    WebDriverWait(app.d, 5).until(lambda d: rows() < loose)
    assert on("Whole word")
    switch("Whole word").click()
    WebDriverWait(app.d, 5).until(lambda d: rows() == loose)

    # Match case: the same letters, told apart.
    type_search("algebra")
    lower = rows()
    assert lower > 0
    switch("Match case").click()
    WebDriverWait(app.d, 5).until(lambda d: rows() < lower)
    switch("Match case").click()
    WebDriverWait(app.d, 5).until(lambda d: rows() == lower)

    # A pattern, read as a pattern.
    switch("Regular expression").click()
    type_search("^alg")
    anchored = rows()
    assert anchored > 0, "^alg matches the courses whose code starts with ALG"
    type_search("alg$")
    assert rows() != anchored or anchored == 0, "an anchor at the other end is a different set"

    # The case that matters most: a half-typed pattern must NOT be read as an
    # empty search and show the whole catalog.
    type_search("(unclosed")
    bad = app.wait_css("section[aria-label='Catalog'] .searchbox-bad")
    assert "not a pattern yet" in bad.text.lower(), bad.text
    assert rows() == 0, "a broken pattern matches nothing, never everything"
    assert app.css("section[aria-label='Catalog'] .searchbox input") \
        .get_attribute("aria-invalid") == "true"

    # Off again, the same text is just text — and the notice goes.
    switch("Regular expression").click()
    WebDriverWait(app.d, 5).until(
        lambda d: not app.css_all("section[aria-label='Catalog'] .searchbox-bad"))

    # A switch is a filter like any other: Ctrl+Z reaches it…
    switch("Match case").click()
    WebDriverWait(app.d, 5).until(lambda d: on("Match case"))
    ActionChains(app.d).key_down(Keys.CONTROL).send_keys("z").key_up(Keys.CONTROL).perform()
    WebDriverWait(app.d, 5).until(lambda d: not on("Match case"))

    # …and it survives a reload. (In that order: the undo stack lives in
    # memory by design, so a reload is the one thing Ctrl+Z cannot reach
    # across — testing it the other way round tests the wrong thing.)
    switch("Match case").click()
    WebDriverWait(app.d, 5).until(lambda d: on("Match case"))
    app.d.refresh()
    app.wait_css("section[aria-label='Catalog']")
    assert on("Match case"), "a switch the reader turned on must survive a reload"

    # My courses has its own set, exactly like every other filter.
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    mine = app.css("section[aria-label='My courses'] "
                   ".search-switch[aria-label='Match case']")
    assert mine.get_attribute("aria-pressed") == "false"
    assert section is not None


ROOM_JS = """
const box = document.querySelector(arguments[0]);
const cs = getComputedStyle(box);
const ctx = document.createElement('canvas').getContext('2d');
ctx.font = `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
const padL = parseFloat(cs.paddingLeft), padR = parseFloat(cs.paddingRight);
const r = box.getBoundingClientRect();
// Who answers at the far end of the text area? Anything but the field itself
// means text put there is under something else.
const at = document.elementFromPoint(
  Math.round(r.right - parseFloat(cs.borderRightWidth) - padR - 2),
  Math.round(r.top + r.height / 2));
return {
  room: Math.round(box.clientWidth - padL - padR),
  placeholder: box.placeholder,
  needs: Math.round(ctx.measureText(box.placeholder).width),
  at_text_end: at ? (at === box ? 'the field' : (at.className || at.tagName)) : null,
};
"""


def t116_the_search_box_shows_its_whole_placeholder(app):
    """The switches sit inside the box's right edge, so the box has to be wide
    enough for BOTH — its placeholder and the strip. R71 sized it for the
    placeholder alone and then spent 6.4rem of that on the buttons, which cut
    the placeholder off mid-word: "Search by code, name or in".

    Measured, not eyeballed: how much room the text area has, against how wide
    the placeholder is in the field's own font. A pixel comparison is the only
    kind that survives a font change, and it is what `.workagents/
    field-clip-probe.py` swept the whole app with.
    """
    app.d.set_window_size(1400, 950)
    app.boot("/", selection=["TOC"])
    sel = "section[aria-label='Catalog'] .searchbox input"

    for tab, label in (("Catalog", "Catalog"), ("My courses", "My courses"),
                       ("Master grid", "Master grid")):
        app.open_tab(tab)
        app.wait_css(f"section[aria-label='{label}'] .searchbox input")
        m = app.d.execute_script(
            ROOM_JS, f"section[aria-label='{label}'] .searchbox input")
        assert m["needs"] <= m["room"], (
            f"{label}: {m['placeholder']!r} needs {m['needs']}px and the box "
            f"gives it {m['room']}px — it is being cut off")
        assert m["at_text_end"] == "the field", (
            f"{label}: {m['at_text_end']!r} is sitting over the end of the "
            "text area")

    app.open_tab("Catalog")
    app.wait_css(sel)

    # Pattern mode has its own, different placeholder; it has to fit the same
    # room. (The first one written for it needed 322px of a 244px field.)
    app.css("section[aria-label='Catalog'] .search-switch[aria-label='Regular expression']").click()
    time.sleep(0.3)
    m = app.d.execute_script(ROOM_JS, sel)
    assert "attern" in m["placeholder"], m["placeholder"]
    assert m["needs"] <= m["room"], (
        f"pattern mode: {m['placeholder']!r} needs {m['needs']}px of "
        f"{m['room']}px")
    app.css("section[aria-label='Catalog'] .search-switch[aria-label='Regular expression']").click()
    time.sleep(0.3)

    # Emptying the box: the app's own button, in the app's own language,
    # because the browser's ✕ landed in the middle of the switches.
    assert not app.css_all(".search-clear"), \
        "nothing to clear yet, so no button to clear it"
    box = app.css(sel)
    box.send_keys("algebra")
    WebDriverWait(app.d, 5).until(lambda d: app.css_all(".search-clear"))
    narrowed = len(app.css_all("section[aria-label='Catalog'] .card"))
    assert narrowed > 0

    app.css("section[aria-label='Catalog'] .search-clear").click()
    WebDriverWait(app.d, 5).until(
        lambda d: not app.css_all("section[aria-label='Catalog'] .search-clear"))
    assert app.css(sel).get_attribute("value") == "", "the box has to be empty"
    assert len(app.css_all("section[aria-label='Catalog'] .card")) > narrowed, \
        "and the list has to come back"
    assert app.d.execute_script(
        "return document.activeElement === document.querySelector(arguments[0]);",
        sel), "the caret belongs in the box the reader was typing in"

    # Its own undo step, never folded into the typing it undid. Pressed as the
    # button, not as Ctrl+Z: the caret is in the box after clearing, and the
    # app hands every shortcut to the field the reader is typing in
    # (`is_editing_context` in dnd.rs) so that the browser's own text editing
    # keeps working. The header button is the undo that reaches filters.
    app.xpath("//button[contains(., 'Undo')]").click()
    WebDriverWait(app.d, 5).until(
        lambda d: app.css(sel).get_attribute("value") == "algebra",
        message="undoing a clear must bring the words back")

    # Phones, at the widths phones actually are. This is where R73's fix
    # quietly failed: it was checked at 430px and held, while at 360px the
    # placeholder was 17px too wide for what three finger-sized switches left
    # behind, and at 320px it was 57px too wide. Below 440px the switches now
    # take their own line under the box, so the field has the whole width.
    for width, height in ((430, 900), (412, 915), (390, 844), (360, 800), (320, 700)):
        # Emulated as a real phone, not just a narrow window: `pointer: coarse`
        # is what grows the switches to finger size, and a resized desktop
        # window never matches it — so measuring in one measures the wrong strip.
        app.d.execute_cdp_cmd("Emulation.setDeviceMetricsOverride", {
            "width": width, "height": height, "deviceScaleFactor": 2, "mobile": True})
        app.d.execute_cdp_cmd("Emulation.setTouchEmulationEnabled", {
            "enabled": True, "maxTouchPoints": 5})
        time.sleep(0.6)
        box = app.css(sel)
        app.d.execute_script("arguments[0].scrollIntoView({block: 'center'});", box)
        m = app.d.execute_script(ROOM_JS, sel)
        assert m["needs"] <= m["room"], (
            f"at {width}px the placeholder {m['placeholder']!r} needs {m['needs']}px "
            f"and the box gives it {m['room']}px")
        assert m["at_text_end"] == "the field", f"at {width}px: {m['at_text_end']!r}"
        # The switches are still there, still reachable, still finger-sized.
        sw = app.css_all("section[aria-label='Catalog'] .search-switch")
        assert len(sw) >= 3, f"at {width}px only {len(sw)} switches"
        for s in sw:
            r = s.rect
            assert r["width"] >= 30 and r["height"] >= 30, \
                f"at {width}px a switch is {r['width']}x{r['height']}"
    app.d.execute_cdp_cmd("Emulation.clearDeviceMetricsOverride", {})
    app.d.execute_cdp_cmd("Emulation.setTouchEmulationEnabled", {"enabled": False})
    app.d.set_window_size(1400, 950)


def t117_the_shorten_popup_has_a_way_out_and_it_is_not_beside_the_send(app):
    """Every other dialog in this app can be left by pressing a button. This
    one could not: its footer held "Back" (to the share dialog) and the one
    control in the app that hands a timetable to a stranger, and nothing that
    simply closed it. Escape and the dark area worked, which is not the same as
    offering a way out.

    The button is the easy half. The half worth a test is WHERE it sits:
    three word buttons do not fit a 320px footer, so the row wraps, and the
    two things that must stay true when it does are that the exits keep the
    left edge — never stacked at the right, directly where the thumb is already
    travelling for the button that sends — and that the primary keeps one
    footprint through all of its labels, or the sticky bar re-wraps under the
    finger that just pressed it.

    The left-edge property rides on a THREE-class selector, because
    `.shorten-dialog .actions` merely ties `.dialog .actions` and loses on
    source order. That failure is silent — every button still works, the row is
    just still right-aligned — so it is asserted here directly.
    """
    app.d.set_window_size(1400, 950)
    app.boot("/", selection=["TOC", "RDBM"])
    long = _open_shorten(app)

    # Left to right: two ways out, then the one thing this popup does.
    footer = app.css(".shorten-dialog .actions")
    assert [b.text for b in footer.find_elements(By.CSS_SELECTOR, "button")] == \
        ["Back", "Close", "Make it short"], \
        [b.text for b in footer.find_elements(By.CSS_SELECTOR, "button")]
    # Four other assertions in this file read the LAST child as the primary,
    # and t110 clicks it. Close must never take that place.
    assert app.css(".shorten-dialog .actions button:last-child").text == \
        "Make it short"
    # "Back" keeps its visible word (speech control, and two xpaths here say
    # it) and gains the destination for anyone hearing the page read out.
    back = app.xpath("//div[contains(@class,'shorten-dialog')]"
                     "//button[normalize-space()='Back']")
    assert back.get_attribute("aria-label") == "Back to sharing"

    GEOM = """
        const dlg = document.querySelector('.dialog');
        const f = document.querySelector('.shorten-dialog .actions');
        const bs = [...f.querySelectorAll('button')];
        const r = (e) => e.getBoundingClientRect();
        const last = bs[bs.length - 1];
        return {
            justify: getComputedStyle(f).justifyContent,
            rows: new Set(bs.map((b) => Math.round(r(b).top))).size,
            back_left: Math.round(r(bs[0]).left),
            close_right: Math.round(r(bs[1]).right),
            exits_bottom: Math.round(Math.max(r(bs[0]).bottom, r(bs[1]).bottom)),
            primary_top: Math.round(r(last).top),
            primary_left: Math.round(r(last).left),
            primary_w: Math.round(r(last).width),
            footer_h: Math.round(r(f).height),
            widest: Math.max(...bs.map((b) => Math.round(r(b).right))),
            leftmost: Math.min(...bs.map((b) => Math.round(r(b).left))),
            // The box the footer is allowed to fill, which is the popup and
            // not the viewport — the popup has its own padding, so a button
            // flush with the viewport edge would still be a bug and one inside
            // the popup is fine. (Until R82 there was a second reason: the app
            // laid out wider than a 320px viewport because of a `nowrap` badge
            // holding a sentence. That is fixed and pinned by t130, which is
            // also why this footer now wraps at 320px.)
            dlg_left: Math.round(r(dlg).left),
            dlg_right: Math.round(r(dlg).right),
        };
    """
    wide = app.d.execute_script(GEOM)
    assert wide["justify"] == "flex-start", (
        "the footer is still inheriting flex-end — the three-class selector "
        f"lost the cascade ({wide['justify']}), so on a narrow screen Close "
        "stacks directly above the button that sends")
    assert wide["primary_left"] - wide["close_right"] > 40, (
        "the way out and the button that sends a timetable away are "
        f"shoulder to shoulder ({wide['primary_left'] - wide['close_right']}px "
        "apart)")

    # Reachable by keyboard, in the order it is read. Tab is sent to whatever
    # currently HAS focus, never to <body>: `send_keys` focuses what it is
    # called on, so tabbing via the body would walk focus out of the dialog and
    # then report a break that isn't there (the trap dialog-a11y documents).
    seen, chain = [], ActionChains(app.d)
    for _ in range(24):
        chain.send_keys(Keys.TAB).perform()
        label = app.d.execute_script(
            "const a = document.activeElement;"
            "return a ? a.tagName + '|' + (a.textContent || '').trim() : '';")
        if label.startswith("BUTTON|") and label[7:] in (
                "Back", "Close", "Make it short"):
            if label[7:] not in seen:
                seen.append(label[7:])
        if len(seen) == 3:
            break
    assert seen == ["Back", "Close", "Make it short"], (
        f"the footer's keyboard order is {seen} — it must read the way it looks")

    # Close means gone — not back to the share dialog behind it.
    app.xpath("//div[contains(@class,'shorten-dialog')]"
              "//button[normalize-space()='Close']").click()
    WebDriverWait(app.d, 5).until(
        lambda d: not app.css_all(".dialog"),
        message="Close left a dialog on screen")
    assert not app.css_all(".share-shorten"), \
        "Close is not Back: it must not leave the share dialog open"

    # And Back still means back, which is why both buttons exist.
    _open_shorten(app)
    app.xpath("//div[contains(@class,'shorten-dialog')]"
              "//button[normalize-space()='Back']").click()
    app.wait_css(".share-shorten button")
    app.xpath("//div[contains(@class,'dialog')]"
              "//button[normalize-space()='Close']").click()
    WebDriverWait(app.d, 5).until(lambda d: not app.css_all(".dialog"))

    # One footprint for every label. "Ask TinyURL again" is a different
    # sentence from "Make it short"; if the button resizes with it, the
    # sticky bar re-wraps the moment it is pressed and again when the link
    # lands — twice per request, under the reader's thumb.
    _plant_short_links(app, [
        {"service": "tinyurl", "long": long, "short": "https://tinyurl.com/abcd"},
    ])
    assert _open_shorten(app) == long
    app.wait_css(".shorten-have .shorten-short")
    made = app.d.execute_script(GEOM)
    assert "again" in app.css(".shorten-dialog .actions button:last-child").text.lower()
    assert made["primary_w"] == wide["primary_w"], (
        f"the primary is {wide['primary_w']}px saying 'Make it short' and "
        f"{made['primary_w']}px saying 'Ask TinyURL again' — min-width is not "
        "holding its footprint")
    assert made["footer_h"] == wide["footer_h"], (
        f"the footer is {wide['footer_h']}px with one label and "
        f"{made['footer_h']}px with the other")

    # Phones, at the widths phones actually are. Three word buttons cannot fit
    # a 320px footer; what matters is that the wrap is the SHAPE this design
    # chose, and that nothing lands off screen.
    for width, height in ((430, 900), (390, 844), (360, 800), (320, 700)):
        app.d.execute_cdp_cmd("Emulation.setDeviceMetricsOverride", {
            "width": width, "height": height, "deviceScaleFactor": 2,
            "mobile": True})
        app.d.execute_cdp_cmd("Emulation.setTouchEmulationEnabled", {
            "enabled": True, "maxTouchPoints": 5})
        time.sleep(0.5)
        g = app.d.execute_script(GEOM)
        # Inside the popup it belongs to, at both ends.
        assert g["leftmost"] >= g["dlg_left"] and g["widest"] <= g["dlg_right"], (
            f"at {width}px a footer button is outside the popup: buttons span "
            f"{g['leftmost']}..{g['widest']}, popup {g['dlg_left']}..{g['dlg_right']}")
        # Wrapped or not, the way out is never adjacent to the send.
        #
        # R82 is the day the CSS comment at styles.css:2260-2269 predicted:
        # fixing the sideways-scroll bug (old §8.20 — a sentence in a
        # `nowrap` badge setting the app's layout floor at ~368px) gave 320px a
        # real 244px footer, three word-buttons stopped fitting on one line,
        # and `justify-content: flex-start` + the primary's `margin-left: auto`
        # became load-bearing. So this now asserts the SHAPE rather than one
        # line of it: either the three sit in reading order across one row, or
        # the two exits share the upper row and the send is on a row of its own
        # BELOW them — which is the arrangement that keeps a thumb travelling
        # to Send away from Close.
        if g["rows"] == 1:
            assert g["back_left"] < g["close_right"] < g["primary_left"], (
                f"at {width}px the footer order broke: back {g['back_left']}, "
                f"close ends {g['close_right']}, primary starts "
                f"{g['primary_left']}")
        else:
            assert g["primary_top"] >= g["exits_bottom"], (
                f"at {width}px the footer wrapped with the send NOT below the "
                f"exits: primary top {g['primary_top']}, exits bottom "
                f"{g['exits_bottom']} — a mis-tap waiting to happen")
            assert g["back_left"] < g["close_right"], (
                f"at {width}px Back is not left of Close: {g!r}")
        assert app.css(".shorten-dialog .actions button:last-child").text.strip(), \
            f"at {width}px the primary has no label"
        # Finger-sized, which is what `pointer: coarse` is emulated for here.
        for b in app.css_all(".shorten-dialog .actions button"):
            assert b.rect["height"] >= 40, \
                f"at {width}px {b.text!r} is only {b.rect['height']}px tall"
    app.d.execute_cdp_cmd("Emulation.clearDeviceMetricsOverride", {})
    app.d.execute_cdp_cmd("Emulation.setTouchEmulationEnabled", {"enabled": False})
    app.d.set_window_size(1400, 950)

    # Cleanly closed, by the button this test is about.
    app.xpath("//div[contains(@class,'shorten-dialog')]"
              "//button[normalize-space()='Close']").click()
    WebDriverWait(app.d, 5).until(lambda d: not app.css_all(".dialog"))


def t118_the_search_looks_only_where_you_tell_it(app):
    """R77: a "Search in" dropdown before the facets decides which parts of a
    course the box reads — code, name, instructor — all three by default.
    The placeholder is the promise, so it changes with the mask; the LAST
    ticked part cannot be unticked (a search that reads nothing is a control
    that cannot act); and the mask is a filter like any other: per scope,
    undoable, persisted."""
    app.boot("/", selection=["TOC"])
    app.open_tab("Catalog")
    section = app.wait_css("section[aria-label='Catalog']")
    box = section.find_element(By.CSS_SELECTOR, ".filterbar input[type='search']")

    def searchin():
        return app.xpath("//section[@aria-label='Catalog']"
                         "//details[starts-with(normalize-space(summary), 'Search in')]")

    def tickbox(label):
        return searchin().find_element(
            By.XPATH, f".//label[normalize-space()='{label}']/input")

    def rows():
        # Cards, not table rows — the catalog is a card list (t115 counts
        # the same selector).
        return app.css_all("section[aria-label='Catalog'] .card")

    # The default: everything read, promised in the placeholder, no badge.
    assert box.get_attribute("placeholder") == "Search by code, name or instructor"
    assert not searchin().find_elements(By.CSS_SELECTOR, ".facet-count"), \
        "all three parts on is the default and wears no badge"

    # An instructor search finds TOC's course…
    box.send_keys("Aiswarya")
    WebDriverWait(app.d, 10).until(lambda d: len(rows()) == 1)

    # …until Instructor is unticked, and then the SAME text finds nothing —
    # and the placeholder stops promising instructors.
    searchin().find_element(By.CSS_SELECTOR, "summary").click()
    tickbox("Instructor").click()
    WebDriverWait(app.d, 10).until(lambda d: len(rows()) == 0)
    assert box.get_attribute("placeholder") == "Search by code or name"
    badge = searchin().find_element(By.CSS_SELECTOR, ".facet-count")
    assert badge.text.strip() == "2", badge.text

    # The floor: with only one part left, that part's box is disabled and
    # says why — never a checkbox that bounces back.
    tickbox("Course name").click()
    WebDriverWait(app.d, 5).until(
        lambda d: box.get_attribute("placeholder") == "Search by code")
    code_box = tickbox("Course code")
    assert code_box.get_attribute("disabled"), \
        "the last part still read must not be uncheckable"
    assert "read something" in (code_box.get_attribute("title") or ""), \
        "the disabled box owes the reason"
    # A code search still works exactly as promised. (Select-all + type, not
    # element.clear(): clear() sets the value without the input event the
    # app listens for — the field-clip-probe lesson.)
    box.send_keys(Keys.CONTROL, "a")
    box.send_keys("TOC")
    WebDriverWait(app.d, 10).until(lambda d: len(rows()) == 1)

    # Undo reaches it like every filter change (header button — the box
    # keeps focus, and Ctrl+Z in a field means the field's own undo).
    app.d.find_element(By.TAG_NAME, "body").click()
    app.xpath("//button[@aria-label='Undo']").click()  # the retyped text
    app.xpath("//button[@aria-label='Undo']").click()  # unticking Name
    WebDriverWait(app.d, 5).until(
        lambda d: box.get_attribute("placeholder") == "Search by code or name")

    # Persistence: the narrowed scope is part of the filters, so it survives
    # a reload — same key, same blob, same synchronous write as t27 pins.
    app.d.refresh()
    app.wait_css("section[aria-label='Catalog'] .filterbar")
    box = app.css("section[aria-label='Catalog'] .filterbar input[type='search']")
    assert box.get_attribute("placeholder") == "Search by code or name"

    # Per scope: My courses has its OWN mask, still reading everything.
    app.open_tab("My courses")
    my_box = app.wait_css("section[aria-label='My courses'] .filterbar input[type='search']")
    assert my_box.get_attribute("placeholder") == "Search by code, name or instructor", \
        "narrowing the Catalog's search must not narrow My courses'"

    # The joined band's tightest fit: at 641–683px the menu used to render a
    # facet's 19rem — its own 13rem lost the cascade — clip past the right
    # viewport edge and mint a page scrollbar (final sweep, search-in-1).
    app.d.set_window_size(660, 900)
    try:
        app.open_tab("Catalog")
        app.wait_css("section[aria-label='Catalog'] .filterbar")
        app.xpath("//section[@aria-label='Catalog']"
                  "//details[starts-with(normalize-space(summary), 'Search in')]"
                  "/summary").click()
        menu = app.wait_css("section[aria-label='Catalog'] .searchin-menu")
        right = app.d.execute_script(
            "return arguments[0].getBoundingClientRect().right;", menu)
        vw = app.d.execute_script(
            "return document.documentElement.clientWidth;")
        assert right <= vw, f"the open menu must fit the viewport ({right} > {vw})"
        assert not app.d.execute_script(
            "const e = document.documentElement;"
            "return e.scrollWidth > e.clientWidth;"), \
            "an open Search-in menu must not mint a page scrollbar"
    finally:
        app.d.set_window_size(1500, 1000)


def t119_today_is_marked_on_the_week_tables(app):
    """R77: on a week view, today's row wears the same mark everywhere — the
    accent bar and weight the halls table has carried since R74 — on My
    timetable and the Master grid too. Single-day views mark nothing (the
    view already says which day it shows), and the printed poster stays
    timeless: it is read all term, and "today" is only true once."""
    # Tuesday, whatever day it really is. This test used to read the host
    # clock, and the fixture grid draws Mon-Fri only — so on a Saturday or a
    # Sunday every positive assertion below was skipped and the test passed
    # by asserting that NOTHING was marked, which is also precisely what a
    # deleted feature looks like. It went green that way on the day of a
    # deploy (R83). The app reads the date through the ordinary `Date`
    # constructor, so pinning it before any script runs makes the real
    # assertions run all seven days.
    day_short = "Tue"
    pinned = app.pin_weekday(1)
    try:
        app.boot("/", selection=["TOC", "RDBM", "MFD"])
        assert app.d.execute_script("return new Date().getDay()") == 2, \
            "the clock pin did not take — the rest of this test would be vacuous"

        for tab, label in (("My timetable", "My timetable"),
                           ("Master grid", "Master grid")):
            app.open_tab(tab)
            app.wait_css(f"section[aria-label='{label}'] table.tt")
            rows = app.css_all(f"section[aria-label='{label}'] table.tt tbody tr")
            marked = app.css_all(f"section[aria-label='{label}'] table.tt tbody tr.today")
            shown_days = [r.find_element(By.CSS_SELECTOR, "th.rowhead").text.strip()
                          for r in rows]
            assert len(rows) > 1 and day_short in shown_days, \
                f"{label}: the fixture week must show {day_short} ({shown_days})"
            assert len(marked) == 1, \
                f"{label}: expected exactly one today row, found {len(marked)}"
            head = marked[0].find_element(By.CSS_SELECTOR, "th.rowhead")
            assert head.text.strip() == day_short, head.text
            # The mark is the halls table's language: an inset accent bar.
            assert head.value_of_css_property("box-shadow") != "none", \
                "the today rowhead must carry the inset bar"

        # The poster prints without it: emulate print media and the bar is
        # gone. This pins the specificity trap the screen rule sets up — the
        # print reset must repeat the screen selector's :not() or it silently
        # loses. Unconditional now: with the day pinned there is always a
        # today row to check.
        app.open_tab("My timetable")
        today_heads = app.css_all(
            "section[aria-label='My timetable'] table.tt tbody tr.today th.rowhead")
        assert today_heads, "there must be a today row to print-check"
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": "print"})
        time.sleep(0.2)
        shadow = today_heads[0].value_of_css_property("box-shadow")
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": ""})
        assert shadow == "none", \
            f"the today bar must not print (box-shadow: {shadow})"
    finally:
        app.unpin_weekday(pinned)


def t120_dialogs_open_hands_off_and_the_page_behind_stays_put(app):
    """R77's two dialog-wide rules. (1) Opening a dialog selects NOTHING:
    focus rests on the dialog itself — not a field, not a button, no text
    selection — and the Tab trap still holds from there in both directions.
    (2) While a dialog is open the page behind it gives up its scroll: a
    wheel spun past the popup's end must not move the app underneath, and
    closing the popup finds the page exactly where it was."""
    app.boot("/", selection=["TOC", "RDBM", "MFD"])
    app.open_tab("Catalog")  # tall page, so there is something to scroll
    app.d.execute_script("window.scrollTo(0, 300);")
    time.sleep(0.2)
    assert app.d.execute_script("return window.scrollY") >= 200

    app.xpath("//button[normalize-space()='Share or import']").click()
    dialog = app.wait_css(".dialog")
    time.sleep(0.4)

    # (1) Hands off: the container has focus, nothing is selected.
    assert app.d.execute_script(
        "return document.activeElement === arguments[0];", dialog), \
        "focus must rest on the dialog itself, not on a control inside it"
    assert app.d.execute_script(
        "const a = document.activeElement;"
        "if (a && 'selectionStart' in a && a.selectionStart != null)"
        "    return a.selectionStart === a.selectionEnd;"
        "const s = getSelection(); return !s || s.isCollapsed;"), \
        "no text may be selected by the act of opening a dialog"
    # Shift+Tab first — the branch a forward walk never exercises: from the
    # container the browser's native previous stop is the page behind the
    # overlay, and trap_tab has to wrap it to the dialog's last control.
    ActionChains(app.d).key_down(Keys.SHIFT).send_keys(Keys.TAB) \
        .key_up(Keys.SHIFT).perform()
    assert app.d.execute_script(
        "return arguments[0].contains(document.activeElement)"
        " && document.activeElement !== arguments[0];", dialog), \
        "Shift+Tab from a fresh dialog escaped the trap"

    # (2) The lock: body scroll is off, the page has not moved, and a real
    # wheel over the dialog cannot reach the page even at its scroll end.
    assert app.d.execute_script(
        "return getComputedStyle(document.body).overflow") == "hidden"
    before = app.d.execute_script("return window.scrollY")
    origin = ScrollOrigin.from_element(dialog)
    for _ in range(3):
        ActionChains(app.d).scroll_from_origin(origin, 0, 1200).perform()
    time.sleep(0.3)
    assert app.d.execute_script("return window.scrollY") == before, \
        "wheeling past the dialog's end moved the page behind it"

    # Closing hands the scroll back, exactly where it was. Escape goes
    # through ActionChains, NOT body.send_keys: WebDriver's element send_keys
    # SCROLLS THE ELEMENT INTO VIEW first, and scrolling <body> into view is
    # a jump to the top — a harness artefact that reads exactly like the
    # scroll-restore bug this test exists to rule out.
    ActionChains(app.d).send_keys(Keys.ESCAPE).perform()
    WebDriverWait(app.d, 5).until(lambda d: not app.css_all(".dialog"))
    assert app.d.execute_script(
        "return getComputedStyle(document.body).overflow") != "hidden", \
        "the scroll lock leaked past the dialog's close"
    assert app.d.execute_script("return window.scrollY") == before
    app.d.execute_script("window.scrollTo(0, 0);")


def t121_a_long_meeting_visibly_fills_every_slot_it_covers(app):
    """R77: a 09:10–14:00 class used to paint one chip in its starting column
    and leave 10:30 and 11:50 looking free. Now every covered column carries a
    continuation band — an inert half-chip in the course's own colour saying
    "TOC · until 14:00" — on My timetable, the Master grid, the phone day
    list and the halls table, and the free-hall finder counts the room as
    taken through the whole span, so the page can never disagree with itself.

    The 14:00 column stays FREE: coverage uses the clash panel's own
    half-open overlap, and a meeting that ends at 14:00 is not in the 14:00
    slot. That boundary is the easiest one to get wrong, so it is pinned."""
    app.boot("/", selection=["TOC"], overrides=LONG_OVR)

    # My timetable GRID: the chip count is untouched — Tue's home column and
    # the ordinary Thu meeting, nothing cloned into the covered cells. Scoped
    # to the grid because the Your-changes panel legitimately shows one more.
    grid_chips = app.chips("TOC", container=".week-grid")
    assert len(grid_chips) == 2, \
        f"a long meeting must not clone its chip ({len(grid_chips)} on the grid)"
    for start in (630, 710):
        cell = app.cell(1, start)
        bands = cell.find_elements(By.CSS_SELECTOR, "span.covered")
        assert len(bands) == 1, f"col {start}: expected one band, got {len(bands)}"
        assert bands[0].text.split() == ["TOC", "until", "14:00"], bands[0].text
        assert "continues here" in bands[0].get_attribute("title")
        # Inert: nothing pressable rides in a covered slot.
        assert not cell.find_elements(By.CSS_SELECTOR, "button"), \
            f"col {start}: a band must not be (or bring) a control"
    assert not app.cell(1, 840).find_elements(By.CSS_SELECTOR, "span.covered"), \
        "a meeting ending AT 14:00 does not cover the 14:00 column (half-open)"

    # Master grid casts the same shadow.
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    for start in (630, 710):
        assert app.css_all(
            f"section[aria-label='Master grid'] td[data-day='1']"
            f"[data-slot='{start}'] span.covered"), f"master grid col {start}"

    # Halls: the room is visibly held through the span, and the finder
    # AGREES — it must not offer Lecture Hall 803 at 10:30 on Tuesday while
    # the row above shows the band holding it.
    app.open_tab("Halls")
    section = app.wait_css("section[aria-label='Lecture halls']")
    day_sel = section.find_element(By.CSS_SELECTOR, "select[aria-label='Day']")
    slot_sel = section.find_element(By.CSS_SELECTOR, "select[aria-label='Time slot']")
    day_sel.find_element(By.CSS_SELECTOR, "option[value='1']").click()
    slot_sel.find_element(By.CSS_SELECTOR, "option[value='630']").click()
    app.wait_css(".finder-result")
    free = [li.text for li in app.css_all(".hall-list li")]
    assert "Lecture Hall 803" not in free, \
        f"the finder offered a hall a 09:10-14:00 booking is holding: {free}"
    # The table opens on today; the long booking is Tuesday's — walk the
    # strip there before reading the row. Scrolled into view first: the
    # finder result that just rendered above can leave the strip under the
    # sticky header, where a native click lands on the wrong element.
    tue = app.xpath("//section[@aria-label='Lecture halls']"
                    "//button[normalize-space()='Tue']")
    app.d.execute_script(
        "arguments[0].scrollIntoView({block: 'center'}); arguments[0].click();", tue)
    bands = app.wait_css(
        "section[aria-label='Lecture halls'] td[data-hall='Lecture Hall 803']"
        "[data-slot='630'] span.covered")
    assert bands, "the hall's own row must show the band holding the room"

    # The phone day list learns it from the same computation.
    app.d.execute_cdp_cmd("Emulation.setDeviceMetricsOverride", {
        "width": 390, "height": 844, "deviceScaleFactor": 2, "mobile": True})
    app.d.execute_cdp_cmd("Emulation.setTouchEmulationEnabled", {
        "enabled": True, "maxTouchPoints": 5})
    try:
        app.open_tab("My timetable")
        app.wait_css(".day-list, .week-grid")
        # The strip opens on today; walk to Tuesday.
        app.xpath("//section[@aria-label='My timetable']"
                  "//button[normalize-space()='Tue']").click()
        row = app.wait_css(".day-list .slotrow[data-day='1'][data-slot='630']")
        assert row.find_elements(By.CSS_SELECTOR, "span.covered"), \
            "the phone day list must cast the same shadow"
    finally:
        app.d.execute_cdp_cmd("Emulation.clearDeviceMetricsOverride", {})
        app.d.execute_cdp_cmd("Emulation.setTouchEmulationEnabled", {"enabled": False})
        app.d.set_window_size(1500, 1000)

    # On the printed poster the covered slots stay visibly taken — the sheet
    # is read all term, and a white 10:30 would be the same lie on paper.
    app.open_tab("My timetable")
    app.wait_css("section[aria-label='My timetable'] table.tt")
    app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": "print"})
    time.sleep(0.2)
    band = app.css("td[data-day='1'][data-slot='630'] span.covered")
    bg = band.value_of_css_property("background-color")
    app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": ""})
    assert bg not in ("rgba(0, 0, 0, 0)", "rgb(255, 255, 255)"), \
        f"the band must print in its course's colour, got {bg}"

    # And the shadow dies with its meeting: putting the class back on CMI's
    # time clears every band (the mirror of the synthetic-column vanish pin,
    # t36). The seeded override never entered the undo stack, so this goes
    # through the Your-changes panel's own button, as a reader would.
    app.xpath("//button[normalize-space()=\"Back to CMI's time\"]").click()
    WebDriverWait(app.d, 5).until(
        lambda d: not app.css_all("span.covered"),
        message="ending the long meeting must clear its bands")


def t122_a_clash_inside_the_covered_span_is_red_where_it_happens(app):
    """R77 follow-up: AAT meets Tue 10:30 — inside TOC's 09:10–14:00 span.
    Both CHIPS already wear the clash red (interval overlap has always been
    the clash rule); the band in AAT's cell must wear it too, or the cell
    shows a red chip beside a calm blue label and the conflict reads as
    one-sided. The 11:50 cell holds no other course, so ITS band stays quiet:
    the red marks where the fight is, not everywhere the span reaches."""
    app.boot("/", selection=["TOC", "AAT"], overrides=LONG_OVR)

    # The chips: the long meeting clashes like any other (pre-existing rule,
    # pinned so it cannot regress out from under the band).
    toc = app.chips("TOC", container=".week-grid")[0]
    assert "clash" in toc.get_attribute("class"), "TOC's home chip must be red"
    aat = [c for c in app.chips("AAT", container=".week-grid")
           if "clash" in c.get_attribute("class")]
    assert aat, "AAT's Tue chip sits inside TOC's span and must be red"

    # The band beside AAT's chip is red, with the chips' own ⚠ language.
    band_630 = app.cell(1, 630).find_element(By.CSS_SELECTOR, "span.covered")
    assert "clash" in band_630.get_attribute("class"), \
        "the covered band shares AAT's cell and must share the red"
    # The 11:50 band fights nothing and stays the course's own quiet colour.
    band_710 = app.cell(1, 710).find_element(By.CSS_SELECTOR, "span.covered")
    assert "clash" not in band_710.get_attribute("class"), \
        "a band only reds where the conflict actually is"

    # And the clash panel names the pair with the REAL span, so the fix and
    # the words agree.
    assert any(
        "09:10–14:00" in p.text and "AAT" in p.text
        for p in app.css_all(".clash-line, [class*='clash']")
    ), "the clash panel must carry the extended time"

    # The words ARE the warning: dimmed to .85 the red band's time composited
    # under the 4.5:1 floor on the alarm wash (the final sweep measured
    # 3.96:1 light / 4.32:1 dark, twice, independently) — a clash band shows
    # its time at full strength. Quiet bands keep the dim.
    until = band_630.find_element(By.CSS_SELECTOR, ".until")
    assert app.d.execute_script(
        "return getComputedStyle(arguments[0]).opacity;", until) == "1", \
        "the clash band's time must not be dimmed below the contrast floor"


def t123_a_damaged_link_changes_nothing(app):
    """A share link whose s= payload will not decode is a DAMAGED link, not
    an instruction. Opened with no readable c= beside it, it changes nothing
    and says so — the empty fallback used to be applied as if the link asked
    for an empty timetable, and the selection silently vanished (final
    sweep, share-import-1). With a readable c=, the codes still open (core's
    tested fallback), and the banner still owns up to what was lost."""
    app.boot("/", selection=["TOC", "RDBM"])
    app.wait_css(".week-grid button.chip")

    def stored_selection():
        return sorted(app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));"))

    # Garbage s=, no c=: nothing changes, and the app says why.
    app.d.get(f"{BASE}/?s=!!!not-a-payload!!!")
    app.wait_css(".header h1")
    assert stored_selection() == ["RDBM", "TOC"], \
        "a damaged link must not touch the selection"
    assert app.chips("TOC", container=".week-grid"), \
        "the timetable still shows the courses"
    banner = app.wait_css(".banner.warn")
    assert "couldn't be read" in banner.text, banner.text
    assert "nothing changed" in banner.text, banner.text

    # An EMPTY s= is as unreadable as garbage.
    app.d.get(f"{BASE}/?s=")
    app.wait_css(".header h1")
    assert stored_selection() == ["RDBM", "TOC"]
    assert "nothing changed" in app.wait_css(".banner.warn").text

    # With a readable c= the codes still open — but never silently: whatever
    # the s= carried (times, credits, own courses) is gone, and a link that
    # half-worked while looking whole would be blamed on the app.
    app.d.get(f"{BASE}/?c=MFD&s=!!!")
    app.wait_css(".header h1")
    WebDriverWait(app.d, 10).until(lambda d: stored_selection() == ["MFD"])
    assert "course codes" in app.wait_css(".banner.warn").text


def t124_cancelling_a_confirm_returns_focus_to_what_asked(app):
    """The confirm layer hands focus back the way the dialogs do. Cancelling
    a confirm raised from INSIDE a dialog puts focus back in that dialog —
    it used to fall to body, and Tab then walked the header buttons hidden
    BEHIND the modal overlay, so Enter pressed an invisible control (final
    sweep, dialogs-focus-1). And a dialog whose opener disabled itself while
    it worked — Sync now, whose click starts the fetch that raises the
    Conflicts dialog — still hands focus back to that opener on Escape: the
    browser drops focus to body the instant a focused button is disabled,
    so the restore has to remember where focus GENUINELY was
    (dialogs-focus-2)."""
    app.boot("/", selection=["TOC"])

    # Part 1: confirm over a dialog. My data → "Delete all app data" → Esc.
    app.xpath("//button[normalize-space()='My data']").click()
    dlg = app.wait_css(".dialog")
    btn = dlg.find_element(
        By.XPATH, ".//button[contains(normalize-space(),'Delete all app data')]")
    app.d.execute_script("arguments[0].scrollIntoView({block:'center'});", btn)
    time.sleep(0.2)
    btn.click()
    app.wait_css(".dialog.confirm")
    time.sleep(0.3)
    ActionChains(app.d).send_keys(Keys.ESCAPE).perform()
    WebDriverWait(app.d, 5).until(
        lambda d: not d.find_elements(By.CSS_SELECTOR, ".dialog.confirm"))
    time.sleep(0.3)
    dlg = app.css(".dialog")
    assert app.d.execute_script(
        "return arguments[0].contains(document.activeElement);", dlg), \
        "cancelling the confirm must put focus back inside the dialog"
    # …and the Tab trap holds again: the next Tab stays inside.
    ActionChains(app.d).send_keys(Keys.TAB).perform()
    assert app.d.execute_script(
        "return arguments[0].contains(document.activeElement);", dlg), \
        "Tab after the cancel must not reach the page behind the overlay"
    ActionChains(app.d).send_keys(Keys.ESCAPE).perform()
    app.wait_gone(".dialog")

    # Part 2: the Conflicts dialog's opener disabled itself.
    cached, overrides, gone = cache_from_before_cmi_moved_toc()
    serve_cmi()
    try:
        app.boot("/", selection=["TOC", gone], overrides=overrides,
                 raw_snapshot=cached)
        app.xpath("//button[normalize-space()='Sync now']").click()
        app.wait_css(".dialog", timeout=30)
        time.sleep(0.4)
        ActionChains(app.d).send_keys(Keys.ESCAPE).perform()
        app.wait_gone(".dialog")
        time.sleep(0.3)
        assert app.d.execute_script(
            "const a = document.activeElement;"
            "return a && a.tagName === 'BUTTON'"
            " && a.textContent.trim() === 'Sync now';"), \
            "Escape must hand focus back to the Sync now that started this"
    finally:
        stop_serving_cmi()


def t125_print_stays_light_whatever_the_theme(app):
    """The print stylesheet promises paper. Its bare :root token reset
    silently LOST to the stamped dark theme's selector — every OS-dark
    reader printed near-black cards, a dark halls panel and legend marks in
    near-white-on-white (final sweep, theme-print-1) — and two smaller print
    lies rode along: hall NAMES letter-stacked inside a 46px clamp sized for
    day names, and today's rowhead printed THINNER than its siblings because
    the today-reset said `inherit` and took the row's 400."""
    # Tuesday, so the today-weight check below runs every day of the week
    # rather than skipping itself on a weekend — see t119 for the whole
    # argument and the fixture's Mon-Fri grid (R83).
    pinned = app.pin_weekday(1)
    app.boot("/", selection=["TOC", "RDBM", "MFD"])
    # Stamp dark exactly the way the boot script does. Navigation happens
    # with print OFF — the tab rail is display:none on paper — and every
    # measurement with print ON.
    app.d.execute_script("document.documentElement.dataset.theme = 'dark';")

    def print_media(on):
        app.d.execute_cdp_cmd(
            "Emulation.setEmulatedMedia", {"media": "print" if on else ""})

    try:
        print_media(True)
        # The tokens resolve to the light palette even under the stamp.
        surface = app.d.execute_script(
            "return getComputedStyle(document.documentElement)"
            ".getPropertyValue('--surface').trim();")
        assert surface in ("#fff", "#ffffff"), \
            f"print must reset the dark surface, got {surface}"
        muted = app.d.execute_script(
            "return getComputedStyle(document.documentElement)"
            ".getPropertyValue('--muted').trim();")
        assert muted == "#5d6675", \
            f"print must reset EVERY dark token, not just the big three ({muted})"

        # Legend marks print in ink, not the dark theme's near-white.
        print_media(False)
        app.open_tab("Master grid")
        app.wait_css("section[aria-label='Master grid'] table.tt")
        print_media(True)
        mark_rgb = app.d.execute_script(
            "const m = document.querySelector('span.legend-mark');"
            "const c = getComputedStyle(m).color;"
            "return c.match(/\\d+/g).slice(0, 3).map(Number);")
        assert sum(mark_rgb) < 400, \
            f"legend marks must print dark on white, got rgb{tuple(mark_rgb)}"

        # Today's rowhead prints the SAME weight as the other days. The
        # clock is pinned to a weekday, so there is always a today row and
        # this assertion always runs.
        weights = app.d.execute_script(
            "return [...document.querySelectorAll("
            "  \"section[aria-label='Master grid'] table.tt tbody tr\")]"
            ".map(r => [r.classList.contains('today'),"
            "  getComputedStyle(r.querySelector('th.rowhead')).fontWeight]);")
        today = [w for is_today, w in weights if is_today]
        others = {w for is_today, w in weights if not is_today}
        assert today and today[0] in others, \
            f"today must print the siblings' weight, got {today} vs {others}"

        # Hall names get their width back: no nine-line letter stack.
        print_media(False)
        app.open_tab("Halls")
        section = app.wait_css("section[aria-label='Lecture halls']")
        section.find_element(
            By.XPATH, ".//div[@aria-label='Day view']//button[normalize-space()='Week']"
        ).click()
        app.wait_css("section[aria-label='Lecture halls'] table.tt th.hallhead")
        print_media(True)
        # A rowspan-5 hall cell is legitimately tall; what letter-stacking
        # actually looks like is the hall NAME wrapping to many lines.
        stacked = app.d.execute_script(
            "return [...document.querySelectorAll("
            "  \"section[aria-label='Lecture halls'] table.tt"
            "   th.rowhead .hall-name\")]"
            ".map(n => n.getBoundingClientRect().height /"
            "  (parseFloat(getComputedStyle(n).lineHeight) || 16))"
            ".filter(lines => lines > 2.5).length;")
        assert stacked == 0, \
            f"{stacked} hall names letter-stack in the print clamp"
    finally:
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": ""})
        # An injected script outlives the test that added it, so this has to
        # happen however the test ends: leaving it in would shift the clock
        # for every test after this one.
        app.unpin_weekday(pinned)


def t105_arrow_keys_walk_the_tab_rail(app):
    """The rail has always claimed role=tablist; now it behaves like one.
    Both axes work (the rail is a column on a desktop and a bar on a phone),
    Home/End jump to the ends, the whole rail is ONE Tab stop, and — the
    thing the user worried about — arrows still scroll the page whenever a
    tab is not the thing in focus."""
    app.boot("/", selection=["TOC", "RDBM"])
    app.wait_css("nav.tabs")

    def selected():
        return app.css("nav.tabs button[aria-selected='true']").text

    def focus_selected_tab():
        app.d.execute_script(
            "document.querySelector(\"nav.tabs button[tabindex='0']\").focus();")

    # Exactly one Tab stop, not five — this is what makes the arrows worth
    # having rather than a duplicate of Tab.
    stops = app.css_all("nav.tabs button[tabindex='0']")
    assert len(stops) == 1, f"the rail must be one Tab stop, found {len(stops)}"
    assert len(app.css_all("nav.tabs button")) == 5

    focus_selected_tab()
    assert selected() == "My timetable"

    body = app.d.switch_to.active_element
    body.send_keys(Keys.ARROW_DOWN)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "My courses")
    # The other axis does the same thing, because the rail changes direction
    # with the viewport and a dead key is worse than a redundant one.
    app.d.switch_to.active_element.send_keys(Keys.ARROW_RIGHT)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Master grid")
    app.d.switch_to.active_element.send_keys(Keys.ARROW_UP)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "My courses")
    app.d.switch_to.active_element.send_keys(Keys.ARROW_LEFT)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "My timetable")

    # Wraps at both ends.
    app.d.switch_to.active_element.send_keys(Keys.ARROW_UP)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Halls")
    app.d.switch_to.active_element.send_keys(Keys.ARROW_DOWN)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "My timetable")

    app.d.switch_to.active_element.send_keys(Keys.END)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Halls")
    app.d.switch_to.active_element.send_keys(Keys.HOME)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "My timetable")

    # Focus follows the choice, so the next arrow continues from here.
    assert app.d.switch_to.active_element.text == "My timetable"

    # THE thing the user was worried about: with focus anywhere else, the
    # arrows still belong to the page. The handler lives on the nav, so it
    # cannot even see this event.
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog']")
    app.d.execute_script("window.scrollTo(0, 0); document.body.focus();")
    before = app.d.execute_script("return window.scrollY;")
    app.d.find_element(By.TAG_NAME, "body").send_keys(Keys.ARROW_DOWN)
    time.sleep(0.4)
    after = app.d.execute_script("return window.scrollY;")
    assert after >= before, "arrows outside the rail must still scroll the page"
    assert selected() == "Catalog", \
        "an arrow pressed outside the rail must not change tab"


def t126_every_section_prints_itself_and_only_itself(app):
    """Every one of the five sections is printable, offers its own Print
    button, and puts exactly ONE sheet on the paper.

    Both halves were asked for. Until R80 only My timetable had a Print
    button, so the other four could be printed only by knowing about Ctrl+P —
    and three of them (My courses, Master grid, Catalog) had no print design
    at all: no title, no term, no provenance, and every row carrying its
    buttons. A 13-page Catalog printout with nothing on any page saying what
    it was is why "it concatenates everything into one PDF" is a reasonable
    thing to conclude from the output.

    It never did. Only one tab is mounted at a time, which is what the
    cross-section assertion below pins: whatever else changes, printing the
    Master grid can never also print the Catalog."""
    sections = {
        "My timetable": "My timetable",
        "My courses": "My courses",
        "Master grid": "Master grid",
        "Catalog": "Catalog",
        "Halls": "Hall bookings",   # the sheet's title is not the tab's label
    }
    app.boot("/", selection=["TOC", "RDBM", "MFD"])
    try:
        app.dismiss_toasts()
    except Exception:
        pass
    for label, sheet_title in sections.items():
        app.open_tab(label)
        sel = "Lecture halls" if label == "Halls" else label
        app.wait_css(f"section[aria-label='{sel}']")

        # Its own Print button, and a live one — every section has something
        # to print in this state.
        buttons = [b for b in app.css_all(
            f"section[aria-label='{sel}'] .toolbar button")
            if b.text.strip() == "Print"]
        assert len(buttons) == 1, \
            f"{label}: expected exactly one Print button, found {len(buttons)}"
        assert buttons[0].is_enabled(), f"{label}: Print is disabled"

        # Its own masthead, naming this sheet. `print-only` means it is
        # `display: none` on screen, so read the DOM rather than `.text`.
        titles = app.d.execute_script("""
            return [...document.querySelectorAll('.print-masthead .pm-title')]
                .map((e) => e.textContent.trim());
        """)
        assert titles == [sheet_title], \
            f"{label}: masthead titles {titles!r}, expected [{sheet_title!r}]"

        # And nothing else is on the page to print. This is the assertion that
        # keeps one section's sheet from ever becoming five.
        others = [o for o in sections if o != label]
        for other in others:
            osel = "Lecture halls" if other == "Halls" else other
            assert not app.css_all(f"section[aria-label='{osel}']"), \
                f"printing {label} would also print {other}"

    # A section with nothing in it says so instead of offering a blank sheet —
    # the rule the Export button beside it already followed.
    app.boot("/", selection=[])
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    btn = next(b for b in app.css_all(
        "section[aria-label='My courses'] .toolbar button")
        if b.text.strip() == "Print")
    assert not btn.is_enabled(), \
        "Print must be disabled with nothing on the timetable"
    assert "nothing to print" in (btn.get_attribute("title") or ""), \
        f"a disabled Print must say why, got {btn.get_attribute('title')!r}"


def t127_printing_never_repaints_the_app(app):
    """Pressing Print must not put the page you are looking at into print
    media.

    `window.print()` does exactly that, and — because the call sits on the
    stack until the modal closes — leaves it there. Measured in real Brave 151
    (`.workagents/print-r80/probes/brave_print_repro.py`): 8.8 seconds of a
    dark app repainted as a white sheet with no chrome, against 17ms for the
    same print started with Ctrl+P, which the browser can switch back out of
    immediately. Anything that forces a repaint in that window paints the
    sheet; under print emulation the page measures 243-252/255 brightness even
    in the dark theme.

    So the button prints a copy in a context of its own (`domx::print_sheet`).
    This test pins three things about that context, each of which has been
    wrong once:

    * it is a separate TOP-LEVEL context. A hidden iframe was tried first and
      does NOT work — Chromium sets the printing state across the frame tree.
    * it is opened with NO features string. A features string makes it a popup
      sized by this code, and on a 2560px screen the guessed 1100px left
      Chromium's preview about 610px wide beside its ~380px settings panel: a
      postage-stamp sheet, a scrollbar, a white void. Without features it is a
      tab in the window the reader already sized, so the dialog is the Ctrl+P
      dialog. Verified by screenshot in real Brave and real Chromium at
      2560x1600 (`probes/print_dialog_look.py`); pinned here so it cannot come
      back silently.
    * the sheet and the app's stylesheet actually arrive in it — an empty or
      unstyled print tab prints a blank page, and nothing in the app's own DOM
      would show it.

    Headless has no print dialog, so `print()` here is close to a no-op; what
    it does still do is fire `beforeprint` at whichever window is printed,
    which is the signal the second assertion reads."""
    app.boot("/", selection=["TOC", "RDBM"])
    try:
        app.dismiss_toasts()
    except Exception:
        pass
    app.d.execute_script("""
        window.__openCalls = [];
        window.__beforePrint = 0;
        window.addEventListener('beforeprint', () => { window.__beforePrint++; });
        const real = window.open.bind(window);
        window.open = (u, t, f) => {
          window.__openCalls.push([t, f === undefined ? null : String(f)]);
          const w = (window.__printWin = real(u, t, f));
          // Count the print tab's OWN print() calls. Same origin, so this
          // patch reaches into it — and it has to be installed by polling,
          // because the tab is still loading when open() returns and the
          // patch must be in place before its MutationObserver fires (the app
          // needs a tick to inject the sheet, then print.html waits 90ms).
          // Without this the test passed with print.html's window.print()
          // DELETED: its "Printing…" status line is set just before the call,
          // so the status alone proves nothing (R82's test-quality agent).
          window.__printed = 0;
          const arm = setInterval(() => {
            try {
              if (!w || w.closed) { clearInterval(arm); return; }
              if (w.__armed) return;
              w.__armed = true;
              const p = w.print;
              w.print = function () { window.__printed++; return p.apply(this, arguments); };
              clearInterval(arm);
            } catch (e) { /* mid-navigation: try again next tick */ }
          }, 5);
          return w;
        };
    """)
    app_handle = app.d.current_window_handle
    btn = next(b for b in app.css_all(
        "section[aria-label='My timetable'] .toolbar button")
        if b.text.strip() == "Print")
    btn.click()
    # Polled, not slept: the app's own budget for this tab is 20s, and a
    # fixed 3s wait was shorter than the thing it was waiting for.
    app.wait_for_print_tab(app_handle)

    app.d.switch_to.window(app_handle)
    calls = app.d.execute_script("return window.__openCalls;")
    assert calls == [["cmitt-print", None]], (
        "Print must open its own tab and pass NO window features (a features "
        f"string is a popup this code has to size): calls were {calls!r}")
    fired = app.d.execute_script("return window.__beforePrint;")
    assert fired == 0, \
        ("the live document was printed — beforeprint fired on it "
         f"{fired} time(s), so the app repaints as the sheet while the "
         "dialog is open")

    # What the print tab is holding, read THROUGH the opener's own reference
    # to it: same origin, so no tab switch and no race with the tab closing
    # itself. It may already have closed — its `afterprint` handler does that —
    # and that is itself the answer, since only printing gets it there.
    got = app.d.execute_script("""
        const w = window.__printWin;
        if (!w) return {opened: false};
        if (w.closed) return {opened: true, closed: true};
        const d = w.document;
        const sheet = d.getElementById('sheet');
        const css = d.getElementById('cmitt-app-css');
        return {
          opened: true, closed: false,
          url: d.location.pathname,
          title: d.title,
          status: (d.getElementById('status') || {}).textContent,
          sheets: sheet ? sheet.children.length : -1,
          mine: !!(sheet && sheet.querySelector(
              'section[aria-label="My timetable"] .print-masthead')),
          css: css ? css.textContent.length : 0,
        };
    """)
    assert got.get("opened"), "Print opened nothing at all"
    if not got.get("closed"):
        assert str(got.get("url", "")).endswith("print.html"), \
            f"the tab Print opened is not print.html: {got!r}"
        assert got["sheets"] == 1 and got["mine"], \
            f"the sheet did not arrive in the print tab: {got!r}"
        assert got["css"] > 10000, \
            f"the app's stylesheet did not arrive in the print tab: {got!r}"
        # The print header prints document.title, so it has to be the app's
        # name rather than "print.html".
        assert got["title"] == "CMI Timetable Planner", \
            f"the print tab is titled {got['title']!r}"
        # And it raised the dialog ITSELF rather than waiting to be printed
        # from the app. The counter installed on the tab's own `print` is the
        # evidence; the status line is not, because it is written on the line
        # before the call and survives the call's deletion.
        printed = app.d.execute_script("return window.__printed;")
        assert printed and printed >= 1, (
            "print.html never called print() on itself — the sheet would sit "
            f"in a tab and nothing would print (calls: {printed!r}, status "
            f"{got['status']!r})")
        assert "Printing" in str(got["status"]) or "Printed" in str(got["status"]), \
            f"the print tab never said what it was doing: status {got['status']!r}"

        # Press it again. The tab is named, so this one refills the same tab —
        # and it has to REPLACE the sheet, not stack a second one under it.
        # (Chromium RE-NAVIGATES the named tab, so the fresh document is what
        # does the clearing; `domx.rs`'s own `set_inner_html("")` is belt and
        # braces. Recorded so this assertion is not mistaken for coverage of
        # that line — R83's test-quality read.)
        btn.click()
        # 25s, not 12: the app's own budget for a print tab to load and be
        # filled is 20s, and a test that waits less than the thing it is
        # waiting for is a red light that means "this machine is busy".
        WebDriverWait(app.d, 25).until(
            lambda d: d.execute_script(
                "try { const s = window.__printWin.document"
                ".getElementById('sheet');"
                "return !!(s && s.children.length); } catch (e) { return false; }"),
            message="the second press never refilled the print tab")
        again = app.d.execute_script("""
            const d = window.__printWin.document;
            const sheet = d.getElementById('sheet');
            return {sheets: sheet ? sheet.children.length : -1,
                    styles: d.querySelectorAll('#cmitt-app-css').length};
        """)
        assert again == {"sheets": 1, "styles": 1}, \
            f"a second press stacked things up in the print tab: {again!r}"

        for handle in [h for h in app.d.window_handles if h != app_handle]:
            app.d.switch_to.window(handle)
            app.d.close()
        app.d.switch_to.window(app_handle)


def t128_tight_rows_still_print_the_real_time(app):
    """A printed sheet must not depend on the window it was printed from — and
    the ROW HEIGHT is part of that window.

    `.density-compact .chip .hall { display: none }` (0,3,0) out-specified the
    print block's `.chip .hall` (0,2,0). On the Master grid that span is not a
    hall: `show_hall` is false there, so it carries the SUBLABEL — the meeting's
    real time when it differs from the column it borrows. Tight rows therefore
    printed a class under a heading that gave the wrong time, with nothing on
    the paper correcting it. `App::device_density` returns Compact for any
    phone-sized viewport, so a reader who never touched the toggle got the
    lossy sheet (R82's audit measured it: the desktop PDF contains "09:10–14:00",
    the phone PDF does not).

    The band under a covered slot rode along: `.density-compact .covered`
    (0,2,0) beat print's `.covered` (0,1,0) and printed BIGGER than the chips
    above it, inverting the hierarchy."""
    # TOC's Tuesday class, moved to 09:30-10:20 — entirely INSIDE the
    # 09:10-10:25 column, so no synthetic column is minted and no continuation
    # band is cast. The chip's second line is then the ONLY thing on the sheet
    # that says when the class is, which is what makes this a data defect
    # rather than a cosmetic one.
    inside_the_column = {
        "next_id": 1,
        "items": [{
            "id": 0, "course": "TOC",
            "base": {"day": "Tue", "slot": {"start_min": 550, "end_min": 625},
                     "hall": "Lecture Hall 803", "temp_booking": False},
            "to": {"day": "Tue", "slot": {"start_min": 570, "end_min": 620},
                   "hall": "Lecture Hall 803", "temp_booking": False},
            "created_at": 1754000000000.0}],
        "credits": [],
    }
    app.d.set_window_size(430, 900)
    try:
        app.boot("/", selection=["TOC", "RDBM", "MFD"],
                 overrides=inside_the_column)
        app.open_tab("Master grid")
        app.wait_css("section[aria-label='Master grid'] table.tt")
        assert app.css_all("section[aria-label='Master grid'] .density-compact"), \
            "this test is meaningless without the compact grid"
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": "print"})
        got = app.d.execute_script("""
            const hall = document.querySelector(
                "section[aria-label='Master grid'] table.tt td .chip .hall");
            const band = document.querySelector(
                "section[aria-label='Master grid'] .covered");
            return {
              hall: hall ? getComputedStyle(hall).display : 'none-found',
              hallPx: hall ? getComputedStyle(hall).fontSize : null,
              bandPx: band ? getComputedStyle(band).fontSize : null,
            };
        """)
        # `block` and 7.8px specifically, not merely "not none": the print
        # block has to be the rule that GOVERNS this line. An `inline` here
        # would mean neither print nor density is winning it, which is the
        # same class of accident as the bug (and is what a broken stylesheet
        # looks like, so it also keeps this test honest about its own setup).
        assert got["hall"] == "block", (
            "tight rows print a chip with no second line, so a meeting whose "
            f"real time differs from its column loses it: {got!r}")
        assert got["hallPx"] and abs(float(got["hallPx"].rstrip("px")) - 7.8) < 0.6, (
            f"the print block must set this line's size, not the screen: {got!r}")
        if got["bandPx"]:
            assert float(got["bandPx"].rstrip("px")) <= 9.5, (
                "the continuation band must not print bigger than the chips "
                f"above it: {got!r}")
    finally:
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": ""})
        app.d.set_window_size(1500, 1000)


def t129_the_print_tab_puts_nothing_dark_on_the_paper(app):
    """The print tab carries its own screen design in the reader's theme, and
    exactly one declaration of that design must not reach paper.

    `color-scheme: dark` sat at the top level of print.html's stylesheet, so it
    applied in PRINT media too. The root element has no background of its own,
    so Chromium painted the page CANVAS with its dark UA default while
    `body { background: #fff }` covered only the page area — an 11-12mm
    near-black frame on all four sides of every page, ~18% of every sheet, for
    any reader with the theme dark and "Background graphics" ticked. Measured
    off the PDFs in R82's audit: page corner rgb(18,18,18), against
    rgb(255,255,255) once `color-scheme: light` is forced.

    It cannot be fixed from styles.css: print.html hard-codes
    data-theme="light", so the app's print reset only matches the bare `:root`
    (0,1,0) and loses to `:root[data-appearance="dark"]` (0,2,0). The fix lives
    in print.html, and this test reads the print tab itself — nothing else in
    the suite loads that document."""
    app.boot("/", selection=["TOC", "RDBM"])
    try:
        app.dismiss_toasts()
    except Exception:
        pass
    app.d.execute_script(
        "localStorage.setItem('cmitt.v1.prefs', JSON.stringify("
        "{theme: 'Dark', last_update_attempt: Date.now()}));")
    app.d.refresh()
    app.wait_css("section[aria-label='My timetable']")
    app_handle = app.d.current_window_handle
    btn = next(b for b in app.css_all(
        "section[aria-label='My timetable'] .toolbar button")
        if b.text.strip() in ("Print", "Printing…"))
    btn.click()
    app.wait_for_print_tab(app_handle)
    handles = [h for h in app.d.window_handles if h != app_handle]
    assert handles, "Print opened no tab"
    try:
        app.d.switch_to.window(handles[0])
        # The tab took the reader's theme for the SCREEN…
        appearance = app.d.execute_script(
            "return document.documentElement.dataset.appearance;")
        assert appearance == "dark", \
            f"the print tab ignored the reader's dark theme: {appearance!r}"
        screen_scheme = app.d.execute_script(
            "return getComputedStyle(document.documentElement).colorScheme;")
        assert "dark" in screen_scheme, \
            f"on screen the dark tab should be dark: {screen_scheme!r}"
        # …and nothing of it on PAPER.
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": "print"})
        printed = app.d.execute_script("""
            return {
              scheme: getComputedStyle(document.documentElement).colorScheme,
              body: getComputedStyle(document.body).backgroundColor,
              prep: getComputedStyle(document.querySelector('.pw-prep')).display,
            };
        """)
        assert "dark" not in printed["scheme"], (
            "a dark color-scheme on the print tab's root paints the page "
            f"canvas — an 11mm black frame on every page: {printed!r}")
        assert printed["body"] in ("rgb(255, 255, 255)", "rgba(0, 0, 0, 0)"), \
            f"the printed page must be white: {printed!r}"
        assert printed["prep"] == "none", \
            f"the tab's own screen card must not print: {printed!r}"
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": ""})
        app.d.close()
    finally:
        app.d.switch_to.window(app_handle)


def t130_a_phone_never_scrolls_sideways(app):
    """The app must not scroll sideways on a phone — and until R82 it did, at
    320px and 360px, with no dialog open at all.

    One element set the floor: a whole sentence inside a `.badge`, which is
    `white-space: nowrap`. "CMI lists these courses but hasn't put them on the
    timetable" measured 338px and could not break, so the document's minimum
    width became ~368px and two more elements were dragged out of view behind
    it (the "Halls" tab at r=346, the toast rail at r=329). The same defect in
    `ui::status_badges` cut a sentence off mid-word inside the details dialog at
    every phone width. Both now carry `.badge.wraps`.

    This is the assertion open bug 8.20 asked any fix to come with, including
    its condition: measured WITH THE TRAY ON SCREEN, which needs a selected
    course that has no time (SVA is unscheduled in the fixture). A fixture
    without one never renders the sentence, and the suite passed for months."""
    try:
        for width in (320, 360, 390):
            app.d.execute_cdp_cmd("Emulation.setDeviceMetricsOverride", {
                "width": width, "height": 760, "deviceScaleFactor": 2,
                "mobile": True,
            })
            app.boot("/", selection=["TOC", "SVA", "RDBM"])
            app.wait_css("section[aria-label='My timetable']")
            got = app.d.execute_script("""
                const doc = document.documentElement;
                const tray = [...document.querySelectorAll('h3')]
                    .find((h) => h.textContent.includes('No fixed slot yet'));
                const badge = tray && tray.querySelector('.badge');
                // Content inside a horizontally scrollable box is SUPPOSED to
                // extend past the viewport — that is what the box is for (the
                // week table in `.grid-scroll`, the tab rail). What must not
                // happen is an element reaching past the edge with nothing to
                // scroll it, because then it is simply unreachable.
                const scrollable = (el) => {
                  for (let p = el.parentElement; p; p = p.parentElement) {
                    const ox = getComputedStyle(p).overflowX;
                    if (ox === 'auto' || ox === 'scroll') return true;
                  }
                  return false;
                };
                const wide = [...document.querySelectorAll('body *')]
                    .filter((el) => el.getBoundingClientRect().right
                                    > doc.clientWidth + 1 && !scrollable(el))
                    .slice(0, 4)
                    .map((el) => (el.tagName + '.' + (el.className || '')).slice(0, 48)
                                 + ' r=' + Math.round(el.getBoundingClientRect().right));
                return {
                  client: doc.clientWidth,
                  scroll: document.scrollingElement.scrollWidth,
                  trayShown: !!tray,
                  badgeWrap: badge ? getComputedStyle(badge).whiteSpace : null,
                  overflowing: wide,
                };
            """)
            assert got["trayShown"], (
                "the tray with the long sentence must be on screen or this "
                f"test proves nothing: {got!r}")
            assert got["badgeWrap"] == "normal", \
                f"the sentence badge must be allowed to wrap: {got!r}"
            assert got["scroll"] <= got["client"] + 1, (
                f"the document scrolls sideways at {width}px: {got!r}")
            assert not got["overflowing"], (
                f"elements hang outside the viewport at {width}px: {got!r}")

            # …and with a filter menu OPEN, which is when it used to be worst:
            # a 304px menu anchored to a summary in the right half of a
            # wrapped bar ran up to 204px past the edge (R83).
            #
            # Asked by SCROLLING, not by comparing widths. `clientWidth`
            # includes the channel `scrollbar-gutter: stable` reserves, so it
            # over-reports the content edge by the gutter and the comparison
            # above cannot see the last two pixels — which is exactly the
            # amount `place_facet_menu` used to overhang by, at every width,
            # until it stopped reading `clientWidth` (R84).
            app.open_tab("Catalog")
            app.wait_css("section[aria-label='Catalog'] details.facet")
            facets = app.css_all("section[aria-label='Catalog'] details.facet")
            assert len(facets) >= 5, f"expected a filter bar to test: {len(facets)}"
            for i in range(len(facets)):
                worst = app.d.execute_script(
                    "const f = document.querySelectorAll("
                    "  \"section[aria-label='Catalog'] details.facet\")[arguments[0]];"
                    "document.querySelectorAll('details.facet[open]')"
                    "  .forEach((d) => d.removeAttribute('open'));"
                    "f.setAttribute('open', '');"
                    "f.dispatchEvent(new Event('toggle'));"
                    "return new Promise((done) => requestAnimationFrame(() =>"
                    "  requestAnimationFrame(() => {"
                    "    const x0 = window.scrollX;"
                    "    window.scrollTo(9999, window.scrollY);"
                    "    const x = window.scrollX;"
                    "    window.scrollTo(x0, window.scrollY);"
                    "    const name = f.querySelector('summary')?.textContent?.trim();"
                    "    done({x, name});"
                    "  })));",
                    i)
                assert worst["x"] == 0, (
                    f"the page scrolls {worst['x']}px sideways at {width}px with the "
                    f"{worst['name']!r} menu open")
            app.d.execute_script(
                "document.querySelectorAll('details.facet[open]')"
                "  .forEach((d) => d.removeAttribute('open'));")
    finally:
        app.d.execute_cdp_cmd("Emulation.clearDeviceMetricsOverride", {})

    # There is no desktop counterpart to the check above, and the reason is
    # worth writing down. The residual bug it would target — a menu placed
    # against `documentElement.clientWidth`, which INCLUDES the channel
    # `scrollbar-gutter: stable` reserves, overhanging the real content edge
    # by `gutter - 8` pixels — needs the gutter reserved AND the nudge to
    # fire, and those two want opposite pages: the gutter only shows itself
    # on a page short enough not to scroll, and the nudge only fires when a
    # facet has enough options to open a wide menu near the right edge. Every
    # combination tried here reproduced neither (measured: 320/360/1500 px,
    # mobile and not, Catalog filtered empty and My courses seeded). R84's
    # visual verification DID measure it, on its own state, and the fix is in
    # `domx::place_facet_menu` — it now reads `getBoundingClientRect()`, the
    # border box itself. Recorded rather than pinned, so nobody counts this
    # test as coverage of it.


def t131_a_link_that_names_nothing_here_keeps_your_timetable(app):
    """A link cannot empty a timetable by naming courses that do not exist.

    `apply_url_state` ended in `*sel = known`, and `known` is what SURVIVED
    resolution. Open an old bookmark, a link from a semester whose codes CMI
    has retired, or a friend's hand-made courses sent without their
    definitions, and `known` is empty — so the reader's stored selection was
    overwritten with nothing, permanently, while the banner said "Everything
    else in the link opened as usual". Two independent R82 audit agents rated
    it a blocker; it predates the unpushed work, so it was live.

    Both halves are pinned here: the timetable survives, AND the banner stops
    claiming that everything else opened."""
    app.boot("/", selection=["TOC", "RDBM", "MFD"])
    before = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));")
    assert len(before) == 3, f"fixture did not seed three courses: {before!r}"

    # Every code unknown: nothing in this link exists here.
    app.d.get(f"{BASE}/?c=ZZZ1,ZZZ2")
    app.wait_css("section[aria-label='My timetable']")
    time.sleep(1.0)
    after = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));")
    assert after == before, (
        f"a link naming only unknown courses emptied the timetable: "
        f"{before!r} -> {after!r}")
    note = app.d.execute_script(
        "const n = document.querySelector('.unknown-codes .banner-note');"
        "return n ? n.textContent.trim() : null;")
    assert note, "the unknown-codes banner did not appear"
    assert "left exactly as it was" in note, (
        f"the banner must not promise the rest of the link opened: {note!r}")

    # And the mixed case still works: the known half opens, the rest is named.
    app.d.get(f"{BASE}/?c=TOC,ZZZ9")
    app.wait_css("section[aria-label='My timetable']")
    time.sleep(1.0)
    mixed = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));")
    assert mixed == ["TOC"], f"the known half of a link must still open: {mixed!r}"
    note2 = app.d.execute_script(
        "const n = document.querySelector('.unknown-codes .banner-note');"
        "return n ? n.textContent.trim() : null;")
    assert note2 and "opened as usual" in note2, (
        f"with a course actually opened, the banner should say so: {note2!r}")


def t132_a_damaged_link_with_no_codes_changes_nothing(app):
    """`?c=&s=<garbage>`: the readable half carries no codes, so there is
    nothing to open — and FEATURES.md promises a damaged link changes nothing.

    The guard tested `c.is_none()`, which a blank `c=` passes, so this fell
    through to the apply path with an empty selection and wiped the timetable.
    t123 covers the damaged link WITHOUT a c= at all; this is its sibling."""
    app.boot("/", selection=["TOC", "RDBM"])
    before = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));")
    app.d.get(f"{BASE}/?c=&s=not-a-real-payload")
    app.wait_css("section[aria-label='My timetable']")
    time.sleep(1.0)
    after = app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));")
    assert after == before, (
        f"a damaged link with a blank c= emptied the timetable: "
        f"{before!r} -> {after!r}")
    assert app.css_all(".banner.warn"), \
        "a damaged link must still say it was damaged"


def t133_one_working_helper_site_is_enough(app):
    """R84 — the outage that started it: this app can only reach CMI through
    somebody else's server (CMI's pages carry no `Access-Control-Allow-Origin`,
    so a browser will not let a page on github.io read them), and it shipped
    exactly TWO helper sites. One went down and the other started charging,
    within days of each other, and every reader's Sync died with no route
    left. Four now, each a different operator — so this asserts what that is
    for: any ONE of them alive carries the sync on its own, and the app says
    which one it used."""
    # Every shipped relay, one at a time: seven operators now, and the point
    # of seven is that ANY ONE of them carries a sync on its own.
    for host, shown in (
        ("proxy.cors.sh", "cors.sh"),
        ("cors-get-proxy.sirjosh.workers.dev", "cors-get-proxy"),
        ("corsmirror.onrender.com", "corsmirror"),
        ("r.jina.ai", "r.jina.ai"),
        ("api.allorigins.win", "allorigins.win"),
        ("api.codetabs.com", "codetabs.com"),
        ("api.cors.lol", "cors.lol"),
    ):
        serve_cmi()
        serve_relays(only={host})
        try:
            app.boot("/", seed=False)
            app.wait_css(".tabs .tab", timeout=30)
            app.wait_gone(".welcome-card")
            title = app.css(".sync-pill").get_attribute("title")
            assert f"through the helper site {shown}" in title, \
                f"only {host} was alive, so it had to be the route: {title}"
            # And nothing reached cmi.ac.in itself: a live relay is still the
            # first tier, whichever of the four it happens to be.
            tiers = fetch_log_tiers(app)
            assert all(t.startswith("proxy:") for t in tiers), tiers
        finally:
            stop_serving_cmi()


def t134_a_helper_site_you_supply_is_tried_first(app):
    """R84 — the half of the repair that does not need a new version of the
    app. When every helper site the app knows is gone, a reader (or CMI) can
    paste one of their own into My data, and it is tried BEFORE the shipped
    list. Here every shipped relay is dead and the reader's own is the only
    thing answering, which is exactly the state that broke the live app."""
    serve_cmi()
    serve_relays(only={"helper.example"})
    try:
        app.boot("/", seed=False, prefs={
            "helper_site": "https://helper.example/get?url={url}",
        })
        app.wait_css(".tabs .tab", timeout=30)
        app.wait_gone(".welcome-card")
        title = app.css(".sync-pill").get_attribute("title")
        assert "through the helper site your helper site" in title, title
        tiers = fetch_log_tiers(app)
        assert all(t.startswith("proxy:") for t in tiers), tiers
        # FIRST, and alone: the reader's own helper site gets a head start,
        # so a run where it answers inside `HELPER_HEAD_START_MS` never asks
        # a shipped relay at all. That is the promise — someone who set up
        # their own does not go on handing CMI's address to four strangers.
        assert set(tiers) == {"proxy:your helper site"}, tiers

        # And the app remembers what worked, so the next sync starts there.
        stored = app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.prefs')||'{}')")
        assert stored.get("last_good_route") == "your helper site", stored
    finally:
        stop_serving_cmi()


def t135_you_can_load_the_timetable_from_cmis_own_page(app):
    """R84 — the route that cannot rot. Every other way in needs some server
    to agree to hand CMI's bytes to a page served from github.io. Opening a
    page is the one thing a browser never has to ask anyone about, so when
    nothing will fetch the timetable the reader can open CMI themselves and
    hand it over. Same parser, same validation gate, same adoption as a live
    fetch — and the app says the data came that way rather than claiming it
    fetched it."""
    # Exactly what a reader met: CMI's pages answer, they carry no
    # `Access-Control-Allow-Origin`, so the browser will not let the app read
    # them — and every helper site is unavailable. No tabs to wait for (an app
    # with no data has none, t25), so the welcome card says the first render
    # happened.
    serve_cmi(cors=False)
    try:
        app.boot("/", seed=False)
        app.wait_css(".welcome-card", timeout=30)
        banner = app.wait_css(".banner.warn", timeout=60)
        assert "isn't allowed to read it" in banner.text, banner.text
        # The banner offers the way out as a BUTTON, not as advice to read.
        button = banner.find_element(
            By.XPATH, ".//button[contains(., \"Load it from CMI's page\")]")
        button.click()
    finally:
        stop_serving_cmi()
    app.wait_css(".dialog")

    with open(os.path.join(FIXTURES, "timetable.php.html"), encoding="utf-8") as f:
        timetable = f.read()
    with open(os.path.join(FIXTURES, "lecturehalls.php.html"), encoding="utf-8") as f:
        halls = f.read()

    boxes = app.css_all(".dialog textarea.load-paste")
    assert len(boxes) == 2, f"one box per CMI page, got {len(boxes)}"
    load = app.xpath("//div[@class='dialog']//button[normalize-space()='Load these pages']")
    assert not load.is_enabled(), "nothing pasted yet — there is nothing to load"

    # Pasting the page's TEXT rather than the page is the commonest mistake,
    # and it gets its own sentence instead of a gate failure about CMI.
    app.d.execute_script(
        "arguments[0].value = arguments[1];"
        "arguments[0].dispatchEvent(new Event('input', {bubbles: true}));",
        boxes[0], "Monday 09:10 ALG3 Lecture Hall 5")
    app.d.execute_script(
        "arguments[0].value = arguments[1];"
        "arguments[0].dispatchEvent(new Event('input', {bubbles: true}));",
        boxes[1], "Lecture Hall 5")
    load.click()
    error = app.wait_css(".dialog .form-error").text
    assert "the text off it" in error and "Ctrl+U" in error, error

    for box, body in zip(boxes, (timetable, halls)):
        app.d.execute_script(
            "arguments[0].value = arguments[1];"
            "arguments[0].dispatchEvent(new Event('input', {bubbles: true}));",
            box, body)
    load.click()
    app.wait_gone(".dialog")
    app.wait_toast("Timetable updated from CMI's own page.")

    title = app.css(".sync-pill").get_attribute("title")
    assert "from CMI's page, loaded by you" in title, title
    # Real data, through the real gate — not a placeholder that says "synced".
    courses = app.d.execute_script(
        "return (JSON.parse(localStorage.getItem('cmitt.v1.snapshot')||'{}')"
        ".courses||[]).length")
    assert courses > 10, f"the pages had to produce a real catalog, got {courses}"
    # And the failure banner that sent them here is gone.
    assert not app.css_all(".banner.warn"), "the banner outlived the fix"


def t136_a_sync_failure_says_which_thing_failed(app):
    """R84 — the app told everyone the same thing however it failed: "CMI's
    website couldn't be reached". It said it to readers who had CMI's page
    open and working in the next tab, which is how the outage went unreported
    for as long as it did.

    Three failures, three sentences. CMI answering an ERROR is visible only
    because a status the page can read proves the cross-origin rule was not
    what stopped it — that is the whole signal, and it is why this case can
    never be confused with the others."""
    def banner_after_boot():
        app.boot("/", seed=False)
        app.wait_css(".welcome-card", timeout=30)
        return app.wait_css(".banner.warn", timeout=60).text

    def offers_the_way_out():
        """The BUTTON, by its label. `.banner.warn button` also matches the
        unconditional Dismiss, so asserting on that proved nothing — a
        mutation deleting the action entirely left it green (R84 review)."""
        return app.d.find_elements(
            By.XPATH,
            "//div[contains(@class,'banner')]"
            "//button[contains(., \"Load it from CMI's page\")]")

    # (1) CMI answers, with an error, on every route. The app can READ a
    # status, which is only possible when the cross-origin rule allowed it —
    # so whatever went wrong, it was not that.
    serve_cmi()
    _cmi["bodies"] = {}          # up, but nothing at either path -> 503
    try:
        text = banner_after_boot()
        assert "answered, but with an error" in text, text
        assert "HTTP 503" in text, text
        assert "not your connection" in text, text
        # …and NO way-out button here. The direct route runs in the reader's
        # own browser, so a 503 to the app is a 503 in the tab they would
        # open by hand: offering a route that cannot work is worse than
        # offering none.
        assert not offers_the_way_out(), text
    finally:
        stop_serving_cmi()

    # (2) The real cmi.ac.in, exactly: the pages answer 200 and send no
    # `Access-Control-Allow-Origin`, so the browser refuses to let the app
    # read them. No status is visible, and only the `no-cors` probe can tell
    # this apart from a dead network. This is the case the live outage was,
    # and the one the app used to report as "CMI couldn't be reached".
    serve_cmi(cors=False)
    try:
        text = banner_after_boot()
        assert "isn't allowed to read it" in text, text
        assert "Every helper site it knows is unavailable" in text, text
        assert offers_the_way_out(), text
    finally:
        stop_serving_cmi()

    # (3) Nothing at that address at all — the connection is dropped without
    # an answer, so even the probe gets nothing back.
    _cmi["dead"] = True
    try:
        text = banner_after_boot()
        assert "Nothing answered" in text, text
        assert offers_the_way_out(), text
    finally:
        _cmi["dead"] = False
        stop_serving_cmi()

    # (4) ONE of CMI's two pages errors and the other is fine. The direct
    # tier logs an entry per page, so a version of this that read only the
    # LAST one missed the 503 entirely and fell through to "CMI is up and we
    # aren't allowed to read it" — the very sentence this feature exists to
    # stop being said wrongly. Found by R84's own adversarial review, with a
    # repro; this is that repro.
    serve_cmi()
    with open(os.path.join(FIXTURES, "lecturehalls.php.html"), encoding="utf-8") as f:
        halls_body = f.read()
    _cmi["bodies"] = {"/practical/lecturehalls.php": halls_body}
    try:
        text = banner_after_boot()
        assert "answered, but with an error" in text, \
            f"a status on EITHER page proves the browser let us read it: {text}"
        assert "isn't allowed to read it" not in text, text
        assert not offers_the_way_out(), text
    finally:
        stop_serving_cmi()


def t137_the_route_that_worked_last_time_is_the_only_one_asked(app):
    """R84. Remembering which route worked is only worth anything if the app
    actually withholds the others — and sorting a list it then races does
    not, because every entry starts in the same tick. (The app's own fetch
    log cannot show this: the losers are dropped before they log. The
    stand-in counts what it was really asked, by Host header.)

    So the remembered relay gets the same head start the reader's own helper
    site gets: asked alone, with the rest brought in only if it fails or
    stays silent. A returning reader's sync is then one request to one relay
    instead of four to four — faster, and three fewer strangers shown which
    CMI page a student is fetching."""
    serve_cmi()
    serve_relays()                      # ALL four alive, so nothing is forced
    try:
        app.boot("/", seed=False, prefs={"last_good_route": "codetabs.com"})
        app.wait_css(".tabs .tab", timeout=30)
        app.wait_gone(".welcome-card")
        title = app.css(".sync-pill").get_attribute("title")
        assert "through the helper site codetabs.com" in title, \
            f"the remembered route had to win when every route was alive: {title}"
        tiers = fetch_log_tiers(app)
        assert set(tiers) == {"proxy:codetabs.com"}, \
            f"only the remembered route may be asked: {tiers}"
    finally:
        stop_serving_cmi()

    # There is deliberately no "first sync, nothing remembered" variant of the
    # assertion above. It would exercise the same code path — the head start
    # does not care WHY a route is first — while depending on the leading
    # relay answering inside 2.5s, which a loaded machine cannot promise: an
    # early version of this test failed on a build box running three other
    # browsers, and a test that fails because the machine is busy teaches
    # people to ignore it.

    # A remembered route that has since DIED must not strand the reader:
    # the head start expires and the others come in behind it.
    serve_cmi()
    serve_relays(only={"proxy.cors.sh"})
    try:
        app.boot("/", seed=False, prefs={"last_good_route": "codetabs.com"})
        app.wait_css(".tabs .tab", timeout=40)
        app.wait_gone(".welcome-card")
        title = app.css(".sync-pill").get_attribute("title")
        assert "through the helper site cors.sh" in title, title
        # And the app now remembers the one that actually worked.
        stored = app.d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.prefs')||'{}')")
        assert stored.get("last_good_route") == "cors.sh", stored
    finally:
        stop_serving_cmi()


def t138_a_link_that_replaces_your_courses_says_so(app):
    """Old §8.23. A share link is written over the planner wholesale, and only
    ONE of the two things it can destroy ever said a word: the incoming times
    and credits replacing the reader's own. The COURSES being thrown away were
    never weighed — so a reader with a timetable and no meeting edits opened a
    friend's link, or their own older bookmark, and it was replaced in
    silence, with the Undo button going from disabled to enabled as the only
    sign. One reload later it was unrecoverable. The identical action DID
    announce itself if that reader happened to hold one override.

    Pinned here: the notice appears, it says what was actually lost, Undo puts
    it back — and a link that takes nothing away stays quiet."""
    app.boot("/", selection=["TOC", "RDBM", "MFD"])

    # A friend's link, on a planner with courses and no edits of any kind.
    app.d.get(f"{BASE}/?c=ISS")
    app.wait_css("section[aria-label='My timetable']")
    WebDriverWait(app.d, 10).until(
        lambda d: "replaced the courses you had picked" in app.toasts_text(),
        message=f"a silent replacement: toasts were {app.toasts_text()!r}")
    toast = app.toasts_text()
    assert "times and credits" not in toast, \
        f"nothing of the sort was lost — say what was: {toast!r}"
    assert app.d.execute_script(
        "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));") == ["ISS"]

    # …and it is genuinely undoable from that toast, which is the whole point
    # of announcing it.
    app.xpath("//div[contains(@class,'toast')]//button[normalize-space()='Undo']").click()
    WebDriverWait(app.d, 5).until(
        lambda d: d.execute_script(
            "return JSON.parse(localStorage.getItem('cmitt.v1.selection'));")
        == ["TOC", "RDBM", "MFD"],
        message="Undo did not put the replaced timetable back")

    # A link that takes nothing away says nothing: same codes, empty planner,
    # and reopening the link you are already on.
    app.boot("/", selection=[])
    app.d.get(f"{BASE}/?c=TOC")
    app.wait_css("section[aria-label='My timetable']")
    time.sleep(1.0)
    assert "replaced" not in app.toasts_text(), \
        f"nothing was replaced on an empty planner: {app.toasts_text()!r}"
    app.d.get(f"{BASE}/?c=TOC")
    app.wait_css("section[aria-label='My timetable']")
    time.sleep(1.0)
    assert "replaced" not in app.toasts_text(), \
        f"reopening the same link replaces nothing: {app.toasts_text()!r}"


def t139_a_second_tabs_change_is_adopted_when_safe(app):
    """Old §8.22, closed. Every tab holds the whole store in memory and
    writes it back wholesale, so with two tabs open the one that saved LAST
    used to win — the other's work gone in silence. R84 added a banner;
    R87 adopts.

    The design decision the entry was open for, decided: an IDLE tab catches
    up on its own (no banner, no reload), and the adoption is one undoable
    step — Ctrl+Z is the deliberate "keep mine", and it persists, so the
    stack can never silently re-clobber the other tab. A tab that is BUSY
    (any open dialog — a clean editor's Save commits its whole store) gets
    the sticky notice instead and catches up the moment the dialog closes,
    at which point the notice retires by itself."""
    app.boot("/", selection=["TOC"])
    first = app.d.current_window_handle
    assert not app.css_all(".banner.warn"), "no warning before anything happens"

    def storage_selection(d):
        return d.execute_script(
            "return localStorage.getItem('cmitt.v1.selection') || ''")

    # --- another tab makes a real change -----------------------------------
    app.d.switch_to.new_window("tab")
    second = app.d.current_window_handle
    app.d.get(f"{BASE}/")
    app.wait_css("section[aria-label='My timetable']")
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .card")

    def add_in_second(code):
        row = app.xpath("//section[@aria-label='Catalog']//div[contains(@class,'card')]"
                        f"[.//button[starts-with(@aria-label, '{code},')]]")
        app.d.execute_script("arguments[0].scrollIntoView({block: 'center'});", row)
        row.find_element(By.XPATH, ".//button[normalize-space()='Add']").click()
        WebDriverWait(app.d, 5).until(
            lambda d: code in storage_selection(d),
            message=f"the second tab did not save {code}")

    add_in_second("RDBM")

    # --- the idle tab adopts, with no banner and no reload -----------------
    app.d.switch_to.window(first)
    WebDriverWait(app.d, 10).until(
        lambda d: app.chips("RDBM"),
        message="the idle first tab did not adopt the other tab's change")
    assert not app.css_all(".banner.warn"), \
        "an idle tab that adopted must not also warn"

    # --- and the adoption is one honest undo step ---------------------------
    # Ctrl+Z is the deliberate "keep mine": it must both restore this tab's
    # version AND persist it, or the undo stack is the old bug wearing a
    # keyboard shortcut.
    app.xpath("//button[@aria-label='Undo']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: "RDBM" not in storage_selection(d),
        message="undoing the adoption must persist this tab's own version")
    # toasts_text() returns ONE joined string, not a list — an `any(...)`
    # over it iterates characters and can never match (found the hard way).
    WebDriverWait(app.d, 5).until(
        lambda d: "changes from another tab" in app.toasts_text(),
        message="the undo toast must say what was undone")
    app.xpath("//button[@aria-label='Redo']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: "RDBM" in storage_selection(d),
        message="redo must converge back to the other tab's version")

    # --- a busy tab is warned instead, and catches up when it is done ------
    app.xpath("//button[normalize-space()='My data']").click()
    app.wait_css(".dialog")

    app.d.switch_to.window(second)
    add_in_second("SVA")

    app.d.switch_to.window(first)
    banner = WebDriverWait(app.d, 10).until(
        lambda d: next(iter(app.css_all(".banner.warn")), None),
        message="a busy tab must still be told")
    assert "Another tab of this app has changed your timetable" in banner.text, banner.text
    assert "catch up on its own" in banner.text, banner.text
    # Not adopted yet: the My data dialog lists the selection reactively,
    # and SVA must not be in it while the dialog is open.
    assert "SVA" not in app.css(".dialog").text, \
        "a tab with an open dialog must not adopt under it"

    app.css(".dialog").send_keys(Keys.ESCAPE)
    WebDriverWait(app.d, 10).until(
        lambda d: app.chips("SVA"),
        message="closing the dialog must let the deferred adoption land")
    WebDriverWait(app.d, 10).until(
        lambda d: not app.css_all(".banner.warn"),
        message="the notice must retire once this tab has caught up")

    app.d.switch_to.window(second)
    app.d.close()
    app.d.switch_to.window(first)


def t141_the_printed_clash_strip_says_what_the_screen_says(app):
    """A wall poster has to shout about overlaps at least as loudly as the
    screen, and say the same thing. It did not, twice over.

    It formatted `a_slot` for BOTH courses, so a poster of a term with a
    stretched class told its reader that the other course ran 09:10-14:00 —
    the on-screen panel had been fixed to wear a code per range and the strip
    forty lines above it had not (R83). And it listed raw clashes while the
    panel grouped them by pair, so two courses meeting at the same hour twice
    a week were one problem on screen and two on paper (R84).

    Both halves are the same rule: the sheet and the screen are one app."""
    # TOC stretched to Tue 09:10-14:00 runs over AAT's Tue 10:30 class, so the
    # two ranges genuinely differ — which is what makes the wrong version
    # wrong rather than accidentally right. AAT also gets a SECOND meeting
    # inside that span, so the pair collides twice: without it the grouping
    # half of this test would pass on one collision and prove nothing.
    twice = {
        "next_id": 2,
        "items": [
            LONG_OVR["items"][0],
            {
                "id": 1, "course": "AAT", "base": None,
                "to": {"day": "Tue",
                       "slot": {"start_min": 720, "end_min": 795},
                       "hall": "Lecture Hall 5", "temp_booking": False},
                "created_at": 1754000001000.0,
            },
        ],
        "credits": [],
    }
    app.boot("/", selection=["TOC", "AAT"], overrides=twice)
    app.wait_css("section[aria-label='My timetable'] table.tt")

    panel = app.css(".clash-list").text
    assert "TOC" in panel and "AAT" in panel, panel

    app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": "print"})
    time.sleep(0.3)
    strip = app.css(".print-clashes").text
    app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": ""})

    # Each range wears its own code, on paper as on screen.
    assert "TOC 09:10–14:00" in strip, \
        f"the strip must name whose time each range is: {strip!r}"
    assert "AAT 10:30–11:45" in strip, strip
    # …and it must not hand AAT the other course's hours, which is the exact
    # sentence the old `a_slot`-only format printed.
    assert "AAT (Tue 09:10–14:00)" not in strip, strip

    # Two real collisions for one pair…
    assert strip.count("12:00") == 1, \
        f"the second collision must be on the sheet at all: {strip!r}"
    # …and ONE entry for it, the way the screen panel counts.
    assert strip.count("TOC × AAT") == 1, \
        f"a pair is one problem, however many times it happens: {strip!r}"
    assert len(app.css_all(".clash-list li")) == 1, \
        "the screen groups the pair too, or these two are not being compared"


def t140_a_footnote_never_explains_a_mark_that_is_not_there(app):
    """R79's rule, now asked of every sheet that can be filtered.

    A printed sheet ends in a key: "✎ times you set yourself", "⚠ marks a
    clash", "✓ already on your timetable". A key to a mark that is nowhere on
    the paper sends the reader hunting the sheet for it — so the predicate has
    to be "is this mark ON THIS SHEET", and on a page with a filter bar the
    sheet is what the filters left.

    Three of these asked the STORE instead, and each was found by printing the
    filtered page rather than by reading the code: My courses asked "does the
    timetable clash anywhere" beside a `*` clause that had been fixed to ask
    the sheet, so a sheet with no marks at all still explained ⚠; the Master
    grid asked "does the reader have courses" rather than "is there a ✓ here"
    (R82/R83/R84).

    The marks live in `::before` content as well as in text — `.chip.clash`
    draws its ⚠ that way — so this counts what is PAINTED, which is the only
    thing a reader sees."""
    marks_on_sheet = """
        const sec = document.querySelector(arguments[0]);
        const foot = sec.querySelector('.print-footnote');
        const seen = new Set();
        for (const el of sec.querySelectorAll('*')) {
          if (foot && foot.contains(el)) continue;
          if (!el.getClientRects().length) continue;
          let text = el.childNodes.length
            ? [...el.childNodes].filter((n) => n.nodeType === 3)
                .map((n) => n.textContent).join('')
            : '';
          for (const pseudo of ['::before', '::after']) {
            const c = getComputedStyle(el, pseudo).content;
            if (c && c !== 'none' && c !== 'normal') text += c;
          }
          for (const m of ['\u2713', '\u26a0', '\u270e', '*']) {
            if (text.includes(m)) seen.add(m);
          }
        }
        return {marks: [...seen],
                footnote: foot ? foot.querySelector('span').textContent : null};
    """

    def read(section):
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": "print"})
        time.sleep(0.3)
        got = app.d.execute_script(marks_on_sheet, section)
        app.d.execute_cdp_cmd("Emulation.setEmulatedMedia", {"media": ""})
        return got

    def check(section, where):
        got = read(section)
        foot = got["footnote"] or ""
        for mark, name in (("\u2713", "✓"), ("\u26a0", "⚠"), ("\u270e", "✎")):
            if mark in foot and mark not in got["marks"]:
                raise AssertionError(
                    f"{where}: the footnote explains {name} and the sheet "
                    f"carries none — footnote {foot!r}, marks {got['marks']!r}")
        return got

    # TOC stretched to 09:10–14:00 runs over AAT's Tuesday class, so the
    # timetable really does clash — which is what makes the filtered sheet's
    # silence meaningful.
    app.boot("/", selection=["TOC", "AAT", "RDBM"], overrides=LONG_OVR)

    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses'] .card")
    unfiltered = check("section[aria-label='My courses']", "My courses, unfiltered")
    assert "\u26a0" in unfiltered["footnote"], (
        "the fixture must actually clash or this test proves nothing: "
        f"{unfiltered!r}")

    search = app.css("section[aria-label='My courses'] input[type='search']")
    search.clear()
    search.send_keys("RDBM")
    time.sleep(0.6)
    check("section[aria-label='My courses']", "My courses, filtered to one calm course")

    # The Master grid's ✓ key, on a grid filtered to courses the reader does
    # not have. Its Print button is disabled with NOTHING shown (R84), so this
    # filters to something real that simply is not on the timetable.
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    grid_search = app.css("section[aria-label='Master grid'] input[type='search']")
    grid_search.clear()
    grid_search.send_keys("Quantum")
    time.sleep(0.6)
    check("section[aria-label='Master grid']", "Master grid, filtered")

    # And the Catalog, which R83 already fixed — kept here so all three sheets
    # answer the same question in one place.
    app.open_tab("Catalog")
    app.wait_css("section[aria-label='Catalog'] .card")
    cat_search = app.css("section[aria-label='Catalog'] input[type='search']")
    cat_search.clear()
    cat_search.send_keys("Quantum")
    time.sleep(0.6)
    check("section[aria-label='Catalog']", "Catalog, filtered")


def t142_a_day_ticked_on_the_catalog_does_not_haunt_my_courses(app):
    """My courses keeps its OWN Day and Time-slot filters — including inside
    the menus, not just in the count badge.

    CONTEXT 8.18: every facet read its ticked values with
    `filters_in(scope.mine())` EXCEPT Day and Time slot, which read the shared
    Catalog set. The badge and the menu therefore disagreed, and because
    `with_picked` injects an unknown ticked value using its raw KEY as the
    label, a day ticked on the Catalog appeared in My courses' Day menu as a
    bare index — a row reading "0" where every other row names a weekday.

    TOC and ISS both meet Tue+Thu only, so Monday can never be one of My
    courses' own options and the injected row has nowhere to hide.
    """
    app.boot("/?c=TOC,ISS")

    def day_menu(section):
        """Open the Day facet inside one section and return its option rows."""
        app.open_tab(section)
        app.wait_css(f"section[aria-label='{section}'] .filterbar")
        # Close whatever is open, so `details.facet[open]` is unambiguous.
        for d in app.css_all("details.facet[open]"):
            app.d.execute_script("arguments[0].removeAttribute('open')", d)
        app.xpath(
            f"//section[@aria-label='{section}']"
            "//details[contains(@class,'facet')]"
            "/summary[starts-with(normalize-space(),'Day')]"
        ).click()
        app.wait_css(f"section[aria-label='{section}'] details.facet[open] .menu")
        return app.css_all(
            f"section[aria-label='{section}'] details.facet[open] .menu label.opt")

    def labels(rows):
        return [r.text.strip() for r in rows if r.text.strip()]

    def badge(section):
        """The count the summary shows, via its aria-label ('Day, 1 selected')."""
        s = app.xpath(
            f"//section[@aria-label='{section}']"
            "//details[contains(@class,'facet')]"
            "/summary[starts-with(normalize-space(),'Day')]"
        )
        return s.get_attribute("aria-label") or ""

    WEEKDAYS = {"Monday", "Tuesday", "Wednesday", "Thursday",
                "Friday", "Saturday", "Sunday"}

    # Tick Monday on the Catalog — a day none of the selected courses meet on.
    rows = day_menu("Catalog")
    monday = next((r for r in rows if r.text.strip() == "Monday"), None)
    assert monday is not None, f"the Catalog Day menu should offer Monday, got {labels(rows)}"
    monday.find_element(By.CSS_SELECTOR, "input[type='checkbox']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: "1 selected" in badge("Catalog"),
        message="ticking Monday must show on the Catalog's own Day badge")

    # My courses must be untouched — in the BADGE and in the MENU. The badge
    # was always right; the menu is what 8.18 was about, so assert both or the
    # test passes on a half-fix.
    rows = day_menu("My courses")
    got = labels(rows)
    assert badge("My courses") == "Day", \
        f"My courses has no day of its own ticked, badge said {badge('My courses')!r}"
    stray = [l for l in got if l not in WEEKDAYS]
    assert not stray, \
        f"the Catalog's Monday leaked into My courses' Day menu as {stray} (rows: {got})"
    assert "Monday" not in got, \
        f"My courses' courses never meet on Monday, yet its Day menu offers it: {got}"
    for r in rows:
        assert not r.find_element(By.CSS_SELECTOR, "input[type='checkbox']").is_selected(), \
            f"nothing is ticked on My courses, but {r.text.strip()!r} is"

    # And the reverse: a day ticked HERE stays here.
    tuesday = next((r for r in rows if r.text.strip() == "Tuesday"), None)
    assert tuesday is not None, f"My courses meet on Tuesday, menu offered {got}"
    tuesday.find_element(By.CSS_SELECTOR, "input[type='checkbox']").click()
    WebDriverWait(app.d, 10).until(
        lambda d: "1 selected" in badge("My courses"),
        message="ticking Tuesday must show on My courses' own Day badge")

    rows = day_menu("Catalog")
    got = labels(rows)
    assert "1 selected" in badge("Catalog"), \
        f"the Catalog still has only its own Monday, badge said {badge('Catalog')!r}"
    checked = [r.text.strip() for r in rows
               if r.find_element(By.CSS_SELECTOR, "input[type='checkbox']").is_selected()]
    assert checked == ["Monday"], \
        f"the Catalog should still show exactly its own Monday ticked, got {checked}"
    stray = [l for l in got if l not in WEEKDAYS]
    assert not stray, f"My courses' Tuesday leaked into the Catalog's Day menu as {stray}"



def t143_the_developer_rail_cannot_eject_you_by_accident(app):
    """The dev rail is the planner rail's twin — one Tab stop, arrows that
    wrap, a wheel that stops at the ends — with one addition and one rule:
    the ← Back exit sits first, and NO walking gesture can land on it. All
    three R87 design drafts shipped a rail where Home (or one overshot
    arrow) silently threw the reader out of the mode; every judge flagged
    it, and this test is the fence."""
    app.boot("/#/developer")
    app.wait_css("section[aria-label='Developer mode']")

    tabs = [t.text for t in app.css_all(".tabs .tab")]
    assert tabs == ["Overview", "Tweaks", "Sync", "Storage"], tabs
    exit_btn = app.css(".tabs .tab-exit")
    assert exit_btn.get_attribute("role") != "tab", \
        "the exit navigates away — a tab claiming otherwise breaks ARIA"
    stops = [t for t in app.css_all(".tabs .tab") if t.get_attribute("tabindex") == "0"]
    assert len(stops) == 1, "the categories must stay ONE Tab stop"

    def selected():
        return app.css("nav.tabs button[aria-selected='true']").text

    def in_dev(d):
        return d.execute_script("return location.hash").startswith("#/developer")

    # Arrows walk categories; the walk WRAPS — and never lands on the exit.
    active = app.css(".tabs .tab[tabindex='0']")
    active.send_keys(Keys.ARROW_RIGHT)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Tweaks")
    app.css(".tabs .tab[tabindex='0']").send_keys(Keys.END)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Storage")
    app.css(".tabs .tab[tabindex='0']").send_keys(Keys.HOME)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Overview")
    assert in_dev(app.d), "Home must land on the first CATEGORY, not the exit"
    app.css(".tabs .tab[tabindex='0']").send_keys(Keys.ARROW_LEFT)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Storage")
    assert in_dev(app.d), "the wrap must skip the exit, not press it"

    # Focus SURVIVES a step (R88). A category step is a route change, and
    # until R88 the rail rebuilt on every one: the freshly focused button
    # was unmounted, document.activeElement fell to <body>, and the SECOND
    # arrow press was dead. Selenium's send_keys refocuses whatever it just
    # found, which is exactly how the old test masked this — so this block
    # deliberately never re-finds an element between presses.
    app.css(".tabs .tab[tabindex='0']").send_keys(Keys.HOME)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Overview")
    app.d.switch_to.active_element.send_keys(Keys.ARROW_RIGHT)
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Tweaks")
    focused = app.d.execute_script(
        "const a = document.activeElement;"
        "return a && a.classList && a.classList.contains('tab')"
        " ? a.textContent.trim() : (a ? a.tagName : 'nothing')")
    assert focused == "Tweaks", \
        f"one step in, focus must rest on the Tweaks tab, not {focused!r}"
    app.d.switch_to.active_element.send_keys(Keys.ARROW_RIGHT)
    WebDriverWait(
        app.d, 5,
    ).until(
        lambda d: selected() == "Sync",
        message="the SECOND arrow press must work without anyone refocusing")
    # A mouse click keeps focus on the pressed category too — a screen
    # reader follows activeElement, and <body> is nowhere.
    next(b for b in app.css_all(".tabs .tab") if b.text == "Storage").click()
    WebDriverWait(app.d, 5).until(lambda d: selected() == "Storage")
    clicked = app.d.execute_script(
        "const a = document.activeElement;"
        "return a ? (a.textContent.trim() || a.tagName) : 'nothing'")
    assert clicked == "Storage", \
        f"a click must not dump focus to the body (got {clicked!r})"

    # The wheel steps categories and STOPS at the ends — a trackpad flick
    # must never eject the whole mode.
    rail = app.css("nav.tabs")

    def wheel(dy, times=1):
        for _ in range(times):
            app.d.execute_script(
                """const el = arguments[0];
                   const r = el.getBoundingClientRect();
                   el.dispatchEvent(new WheelEvent('wheel', {
                     deltaY: arguments[1], bubbles: true, cancelable: true,
                     clientX: r.left + r.width / 2,
                     clientY: r.top + r.height / 2}));""",
                rail, dy)
            time.sleep(0.15)

    wheel(120, times=3)
    assert selected() == "Storage" and in_dev(app.d), selected()
    wheel(-120, times=8)
    assert selected() == "Overview" and in_dev(app.d), \
        "the wheel must stop at the first category, never exit"

    # The deliberate exits work, and focus lands somewhere real.
    app.css(".tabs .tab-exit").click()
    app.wait_css("section[aria-label='My timetable']")
    WebDriverWait(app.d, 5).until(
        lambda d: d.execute_script(
            "const a = document.activeElement;"
            "return a && a.matches(\"nav.tabs button.tab[tabindex='0']\")"),
        message="after ← Back, focus must land on the planner rail")

    # And the phone that fits five planner tabs fits this mode too — every
    # category, at the narrowest width still sold (t130's method: a window
    # cannot go this narrow, device metrics can).
    try:
        app.d.execute_cdp_cmd("Emulation.setDeviceMetricsOverride", {
            "width": 320, "height": 760, "deviceScaleFactor": 2,
            "mobile": True,
        })
        app.boot("/#/developer", fresh=False)
        app.wait_css("section[aria-label='Developer mode']")
        for label in ("Overview", "Tweaks", "Sync", "Storage"):
            app.open_tab(label)
            # Same async-hashchange beat as t02: measure the category that
            # actually arrived, not the one on its way out.
            app.wait_css(f"#panel-dev-{label.lower()}")
            overflow = app.d.execute_script(
                "return document.documentElement.scrollWidth"
                " - document.documentElement.clientWidth")
            assert overflow <= 1, \
                f"{label} forces {overflow}px of sideways scroll at 320px"
    finally:
        app.d.execute_cdp_cmd("Emulation.clearDeviceMetricsOverride", {})


def _reveal_tweak(app, search_term):
    """From anywhere: open the Tweaks page and reveal a row by searching for
    it — a live query overrides the groups' collapse, so this works whether
    the row's group ships open or closed. Returns the search box."""
    app.d.get(f"{BASE}/#/developer/tweaks")
    app.wait_css("section[aria-label='Developer mode']")
    box = app.css("input[aria-label='Search the tweaks']")
    if app.css_all("button[aria-label='Clear search']"):
        app.css("button[aria-label='Clear search']").click()
    box.send_keys(search_term)
    time.sleep(0.2)
    return box


def _flip_tweak(app, search_term, label):
    """Reveal one tweak row by search and flip its checkbox."""
    _reveal_tweak(app, search_term)
    app.xpath(f"//label[contains(@class,'opt')][.//span[normalize-space()="
              f'"{label}"]]//input').click()


def _pick_seg(app, search_term, group_aria, option_label):
    """Reveal a segmented tweak by search and pick one of its options."""
    _reveal_tweak(app, search_term)
    app.xpath(f"//div[@role='radiogroup'][@aria-label='{group_aria}']"
              f"//button[normalize-space()='{option_label}']").click()


def t144_the_tweaks_search_is_a_real_search_box(app):
    """The tweaks search carries the same three switches every search box in
    the app has (t115's contract), narrows rows AND whole groups as you
    type, reaches inside collapsed groups (a match renders its group
    expanded), treats a half-typed pattern as zero rows plus one explanation
    — never two — and forgets everything on reload: a lens, not work
    product."""
    app.boot("/#/developer/tweaks")
    app.wait_css("section[aria-label='Developer mode']")
    box = app.css("input[aria-label='Search the tweaks']")

    def visible_rows():
        return len(app.css_all(".tweak"))

    def visible_groups():
        return [t.text.replace("▸", "").strip()
                for t in app.css_all(".tweak-group .group-toggle")]

    def expanded_groups():
        return [t.text.replace("▸", "").strip()
                for t in app.css_all(
                    ".tweak-group .group-toggle[aria-expanded='true']")]

    # The R88 taxonomy: eleven groups, in the order a human wants them;
    # the shipped page's three open, the eight new ones closed.
    assert visible_groups() == [
        "Marks", "The week grid", "Colour and motion", "Opening the app",
        "Notices and dialogs", "Wheel and swipe", "Editing and undo",
        "Syncing", "Printing", "Calendar files", "Developer mode",
    ], visible_groups()
    assert expanded_groups() == ["Marks", "The week grid", "Colour and motion"]
    open_rows = visible_rows()
    assert open_rows == 16, \
        f"the three open groups hold 3+8+5 rows, got {open_rows}"

    # A search reaches INSIDE a collapsed group: the one matching group
    # renders expanded, every non-matching group hides entirely.
    box.send_keys("console")
    WebDriverWait(app.d, 5).until(lambda d: visible_rows() == 1)
    assert visible_groups() == ["Developer mode"], visible_groups()
    assert expanded_groups() == ["Developer mode"], \
        "a match must expand its group, or the search found nothing visible"
    # The pointer line is the recovery path and never hides.
    assert "live in My data" in app.css(
        "section[aria-label='Developer mode']").text

    # The three switches, by their established aria.
    for name in ("Match case", "Whole word", "Regular expression"):
        sw = app.css(f"button[aria-label='{name}']")
        assert sw.get_attribute("aria-pressed") == "false"

    # Clearing hands the caret back and restores the resting page.
    app.css("button[aria-label='Clear search']").click()
    WebDriverWait(app.d, 5).until(lambda d: visible_rows() == open_rows)
    assert app.d.execute_script(
        "return document.activeElement === arguments[0]", box), \
        "clearing must hand the caret back"
    assert expanded_groups() == ["Marks", "The week grid", "Colour and motion"], \
        "clearing the search must restore the reader's own open/closed state"

    # Whole word: "mark" alone stops matching "marks".
    app.css("button[aria-label='Whole word']").click()
    box.send_keys("highlight")
    WebDriverWait(app.d, 5).until(lambda d: visible_rows() == 1)

    # A half-typed pattern: zero rows, ONE explanation.
    app.css("button[aria-label='Whole word']").click()
    app.css("button[aria-label='Regular expression']").click()
    app.css("button[aria-label='Clear search']").click()
    box.send_keys("(unclosed")
    err = app.wait_css("#search-pattern-error")
    assert "Not a pattern yet" in err.text, err.text
    assert visible_rows() == 0, "a broken pattern matches NOTHING"
    assert not app.css_all(".empty.panel"), \
        "the error line is the explanation — an empty-state beside it is two"
    assert box.get_attribute("aria-invalid") == "true"

    # A working pattern narrows.
    app.css("button[aria-label='Clear search']").click()
    box.send_keys("ticks|rows")
    WebDriverWait(app.d, 5).until(
        lambda d: 0 < visible_rows() and not app.css_all(
            "#search-pattern-error"))

    # Nonsense gets the honest empty state.
    app.css("button[aria-label='Regular expression']").click()
    app.css("button[aria-label='Clear search']").click()
    box.send_keys("zzzz")
    WebDriverWait(app.d, 5).until(lambda d: visible_rows() == 0)
    assert app.css_all(".empty.panel"), "no match deserves its sentence"
    assert "live in My data" in app.css(
        "section[aria-label='Developer mode']").text

    # Session-only, search AND collapse alike: a REAL reload starts clean.
    # (`boot` to the same URL is a same-document hash change — the exact
    # thing the mode guarantees — so refresh() is the only honest way to
    # ask this question. The Syncing group is opened with no search live,
    # because a hidden group has no toggle and a held-open one ignores it.)
    app.css("button[aria-label='Clear search']").click()
    next(t for t in app.css_all(".tweak-group .group-toggle")
         if "Syncing" in t.text).click()
    app.d.refresh()
    app.wait_css("section[aria-label='Developer mode']")
    box = app.css("input[aria-label='Search the tweaks']")
    assert box.get_attribute("value") == ""
    assert visible_rows() == open_rows
    assert expanded_groups() == ["Marks", "The week grid", "Colour and motion"]


def t145_hiding_a_mark_hides_the_sign_never_the_fact(app):
    """The tweak a real user asked for, end to end: with the ⚠/✎/✓ marks
    ticked off, every chip, badge, legend line and print footnote loses the
    MARK — while the Clashes panel, the printed clash strip, "Your changes"
    and the credit numbers keep saying everything. Paint and prose read the
    same pref, so t140's rule (a footnote never explains a mark that is not
    there) holds in the new direction too."""
    # TOC×ISS still clash on Thu; TOC_OVR moves the Tue meeting (a ✎) and
    # sets custom credits (a second ✎).
    app.boot("/?c=TOC,ISS", selection=["TOC", "ISS"], overrides=TOC_OVR)
    app.wait_css("button.chip.clash")

    def poster_footnote():
        return app.d.execute_script(
            "const el = document.querySelector("
            "  \"section[aria-label='My timetable'] .print-footnote\");"
            "return el ? el.textContent : '';")

    assert "⚠" in poster_footnote() and "✎" in poster_footnote()

    # Flip all three marks off, from the Tweaks page.
    app.d.get(f"{BASE}/#/developer/tweaks")
    app.wait_css("section[aria-label='Developer mode']")
    for label in ("Mark clashes with ⚠ and a red border",
                  "Mark what you changed with ✎",
                  "Tick your courses with ✓ on the Master grid and Halls"):
        app.xpath(f"//label[contains(@class,'opt')][.//span[normalize-space()="
                  f'"{label}"]]//input').click()
    app.wait_toast("The ✓ ticks are hidden")
    app.css(".tabs .tab-exit").click()

    # My timetable: the signs are gone…
    app.wait_css("section[aria-label='My timetable']")
    assert not app.css_all("button.chip.clash"), "the chip clash class must go"
    labels = " ".join(
        c.get_attribute("aria-label") or "" for c in app.css_all("button.chip"))
    assert "clashes with" not in labels, "aria must not warn of a hidden mark"
    fn = poster_footnote()
    assert "⚠" not in fn and "✎ you changed this" not in fn, fn
    # …the facts are not.
    panel = app.xpath("//div[contains(@class,'panel')][.//h3[contains(.,'Clashes')]]")
    assert "TOC" in panel.text and "ISS" in panel.text, \
        "the Clashes panel states facts and never follows the tweak"
    strip = app.d.execute_script(
        "const el = document.querySelector('.print-clashes');"
        "return el ? el.textContent : '';")
    assert "TOC" in strip and "ISS" in strip, \
        "the printed clash strip is self-explaining and stays"

    # My courses: badge marks follow, numbers stay.
    app.open_tab("My courses")
    cards = app.wait_css("section[aria-label='My courses']")
    assert "⚠ clash" not in cards.text
    assert "3 cr" in cards.text and "3 cr ✎" not in cards.text, \
        "the custom credit NUMBER stays; only its ✎ goes"
    assert "your time" in cards.text and "✎ your time" not in cards.text, \
        "the moved meeting stays named; only its ✎ goes"

    # Master grid: no ✓, no ⚠, and no legend lines explaining either.
    app.open_tab("Master grid")
    grid = app.wait_css("section[aria-label='Master grid']")
    assert not app.css_all("section[aria-label='Master grid'] .sel-mark")
    assert not app.css_all("section[aria-label='Master grid'] .wontfit")
    assert "on your timetable" not in app.d.execute_script(
        "return document.querySelector("
        "  \"section[aria-label='Master grid'] .grid-legend\").textContent;")

    # Catalog: the row badge keeps its words and loses only its ✎, and the
    # print footnote's ⚠ clause is gone entirely. R88's print verifier found
    # both the hard way: a marks-off Catalog PDF whose only ⚠ was inside the
    # sentence explaining it, above a row still painting "✎ your times".
    app.open_tab("Catalog")
    cat = app.wait_css("section[aria-label='Catalog']")
    assert "your times" in cat.text, \
        "the fact (times are the reader's own) must stay in words"
    assert "✎ your times" not in cat.text, \
        "the ✎ sign must follow the tweak on the Catalog row badge"
    cat_fn = app.d.execute_script(
        "const el = document.querySelector("
        "  \"section[aria-label='Catalog'] .print-footnote\");"
        "return el ? el.textContent : '';")
    assert "⚠" not in cat_fn and "✎" not in cat_fn, \
        f"the Catalog footnote must not explain hidden marks: {cat_fn!r}"

    # The choice survives a reload, and ticking back restores everything.
    # (The active tab persisted too — Catalog — so walk home first.)
    app.boot("/", fresh=False)
    app.open_tab("My timetable")
    app.wait_css("section[aria-label='My timetable']")
    assert not app.css_all("button.chip.clash"), "the tweak must persist"
    app.d.get(f"{BASE}/#/developer/tweaks")
    app.wait_css("section[aria-label='Developer mode']")
    app.xpath("//label[contains(@class,'opt')][.//span[normalize-space()="
              '"Mark clashes with ⚠ and a red border"]]//input').click()
    app.wait_toast("Clash marks are back")
    app.css(".tabs .tab-exit").click()
    app.wait_css("button.chip.clash")


def t146_the_door_in_my_data_opens_and_escape_walks_back(app):
    """The way in: My data → "Under the hood" → Open developer mode — a
    same-document hop that lands focus on the mode itself. The way back:
    Escape — except while typing in the tweaks search, where Escape belongs
    to the box (the trap two of three R87 designs shipped)."""
    app.boot("/")
    app.d.execute_script("window.__no_reload_marker = 1")
    app.xpath("//button[normalize-space()='My data']").click()
    app.wait_css(".dialog")
    door = app.xpath("//button[normalize-space()='Open developer mode']")
    # The dialog's action row is sticky; centre the button first or the
    # click lands on the footer floating over it.
    app.d.execute_script("arguments[0].scrollIntoView({block: 'center'});", door)
    door.click()

    app.wait_css("section[aria-label='Developer mode']")
    WebDriverWait(app.d, 5).until(lambda d: not app.css_all(".dialog"),
                                  message="the dialog must close under the mode")
    assert app.d.execute_script("return window.__no_reload_marker") == 1, \
        "entering developer mode must never reload the document"
    WebDriverWait(app.d, 5).until(
        lambda d: d.execute_script(
            "const a = document.activeElement;"
            "return a && a.id && a.id.startsWith('panel-dev-')"),
        message="focus must land on the mode, not fall back to the opener")

    # Escape in the tweaks search stays in the mode.
    app.open_tab("Tweaks")
    box = app.wait_css("input[aria-label='Search the tweaks']")
    box.send_keys("mark")
    box.send_keys(Keys.ESCAPE)
    time.sleep(0.3)
    assert app.d.execute_script("return location.hash").startswith("#/developer"), \
        "Escape while typing belongs to the search box"

    # Escape anywhere else is the way back, and focus lands on the rail.
    # (A <section> cannot take keys; press it from a focused rail button —
    # which is also the realistic hand position.)
    active_tab = app.css(".tabs .tab[tabindex='0']")
    active_tab.click()
    active_tab.send_keys(Keys.ESCAPE)
    app.wait_css("section[aria-label='My timetable']")
    WebDriverWait(app.d, 5).until(
        lambda d: d.execute_script(
            "const a = document.activeElement;"
            "return a && a.matches(\"nav.tabs button.tab[tabindex='0']\")"),
        message="after Escape, focus must land on the planner rail")

    # Escape works from a tweak CHECKBOX too. The key has no native job on a
    # tick-box, so the editing guard must not swallow it there — both R88
    # verifiers found the same dead key on every tweak row. (Focus without
    # clicking: a click would flip the tweak.)
    app.d.get(f"{BASE}/#/developer/tweaks")
    app.wait_css("section[aria-label='Developer mode']")
    cb = app.css("label.opt input[type='checkbox']")
    app.d.execute_script("arguments[0].focus()", cb)
    app.d.switch_to.active_element.send_keys(Keys.ESCAPE)
    app.wait_css("section[aria-label='My timetable']")
    assert not app.d.execute_script(
        "return location.hash").startswith("#/developer"), \
        "Escape on a checkbox is navigation, not a dead key"



def t147_the_week_grid_tweaks_do_not_fight_each_other(app):
    """"Highlight today's row" ticked off makes today's EMPTY halls row fall
    back to the quiet dim — deliberate: the row stops being special, so it
    takes whatever any other empty day gets. But with "Dim days with no
    classes" ALSO off, no dim may survive anywhere: until R88 the fallback
    rule out-specified the no-dim rule and today's empty row was the ONE row
    still faded — the removed mark coming back inverted."""
    pinned = app.pin_weekday(4)  # Friday: some hall always sits empty then
    try:
        app.boot("/", selection=["TOC", "ISS"])
        app.open_tab("Halls")
        section = app.wait_css("section[aria-label='Lecture halls']")
        next(b for b in section.find_elements(By.CSS_SELECTOR, "[role='radio']")
             if b.text == "Week").click()
        app.wait_css("table.tt.halls-merged")
        assert app.css_all("table.tt.halls-merged tr.quiet.today"), \
            "the fixture must give some hall an empty Friday"

        def quiet_today_dim():
            return float(app.d.execute_script(
                "const th = document.querySelector("
                "  'table.tt.halls-merged tr.quiet.today th.dayhead');"
                "return getComputedStyle(th).opacity;"))

        def flip(label):
            app.d.get(f"{BASE}/#/developer/tweaks")
            app.wait_css("section[aria-label='Developer mode']")
            app.xpath("//label[contains(@class,'opt')][.//span[normalize-space()="
                      f'"{label}"]]//input').click()
            app.css(".tabs .tab-exit").click()
            app.wait_css("table.tt.halls-merged")

        assert quiet_today_dim() == 1, "by default, today outranks quiet"
        flip("Highlight today's row")
        assert quiet_today_dim() < 1, \
            "with today's mark off, the empty row falls back to the quiet dim"
        flip("Dim days with no classes")
        assert quiet_today_dim() == 1, \
            "with BOTH off, no dim may survive on today's empty row"
        # And the second tweak alone never dims anything either.
        flip("Highlight today's row")
        assert quiet_today_dim() == 1
    finally:
        app.unpin_weekday(pinned)


def t148_the_tweak_groups_open_and_close_by_hand(app):
    """The eleven groups' disclosure is the reader's own: a closed group
    opens on one press of its heading and closes on the next, a live search
    holds matching groups open without touching those choices, and clearing
    the search hands them back exactly as they were."""
    app.boot("/#/developer/tweaks")
    app.wait_css("section[aria-label='Developer mode']")

    def toggle(title):
        return next(t for t in app.css_all(".tweak-group .group-toggle")
                    if title in t.text)

    def rows_in(title):
        return app.d.execute_script(
            "const g = [...document.querySelectorAll('.tweak-group')].find("
            "  x => x.querySelector('.group-toggle').textContent.includes("
            f"    '{title}'));"
            "return g ? g.querySelectorAll('.tweak').length : -1;")

    # Closed ships closed; one press opens; the next closes.
    assert toggle("Syncing").get_attribute("aria-expanded") == "false"
    assert rows_in("Syncing") == 0
    toggle("Syncing").click()
    WebDriverWait(app.d, 5).until(lambda d: rows_in("Syncing") == 4)
    assert toggle("Syncing").get_attribute("aria-expanded") == "true"
    # The Syncing lede states the honesty floor.
    assert "never stops saying how old" in app.css(
        "section[aria-label='Developer mode']").text
    toggle("Syncing").click()
    WebDriverWait(app.d, 5).until(lambda d: rows_in("Syncing") == 0)

    # A live search holds a matching group open — but the reader's own
    # choice underneath is untouched, and a click mid-search is inert
    # rather than silently flipping hidden state.
    box = app.css("input[aria-label='Search the tweaks']")
    box.send_keys("helper site")
    WebDriverWait(app.d, 5).until(
        lambda d: toggle("Syncing").get_attribute("aria-expanded") == "true")
    toggle("Syncing").click()
    time.sleep(0.2)
    assert toggle("Syncing").get_attribute("aria-expanded") == "true", \
        "mid-search the group stays held open"
    app.css("button[aria-label='Clear search']").click()
    WebDriverWait(app.d, 5).until(
        lambda d: toggle("Syncing").get_attribute("aria-expanded") == "false",
        message="clearing must restore the reader's own closed state")


def t149_dialogs_and_notices_obey_their_tweaks(app):
    """Three behaviour tweaks end to end: the dialog scrim stops closing on
    a stray click (Escape still works), a notice can be kept until dismissed
    by hand or hurried to three seconds, and the printed credit line follows
    its tweak while the sheet's facts stay."""
    app.boot("/?c=TOC", selection=["TOC"])

    # Default: a click on the dark area closes the dialog.
    app.xpath("//button[normalize-space()='My data']").click()
    app.wait_css(".dialog")
    app.d.execute_script(
        "document.querySelector('.overlay').dispatchEvent("
        "  new MouseEvent('click', {bubbles: true}))")
    WebDriverWait(app.d, 5).until(lambda d: not app.css_all(".dialog"))

    # Tweaked off: the same click does nothing; Escape still leaves.
    _flip_tweak(app, "dark area", "Close a dialog by clicking the dark area")
    app.css(".tabs .tab-exit").click()
    app.wait_css("section[aria-label='My timetable']")
    app.xpath("//button[normalize-space()='My data']").click()
    app.wait_css(".dialog")
    app.d.execute_script(
        "document.querySelector('.overlay').dispatchEvent("
        "  new MouseEvent('click', {bubbles: true}))")
    time.sleep(0.4)
    assert app.css_all(".dialog"), "the scrim click must be inert now"
    app.css(".dialog").send_keys(Keys.ESCAPE)
    WebDriverWait(app.d, 5).until(lambda d: not app.css_all(".dialog"))

    # "Until dismissed": the flip's own toast is the proof — it outlives the
    # shipped six seconds and goes only by its ✕.
    _pick_seg(app, "Notices stay for", "Notices stay for", "Until dismissed")
    app.wait_toast("until you close them")
    time.sleep(7.5)
    assert "until you close them" in app.toasts_text(), \
        "an until-dismissed notice must outlive the 6s default"
    app.css(".toast button[aria-label='Dismiss']").click()
    WebDriverWait(app.d, 5).until(
        lambda d: "until you close them" not in app.toasts_text())

    # "3 s": the flip's toast hurries itself away.
    _pick_seg(app, "Notices stay for", "Notices stay for", "3 s")
    app.wait_toast("3 seconds")
    time.sleep(4.5)
    assert "3 seconds" not in app.toasts_text(), \
        "a 3-second notice must be gone by 4.5s"

    # The printed credit follows its tweak; the facts before it stay.
    stats = lambda: app.d.execute_script(
        "const el = document.querySelector('.pm-stats');"
        "return el ? el.textContent : '';")
    app.css(".tabs .tab-exit").click()
    app.wait_css("section[aria-label='My timetable']")
    assert "made with the CMI Timetable Planner" in stats()
    _flip_tweak(app, "Sign each sheet",
                "Sign each sheet “made with the CMI Timetable Planner”")
    app.css(".tabs .tab-exit").click()
    app.wait_css("section[aria-label='My timetable']")
    assert "made with" not in stats(), "the sheet prints unsigned now"
    assert "course" in stats(), "the facts on the stats line stay"


def t150_weekend_rows_are_a_choice_not_a_growth(app):
    """The week grids ship Mon–Fri and grow a weekend row only when
    something meets there; the tweak seeds all seven days — so a reader can
    drag a course of their own onto a Saturday that doesn't exist yet — and
    ticking it back shrinks the week again."""
    app.boot("/?c=TOC", selection=["TOC"])

    def day_heads(section):
        return [th.text.strip() for th in app.css_all(
            f"section[aria-label='{section}'] table.tt tbody tr > th.rowhead")]

    app.open_tab("My timetable")
    app.wait_css("section[aria-label='My timetable'] table.tt")
    before = day_heads("My timetable")
    assert "Sat" not in before and "Sun" not in before, before

    _flip_tweak(app, "Saturday and Sunday",
                "Show Saturday and Sunday even when empty")
    app.css(".tabs .tab-exit").click()
    app.wait_css("section[aria-label='My timetable'] table.tt")
    WebDriverWait(app.d, 5).until(
        lambda d: "Sat" in day_heads("My timetable")
        and "Sun" in day_heads("My timetable"))
    # The Master grid reads the same seed.
    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    heads = day_heads("Master grid")
    assert "Sat" in heads and "Sun" in heads, heads

    _flip_tweak(app, "Saturday and Sunday",
                "Show Saturday and Sunday even when empty")
    app.css(".tabs .tab-exit").click()
    app.wait_css("section[aria-label='Master grid'] table.tt")
    WebDriverWait(app.d, 5).until(
        lambda d: "Sat" not in day_heads("Master grid"))


def t151_the_landing_section_is_a_choice(app):
    """"Open the app on" replaces the remembered section on a REAL load —
    and only then: switching sections in a running session works exactly as
    before, and putting the choice back hands landing to the remembered
    section again."""
    app.boot("/?c=TOC", selection=["TOC"])
    _reveal_tweak(app, "Open the app on")
    sel = app.css("select[aria-label='Open the app on']")
    Select(sel).select_by_visible_text("Catalog")
    app.wait_toast("opens on Catalog")
    # Leave the mode first: a #/developer URL reloads INTO developer mode
    # (the mode's own promise) — the landing choice picks the planner
    # section, never the route.
    app.css(".tabs .tab-exit").click()
    app.wait_css("section[aria-label='My timetable']")

    app.d.refresh()
    app.wait_css("section[aria-label='Catalog']")
    # A running session still walks anywhere.
    app.open_tab("Halls")
    app.wait_css("section[aria-label='Lecture halls']")

    _reveal_tweak(app, "Open the app on")
    Select(app.css("select[aria-label='Open the app on']")).select_by_visible_text(
        "The section I left (how the app ships)")
    app.wait_toast("wherever you last were")
    app.css(".tabs .tab-exit").click()
    # The exit is an async hash hop; the planner SECTION arriving is the
    # proof the planner rail is back (a bare `.tab` wait matches the dev
    # rail's own categories and proves nothing). And the section is CATALOG:
    # _reveal_tweak's d.get drops the ?c= query, so it is a REAL reload —
    # and that boot ran while the landing choice still said Catalog,
    # overwriting the Halls pick above. The tweak applying on that reload is
    # itself the behaviour under test, so assert it rather than dodge it.
    app.wait_css("section[aria-label='Catalog']")
    app.open_tab("My courses")
    app.wait_css("section[aria-label='My courses']")
    app.d.refresh()
    app.wait_css("section[aria-label='My courses']")


def t152_a_mouse_may_earn_dragging_without_the_toggle(app):
    """t09 pins the shipped gate: drags do nothing until ✎ Edit layout. This
    is the advanced-user override — with "Keep drags behind ✎ Edit layout"
    unticked, a MOUSE drag moves a chip with no toggle armed, the move is a
    real override (undoable, listed), and re-ticking restores the gate."""
    app.boot("/?c=TOC")
    _flip_tweak(app, "Keep drags behind", "Keep drags behind ✎ Edit layout")
    app.css(".tabs .tab-exit").click()
    # Same async hop as t151: wait for the planner section before asking
    # the planner rail for anything.
    app.wait_css("section[aria-label='My timetable']")

    app.open_tab("Master grid")
    app.wait_css("section[aria-label='Master grid'] table.tt")
    target = app.cell(2, 1020)  # Wed 17:00 — empty
    # No Edit layout press anywhere in this test.
    app.drag(app.chip("TOC", "td[data-day='1'][data-slot='550']"), target)
    app.wait_toast("Moved TOC")
    moved = app.chip("TOC", "td[data-day='2'][data-slot='1020']")
    assert "overridden" in moved.get_attribute("class")

    # Re-tick: the gate is back, exactly t09's opening state.
    _flip_tweak(app, "Keep drags behind", "Keep drags behind ✎ Edit layout")
    app.css(".tabs .tab-exit").click()
    app.wait_css("section[aria-label='Master grid'] table.tt")
    app.drag(app.chip("TOC", "td[data-day='2'][data-slot='1020']"),
             app.cell(1, 550))
    time.sleep(0.4)
    assert app.chips("TOC", "td[data-day='2'][data-slot='1020']"), \
        "with the tweak re-ticked, a toggle-less drag must be inert again"


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
    t91_a_sync_in_one_tab_reaches_the_other,
    t92_the_pill_refreshes_at_the_pace_the_words_change,
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
    t89_the_digest_narrows_to_the_readers_own_courses,
    t90_a_grid_chip_shows_the_tick_without_being_rebuilt,
    t93_the_grid_picks_its_row_height_from_the_screen,
    t94_a_chosen_row_height_is_never_overruled,
    t95_two_students_combine_their_timetables,
    t97_a_planner_with_nothing_in_it_is_never_asked_what_to_replace,
    t96_a_disagreement_over_one_class_keeps_your_own,
    t98_a_file_never_argues_with_itself,
    t99_the_same_file_twice_says_what_it_refused,
    t100_replacing_twice_with_one_file_is_one_change,
    t101_an_import_that_undeletes_a_course_says_so,
    t102_credit_note_names_what_your_own_number_replaced,
    t103_putting_a_class_back_where_it_was_changes_nothing,
    t104_catalog_row_can_delete_a_course,
    t105_arrow_keys_walk_the_tab_rail,
    t107_a_short_link_is_remembered_for_each_service,
    t108_a_link_made_before_the_timetable_changed_is_not_offered_as_current,
    t109_nothing_is_sent_until_the_button_is_pressed,
    t110_a_shortener_that_cannot_be_reached_says_so_and_invents_nothing,
    t111_the_chosen_service_is_warmed_and_only_the_chosen_one,
    t112_a_chosen_day_view_survives_a_refresh,
    t113_the_shortening_service_you_picked_is_the_one_you_come_back_to,
    t114_the_app_asks_before_it_updates_itself,
    t115_the_search_box_has_the_three_switches_every_editor_has,
    t116_the_search_box_shows_its_whole_placeholder,
    t117_the_shorten_popup_has_a_way_out_and_it_is_not_beside_the_send,
    t118_the_search_looks_only_where_you_tell_it,
    t119_today_is_marked_on_the_week_tables,
    t120_dialogs_open_hands_off_and_the_page_behind_stays_put,
    t121_a_long_meeting_visibly_fills_every_slot_it_covers,
    t122_a_clash_inside_the_covered_span_is_red_where_it_happens,
    t123_a_damaged_link_changes_nothing,
    t124_cancelling_a_confirm_returns_focus_to_what_asked,
    t125_print_stays_light_whatever_the_theme,
    t106_the_wheel_over_the_rail_walks_the_sections,
    t126_every_section_prints_itself_and_only_itself,
    t127_printing_never_repaints_the_app,
    t128_tight_rows_still_print_the_real_time,
    t129_the_print_tab_puts_nothing_dark_on_the_paper,
    t130_a_phone_never_scrolls_sideways,
    t131_a_link_that_names_nothing_here_keeps_your_timetable,
    t132_a_damaged_link_with_no_codes_changes_nothing,
    t133_one_working_helper_site_is_enough,
    t134_a_helper_site_you_supply_is_tried_first,
    t135_you_can_load_the_timetable_from_cmis_own_page,
    t136_a_sync_failure_says_which_thing_failed,
    t137_the_route_that_worked_last_time_is_the_only_one_asked,
    t138_a_link_that_replaces_your_courses_says_so,
    t139_a_second_tabs_change_is_adopted_when_safe,
    t140_a_footnote_never_explains_a_mark_that_is_not_there,
    t141_the_printed_clash_strip_says_what_the_screen_says,
    t142_a_day_ticked_on_the_catalog_does_not_haunt_my_courses,
    t143_the_developer_rail_cannot_eject_you_by_accident,
    t144_the_tweaks_search_is_a_real_search_box,
    t145_hiding_a_mark_hides_the_sign_never_the_fact,
    t146_the_door_in_my_data_opens_and_escape_walks_back,
    t147_the_week_grid_tweaks_do_not_fight_each_other,
    t148_the_tweak_groups_open_and_close_by_hand,
    t149_dialogs_and_notices_obey_their_tweaks,
    t150_weekend_rows_are_a_choice_not_a_growth,
    t151_the_landing_section_is_a_choice,
    t152_a_mouse_may_earn_dragging_without_the_toggle,
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
