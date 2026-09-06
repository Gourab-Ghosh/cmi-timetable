#!/usr/bin/env python3
"""Probe the PUBLISHED site over the real internet, after a deploy.

    .venv/bin/python live_probe.py

This is NOT the e2e suite and does not replace it. `test_app.py` blackholes
every non-localhost hostname and stands in for cmi.ac.in on localhost, which
makes it deterministic and offline — and structurally unable to notice that
the public CORS relay chain has died, that Pages is serving last week's wasm,
or that the subpath deploy has broken hash routing. This driver has NO
--host-resolver-rules, so cmi.ac.in and the relays are the real ones.

Run it after every `deploy.sh --push` and expect 10/10.

WANT_WASM must be updated to the fingerprint the deploy script reports;
check 2 is what proves the deploy was not a no-op.

THREE SELECTOR TRAPS, each of which has produced a false FAIL. Do not
"simplify" them back:

  * Chips that ADD a course are on the **Master grid**. The default tab
    (My timetable) has none, and Catalog's chips open DETAILS instead. A
    probe that grabs `.chip` on whatever tab loaded first tests nothing.
  * **Catalog renders no `<table>`** — it is 79 `.card`/`.row` pairs. An
    "every section renders" check that looks only for a table reports the
    Catalog as blank.
  * Check 6 deliberately requests a path Pages answers **404** to, because
    that 404 IS the bounce mechanism. The SEVERE console entry it leaves is
    the feature working; check 8 reloads clean and filters that URL.

And the trap that outranks all three (R94): the five sections are in-app
TABS held in prefs. The ONLY hash routes are Planner and Developer(DevTab).
A check that sets `location.hash = '#/grid'` and asserts the hash back is a
check that passes against a dead app. Click `.tabs .tab` by its text.
"""
import os, sys, time, json
from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.chrome.options import Options
from selenium.webdriver.support.ui import WebDriverWait

URL = os.environ.get("LIVE_URL", "https://gourab-ghosh.github.io/cmi-timetable/")
# ALWAYS pass this from what deploy.sh printed:
#   WANT_WASM=<hash> .venv/bin/python live_probe.py
# app/build.rs embeds APP_BUILD_TIME, so the wasm hash is a fingerprint of the
# BUILD, not of the source -- two builds of the same commit differ. The default
# below is therefore stale by design; it is here so an un-parameterised run
# fails loudly rather than silently comparing nothing.
WANT_WASM = os.environ.get("WANT_WASM", "1084f4f8463927ac")
results = []

def check(name, fn):
    try:
        detail = fn()
        results.append((True, name, detail or ""))
        print(f"PASS  {name}  {detail or ''}", flush=True)
    except Exception as e:
        results.append((False, name, str(e)[:200]))
        print(f"FAIL  {name}  {str(e)[:200]}", flush=True)

o = Options()
o.add_argument("--headless=new")
o.add_argument("--no-sandbox")
o.add_argument("--disable-dev-shm-usage")
o.add_argument("--window-size=1280,900")
o.binary_location = "/usr/bin/chromium"
d = webdriver.Chrome(options=o)
d.set_page_load_timeout(60)
W = lambda: WebDriverWait(d, 40)

try:
    d.get(URL)
    check("1 app mounts at the subpath",
          lambda: W().until(lambda x: x.find_elements(By.CSS_SELECTOR, ".tabs .tab"))
                  and f"{len(d.find_elements(By.CSS_SELECTOR,'.tabs .tab'))} tabs")

    check("2 serves this build",
          lambda: (WANT_WASM in d.page_source or WANT_WASM in d.execute_script(
              "return [...document.querySelectorAll('link,script')].map(e=>e.href||e.src).join(' ')")
          ) and WANT_WASM or (_ for _ in ()).throw(AssertionError("wasm fingerprint not "+WANT_WASM)))

    def synced():
        for _ in range(90):
            n = d.execute_script(
                "try{const r=localStorage.getItem('cmitt.v1.snapshot');"
                "return r?(JSON.parse(r).courses||[]).length:0}catch(e){return -1}")
            if n and n > 0: return f"{n} real courses from cmi.ac.in via a live relay"
            time.sleep(2)
        raise AssertionError("no courses synced in 180s — relay chain may be dead")
    check("3 first visit syncs the REAL cmi.ac.in", synced)

    def pick_survives():
        # Chips that ADD are on the Master grid; Catalog chips open details.
        grid = [t for t in d.find_elements(By.CSS_SELECTOR, ".tabs .tab")
                if t.text.strip() == "Master grid"][0]
        grid.click(); time.sleep(2)
        chips = [c for c in d.find_elements(By.CSS_SELECTOR, ".chip") if c.text.strip()]
        if not chips: raise AssertionError("no chips on the Master grid")
        label = chips[0].text.strip().split()[0]
        d.execute_script("arguments[0].scrollIntoView({block:'center'})", chips[0])
        time.sleep(0.4); chips[0].click(); time.sleep(2)
        if label not in d.current_url:
            raise AssertionError(f"picking {label!r} did not reach the URL: {d.current_url}")
        d.refresh(); time.sleep(4)
        if label not in d.current_url:
            raise AssertionError(f"pick {label!r} lost from the URL on reload")
        held = d.execute_script(
            "try{return sessionStorage.getItem('cmitt.v1.selection')||''}catch(e){return 'ERR'}")
        if label not in (held or ""):
            raise AssertionError(f"pick {label!r} not in this tab's sessionStorage: {held!r}")
        return f"{label!r} picked, in the URL and this tab's sessionStorage after a reload"
    check("4 a pick survives reload (per-tab storage, real browser)", pick_survives)

    def deep_link():
        d.get(URL + "#/developer/tweaks"); time.sleep(3)
        if "/developer" not in d.current_url:
            raise AssertionError("hash route lost: " + d.current_url)
        return "#/developer/tweaks holds"
    check("5 hash deep link works on Pages", deep_link)

    def bounce():
        d.get(URL + "some/deep/path"); time.sleep(4)
        if not d.find_elements(By.CSS_SELECTOR, ".tabs .tab"):
            raise AssertionError("404.html did not bounce into the app")
        return "bare deep path bounced through 404.html"
    check("6 a bare deep path bounces into the app", bounce)

    def sw():
        d.get(URL); time.sleep(4)
        name = d.execute_script(
            "return navigator.serviceWorker.getRegistration().then(r=>r?'registered':'none')")
        for _ in range(20):
            keys = d.execute_script("return caches.keys().then(k=>k)")
            if keys: return f"{name}, caches: {keys}"
            time.sleep(1)
        raise AssertionError("service worker registered no cache")
    check("7 service worker registers and precaches", sw)

    def console_clean():
        # The app races SEVEN relays in parallel (app/src/fetch.rs) and expects
        # losers -- cors.lol is documented in-source as "1 complete sync in 7
        # rounds", kept for operator independence. A relay that CORS-rejects or
        # 429s makes the BROWSER log a SEVERE that no JS can suppress, and the
        # chain absorbing it is the design working. Check 3 already proves the
        # sync succeeded, so what must stay zero here is errors from OUR OWN
        # code and OUR OWN origin's assets.
        THIRD_PARTY = ("cors.sh", "cors-get-proxy", "corsmirror", "r.jina.ai",
                       "allorigins.win", "codetabs.com", "cors.lol",
                       "cmi.ac.in", "some/deep/path")
        d.get(URL); time.sleep(6)
        raw = [l for l in d.get_log("browser") if l["level"] == "SEVERE"]
        ours = [l for l in raw
                if not any(h in l["message"] for h in THIRD_PARTY)]
        if ours:
            raise AssertionError(f"{len(ours)} SEVERE from our own code: "
                                 f"{ours[0]['message'][:150]}")
        skipped = len(raw) - len(ours)
        note = "no SEVERE from our own code"
        if skipped:
            note += f" ({skipped} relay/CMI fetch failures ignored — the race absorbed them)"
        return note

    check("8 console has no severe errors", console_clean)

    def phone():
        d.set_window_size(390, 844); time.sleep(3)
        sw_, cw = d.execute_script(
            "return [document.documentElement.scrollWidth, document.documentElement.clientWidth]")
        if sw_ > cw + 1: raise AssertionError(f"sideways scroll: {sw_} > {cw}")
        return f"390px: scrollWidth {sw_} == clientWidth {cw}"
    check("9 no sideways scroll at phone width", phone)

    def tabs_click():
        d.set_window_size(1280, 900); time.sleep(2)
        seen = []
        for name in ["My timetable", "My courses", "Master grid", "Catalog", "Halls"]:
            t = [x for x in d.find_elements(By.CSS_SELECTOR, ".tabs .tab")
                 if x.text.strip() == name]
            if not t: raise AssertionError(f"tab {name!r} is missing")
            t[0].click(); time.sleep(1.6)
            n = d.execute_script("""
              const m=document.querySelector('main')||document.body;
              return {sec:m.querySelectorAll('section').length,
                      body:m.querySelectorAll('table,.card,.chip,.row,.empty').length,
                      chars:(m.innerText||'').trim().length};""")
            if not n["sec"] or not n["body"] or n["chars"] < 40:
                raise AssertionError(f"section {name!r} rendered nothing real: {n}")
            seen.append(f"{name}({n['body']})")
        return "all five render: " + ", ".join(seen)
    check("10 every in-app section renders (clicked, not hash-faked)", tabs_click)
finally:
    d.quit()

ok = sum(1 for r in results if r[0])
print(f"\n{ok}/{len(results)} passed", flush=True)
sys.exit(0 if ok == len(results) else 1)
