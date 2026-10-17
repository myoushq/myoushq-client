#!/usr/bin/env python3
"""The worker's review hook: the one place that decides what runs.

The worker calls it before every request, with one JSON object on stdin
(protocol.md 7.2):

    {"op": "exec" | "get" | "put", "id": ..., "sender": <npub>, "alias": ...,
     "cmd": ...}            for exec
     "path": ..., "size": ...}   for get / put

Exit 0 to allow. Any other status refuses, and the first line of stderr
goes back to the requester as the reason.

Policy: only approved contacts' messages reach the worker at all, so
"everything" below means the owner's own paired agents. The pause file
refuses everything. Then the review level in <home>/review.json (the
desktop app's setting) decides:

- "trust": everything runs.
- "changes": reads run (get, and exec of a read-only command whose
  flags are known safe and whose paths stay in the work directory, see
  read_only); anything else asks the owner.
- "all": every request asks the owner.

Without a review.json the level is $MYOUS_DEFAULT_REVIEW (the container
sets "changes"), or "trust" when that is unset. A review.json, or a
MYOUS_DEFAULT_REVIEW, that can't be read as a level asks about everything
("all") until it is fixed.

Asking: the hook writes <home>/approvals/<id>.json (the request) and waits
up to MYOUS_APPROVAL_WAIT seconds (default 120, inside an agent's default
wait for a reply) for <home>/approvals/<id>.answer containing "allow" or
"refuse"; the desktop app shows the question and writes the answer. No
answer in time is a refusal that says so. The id names those files, so it
must be a plain name: 1 to 64 characters from A-Z, a-z, 0-9, - and _ (the
rule the workers and the apps hold ids to); a request with any other id
is refused rather than asked about.

The worker itself logs every request and decision to worker.log.
"""
from __future__ import annotations

import json
import os
import re
import shlex
import stat
import sys
import time

HOME = os.path.expanduser(os.environ.get("MYOUS_HOME", "~/.myous"))
PAUSE_FILE = os.environ.get("MYOUS_PAUSE_FILE", os.path.join(HOME, "worker.paused"))
REVIEW_FILE = os.path.join(HOME, "review.json")
APPROVALS = os.path.join(HOME, "approvals")
WAIT = float(os.environ.get("MYOUS_APPROVAL_WAIT", "120"))
ID_RULE = re.compile(r"[A-Za-z0-9_-]{1,64}")
BAD_ID = "bad request: the id must be 1 to 64 characters from A-Z, a-z, 0-9, - and _"

# Commands that only read, for the "changes" level. A command line is
# read-only when every segment (split on |, &&, ||, ;, &) starts with one
# of these, nothing redirects output, substitutes commands, expands a
# variable or runs code inside one (find -exec, awk '{...}'), every flag
# is one known safe for the command (SAFE_FLAGS), and no operand points
# outside the worker's work directory (W3: see _escapes_work; the work
# directory is the command's cwd, so a plain relative path stays inside
# it). A single & backgrounds a command and starts another, so it is a
# separator too: without it, `echo & rm x` was judged by `echo` alone and
# ran `rm x` unreviewed.
READ_ONLY = {
    "ls", "cat", "head", "tail", "less", "more", "grep", "rg", "find", "wc", "stat", "file", "pwd",
    "du", "df", "uname", "date", "whoami", "id", "echo", "printf", "sort", "uniq", "cut", "tr",
    "awk", "diff", "tree", "which", "type", "true", "test", "basename", "dirname", "realpath",
    "readlink", "md5sum", "sha256sum", "shasum", "hostname", "nproc", "free", "uptime",
}
# $'...' and $"..." are bash's (and macOS sh's) quoting: $'\x2fetc' is
# /etc there, a path the review can't see in the split words (dash leaves
# them as they are). So any $ before a quote asks, a regex's 'x$' too.
UNSAFE_TOKENS = (">", "<", "$(", "`", "sed -i", "tee ", "{", "-exec", "-ok", "-delete", "$'", '$"')
# A few allowlisted commands still write with the right flag: `sort -o/
# --output FILE` writes (and overwrites) a file, and GNU find's -fprint,
# -fprintf, -fprint0 and -fls write one. These are command-specific (a
# bare "-o" is harmless for grep), so they can't be global UNSAFE_TOKENS;
# _writes_a_file checks them against the command's own name.
FIND_WRITE_PRIMARIES = ("-fprint", "-fprintf", "-fprint0", "-fls")
# On Windows the worker's shell is PowerShell: its read-only commands, and
# what runs code inside an otherwise harmless one ((...), & and script blocks).
WINDOWS = os.name == "nt"
READ_ONLY_POWERSHELL = {
    "get-childitem", "gci", "dir", "get-content", "gc", "type", "get-item", "gi", "get-itemproperty",
    "get-location", "gl", "select-string", "sls", "measure-object", "measure", "get-date", "get-process",
    "gps", "get-command", "gcm", "test-path", "resolve-path", "get-filehash", "get-psdrive", "get-volume",
    "get-service", "write-output", "write-host", "format-table", "ft", "format-list", "fl", "out-string",
    "select-object", "select", "sort-object", "where.exe", "findstr", "ipconfig", "systeminfo",
}
UNSAFE_POWERSHELL = ("(", "&", "@", "--%", "Invoke-", "iex")
# The Windows programs among those: their switches aren't PowerShell
# parameters, so they take SAFE_FLAGS's rule (no entry: no flags), and a
# /switch is checked like a path (so it asks).
WINDOWS_PROGRAMS = ("where.exe", "findstr", "ipconfig", "systeminfo")

# At the "changes" level a read-only command runs unasked only when every
# flag it carries is one known safe for it: one that only reads, or
# formats what it prints. A flag that names a file to write, a program to
# run or a command string, or that changes the system, is never here, and
# a flag that isn't here asks: "read-only" commands write and run things
# through flags (sort -o, tree -o, less -o, rg --pre), and a list of the
# bad ones is always behind. Per command: the safe flags; those of them
# that take a value (attached, -n20 -d: --lines=20, or the next word); and
# those whose value is a file, which must stay in the work directory like
# an operand. Values are not checked as paths, so a flag is listed as
# taking one only where every version of the command (GNU, BSD, macOS)
# takes one; where one doesn't (grep -C and --context on macOS, stat -f,
# more -p), it is listed without, and the word after it is checked as an
# operand. Short flags combine (-la is -l -a; digits are flags of head,
# tail and grep: -20). A word that is itself a listed flag is that flag
# (find -name, test -nt). A command without an entry allows no flags.
SAFE_FLAGS = {
    "awk": ("-F -v", "-F -v", ""),
    "basename": ("-a -s -z --multiple --suffix --zero", "-s --suffix", ""),
    "cat": ("-A -E -T -b -e -n -s -t -u -v --number --number-nonblank --show-all --show-ends --show-nonprinting "
            "--show-tabs --squeeze-blank", "", ""),
    "cut": ("-b -c -d -f -n -s -z --bytes --characters --complement --delimiter --fields --only-delimited "
            "--output-delimiter --zero-terminated",
            "-b -c -d -f --bytes --characters --delimiter --fields --output-delimiter", ""),
    "date": ("-I -R -j -r -u --iso-8601 --reference --rfc-2822 --rfc-3339 --rfc-email --universal --utc",
             "-r --reference --rfc-3339", "-r --reference"),
    "df": ("-H -P -T -a -h -i -k -l --all --human-readable --inodes --local --portability --print-type --si", "", ""),
    "diff": ("-B -N -U -a -b -c -i -q -r -s -t -u -w -x -y --brief --color --context --exclude --expand-tabs "
             "--from-file --ignore-all-space --ignore-blank-lines --ignore-case --ignore-space-change --new-file "
             "--recursive --report-identical-files --side-by-side --strip-trailing-cr --text --to-file --unified",
             "-U -x --exclude --from-file --to-file", "--from-file --to-file"),
    "dirname": ("-z --zero", "", ""),
    "du": ("-a -c -d -h -k -m -s -x --all --apparent-size --exclude --human-readable --max-depth --one-file-system "
           "--summarize --total", "-d --exclude --max-depth", ""),
    "echo": ("-E -e -n", "", ""),
    "file": ("-I -L -N -b -h -i -k --brief --dereference --keep-going --mime --mime-encoding --mime-type "
             "--no-dereference --no-pad", "", ""),
    "find": ("-a -amin -and -anewer -atime -cmin -cnewer -ctime -depth -empty -executable -false -gid -group -iname "
             "-inum -ipath -iregex -iwholename -links -ls -maxdepth -mindepth -mmin -mount -mtime -name -newer "
             "-nogroup -not -nouser -o -or -path -perm -print -print0 -printf -prune -quit -readable -regex "
             "-regextype -samefile -size -true -type -uid -user -wholename -writable -xdev -xtype",
             "-amin -anewer -atime -cmin -cnewer -ctime -gid -group -iname -inum -ipath -iregex -iwholename -links "
             "-maxdepth -mindepth -mmin -mtime -name -newer -path -perm -printf -regex -regextype -samefile -size "
             "-type -uid -user -wholename -xtype", "-anewer -cnewer -newer -samefile"),
    "free": ("-b -g -h -k -m -t -w --bytes --giga --human --kilo --mega --total --wide", "", ""),
    "grep": ("-0 -1 -2 -3 -4 -5 -6 -7 -8 -9 -A -B -C -E -F -G -H -I -L -P -R -a -b -c -e -f -h -i -l -m -n -o -q -r -s "
             "-v -w -x --after-context --basic-regexp --before-context --byte-offset --color --colour --context "
             "--count --dereference-recursive --exclude --exclude-dir --extended-regexp --file --files-with-matches "
             "--files-without-match --fixed-strings --ignore-case --include --invert-match --line-number "
             "--line-regexp --max-count --no-filename --no-messages --only-matching --quiet --recursive --regexp "
             "--silent --text --with-filename --word-regexp",
             "-e -f --exclude --exclude-dir --file --include --regexp", "-f --file"),
    "head": ("-0 -1 -2 -3 -4 -5 -6 -7 -8 -9 -c -n -q -v --bytes --lines --quiet --silent --verbose",
             "-c -n --bytes --lines", ""),
    "id": ("-G -g -n -p -r -u --group --groups --name --real --user", "", ""),
    "less": ("-E -F -I -J -M -N -Q -R -S -W -X -e -f -g -i -m -n -p -q -r -s -w -x --IGNORE-CASE --LINE-NUMBERS "
             "--RAW-CONTROL-CHARS --chop-long-lines --ignore-case --no-init --pattern --quit-at-eof "
             "--quit-if-one-screen --squeeze-blank-lines --tabs", "-p -x --pattern --tabs", ""),
    "ls": ("-1 -A -C -F -G -L -R -S -a -c -d -h -i -k -l -m -n -o -p -r -s -t -u -x --all --almost-all --classify "
           "--color --dereference --directory --full-time --group-directories-first --human-readable --inode "
           "--numeric-uid-gid --recursive --reverse --size", "", ""),
    "md5sum": ("-b -c -t -w -z --binary --check --ignore-missing --quiet --status --strict --tag --text --warn --zero",
               "", ""),
    "more": ("-c -d -f -l -p -s -u", "", ""),
    "nproc": ("--all", "", ""),
    "pwd": ("-L -P", "", ""),
    "readlink": ("-e -f -m -n -q -s -v -z --canonicalize --canonicalize-existing --canonicalize-missing "
                 "--no-newline --quiet --silent --verbose --zero", "", ""),
    "realpath": ("-L -P -e -m -q -s -z --canonicalize-existing --canonicalize-missing --logical --no-symlinks "
                 "--physical --quiet --strip --zero", "", ""),
    "rg": ("-0 -A -B -C -F -H -I -M -N -S -T -U -a -c -d -e -f -g -i -j -l -m -n -o -p -q -r -s -t -u -v -w -x "
           "--after-context --before-context --case-sensitive --color --column --context --count --count-matches "
           "--file --files --files-with-matches --files-without-match --fixed-strings --glob --heading --hidden "
           "--iglob --ignore-case --invert-match --json --line-number --line-regexp --max-columns --max-count "
           "--max-depth --no-filename --no-heading --no-ignore --no-line-number --null --only-matching --pretty "
           "--quiet --regexp --replace --smart-case --sort --sortr --stats --threads --trim --type --type-list "
           "--type-not --vimgrep --with-filename --word-regexp",
           "-A -B -C -M -T -d -e -f -g -j -m -r -t --after-context --before-context --color --context --file --glob "
           "--iglob --max-columns --max-count --max-depth --regexp --replace --sort --sortr --threads --type "
           "--type-not", "-f --file"),
    "sha256sum": ("-b -c -t -w -z --binary --check --ignore-missing --quiet --status --strict --tag --text --warn "
                  "--zero", "", ""),
    "shasum": ("-0 -a -b -c -t --algorithm --binary --check --ignore-missing --quiet --status --strict --tag --text "
               "--warn", "-a --algorithm", ""),
    "sort": ("-C -M -V -b -c -d -f -g -h -i -k -m -n -r -s -t -u -z --check --dictionary-order --field-separator "
             "--general-numeric-sort --human-numeric-sort --ignore-case --ignore-leading-blanks "
             "--ignore-nonprinting --key --merge --month-sort --numeric-sort --reverse --stable --unique "
             "--version-sort --zero-terminated", "-k -t --field-separator --key", ""),
    "stat": ("-L -c -f -l -t --dereference --file-system --format --printf --terse", "-c --format --printf", ""),
    "tail": ("-0 -1 -2 -3 -4 -5 -6 -7 -8 -9 -F -c -f -n -q -r -v --bytes --follow --lines --quiet --retry --silent "
             "--verbose", "-c -n --bytes --lines", ""),
    "test": ("-G -L -N -O -S -a -b -c -d -e -ef -eq -f -g -ge -gt -h -k -le -lt -n -ne -nt -o -ot -p -r -s -t -u -w "
             "-x -z", "", ""),
    "tr": ("-C -c -d -s -t --complement --delete --squeeze-repeats --truncate-set1", "", ""),
    "tree": ("-A -C -D -F -I -J -L -N -P -Q -S -U -X -a -c -d -f -g -h -i -n -p -r -s -t -u -v --dirsfirst --du "
             "--ignore-case --matchdirs --noreport --prune", "-I -L -P", ""),
    "type": ("-P -a -p -t", "", ""),
    "uname": ("-a -i -m -n -o -p -r -s -v --all --hardware-platform --kernel-name --kernel-release --kernel-version "
              "--machine --nodename --operating-system --processor", "", ""),
    "uniq": ("-D -c -d -f -i -s -u -w --all-repeated --check-chars --count --ignore-case --repeated --skip-chars "
             "--skip-fields --unique", "-f -s -w --check-chars --skip-chars --skip-fields", ""),
    "uptime": ("-p -s --pretty --since", "", ""),
    "wc": ("-L -c -l -m -w --bytes --chars --lines --max-line-length --words", "", ""),
    "which": ("-a -s", "", ""),
}
# Operands that aren't only paths in the work directory: awk's first
# operand is its program, which must not hold AWK_UNSAFE (system() and
# getline run and read things without the braces UNSAFE_TOKENS catches);
# date's must be +FORMAT (any other sets the clock); hostname NAME sets the
# name; uniq's second operand is a file it writes; less and more run a
# +command.
OPERANDS = {"awk": "program", "date": "+format", "hostname": "none", "less": "no+", "more": "no+", "uniq": "one"}
AWK_UNSAFE = ("system", "getline", "|", ">", "{", "@", "environ")   # in the program, in lower case
# $NAME, ${...}, $(...), $1, $? (and PowerShell's $env:X, $HOME): the
# shell puts in a value the review can't see, a path ($HOME/.ssh) or a
# secret (echo $AWS_SECRET_ACCESS_KEY). Quotes don't help: the split words
# no longer say whether '$x' was quoted, so any of these asks.
EXPANSION = re.compile(r"\$[A-Za-z0-9_{(?!$#@*-]")


LEVELS = ("trust", "changes", "all")
# A review.json that is present but can't be read as a valid level fails
# closed to "all" (ask about everything), not open to "trust": a worker
# whose setting file was truncated or corrupted (W2 used a truncated
# review.json to drop the worker to "trust") asks rather than running
# everything unreviewed. An absent file is a worker that was never
# configured, and keeps the default (default_level).
FAIL_CLOSED = "all"
LOG_FILE = os.path.join(HOME, "worker.log")


def note(record: dict) -> None:
    """One line in worker.log, as the worker writes its own (the owner's
    app shows the log): for what the review can't say in a reason."""
    try:
        with open(LOG_FILE, "a") as f:
            f.write(json.dumps(dict(record, at=int(time.time())), sort_keys=True) + "\n")
    except OSError:
        pass


def default_level() -> str:
    """The level of a worker that has never been configured (no
    review.json): $MYOUS_DEFAULT_REVIEW, or "trust" when that is unset or
    empty. The container's entrypoint sets it to "changes", so a fresh
    container asks before anything that changes things, as direct mode (the
    desktop app) already does; a level picked in the app writes
    review.json, which then wins. A value that isn't a level fails closed
    like a corrupt review.json (an unreadable safety setting asks, it
    doesn't run everything), and says so in worker.log."""
    lv = os.environ.get("MYOUS_DEFAULT_REVIEW", "")
    if not lv:
        return "trust"
    if lv in LEVELS:
        return lv
    note({"op": "review", "value": lv,
          "error": "MYOUS_DEFAULT_REVIEW isn't trust, changes or all, so every request is asked about (all)"})
    return FAIL_CLOSED


def _not_json(name: str):
    raise ValueError(f"{name} is not JSON")   # NaN and Infinity, which Python's json would take


def _read_regular(path: str) -> bytes:
    """A regular file's bytes, opened without following a link and without
    waiting on a pipe; anything else raises OSError."""
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(fd, "rb") as f:
        if not stat.S_ISREG(os.fstat(f.fileno()).st_mode):
            raise OSError(f"{path} is not a regular file")
        return f.read()


def level() -> str:
    """review.json's level. Absent: the default (default_level). Anything
    else that isn't a regular file holding a JSON object, in UTF-8, with one
    of LEVELS as its "level": FAIL_CLOSED. A link, a pipe, a directory or a
    socket is not read (a pipe would block the review), as the app shows
    them: as "all"."""
    try:
        if not stat.S_ISREG(os.lstat(REVIEW_FILE).st_mode):
            return FAIL_CLOSED
    except FileNotFoundError:
        return default_level()  # never configured
    except OSError:
        return FAIL_CLOSED
    try:
        data = json.loads(_read_regular(REVIEW_FILE).decode("utf-8"), parse_constant=_not_json)
    except (OSError, ValueError):
        return FAIL_CLOSED  # unreadable, not UTF-8 or not JSON
    lv = data.get("level") if isinstance(data, dict) else None
    return lv if lv in LEVELS else FAIL_CLOSED


def _writes_a_file(words: list[str]) -> bool:
    """Whether an otherwise read-only command writes a file through one of
    its own flags: `sort -o/--output FILE` (also `-oFILE`), and GNU find's
    -fprint/-fprintf/-fprint0/-fls."""
    name, rest = words[0], words[1:]
    if name == "sort":
        for a in rest:
            if a in ("-o", "--output") or a.startswith("--output=") or (a.startswith("-o") and not a.startswith("--") and len(a) > 2):
                return True
    elif name == "find":
        if any(a in FIND_WRITE_PRIMARIES for a in rest):
            return True
    return False


def _unquoted(word: str, windows: bool) -> str:
    """A word as the command gets it: non-POSIX shlex (Windows) keeps the
    quotes around a word, and one layer of them comes off."""
    if windows and len(word) >= 2 and word[0] == word[-1] and word[0] in "'\"":
        return word[1:-1]
    return word


def _escapes_work(token: str, windows: bool) -> bool:
    """Whether an operand, or a file a flag names, could point outside the
    work directory (W3). The work directory is the command's cwd, so a
    plain relative path stays inside it; an absolute path, a ~ the shell
    expands to a home directory, a `..` component, or a pattern the shell
    can expand to one (dash's and macOS sh's `.*` matches `..`) can leave
    it. On Windows also a drive or any other PowerShell drive (C:, env:,
    HKCU:: any colon), a path that starts with a backslash (UNC too), and
    each part of a comma list (an array to PowerShell). Conservative: a
    word that merely looks like a path (a /regex/, say) is treated as one,
    so the owner is asked rather than the command run unseen -- more
    prompts, never fewer."""
    t = _unquoted(token, windows)
    if windows and "," in t:
        return any(_escapes_work(part, windows) for part in t.split(","))
    if not t:
        return False
    if t[0] in "~/" or (windows and (t[0] == "\\" or ":" in t)):
        return True
    for part in re.split(r"[\\/]" if windows else "/", t):
        if part == ".." or (part.startswith(".") and any(c in part for c in "*?[")):
            return True
    return False


def _flags_safe(words: list[str], windows: bool) -> bool:
    """Whether a read-only command's flags are all known safe for it
    (SAFE_FLAGS), its operands are what OPERANDS allows, and they and the
    files its flags name stay in the work directory (_escapes_work). After
    `--` every word is an operand; a bare `-` is stdin."""
    name = words[0].lower() if windows else words[0]
    flags, valued, files = (set(s.split()) for s in SAFE_FLAGS.get(name, ("", "", "")))
    operands: list[str] = []
    rest = words[1:]
    while rest:
        word, rest = rest[0], rest[1:]
        w = _unquoted(word, windows)
        if w != word and w in ("-", "--"):
            return False   # quoted (Windows): PowerShell may join it to the next word
        if w == "--":
            operands += [_unquoted(x, windows) for x in rest]
            break
        if w == "-" or not w.startswith("-"):
            operands.append(w)
            continue
        if w.startswith("--") or w in flags:
            flag, eq, value = w.partition("=") if w.startswith("--") else (w, "", "")
            if flag not in flags:
                return False
            if flag in valued and not eq:
                if not rest:
                    return False
                value, rest = _unquoted(rest[0], windows), rest[1:]
            if flag in files and _escapes_work(value, windows):
                return False
            continue
        for j in range(1, len(w)):   # -la, -n20, -d:
            flag = "-" + w[j]
            if flag not in flags:
                return False
            if flag in valued:
                value = w[j + 1:]
                if not value:
                    if not rest:
                        return False
                    value, rest = _unquoted(rest[0], windows), rest[1:]
                if flag in files and _escapes_work(value, windows):
                    return False
                break
    rule = OPERANDS.get(name, "")
    if rule == "program":
        if not operands or any(bad in operands[0].lower() for bad in AWK_UNSAFE):
            return False
        operands = operands[1:]
    elif (rule == "none" and operands) or (rule == "one" and len(operands) > 1):
        return False
    elif rule == "+format" and any(not o.startswith("+") for o in operands):
        return False
    elif rule == "no+" and any(o.startswith("+") for o in operands):
        return False
    return not any(_escapes_work(o, windows) for o in operands)


def read_only(cmd: str, windows: bool = WINDOWS) -> bool:
    if any(tok in cmd for tok in UNSAFE_TOKENS) or EXPANSION.search(cmd):
        return False
    if windows and any(tok.lower() in cmd.lower() for tok in UNSAFE_POWERSHELL):
        return False
    # && before &: once && is gone, the remaining & are single backgrounders.
    for sep in ("||", "&&", "|", ";", "&"):
        cmd = cmd.replace(sep, "\n")
    for segment in cmd.splitlines():
        try:
            words = shlex.split(segment, posix=not windows)
        except ValueError:
            return False
        if not words:
            continue
        verb = words[0].lower() if windows else words[0]
        if windows and verb in READ_ONLY_POWERSHELL and verb not in WINDOWS_PROGRAMS:
            # PowerShell's own cmdlets and aliases: their parameters are
            # PowerShell's (-Recurse, -Path x, -Path:x); their paths must
            # stay in the work directory.
            for w in words[1:]:
                if _escapes_work(w.partition(":")[2] if w.startswith("-") else w, windows):
                    return False
            continue
        if verb not in READ_ONLY and not (windows and verb in WINDOWS_PROGRAMS):
            return False
        if _writes_a_file(words) or not _flags_safe(words, windows):
            return False
    return True


def ask(req: dict) -> tuple[bool, str]:
    """Write the question, wait for the owner's answer file."""
    rid = req.get("id")
    if not isinstance(rid, str) or not ID_RULE.fullmatch(rid):
        return False, BAD_ID   # the owner's app could not answer it
    os.makedirs(APPROVALS, exist_ok=True)
    question = os.path.join(APPROVALS, rid + ".json")
    answer = os.path.join(APPROVALS, rid + ".answer")
    # Only an answer written for *this* question counts. A leftover
    # <id>.answer (the owner clicked after the wait ran out, so the earlier
    # question was already gone; or a stop killed the hook before it read
    # the answer) must not approve a later request that reuses the id: the
    # agent chooses ids, so it could reuse one the owner once approved.
    try:
        os.unlink(answer)
    except OSError:
        pass
    with open(question + ".tmp", "w") as f:
        json.dump(dict(req, asked_at=int(time.time()), wait=WAIT), f)
    os.replace(question + ".tmp", question)
    deadline = time.time() + WAIT
    try:
        while time.time() < deadline:
            try:
                with open(answer) as f:
                    verdict = f.read().strip().lower()
                os.unlink(answer)
                return verdict == "allow", "refused by the owner"
            except FileNotFoundError:
                time.sleep(0.5)
        return False, "the owner didn't answer in time"
    finally:
        try:
            os.unlink(question)
        except OSError:
            pass


def decide(req: dict, lv: str) -> tuple[bool, str]:
    if os.path.exists(PAUSE_FILE):
        return False, "the worker is paused by its owner"
    if lv == "trust":
        return True, ""
    if lv == "changes" and (req.get("op") == "get" or (req.get("op") == "exec" and read_only(str(req.get("cmd", ""))))):
        return True, ""
    return ask(req)


def main() -> int:
    try:
        req = json.load(sys.stdin)
    except ValueError:
        print("review: bad request", file=sys.stderr)
        return 2
    ok, why = decide(req, level())
    if not ok:
        print(why, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
