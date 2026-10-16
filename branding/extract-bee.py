"""Cut the bee out of bee-source.jpg into bee.png (transparent, soft edges).

usage: extract-bee.py [SRC] [OUT]
The bee is the connected blob of dark line work and cream wings around the
body, with every enclosed region filled. Its edge pixels are un-mixed from the
locally estimated background so no rust fringe is left behind.
"""
import os
import sys

import numpy as np
from PIL import Image
from scipy import ndimage as ndi

HERE = os.path.dirname(os.path.abspath(__file__))
src = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, 'bee-source.jpg')
out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, 'bee.png')

# the bee's box in the 2000px source, and a point inside its body
BOX = (600, 460, 1500, 1300)
SEED = (1100 - BOX[0], 900 - BOX[1])

px = np.asarray(Image.open(src).convert('RGB').crop(BOX)).astype(np.float32)
hi, lo = px.max(axis=2), px.min(axis=2)

lines = hi < 105   # outline, stripe, antennae, gear tips
wings = lo > 165   # cream wings (and the pollen trail, dropped below)
solid = ndi.binary_closing(lines | wings, iterations=2)
solid = ndi.binary_fill_holes(solid)
lab, _ = ndi.label(solid)
bee = lab == lab[SEED[1], SEED[0]]
bee = ndi.binary_fill_holes(bee)

# background colour everywhere, by normalized convolution of the pixels well
# outside the bee
outside = ~ndi.binary_dilation(bee, iterations=6)
w = ndi.gaussian_filter(outside.astype(np.float32), 12)
bg = np.stack([ndi.gaussian_filter(px[..., c] * outside, 12) for c in range(3)], -1) / np.maximum(w, 1e-4)[..., None]

# foreground colour at the edge: the nearest pixel well inside the bee
core = ndi.binary_erosion(bee, iterations=2)
_, (iy, ix) = ndi.distance_transform_edt(~core, return_indices=True)
fg = px[iy, ix]

# alpha by projecting each edge pixel onto the fg-bg line
band = ndi.binary_dilation(bee, iterations=2) & ~core
d = fg - bg
alpha = ((px - bg) * d).sum(-1) / np.maximum((d * d).sum(-1), 1.0)
alpha = np.where(band, alpha.clip(0, 1), core.astype(np.float32))
rgb = np.where(band[..., None], fg, px)

im = np.dstack([rgb, alpha * 255]).clip(0, 255).astype(np.uint8)
im = Image.fromarray(im, 'RGBA')
im.crop(im.getchannel('A').point(lambda v: 255 if v > 8 else 0).getbbox()).save(out)
