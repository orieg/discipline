#!/usr/bin/env bash
# aud-01: markup from a hostile repository in `audit --format html`.
# Every text the repository controls carries a tag, an attribute break-out or a script:
# the origin remote's host, the author, the subject, directive reasons (with --reasons),
# a cross-repository reference, a file path, an inline marker and a configuration key.
# The page must hold no tag, event attribute or link the repository wrote.
. /work/lab-guard.sh || exit 99
set -u
git config --global user.name 'lab<img src=x onerror=alert(1)>'
git config --global user.email 'lab"><svg onload=alert(2)>@lab.invalid'
git config --global init.defaultBranch main
cd /tmp && rm -rf repo && mkdir repo && cd repo && git init -q
git remote add origin 'https://gitea.lab"><img src=x onerror=alert(3)>/o/r.git'
touch AGENTS.md && printf '[gates.time-estimates]\nenabled = true\n' > discipline.toml
git add -A && git commit -qm "chore: base"
mkdir -p src
P='src/a"><img src=x onerror=alert(4)>#?%2e.rs'
printf '%s\n' 'fn a() {} // discipline:allow(time-estimates) ships in 2 weeks <script>alert(5)</script>' \
  'fn b() {} // discipline:allow(<b onmouseover=alert(6)>x</b>) reason' > "$P"
printf '[gates.time-estimates]\nenabled = false\n"<i onclick=alert(7)>" = 1\n' > discipline.toml
git add -A && git commit -qF - <<'MSG'
feat: <script>alert(8)</script> `</code><script>alert(9)</script>` (#7)

allow-dependency: serde <img src=x onerror=alert(10)> "q' `</code><svg onload=alert(11)>`
no-issue: see evil"><b>/x#1 and o/r#2 javascript:alert(12)
MSG
discipline audit --last 5 --format html --reasons -o audit.html 2>err.txt || rc=$?
echo "discipline exit: ${rc:-0}"
python3 - <<'PY'
from html.parser import HTMLParser
src = open("audit.html", encoding="utf-8").read()
class P(HTMLParser):
    def __init__(s):
        super().__init__(); s.small = []; s.tags = {}; s.events = []; s.bad_href = []; s.in_script = False; s.script = ""
    def handle_starttag(s, t, a):
        s.tags[t] = s.tags.get(t, 0) + 1
        if t in ("img", "b", "i"): s.small.append((t, a, s.getpos()[0]))
        s.in_script = t == "script"
        for k, v in a:
            if k.startswith("on"): s.events.append((t, k))
            if k == "href" and not (v.startswith("#") or v.startswith("https://") or v.startswith("http://")):
                s.bad_href.append(v)
    def handle_endtag(s, t):
        if t == "script": s.in_script = False
    def handle_data(s, d):
        if s.in_script: s.script += d
p = P(); p.feed(src)
print("script tags:", p.tags.get("script", 0))
print("img/b/i tags:", sorted({(t, tuple(a)) for t, a, _ in p.small}))
print("svg tags:", p.tags.get("svg", 0) - src.count('class="chart"'))
print("event attributes:", p.events)
print("non-http links:", p.bad_href)
print("alert( in script:", "alert(" in p.script)
print("payloads on the page, escaped:", [n for n in range(1, 13) if f"alert({n})" in src])
PY
