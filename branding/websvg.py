"""websvg.py IN.svg OUT.svg: flatten bee.svg for the web UI.

Bakes every transform into the coordinates and rounds them to whole viewBox
units, so the logo stays a crisp vector while gzipping small enough for the
embedded asset budget (AC-010-05).
"""
import re
import sys
import xml.etree.ElementTree as ET

NS = 'http://www.w3.org/2000/svg'
ET.register_namespace('', NS)


def parse_transform(s):
    m = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)  # a b c d e f
    for name, args in re.findall(r'(\w+)\s*\(([^)]*)\)', s or ''):
        v = [float(x) for x in re.split(r'[\s,]+', args.strip())]
        if name == 'translate':
            t = (1, 0, 0, 1, v[0], v[1] if len(v) > 1 else 0)
        elif name == 'scale':
            t = (v[0], 0, 0, v[1] if len(v) > 1 else v[0], 0, 0)
        else:
            raise ValueError(name)
        m = mul(m, t)
    return m


def mul(p, q):
    a, b, c, d, e, f = p
    A, B, C, D, E, F = q
    return (a * A + c * B, b * A + d * B, a * C + c * D, b * C + d * D, a * E + c * F + e, b * E + d * F + f)


def apply(m, x, y):
    return m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]


def path_abs(d):
    """Yield (cmd, [points]) with absolute coordinates; cmd in M L C Z."""
    toks = re.findall(r'[MmLlCcZz]|-?\d*\.?\d+(?:e-?\d+)?', d)
    i, cmd, cur, start = 0, None, (0.0, 0.0), (0.0, 0.0)
    while i < len(toks):
        if toks[i].isalpha():
            cmd = toks[i]
            i += 1
            if cmd in 'Zz':
                cur = start
                yield 'Z', []
                continue
        n = {'M': 1, 'L': 1, 'C': 3}[cmd.upper()]
        pts = []
        base = cur
        for _ in range(n):
            x, y = float(toks[i]), float(toks[i + 1])
            i += 2
            pts.append((base[0] + x, base[1] + y) if cmd.islower() else (x, y))
        cur = pts[-1]
        if cmd in 'Mm':
            start = cur
            yield 'M', pts
            cmd = 'l' if cmd == 'm' else 'L'  # implicit lineto after a moveto
        else:
            yield cmd.upper(), pts


def fmt(v):
    return str(int(v))


def nums(vals):
    out = ''
    for v in vals:
        s = fmt(v)
        out += s if (out == '' or s.startswith('-')) else ' ' + s
    return out


def flatten_path(d, m):
    out, cur, last = [], (0, 0), None
    for cmd, pts in path_abs(d):
        if cmd == 'Z':
            out.append('z')
            last = None
            continue
        r = [tuple(round(c) for c in apply(m, *p)) for p in pts]
        if cmd == 'M':
            out.append('M' + nums(r[0]))
            last = None
        else:
            rel = [(x - cur[0], y - cur[1]) for x, y in r]
            if cmd == 'L' and rel[0] == (0, 0):
                continue
            letter = cmd.lower()
            body = nums([c for p in rel for c in p])
            out.append(body if letter == last and body.startswith('-') else (' ' + body if letter == last else letter + body))
            last = letter
        cur = r[-1]
    return ''.join(out)


def walk(el, m):
    m = mul(m, parse_transform(el.attrib.pop('transform', None)))
    tag = el.tag.split('}')[-1]
    scale = abs(m[0]) or abs(m[1])
    if 'stroke-width' in el.attrib:
        el.set('stroke-width', f"{float(el.get('stroke-width')) * scale:.3g}")
    if tag == 'path':
        el.set('d', flatten_path(el.get('d'), m))
    elif tag == 'circle':
        cx, cy = apply(m, float(el.get('cx', 0)), float(el.get('cy', 0)))
        el.set('cx', f'{cx:.0f}')
        el.set('cy', f'{cy:.0f}')
        el.set('r', f"{float(el.get('r')) * scale:.1f}")
    el.attrib.pop('id', None) if tag != 'clipPath' else None
    for ch in el:
        walk(ch, m)


src, out = sys.argv[1], sys.argv[2]
root = ET.parse(src).getroot()
for k in ('width', 'height'):
    root.attrib.pop(k, None)
for ch in root:
    walk(ch, (1, 0, 0, 1, 0, 0))
s = ET.tostring(root, encoding='unicode')
s = re.sub(r'>\s+<', '><', s)
open(out, 'w').write(s + '\n')
