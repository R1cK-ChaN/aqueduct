"""Check the commands, section count and link targets of README translations."""

import re
import sys
from dataclasses import dataclass
from pathlib import Path


FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})([^\r\n]*)")
HEADING = re.compile(r"^ {0,3}##(?:[ \t]|\r?$)")
CODE_SPAN = re.compile(r"(?<!`)(`+)(?!`).*?(?<!`)\1(?!`)")
# Start at the destination so that both links in a clickable badge are read.
LINK = re.compile(
    r"""
    (?: \]\( | ^[ ]{0,3}\[[^\]]+\]: )[ \t]*
    ( <[^>\r\n]*> | (?: \\. | [^()\\\s] | \([^()\s]*\) )+ | (?=\)) )
    | <(https?://[^<>\s]+)>
    """,
    re.VERBOSE,
)


@dataclass
class Readme:
    bash_blocks: list[str]
    headings: int
    targets: set[str]


def read_readme(path, language_files):
    bash_blocks = []
    headings = 0
    targets = set()
    # Preserve line endings when comparing commands.
    with path.open(encoding="utf-8", newline="") as lines:
        for line in lines:
            opening = FENCE.match(line)
            if opening:
                block = [line]
                for body_line in lines:
                    block.append(body_line)
                    closing = FENCE.match(body_line)
                    if closing and closing[1].startswith(opening[1]) and not closing[2].strip():
                        break
                if opening[2].split()[:1] == ["bash"]:
                    bash_blocks.append("".join(block))
                continue

            if HEADING.match(line):
                headings += 1
            for link in LINK.finditer(CODE_SPAN.sub("", line)):
                target = (link[2] or link[1]).strip("<>")
                local_target = target.removeprefix("./")
                if local_target.startswith(("#", path.name + "#")):
                    continue
                if headings == 0 and local_target in language_files:
                    continue
                targets.add(target)
    return Readme(bash_blocks, headings, targets)


def main():
    root = Path(__file__).resolve().parents[1]
    source = root / "README.md"
    translations = sorted(root.glob("README.*.md"))
    language_files = {path.name for path in [source, *translations]}
    expected = read_readme(source, language_files)
    errors = []
    for path in translations:
        actual = read_readme(path, language_files)
        if actual.bash_blocks != expected.bash_blocks:
            errors.append(f"{path.name}: fenced bash blocks differ from README.md")
        if actual.headings != expected.headings:
            errors.append(
                f"{path.name}: expected {expected.headings} ## headings, found {actual.headings}"
            )
        for target in sorted(expected.targets - actual.targets):
            errors.append(f"{path.name}: missing link target {target}")
        for target in sorted(actual.targets - expected.targets):
            errors.append(f"{path.name}: unexpected link target {target}")
    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
