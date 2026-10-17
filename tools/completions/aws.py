"""aws spec from the command tables of the installed aws cli (pip install awscli): every
service, its operations and their options. run inside the container, writes json lines to
stdout: first the "aws" spec, its services point with "loadSpec" at the specs of the
following lines, "aws/<service>", which kuterm only reads once that service is typed"""

import json
import re
import sys

from awscli.clidriver import create_clidriver

# descriptions are cut to their first sentence, the popup only shows one line
MAX = 80

# every operation takes these, they go on the aws spec once, for all of them
COMMON = {"cli-input-json", "cli-input-yaml", "generate-cli-skeleton"}


def first_sentence(doc) -> str | None:
    if not doc:
        return None
    text = re.sub(r"<[^>]+>", "", str(doc))
    text = re.sub(r"\s+", " ", text).strip()
    text = re.split(r"(?<=[.!?])\s", text, maxsplit=1)[0]
    if not text:
        return None
    return text if len(text) <= MAX else text[: MAX - 1].rstrip() + "…"


def describe(command) -> str | None:
    for attr in ("DESCRIPTION", "_description"):
        doc = getattr(command, attr, None)
        if doc:
            return first_sentence(doc)
    for attr in ("_operation_model", "service_model"):
        try:
            model = getattr(command, attr, None)
        except Exception:
            continue
        doc = first_sentence(getattr(model, "documentation", None))
        if doc:
            return doc
    return None


# more fixed values than this, like instance types, would bloat the spec
MAX_CHOICES = 20


def choices(arg) -> list:
    """fixed values an argument takes, from the cli or the service model's enum"""
    values = getattr(arg, "choices", None)
    if not values:
        model = getattr(arg, "argument_model", None)
        values = getattr(model, "enum", None)
    values = [str(v) for v in values or [] if str(v)]
    return values if 0 < len(values) <= MAX_CHOICES else []


def options(command, descriptions: bool, skip=COMMON) -> list:
    out = []
    # the driver keeps the global options behind a method, commands in a property
    try:
        table = command.arg_table if hasattr(command, "arg_table") else command._get_argument_table()
    except Exception:
        return out
    for name, arg in sorted(table.items()):
        if name in skip or getattr(arg, "_UNDOCUMENTED", False) or getattr(arg, "positional_arg", False):
            continue
        option = {"name": f"--{name}"}
        if descriptions:
            doc = first_sentence(getattr(arg, "documentation", None))
            if doc:
                option["description"] = doc
        if getattr(arg, "cli_type_name", None) != "boolean" and getattr(arg, "nargs", None) != 0:
            # the option's name already says what the value is
            option["args"] = {}
            if values := choices(arg):
                option["args"]["suggestions"] = values
        if getattr(arg, "required", False):
            option["priority"] = 75
        out.append(option)
    return out


def command_table(command) -> dict:
    """operations of a service, or commands of a customization like s3"""
    for get in (lambda: command._get_command_table(), lambda: command.subcommand_table):
        try:
            table = get()
        except Exception:
            continue
        if table:
            return table
    return {}


def subcommands(command, depth: int) -> list:
    table = command_table(command)
    subs = []
    for name, sub in sorted(table.items()):
        if name == "help":
            continue
        spec = {"name": name}
        doc = describe(sub)
        if doc:
            spec["description"] = doc
        # option descriptions of every operation would make the spec many megabytes
        opts = options(sub, descriptions=False)
        if opts:
            spec["options"] = opts
        if depth < 2:
            nested = subcommands(sub, depth + 1)
            if nested:
                spec["subcommands"] = nested
        subs.append(spec)
    return subs


def line(spec: dict) -> None:
    sys.stdout.write(json.dumps(spec, separators=(",", ":"), ensure_ascii=False) + "\n")


def main() -> None:
    driver = create_clidriver()
    table = driver._get_command_table()
    services, loaded = [], []
    for name, command in sorted(table.items()):
        service = {"name": name}
        if doc := describe(command):
            service["description"] = doc
        if subs := subcommands(command, 1):
            service["loadSpec"] = f"aws/{name}"
            loaded.append({"name": f"aws/{name}", "subcommands": subs})
        services.append(service)
    global_options = options(driver, descriptions=True, skip=())
    global_options += [
        {"name": "--cli-input-json", "description": "Read arguments from a json string or file://", "args": {}},
        {"name": "--cli-input-yaml", "description": "Read arguments from a yaml string or file://", "args": {}},
        {"name": "--generate-cli-skeleton", "description": "Print a skeleton of the input or output",
         "args": {"isOptional": True, "suggestions": ["input", "yaml-input", "output"]}},
    ]
    regions = sorted(driver.session.get_available_regions("ec2"))
    for option in global_options:
        option["isPersistent"] = True
        if option["name"] == "--region" and regions:
            option["args"] = {"name": "region", "suggestions": regions}
    line({
        "name": "aws",
        "description": "Amazon Web Services command line interface",
        "subcommands": services,
        "options": global_options,
    })
    for spec in loaded:
        line(spec)


if __name__ == "__main__":
    main()
