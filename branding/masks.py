"""masks.py MASTER OUT_PREFIX -> OUT_PREFIX-squircle.png, OUT_PREFIX-circle.png (1024, transparent)."""
import sys
import numpy as np
from PIL import Image

src, out = sys.argv[1], sys.argv[2]
im = Image.open(src).convert('RGBA').resize((1024, 1024), Image.LANCZOS)
SS = 4  # supersample masks for smooth edges
N = 1024 * SS
yy, xx = np.mgrid[0:N, 0:N].astype(np.float32)
u, v = (xx + 0.5) / N * 2 - 1, (yy + 0.5) / N * 2 - 1
for name, m in [('squircle', (np.abs(u) ** 5 + np.abs(v) ** 5) <= 1), ('circle', u * u + v * v <= 1)]:
    mask = Image.fromarray((m * 255).astype(np.uint8)).resize((1024, 1024), Image.LANCZOS)
    o = im.copy()
    o.putalpha(mask)
    o.save(f'{out}-{name}.png')
