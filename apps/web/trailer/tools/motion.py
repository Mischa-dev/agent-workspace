# Frame-to-frame motion in a video: python3 -I motion.py video.mp4 out-dir
# Writes diff.npy (mean absolute change per frame, 0-255 grey at 240x135) and
# prints isolated pops and still stretches.
import subprocess
import sys

import numpy as np

video, out = sys.argv[1], sys.argv[2]
W, H = 240, 135
raw = subprocess.run(
    ['ffmpeg', '-v', 'error', '-i', video, '-vf', f'scale={W}:{H}:flags=area,format=gray', '-f', 'rawvideo', '-'],
    check=True, capture_output=True,
).stdout
frames = np.frombuffer(raw, np.uint8).reshape(-1, H, W).astype(np.float32)
diff = np.abs(frames[1:] - frames[:-1]).mean(axis=(1, 2))
np.save(f'{out}/diff.npy', diff)
fps = 60
print('frames', len(frames))
pops = []
for i in range(len(diff)):
    lo, hi = max(0, i - 6), min(len(diff), i + 7)
    nb = np.concatenate([diff[lo:i], diff[i + 1:hi]])
    med = float(np.median(nb))
    if diff[i] > 0.012 and diff[i] > 4 * max(med, 0.002):
        pops.append(((i + 1) / fps, float(diff[i]), med))
for at, v, med in pops:
    if v > 0.15:
        print(f'pop {at:7.3f}  {v:.3f}  (around {med:.4f})')
run = None
for i, v in enumerate(diff):
    if v < 0.0015:
        if run is None:
            run = i
    else:
        if run is not None and i - run >= 36:
            print(f'still {run / fps:.2f}-{i / fps:.2f} ({(i - run) / fps:.2f}s)')
        run = None
if run is not None and len(diff) - run >= 36:
    print(f'still {run / fps:.2f}-end')
