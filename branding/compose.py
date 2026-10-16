"""Compose rustybee icon layers from bee.png.

usage: compose.py OUT [--mode full|bg|fg|mono] [--width FRAC] [--hex-scale K] [--size PX]
  full  gradient + honeycomb + bee (the master)
  bg    gradient + honeycomb only (adaptive icon background)
  fg    bee only on transparent (adaptive icon foreground)
  mono  white bee silhouette on transparent (notification / themed icon)
"""
import argparse
import math
import os

import numpy as np
from PIL import Image, ImageDraw, ImageFilter
from scipy import ndimage as ndi

HERE = os.path.dirname(os.path.abspath(__file__))

p = argparse.ArgumentParser()
p.add_argument('out')
p.add_argument('--mode', default='full', choices=['full', 'bg', 'fg', 'mono'])
p.add_argument('--width', type=float, default=0.62, help='bee width as a fraction of the canvas')
p.add_argument('--hex-scale', type=float, default=1.0)
p.add_argument('--size', type=int, default=1024)
a = p.parse_args()
S = a.size


def background():
    yy, xx = np.mgrid[0:S, 0:S].astype(np.float32)
    d = np.hypot(xx - S / 2, yy - S / 2) / (S / 2 * math.sqrt(2))
    inner, outer = np.array([196, 98, 46]), np.array([160, 62, 32])
    t = np.clip(d, 0, 1)[..., None] ** 1.1
    bg = inner * (1 - t) + outer * t

    # flat-top hex grid centered on the canvas, fading toward the edges
    r = S * 0.075 * a.hex_scale
    w, h = 2 * r, math.sqrt(3) * r
    n = int(S / h) + 2
    lines = Image.new('L', (S, S), 0)
    dr = ImageDraw.Draw(lines)
    for col in range(-n, n + 1):
        for row in range(-n, n + 1):
            cx = S / 2 + col * 0.75 * w
            cy = S / 2 + row * h + (h / 2 if col % 2 else 0)
            pts = [(cx + r * 0.94 * math.cos(math.radians(60 * k)), cy + r * 0.94 * math.sin(math.radians(60 * k))) for k in range(7)]
            dr.line(pts, fill=255, width=max(2, round(S / 160 * a.hex_scale)))
    lines = np.asarray(lines.filter(ImageFilter.GaussianBlur(S / 900)), np.float32) / 255
    fade = np.clip(1 - d * 1.15, 0, 1) ** 1.4
    bg = bg + (np.array([130, 46, 20]) - bg) * (lines * fade * 0.45)[..., None]
    return Image.fromarray(bg.clip(0, 255).astype(np.uint8)).convert('RGBA')


def bee_layer():
    bee = Image.open(os.path.join(HERE, 'bee.png')).convert('RGBA')
    bee = bee.crop(bee.getchannel('A').point(lambda v: 255 if v > 8 else 0).getbbox())
    scale = S * a.width / bee.width
    bee = bee.resize((round(bee.width * scale), round(bee.height * scale)), Image.LANCZOS)
    layer = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ox, oy = (S - bee.width) // 2, (S - bee.height) // 2
    shadow = Image.new('RGBA', bee.size, (150, 70, 10, 0))
    shadow.putalpha(bee.getchannel('A').point(lambda v: v * 0.28))
    shadow = shadow.filter(ImageFilter.GaussianBlur(S / 110))
    layer.alpha_composite(shadow, (ox, oy + S // 90))
    layer.alpha_composite(bee, (ox, oy))
    return layer, (ox, oy, bee)


def mono():
    _, (ox, oy, bee) = bee_layer()
    px = np.asarray(bee).astype(np.float32)
    solid = px[..., 3] > 128
    # whole silhouette, with the dark line work inside it punched back out so the
    # face, dots, stripe and wings read as cut-outs
    body = ndi.binary_fill_holes(solid)
    hi, lo = px[..., :3].max(axis=2), px[..., :3].min(axis=2)
    k = bee.width / 754  # sizes below are tuned on the 754px wide bee.png
    lab, n = ndi.label(ndi.binary_opening(hi < 170, iterations=max(1, round(9 * k))))
    area = ndi.sum(np.ones_like(hi), lab, range(1, n + 1))
    # the three big body dots; the opening drops the rivets, the size cap the belly shading
    dots = np.isin(lab, 1 + np.flatnonzero(area < 3000 * k * k))
    # keep the outer outline solid so the silhouette edge stays one clean line
    inner = ndi.binary_erosion(body, iterations=max(1, round(bee.width * 0.065)))
    dark = ndi.binary_closing((hi < 110) & solid, iterations=max(1, round(6 * k))) & solid
    lines = dark & inner
    # drop the antenna stubs poking in from the outline; the eyes and smile stay
    lab, n = ndi.label(lines)
    area = ndi.sum(np.ones_like(hi), lab, range(1, n + 1))
    edge = np.unique(lab[ndi.binary_dilation(~inner, iterations=2) & (lab > 0)])
    lines &= ~np.isin(lab, [i for i in edge if area[i - 1] < 1500 * k * k])
    # plus the outline between the wings and the body, which sits near the edge
    wings = (lo > 150) & solid
    lines |= dark & ndi.binary_dilation(wings, iterations=max(1, round(30 * k)))
    # a gap between the front and back wing so they read as two wings
    front = ndi.binary_opening(lo > 200, iterations=max(1, round(3 * k)))
    front = ndi.binary_fill_holes(ndi.binary_closing(front, iterations=max(1, round(10 * k))))
    back = wings & (lo <= 200) & ~front
    gap = ndi.binary_dilation(front, iterations=max(1, round(16 * k))) & back
    inner_dark = lines | (dots & inner) | gap
    shape = body & ~inner_dark
    shape = ndi.binary_opening(shape, iterations=max(1, round(S / 400)))
    alpha = ndi.gaussian_filter(shape.astype(np.float32), 0.6) * 255
    sil = np.zeros((*shape.shape, 4), np.uint8)
    sil[..., :3] = 255
    sil[..., 3] = alpha.clip(0, 255).astype(np.uint8)
    layer = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    layer.alpha_composite(Image.fromarray(sil), (ox, oy))
    return layer


if a.mode == 'bg':
    out = background().convert('RGB')
elif a.mode == 'fg':
    out = bee_layer()[0]
elif a.mode == 'mono':
    out = mono()
else:
    out = background()
    out.alpha_composite(bee_layer()[0])
    out = out.convert('RGB')
out.save(a.out)
