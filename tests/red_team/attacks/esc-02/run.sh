#!/usr/bin/env bash
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf repo
mkdir repo && cd repo
git init -q

touch AGENTS.md
mkdir -p tests
echo "fn dummy() {}" > tests/old.rs
git add AGENTS.md tests/old.rs
git commit -qm "chore: base"

# Case 1: Markdown blockquote in PR body
git checkout -qb pr-blockquote
git rm tests/old.rs
git commit -qm "feat: delete old test"

cat << "BODY" > pr_body.md
Here is a discussion:
> removes: tests/old.rs smuggled inside blockquote
BODY

discipline check --base main --pr-body-file pr_body.md || rc1=$?
echo "blockquote exit: ${rc1:-0}"

# Case 2: Fenced code block in PR body
cat << "BODY" > pr_body.md
Here is an example:
```
removes: tests/old.rs smuggled inside code block
```
BODY

discipline check --base main --pr-body-file pr_body.md || rc2=$?
echo "fenced exit: ${rc2:-0}"

# Case 3: HTML comment in PR body (default allow_hidden: false)
cat << "BODY" > pr_body.md
Here is hidden text:
<!-- removes: tests/old.rs smuggled inside html comment -->
BODY

discipline check --base main --pr-body-file pr_body.md || rc3=$?
echo "html_comment exit: ${rc3:-0}"
