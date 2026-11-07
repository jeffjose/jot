"""Makes jot's icon: Solar's prompt in a square (`>_`), from its bold
duotone set, the square in tomato and the prompt in white, as clutch's
and hotshot's are made, so that it reads on a dark panel and a light one.

    python3 tools/app_icon.py ICONS_JSON assets/jot.svg
    convert -background none -density 1024 assets/jot.svg \
        -resize 256x256 assets/jot.png

ICONS_JSON is the set as Iconify publishes it:
https://cdn.jsdelivr.net/npm/@iconify-json/solar/icons.json

The PNG is built into jot, for its windows; the SVG is what
`jot setup` puts in ~/.local/share/icons/hicolor/scalable/apps/.
"""

import json
import re
import sys

# Radix's tomato: jot's own color.
BODY = "#E54D2E"
MARKS = "#FFFFFF"


def main(icons_json, out):
    with open(icons_json) as file:
        body = json.load(file)["icons"]["programming-bold-duotone"]["body"]
    drawn = []
    for path in re.findall(r"<path[^>]*/>", body):
        # The tone Solar draws at half strength is the square; the rest
        # is the prompt on it.
        faint = 'opacity=".5"' in path
        path = path.replace(' opacity=".5"', "")
        drawn.append(path.replace("currentColor", BODY if faint else MARKS))
    # A little room round it, so that it is not hard against the edge of
    # whatever shows it.
    svg = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="1 1 22 22">{"".join(drawn)}</svg>\n'
    with open(out, "w") as file:
        file.write(svg)
    print(f"{len(drawn)} paths")


if __name__ == "__main__":
    main(*sys.argv[1:3])
