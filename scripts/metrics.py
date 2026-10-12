#!/usr/bin/env python3
"""Report reviewability metrics for Krit source files.

Usage:
    python3 -I scripts/metrics.py [--json] [--check-depth N] [--check-lines N] PATH...

Each PATH is a `.krit` file or a directory whose `.krit` files (not those of
subdirectories) are measured. Paths are reported with `/` separators on every
platform. For every file the report gives:

    lines      physical lines
    code       lines that hold code: not blank and not only a comment
    depth      deepest indentation of a code line, in 4-space levels; the
               inside of a text block is string content and is not counted,
               its closing line is
    nested     `match` expressions written inside an arm of another `match`
    dup-err    `Err` arms (of a `match` or a let-else) that repeat an earlier
               `Err` arm of the same file token for token
    record     uses of the `record` keyword
    longest    the longest line, in characters
    authority  string literals that name a resource: the first argument of a
               capability-bearing built-in such as `http_request` or
               `secret`, and the names in `queue "..."` and `schedule "..."`
               entrypoint headers

`--json` prints the same facts, plus the line of the deepest and longest
lines, the total `match` and `Err`-arm counts, and the identity propagation
arms (`Err(e) => Err(e)`), as one JSON document. `--check-depth N` exits with
status 1 when any file is deeper than N levels and `--check-lines N` when any
file is longer than N lines, naming each violation on standard error.

The measurement is lexical: it needs only the Python 3 standard library and
never runs the Krit toolchain, so it also measures files that do not check.
"""

import argparse
import json
import os
import sys

INDENT_WIDTH = 4

# Built-ins whose first argument is a literal resource name
# (spec/CAPABILITIES.md); `queue` and `schedule` headers are handled apart.
AUTHORITY_BUILTINS = frozenset(
    [
        "ai_invoke",
        "cache_delete",
        "cache_get",
        "cache_put",
        "checkpoint_get",
        "checkpoint_put",
        "config_string",
        "db_begin_read",
        "db_begin_write",
        "http_request",
        "object_delete",
        "object_get",
        "object_put",
        "queue_publish",
        "replay_ai",
        "replay_http",
        "search_query",
        "secret",
        "state_delete",
        "state_get",
        "state_put",
        "vector_search",
    ]
)

ENTRYPOINT_KEYWORDS = frozenset(["queue", "schedule"])
LET_ELSE_PATTERNS = frozenset(["Ok", "Err", "Some", "None", "_"])
OPENERS = {"{": "}", "(": ")", "[": "]"}
TWO_CHARACTER_PUNCTUATION = ("=>", "..", "==", "!=", "<=", ">=", "&&", "||", "->")
CLOSERS = frozenset(OPENERS.values())


class Token:
    __slots__ = ("kind", "text", "line", "interpolated")

    def __init__(self, kind, text, line, interpolated=False):
        self.kind = kind
        self.text = text
        self.line = line
        self.interpolated = interpolated


def scan(text):
    """Splits source into tokens and classifies every physical line.

    Returns the tokens, the set of line numbers holding code, and the set of
    line numbers inside a text block (excluding its opening and closing
    lines). Line numbers start at 1.
    """
    tokens = []
    code_lines = set()
    interior_lines = set()
    line = 1
    index = 0
    length = len(text)
    while index < length:
        char = text[index]
        if char == "\n":
            line += 1
            index += 1
        elif char in " \t\r\f\v":
            index += 1
        elif text.startswith("//", index):
            end = text.find("\n", index)
            index = length if end < 0 else end
        elif text.startswith('"""', index):
            start_line = line
            cursor = index + 3
            while True:
                found = text.find('"""', cursor)
                if found < 0:
                    found = length
                    break
                if text[found - 1] != "\\":
                    break
                cursor = found + 3
            body = text[index:found]
            end_line = start_line + body.count("\n")
            code_lines.update(range(start_line, end_line + 1))
            interior_lines.update(range(start_line + 1, end_line))
            tokens.append(Token("text-block", body, start_line))
            line = end_line
            index = min(found + 3, length)
        elif char == '"':
            cursor = index + 1
            while cursor < length and text[cursor] not in '"\n':
                cursor += 2 if text[cursor] == "\\" else 1
            literal = text[index + 1 : min(cursor, length)]
            code_lines.add(line)
            tokens.append(Token("string", literal, line, "\\(" in literal))
            index = cursor + 1 if cursor < length and text[cursor] == '"' else cursor
        elif char.isalpha() or char == "_":
            cursor = index + 1
            while cursor < length and (text[cursor].isalnum() or text[cursor] == "_"):
                cursor += 1
            code_lines.add(line)
            tokens.append(Token("name", text[index:cursor], line))
            index = cursor
        elif char.isdigit():
            cursor = index + 1
            while cursor < length and (text[cursor].isalnum() or text[cursor] == "_"):
                cursor += 1
            code_lines.add(line)
            tokens.append(Token("number", text[index:cursor], line))
            index = cursor
        else:
            width = 2 if text.startswith(TWO_CHARACTER_PUNCTUATION, index) else 1
            code_lines.add(line)
            tokens.append(Token("punct", text[index : index + width], line))
            index += width
    return tokens, code_lines, interior_lines


class Frame:
    __slots__ = ("closer", "match_body", "arm", "arms")

    def __init__(self, closer, match_body):
        self.closer = closer
        self.match_body = match_body
        self.arm = []
        self.arms = []


def structure(tokens):
    """Counts matches, nested matches, and collects match and let-else arms."""
    matches = 0
    nested = 0
    arms = []
    stack = []
    pending_matches = []
    # Open let-else arms: (stack depth, tokens collected so far).
    let_else = []
    for position, token in enumerate(tokens):
        for _, collected in let_else:
            collected.append(token)
        for frame in stack:
            if frame.match_body:
                frame.arm.append(token)
        text = token.text if token.kind == "punct" else None
        if token.kind == "name" and token.text == "match":
            matches += 1
            if any(frame.match_body for frame in stack):
                nested += 1
            pending_matches.append(len(stack))
        elif (
            token.kind == "name"
            and token.text == "else"
            and position + 1 < len(tokens)
            and tokens[position + 1].kind == "name"
            and tokens[position + 1].text in LET_ELSE_PATTERNS
        ):
            let_else.append((len(stack), []))
        elif text in OPENERS:
            # A match body is the first `{` at the depth of its `match`.
            is_match = text == "{" and bool(pending_matches) and pending_matches[-1] == len(stack)
            if is_match:
                pending_matches.pop()
            stack.append(Frame(OPENERS[text], is_match))
        elif text in CLOSERS:
            if stack and stack[-1].closer == text:
                frame = stack.pop()
                if frame.match_body:
                    # The closing brace was appended to the current arm.
                    finish = frame.arm[:-1]
                    if finish:
                        frame.arms.append(finish)
                    arms.extend(frame.arms)
            while pending_matches and pending_matches[-1] > len(stack):
                pending_matches.pop()
        elif text == "," and stack and stack[-1].match_body:
            frame = stack[-1]
            finish = frame.arm[:-1]
            if finish:
                frame.arms.append(finish)
            frame.arm = []
        elif text == ";":
            still_open = []
            for depth, collected in let_else:
                if depth == len(stack):
                    arms.append(collected[:-1])
                else:
                    still_open.append((depth, collected))
            let_else = still_open
    return matches, nested, arms


def authority_literals(tokens):
    count = 0
    for position, token in enumerate(tokens):
        if token.kind != "string" or token.interpolated or position < 1:
            continue
        before = tokens[position - 1]
        after = tokens[position + 1] if position + 1 < len(tokens) else None
        if (
            before.kind == "name"
            and before.text in ENTRYPOINT_KEYWORDS
            and after is not None
            and after.kind == "name"
            and after.text == "fn"
        ):
            count += 1
            continue
        if (
            position >= 2
            and before.kind == "punct"
            and before.text == "("
            and tokens[position - 2].kind == "name"
            and tokens[position - 2].text in AUTHORITY_BUILTINS
            and after is not None
            and after.kind == "punct"
            and after.text in (",", ")")
        ):
            count += 1
    return count


def arm_text(arm):
    return " ".join(
        ('"%s"' % token.text) if token.kind == "string" else token.text for token in arm
    )


def is_propagation(arm):
    words = [token.text for token in arm]
    return (
        len(words) == 9
        and words[0] == "Err"
        and words[1] == "("
        and words[3:6] == [")", "=>", "Err"]
        and words[6] == "("
        and words[7] == words[2]
        and words[8] == ")"
    )


def measure(path):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    tokens, code_lines, interior_lines = scan(text)

    depth = 0
    depth_line = 0
    for number in sorted(code_lines - interior_lines):
        if number > len(lines):
            continue
        source = lines[number - 1].expandtabs(INDENT_WIDTH)
        level = (len(source) - len(source.lstrip(" "))) // INDENT_WIDTH
        if level > depth:
            depth, depth_line = level, number

    longest = 0
    longest_line = 0
    for number, source in enumerate(lines, start=1):
        width = len(source.rstrip("\r"))
        if width > longest:
            longest, longest_line = width, number

    matches, nested, arms = structure(tokens)
    err_arms = [arm for arm in arms if arm and arm[0].kind == "name" and arm[0].text == "Err"]
    seen = set()
    duplicated = 0
    for arm in err_arms:
        key = arm_text(arm)
        if key in seen:
            duplicated += 1
        seen.add(key)

    return {
        "path": path,
        "lines": len(lines),
        "codeLines": len([number for number in code_lines if number <= len(lines)]),
        "maxDepth": depth,
        "maxDepthLine": depth_line,
        "matches": matches,
        "nestedMatches": nested,
        "errArms": len(err_arms),
        "duplicatedErrArms": duplicated,
        "propagationArms": len([arm for arm in err_arms if is_propagation(arm)]),
        "recordKeywords": len([t for t in tokens if t.kind == "name" and t.text == "record"]),
        "longestLine": longest,
        "longestLineNumber": longest_line,
        "authorityLiterals": authority_literals(tokens),
    }


COLUMNS = [
    ("lines", "lines"),
    ("code", "codeLines"),
    ("depth", "maxDepth"),
    ("nested", "nestedMatches"),
    ("dup-err", "duplicatedErrArms"),
    ("record", "recordKeywords"),
    ("longest", "longestLine"),
    ("authority", "authorityLiterals"),
]


def portable(path):
    """`path` with `/` separators, so a report reads the same on every platform."""
    for separator in (os.sep, os.altsep):
        if separator and separator != "/":
            path = path.replace(separator, "/")
    return path


def expand(paths):
    files = []
    for path in paths:
        if os.path.isdir(path):
            files.extend(
                sorted(
                    os.path.join(path, name)
                    for name in os.listdir(path)
                    if name.endswith(".krit") and os.path.isfile(os.path.join(path, name))
                )
            )
        else:
            files.append(path)
    return [portable(path) for path in files]


def render_table(reports):
    width = max([len("file")] + [len(report["path"]) for report in reports])
    header = "file".ljust(width) + "".join(
        "  " + title.rjust(max(len(title), 5)) for title, _ in COLUMNS
    )
    rows = [header]
    for report in reports:
        rows.append(
            report["path"].ljust(width)
            + "".join(
                "  " + str(report[key]).rjust(max(len(title), 5)) for title, key in COLUMNS
            )
        )
    return "\n".join(rows)


def main(argv):
    parser = argparse.ArgumentParser(
        prog="metrics.py",
        description="Report reviewability metrics for Krit source files.",
    )
    parser.add_argument("paths", nargs="+", metavar="PATH", help=".krit file or directory")
    parser.add_argument("--json", action="store_true", help="print one JSON document")
    parser.add_argument(
        "--check-depth",
        type=int,
        metavar="N",
        help="exit 1 when a file is indented deeper than N levels",
    )
    parser.add_argument(
        "--check-lines",
        type=int,
        metavar="N",
        help="exit 1 when a file has more than N lines",
    )
    arguments = parser.parse_args(argv)

    files = expand(arguments.paths)
    if not files:
        parser.error("no .krit files found")
    reports = []
    for path in files:
        try:
            reports.append(measure(path))
        except (OSError, UnicodeDecodeError) as error:
            print("metrics.py: %s: %s" % (path, error), file=sys.stderr)
            return 2

    if arguments.json:
        print(json.dumps({"schema": 1, "files": reports}, indent=2))
    else:
        print(render_table(reports))

    depth_limit = arguments.check_depth
    line_limit = arguments.check_lines
    violations = []
    for report in reports:
        if depth_limit is not None and report["maxDepth"] > depth_limit:
            violations.append(
                "%s:%d: indentation depth %d exceeds %d"
                % (report["path"], report["maxDepthLine"], report["maxDepth"], depth_limit)
            )
        if line_limit is not None and report["lines"] > line_limit:
            violations.append(
                "%s: %d lines exceed %d" % (report["path"], report["lines"], line_limit)
            )
    sys.stdout.flush()
    for violation in violations:
        print("metrics.py: " + violation, file=sys.stderr)
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
