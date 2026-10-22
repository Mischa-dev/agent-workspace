# Shows where a pop happens: python3 -I popframes.py video.mp4 out-dir t1 t2 ...
# For each time, saves the frame before, the frame at, and a heat map of what changed.
import subprocess
import sys

import numpy as np
from PIL import Image

video, out, *stamps = sys.argv[1:]
fps = 60
for stamp in stamps:
    frame = round(float(stamp) * fps)
    pics = []
    for f in (frame - 1, frame):
        raw = subprocess.run(
            ['ffmpeg', '-v', 'error', '-ss', f'{f / fps:.4f}', '-i', video, '-frames:v', '1', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'],
            check=True, capture_output=True,
        ).stdout
        pics.append(np.frombuffer(raw, np.uint8).reshape(1080, 1920, 3))
    a, b = (p.astype(np.int16) for p in pics)
    heat = np.abs(a - b).max(axis=2)
    ys, xs = np.nonzero(heat > 24)
    box = (xs.min(), ys.min(), xs.max(), ys.max()) if len(xs) else None
    print(stamp, 'changed box', box, 'pixels', len(xs))
    sheet = Image.new('RGB', (1920, 1080 * 3 // 2))
    sheet.paste(Image.fromarray(pics[0]).resize((960, 540)), (0, 0))
    sheet.paste(Image.fromarray(pics[1]).resize((960, 540)), (960, 0))
    hm = np.clip(heat * 4, 0, 255).astype(np.uint8)
    sheet.paste(Image.fromarray(hm).convert('RGB').resize((960, 540)), (480, 540))
    sheet.save(f'{out}/pop-{stamp}.png')
