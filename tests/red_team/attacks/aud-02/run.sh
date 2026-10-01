#!/usr/bin/env bash
# aud-02: a file path that walks an `audit --format html` source link out of the repository.
# A browser reads a `%2e%2e` path segment as `..` (WHATWG URL standard, "double-dot URL path
# segment"), so a marker in a file under directories of that name links, unencoded, to
# another repository on the same forge.
. /work/lab-guard.sh || exit 99
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main
cd /tmp && rm -rf repo && mkdir repo && cd repo && git init -q
git remote add origin 'https://gitea.lab/o/r.git'
touch AGENTS.md && git add -A && git commit -qm "chore: base"
D='%2e%2e/%2e%2e/%2e%2e/%2e%2e/%2e%2e/evil/r/src/branch/main'
mkdir -p "$D" src
for f in "$D/x.rs" src/plain.rs; do
  echo 'fn a() {} // discipline:allow(time-estimates) a quoted release plan' > "$f"
done
git add -A && git commit -qm "feat: markers (#2)"
discipline audit --last 5 --format html -o audit.html 2>err.txt || rc=$?
echo "discipline exit: ${rc:-0}"
python3 - <<'PY'
import re
page = open("audit.html", encoding="utf-8").read()
def resolve(u):
    # Dot-segment removal as a browser does it, with the percent-encoded forms of `.`.
    base, _, frag = u.partition("#")
    scheme, _, rest = base.partition("://")
    host, _, path = rest.partition("/")
    out = []
    for seg in path.split("/"):
        s = seg.lower()
        if s in ("..", ".%2e", "%2e.", "%2e%2e"):
            out and out.pop()
        elif s not in (".", "%2e"):
            out.append(seg)
    return f"{scheme}://{host}/" + "/".join(out)
links = re.findall(r'class="src-link" href="([^"]+)"', page)
print("source links:", len(links))
print("outside the repository:", sorted({resolve(h) for h in links if not resolve(h).startswith("https://gitea.lab/o/r/")}))
PY
