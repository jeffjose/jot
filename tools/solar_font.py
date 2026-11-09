"""Builds jot's icon font from Solar's linear icons, drawn at a heavier
line than Solar's own, as jog's is (this is jog's tools/solar_font.py, with
jot's icons).

    uv run --no-project --with fonttools --with picosvg \
        tools/solar_font.py ICONS_JSON tools/solar_icons.txt assets/solar.ttf [STROKE]

ICONS_JSON is the set as Iconify publishes it:
https://cdn.jsdelivr.net/npm/@iconify-json/solar/icons.json

tools/solar_icons.txt has a line for each icon jot uses: the name of its
constant in `canvas::icons`, and the code point the font has it at, in
hex. To add an icon, give it a line there with a code point of its own
and say in PICKS below which of Solar's drawings it is, or draw it in OWN.

The line is 2.25 wide where Solar draws it at 1.5: beside text at 13 to
24 pixels the thinner line is lighter than the letters.
"""

import json
import re
import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path
from picosvg.svg import SVG

# jot's name for an icon, and the Solar drawing that stands for it.
PICKS = {
    "FOCUS": "full-screen", "UNFOCUS": "quit-full-screen", "RESTART": "restart",
    "PLAY": "play", "CLOSE": "close",
    "MINUS": "minus", "CHEVRON_DOWN": "alt-arrow-down", "ARROW_RIGHT": "arrow-right",
    "SIDEBAR": "panel-left", "PLUS": "add", "MONITOR": "monitor", "SERVER": "server-minimalistic",
    "CHEVRON_RIGHT": "alt-arrow-right", "SETTINGS": "settings",
    "LAYERS": "layers-minimalistic", "CHEVRON_UP": "alt-arrow-up",
    "WIN_MAX": "stop", "WIN_RESTORE": "copy",
    "CHECK": "check", "INFO": "info-circle", "DANGER": "danger-circle",
    # Projects' marks: one picked by its name, as its color is.
    "PROJ_PEN": "pen", "PROJ_MOON": "moon", "PROJ_SERVER": "server", "PROJ_CODE": "code",
    "PROJ_PLAY": "play", "PROJ_GALLERY": "gallery", "PROJ_LEAF": "leaf", "PROJ_BOLT": "bolt",
    "PROJ_BOOK": "book", "PROJ_GLOBE": "global", "PROJ_BOX": "box", "PROJ_STAR": "star",
    "PROJ_HEART": "heart", "PROJ_CUP": "cup-hot", "PROJ_ROCKET": "rocket", "PROJ_PALETTE": "palette",
    # The same marks, duotone, in two layers each (see duotone): on tiles.
    "DUO_PEN_BACK": "pen@back", "DUO_PEN_FRONT": "pen@front",
    "DUO_MOON_BACK": "moon@back", "DUO_MOON_FRONT": "moon@front",
    "DUO_SERVER_BACK": "server@back", "DUO_SERVER_FRONT": "server@front",
    "DUO_CODE_BACK": "code@back", "DUO_CODE_FRONT": "code@front",
    "DUO_PLAY_BACK": "play@back", "DUO_PLAY_FRONT": "play@front",
    "DUO_GALLERY_BACK": "gallery@back", "DUO_GALLERY_FRONT": "gallery@front",
    "DUO_LEAF_BACK": "leaf@back", "DUO_LEAF_FRONT": "leaf@front",
    "DUO_BOLT_BACK": "bolt@back", "DUO_BOLT_FRONT": "bolt@front",
    "DUO_BOOK_BACK": "book@back", "DUO_BOOK_FRONT": "book@front",
    "DUO_GLOBAL_BACK": "global@back", "DUO_GLOBAL_FRONT": "global@front",
    "DUO_BOX_BACK": "box@back", "DUO_BOX_FRONT": "box@front",
    "DUO_STAR_BACK": "star@back", "DUO_STAR_FRONT": "star@front",
    "DUO_HEART_BACK": "heart@back", "DUO_HEART_FRONT": "heart@front",
    "DUO_CUP_HOT_BACK": "cup-hot@back", "DUO_CUP_HOT_FRONT": "cup-hot@front",
    "DUO_ROCKET_BACK": "rocket@back", "DUO_ROCKET_FRONT": "rocket@front",
    "DUO_PALETTE_BACK": "palette@back", "DUO_PALETTE_FRONT": "palette@front",
}

LINE = 'fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"'

# Drawings of jot's own, on Solar's 24 by 24 square and in its manner, for
# what Solar has no icon of. None yet.
OWN = {}

EM = 1000
GRID = 24


def outline(body, stroke):
    """The filled outline of a drawing made of lines `stroke` wide, as a
    glyph: Solar's 24 by 24 square taken to the font's em, the right way up."""
    body = re.sub(r'stroke-width="[0-9.]+"', f'stroke-width="{stroke}"', body).replace("currentColor", "#000")
    svg = SVG.fromstring(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {GRID} {GRID}">{body}</svg>')
    pen = TTGlyphPen(None)
    scale = EM / GRID
    to_em = TransformPen(Cu2QuPen(pen, max_err=1.0, reverse_direction=False), (scale, 0, 0, -scale, 0, EM))
    for shape in svg.topicosvg().shapes():
        parse_path(shape.as_path().d, to_em)
    return pen.glyph()


def duotone(body, part):
    """One of a duotone drawing's two layers, as its own drawing: "back",
    the parts Solar draws at half strength, or "front", the rest. jot draws
    the back faintly and the front over it, a font holding one color a
    glyph."""
    import xml.etree.ElementTree as ET

    root = ET.fromstring(f"<svg>{body}</svg>")

    def keep(el, back):
        back = back or el.get("opacity") not in (None, "1")
        kids = list(el)
        if not kids:
            return back == (part == "back")
        for kid in kids:
            if not keep(kid, back):
                el.remove(kid)
        return len(el) > 0

    for kid in list(root):
        if not keep(kid, False):
            root.remove(kid)
    for el in root.iter():
        el.attrib.pop("opacity", None)
    return "".join(ET.tostring(kid, encoding="unicode") for kid in root)


def main():
    icons_json, consts, out = sys.argv[1:4]
    stroke = sys.argv[4] if len(sys.argv) > 4 else "2.25"
    solar = json.load(open(icons_json))["icons"]

    glyphs, cmap, missing = {".notdef": TTGlyphPen(None).glyph()}, {}, []
    for line in open(consts):
        name, point = line.split()
        pick = PICKS.get(name, "")
        # "name@back" or "name@front": one layer of a bold-duotone drawing.
        pick, _, part = pick.partition("@")
        if part:
            drawing = (solar.get(pick + "-bold-duotone") or {}).get("body")
            drawing = drawing and duotone(drawing, part)
        else:
            drawing = OWN.get(name) or (solar.get(pick if pick.endswith("-bold") else pick + "-linear") or {}).get("body")
        if not drawing:
            missing.append(f"{name} ({PICKS.get(name, 'no pick')})")
            continue
        glyphs[name] = outline(drawing, stroke)
        cmap[int(point, 16)] = name
    if missing:
        sys.exit(f"Solar has no drawing for: {', '.join(missing)}")

    font = FontBuilder(EM, isTTF=True)
    font.setupGlyphOrder(list(glyphs))
    font.setupCharacterMap(cmap)
    font.setupGlyf(glyphs)
    # Each glyph's left side bearing is where its outline starts, as
    # TrueType has it: with 0 for all, a renderer that places glyphs by it
    # moved every outline to the left edge, a duotone's two halves (see
    # duotone) onto each other.
    def lsb(g):
        return min((x for x, _ in g.coordinates), default=0) if g.numberOfContours > 0 else 0

    font.setupHorizontalMetrics({name: (EM, lsb(g)) for name, g in glyphs.items()})
    font.setupHorizontalHeader(ascent=EM, descent=0)
    font.setupNameTable({"familyName": "jot icons", "styleName": "Regular"})
    font.setupOS2(sTypoAscender=EM, sTypoDescender=0, usWinAscent=EM, usWinDescent=0)
    font.setupPost()
    font.save(out)
    print(f"{len(glyphs) - 1} icons")


if __name__ == "__main__":
    main()
