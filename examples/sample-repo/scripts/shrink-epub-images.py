#!/usr/bin/env python3
"""Downscale images inside an EPUB to shrink Kindle delivery size, without
touching the print PDF (which uses the full-res originals). Keeps PNG format and
alpha (transparent chapter plates stay transparent); just resizes + optimizes.
Usage: shrink-epub-images.py <book.epub> [max_w]"""
import sys, zipfile, shutil, tempfile, os
from PIL import Image

def main():
    epub = sys.argv[1]; max_w = int(sys.argv[2]) if len(sys.argv) > 2 else 1200
    tmp = tempfile.mkdtemp()
    with zipfile.ZipFile(epub) as z: names = z.namelist(); z.extractall(tmp)
    before = os.path.getsize(epub)
    for root, _, files in os.walk(tmp):
        for fn in files:
            if not fn.lower().endswith((".png", ".jpg", ".jpeg")): continue
            p = os.path.join(root, fn)
            try: im = Image.open(p)
            except Exception: continue
            w, h = im.size
            if w > max_w:
                nh = round(h * max_w / w); im = im.resize((max_w, nh), Image.LANCZOS)
            if fn.lower().endswith(".png"):
                if im.mode == "RGBA":
                    # keep alpha but reduce; quantize RGBA preserves transparency
                    im = im.quantize(colors=256, method=Image.FASTOCTREE)
                else:
                    im = im.convert("RGB").quantize(colors=256, method=Image.FASTOCTREE)
                im.save(p, optimize=True)
            else:
                im.convert("RGB").save(p, quality=82, optimize=True)
    # rezip: mimetype first, stored, no compression on it
    out = epub + ".tmp"
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        if "mimetype" in names:
            z.write(os.path.join(tmp, "mimetype"), "mimetype", compress_type=zipfile.ZIP_STORED)
        for n in names:
            if n == "mimetype": continue
            z.write(os.path.join(tmp, n), n)
    shutil.move(out, epub); shutil.rmtree(tmp)
    after = os.path.getsize(epub)
    print(f"  {os.path.basename(epub)}: {before/1e6:.1f}MB -> {after/1e6:.1f}MB")

if __name__ == "__main__": main()
