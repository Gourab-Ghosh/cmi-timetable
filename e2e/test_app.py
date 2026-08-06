#!/usr/bin/env python3
"""End-to-end browser tests for the CMI Timetable Planner.

Serves the built app (app/dist) on a local port and drives it with Selenium
(headless Chromium). Run after `trunk build --release`:

    python e2e/test_app.py

Environment:
    DIST_DIR    directory to serve   (default: ../app/dist relative to this file)
    CHROME_BIN  browser binary       (default: /usr/bin/chromium)
    PORT        local port           (default: 8977)

Each test boots the app with a fresh localStorage. Background syncing is
suppressed by seeding `cmitt.v1.prefs.last_update_attempt`, so every test
runs against the deterministic bundled snapshot.
"""

import http.server
import json
import os
import sys
import threading
import time
import traceback

from selenium import webdriver
from selenium.webdriver.chrome.options import Options
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait

HERE = os.path.dirname(os.path.abspath(__file__))
DIST = os.environ.get("DIST_DIR", os.path.join(HERE, "..", "app", "dist"))
PORT = int(os.environ.get("PORT", "8977"))
BASE = f"http://127.0.0.1:{PORT}"
CHROME_BIN = os.environ.get("CHROME_BIN", "/usr/bin/chromium")


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
    return webdriver.Chrome(options=opts)


class App:
    def __init__(self, driver):
        self.d = driver
        self.wait = WebDriverWait(driver, 15)

    # -- lifecycle ---------------------------------------------------------

    def boot(self, path="/", fresh=True):
        """Load the app; with fresh=True, wipe storage and suppress the
        background sync so tests run on the bundled snapshot."""
        if fresh:
            self.d.get(f"{BASE}/e2e-blank")  # same-origin 404 page
            self.d.execute_script(
                "localStorage.clear();"
                "localStorage.setItem('cmitt.v1.prefs', arguments[0]);",
                json.dumps({"last_update_attempt": time.time() * 1000.0}),
            )
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
    assert "Unknown course code: XYZQ" in banner.text, banner.text
    app.chip("TOC")  # TOC still selected and rendered
    banner.find_element(By.XPATH, ".//button[normalize-space()='Dismiss']").click()
    app.wait_gone(".banner")


def t05_credits_default_four(app):
    """Unstated credits count as 4; stated ones stay verbatim."""
    app.boot("/?c=TOC,RDBM")
    app.open_tab("My courses")
    section = app.wait_css("section[aria-label='My courses']")
    assert "Total credits: 6" in section.text, section.text  # 4 (assumed) + 2
    assert "4 assumed for 1 course" in section.text, section.text
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
    assert "Your overwrites" in dialog.text
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
    # Slot picked but no day: no result line yet.
    slot_sel.find_element(By.CSS_SELECTOR, "option[value='840']").click()
    assert "Free on" not in section.text
    day_sel.find_element(By.CSS_SELECTOR, "option[value='1']").click()  # Tuesday
    WebDriverWait(app.d, 10).until(lambda d: "Free on Tuesday" in app.css(
        "section[aria-label='Lecture halls']").text)
    text = app.css("section[aria-label='Lecture halls']").text
    # Tue 14:00: Seminar Hall is free, Lecture Hall 6 is not (LIEA).
    line = next(l for l in text.splitlines() if l.startswith("Free on Tuesday"))
    assert "Seminar Hall" in line, line
    assert "Lecture Hall 6" not in line, line


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
    assert "Total credits: 6" in section.text  # 4 assumed + 2 stated
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
    assert "Total credits: 5" in section.text, section.text
    assert "1 set by you" in section.text, section.text
    # The 'Your changes' panel shows official → yours; removing it restores.
    app.open_tab("My timetable")
    panel = app.wait_css("[data-testid='your-changes']")
    assert "credits: 4 (assumed) → 3" in panel.text, panel.text
    app.xpath("//button[contains(.,'1 overwrite')]")  # toolbar pill
    panel.find_element(
        By.XPATH, ".//li[contains(.,'TOC')]//button[normalize-space()='Remove']"
    ).click()
    app.wait_toast("TOC back on official credits")
    app.wait_gone("[data-testid='your-changes']")
    app.open_tab("My courses")
    assert "Total credits: 6" in app.css("section[aria-label='My courses']").text


def t18_overwrites_panel_and_remove_all(app):
    """Meeting moves and credit changes appear together with provenance;
    'Remove all overwrites' restores CMI's data in one step."""
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
    assert "overwrites CMI's Tue 09:10–10:25" in section.text, section.text
    # The panel lists both overwrites; the pill counts them.
    app.open_tab("My timetable")
    panel = app.wait_css("[data-testid='your-changes']")
    assert "→ Wed 17:00–18:15" in panel.text, panel.text
    assert "credits: 4 (assumed) → 2" in panel.text, panel.text
    app.xpath("//button[contains(.,'2 overwrites')]")
    panel.find_element(
        By.XPATH, ".//button[normalize-space()='Remove all overwrites']"
    ).click()
    app.wait_toast("All overwrites removed")
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
    app.xpath("//button[contains(.,'2 overwrites')]")


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
]


def main():
    if not os.path.isdir(DIST):
        sys.exit(f"dist directory not found: {DIST} — run `trunk build --release` first")
    server = serve_dist()
    driver = make_driver()
    app = App(driver)
    failures = []
    try:
        for test in TESTS:
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
    print(f"\n{len(TESTS) - len(failures)}/{len(TESTS)} passed")
    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
