"""Composes square logo tiles for the trailer's agents from the lobehub icon SVGs (MIT, @lobehub/icons-static-svg 1.97.0).

Each tile is the maker's mark on a flat ground, 24 units square, so the app's avatar box can round its corners.
usage: python3 -I tiles.py raw-dir out-dir
"""
import re
import sys
from pathlib import Path

raw, out = Path(sys.argv[1]), Path(sys.argv[2])

# name: (source icon, ground, mark color for one-color icons or None, scale, dx, dy, drop the icon's own first path)
TILES = {
    'claude': ('claude-color', '#F1ECE2', None, 0.7, 0, 0, False),
    'codex': ('codex-color', '#FFFFFF', None, 0.86, 0, 0, True),
    'chatgpt': ('openai', '#10A37F', '#FFFFFF', 0.64, 0, 0, False),
    'grok': ('grok', '#000000', '#FFFFFF', 0.62, 0, 0, False),
    'muse': ('meta-color', '#FFFFFF', None, 0.74, 0, 0, False),
    'hermes': ('hermesagent', '#FFFFFF', '#000000', 1.0, 0, 1.2, False),
    'openclaw': ('openclaw-color', '#0E1424', None, 0.74, 0, 0.3, False),
}

for name, (source, ground, color, scale, dx, dy, drop_first) in TILES.items():
    text = (raw / f'{source}.svg').read_text()
    root = re.match(r'<svg([^>]*)>', text)
    attrs = root.group(1)
    inner = text[root.end():text.rindex('</svg>')]
    inner = re.sub(r'<title>.*?</title>', '', inner)
    if drop_first:
        inner = re.sub(r'^<path[^>]*></path>', '', inner, count=1)
    evenodd = 'fill-rule="evenodd"' in attrs
    offset = 12 - 12 * scale
    group = [f'transform="translate({offset + dx:.3f} {offset + dy:.3f}) scale({scale})"']
    if color:
        group.append(f'fill="{color}"')
    if evenodd:
        group.append('fill-rule="evenodd"')
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="96" height="96">'
        f'<rect width="24" height="24" fill="{ground}"/>'
        f'<g {" ".join(group)}>{inner}</g></svg>'
    )
    (out / f'{name}.svg').write_text(svg)
    print(name, len(svg))
