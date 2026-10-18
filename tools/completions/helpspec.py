"""turn the --help of an installed program into a fig style completion spec.

run inside a container that has the programs installed, never on the host: a recipe runs the
programs it describes. see docs/COMPLETIONS.md. python standard library only"""

import os
import re
import subprocess
import tempfile

# the popup shows one line, longer descriptions end in an ellipsis
MAX_DESCRIPTION = 80

# an option name, then maybe its argument: --file=FILE, --color[=WHEN], -o <file>, -f FILE
OPTION_NAME = re.compile(
    r"(?<![\w-])(--?[A-Za-z0-9?#][\w?#.+-]*)"
    r"(\[=[^\]\s]+\]|=[^\s,|]+|\[[^\]\s]+\]| <[^>]+>(?:\.\.\.)?| [A-Z][A-Z0-9_:.-]*(?:\.\.\.)?(?=[\s,|]|$))?"
)
# argument names that mean files or folders
FILE_ARGS = re.compile(r"^(file|filename|path|files|archive|config|configfile|log|logfile|.*_file|.*file)$", re.I)
DIR_ARGS = re.compile(r"^(dir|directory|folder|dirs|.*dir|.*directory)$", re.I)

# a command line of a commands section: "  install   Install packages", "  start UNIT...   Start",
# "  update - Retrieve new lists of packages"
# "  repos, lr   List repositories" gives aliases
SUBCOMMAND = re.compile(
    r"^\s{1,8}([a-z][a-z0-9._-]*(?:, ?[a-z?][a-z0-9._-]*)*)(?:\s+(?:[^\s].*?))?(?:\s{2,}|\s+-\s+)(\S.*)$"
)


def run(*cmd: str) -> str:
    """output of a command, stdout and stderr together, in plain english and a known width.
    written to files: node programs drop what is still buffered in a pipe when they exit"""
    env = dict(os.environ, LANG="C", LC_ALL="C", COLUMNS="120", PAGER="cat", MANPAGER="cat", NO_COLOR="1")
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        try:
            subprocess.run(cmd, stdout=out, stderr=err, timeout=30, env=env, stdin=subprocess.DEVNULL)
        except (OSError, subprocess.TimeoutExpired):
            return ""
        out.seek(0)
        err.seek(0)
        stdout = out.read().decode(errors="replace")
        stderr = err.read().decode(errors="replace")
    return stdout + "\n" + stderr


def column(prefix: str) -> int:
    """display column after `prefix`, with tabs at the usual 8 column stops"""
    return len(prefix.expandtabs(8))


def sentence(text: str | None) -> str | None:
    """first sentence of `text`, capitalized and short enough for one line"""
    if not text:
        return None
    text = re.sub(r"\s+", " ", text).strip().strip("-:; ")
    # crontab writes "(edit user's crontab)"
    if text.startswith("(") and text.endswith(")"):
        text = text[1:-1]
    # a sentence ends at ! or ?, or at a period followed by a capitalized word, so that
    # "e.g." and the like don't cut the description
    text = re.split(r"(?<=[!?])\s|(?<=\.)\s+(?=[A-Z][a-z])|;\s", text, maxsplit=1)[0].rstrip(".,;: ")
    # dpkg puts the arguments of its commands where the description goes
    if not text or text.startswith("<"):
        return None
    text = text[0].upper() + text[1:]
    return text if len(text) <= MAX_DESCRIPTION else text[: MAX_DESCRIPTION - 1].rstrip() + "…"


def arg_spec(raw: str) -> dict:
    """the argument of an option from how help writes it, like =FILE or <dir>"""
    optional = raw.startswith("[")
    words = re.match(r"[\w-]+", raw.strip("[]= .<>").lower())
    name = words.group(0).strip("-_") if words else ""
    name = name or "value"
    arg: dict = {"name": name}
    if DIR_ARGS.match(name):
        arg["template"] = "folders"
    elif FILE_ARGS.match(name):
        arg["template"] = "filepaths"
    if optional:
        arg["isOptional"] = True
    return arg


def split_columns(text: str) -> str:
    """help that lists two options per line, like zip's, as one option per line"""
    return re.sub(r"(?m)^(\s+-\S.*?\S)\s{2,}(-\S)", r"\1\n  \2", text)


def options(text: str) -> list:
    """every option line of a help text, in order, without repeats"""
    found: list = []
    seen: set = set()
    lines = text.splitlines()
    for ix, line in enumerate(lines):
        stripped = line.lstrip()
        if not stripped.startswith("-") or len(line) - len(stripped) > 12:
            continue
        # the names run until two spaces or a tab, the description follows
        parts = re.split(r"(\s{2,}|\t)", stripped, maxsplit=1)
        head = parts[0].strip()
        description = parts[2].strip() if len(parts) == 3 else ""
        # commander and newer clap wrap a long description onto the lines below, aligned
        # with the description column; a new option starts in the name column
        indent = column(line[: len(line) - len(stripped)])
        desc_col = column(line[: len(line) - len(stripped)] + parts[0] + parts[1]) if len(parts) == 3 else None
        jx = ix + 1
        while jx < len(lines):
            following = lines[jx].strip()
            following_indent = column(lines[jx][: len(lines[jx]) - len(lines[jx].lstrip())])
            continues = following_indent >= desc_col if desc_col is not None else following_indent > indent
            header = following.endswith(":") and following_indent <= 4
            if not following or not continues or header:
                break
            # with the description on its own line, the first one sets the column, and an
            # option right below means this one has no description
            if desc_col is None:
                if following.startswith("-"):
                    break
                desc_col = following_indent
            description = f"{description} {following}".strip()
            jx += 1
        names, args = [], []
        for match in OPTION_NAME.finditer(head):
            names.append(match.group(1))
            if match.group(2):
                args.append(match.group(2))
        names = [n for n in names if n not in seen and n not in ("-", "--")]
        if not names:
            continue
        seen.update(names)
        option: dict = {"name": names if len(names) > 1 else names[0]}
        if desc := sentence(description):
            option["description"] = desc
        if args:
            option["args"] = arg_spec(args[0].strip())
        found.append(option)
    return found


def subcommands(text: str, sections: str = r"commands", skip: tuple = ()) -> list:
    """lines like "  name   description" under headers matching `sections`"""
    found: list = []
    seen: set = set(skip)
    inside = False
    header = re.compile(sections, re.I)
    lines = text.splitlines()
    for ix, line in enumerate(lines):
        stripped = line.strip()
        # help sometimes puts commented examples between the entries of a section
        if stripped.endswith(":") and not stripped.startswith("#") and len(line) - len(line.lstrip()) <= 4:
            inside = bool(header.search(stripped))
            continue
        if not inside:
            continue
        match = SUBCOMMAND.match(line)
        if not match:
            continue
        names = [n for n in re.split(r", ?", match.group(1)) if re.fullmatch(r"[a-z0-9._-]+", n)]
        if not names or names[0] in seen:
            continue
        seen.update(names)
        # a long description wraps to the lines below its column
        desc_col = column(line[: match.start(2)])
        description = match.group(2)
        jx = ix + 1
        while jx < len(lines):
            following = lines[jx].strip()
            following_indent = column(lines[jx][: len(lines[jx]) - len(lines[jx].lstrip())])
            if not following or following_indent < desc_col or following.endswith(":"):
                break
            description = f"{description} {following}"
            jx += 1
        sub: dict = {"name": names if len(names) > 1 else names[0]}
        if desc := sentence(description):
            sub["description"] = desc
        found.append(sub)
    return found


def merge(items: list, overrides: dict) -> list:
    """overrides by name: a dict is merged into the item, None drops it"""
    out = []
    for item in items:
        names = item["name"] if isinstance(item["name"], list) else [item["name"]]
        key = next((n for n in names if n in overrides), None)
        if key is None:
            out.append(item)
        elif overrides[key] is not None:
            out.append({**item, **overrides[key]})
    return out


def spec(
    name: str,
    description: str,
    *,
    help: tuple = ("--help",),
    sections: str | None = None,
    sub_help: tuple | None = None,
    top: tuple = (),
    args: dict | list | None = None,
    sub_args: dict | None = None,
    option_overrides: dict | None = None,
    sub_overrides: dict | None = None,
    extra_subcommands: list = (),
    extra_options: list = (),
    persistent: bool = False,
    columns: bool = False,
) -> dict:
    """a spec for `name` from its help.

    help: arguments that print the help. sections: regex of the headers commands are listed
    under, none for a command without subcommands. sub_help: arguments that print the help of
    a subcommand, "{}" stands for its name. top: subcommands listed first. args: what the
    command takes, sub_args: what each subcommand takes, by name"""
    text = run(name, *help)
    if columns:
        text = split_columns(text)
    if not text.strip():
        raise SystemExit(f"{name} printed no help, is it installed?")
    result: dict = {"name": name, "description": description}
    if sections:
        subs = subcommands(text, sections) + list(extra_subcommands)
        subs = merge(subs, sub_overrides or {})
        for sub in subs:
            first = sub["name"][0] if isinstance(sub["name"], list) else sub["name"]
            if first in top:
                sub["priority"] = 76 - top.index(first) // 4
            if sub_help:
                sub_text = run(name, *(part.replace("{}", first) for part in sub_help))
                if sub_opts := options(sub_text):
                    sub["options"] = merge(sub_opts, option_overrides or {})
            if sub_args and first in sub_args:
                sub["args"] = sub_args[first]
        if subs:
            result["subcommands"] = subs
    opts = merge(options(text) + list(extra_options), option_overrides or {})
    if persistent:
        opts = [{**o, "isPersistent": True} for o in opts]
    if opts:
        result["options"] = opts
    if args is not None:
        result["args"] = args
    return result
