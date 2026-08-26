"""Generate bt-battery-bar application icon (multi-size .ico).

Design (Microsoft Fluent style):
- Rounded battery body in Windows accent blue gradient (#0067C0 -> #4AA2E8)
- White Bluetooth rune centered -> reads as "Bluetooth battery" at a glance
- Flat geometry, crisp at 16x16 (tray / taskbar minimum size), transparent bg

Usage:  python tools/make_icon.py [output.ico]
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw

ACCENT_TOP = (74, 162, 232)    # lighter fluent blue
ACCENT_BOT = (0, 103, 192)     # #0067C0 fluent dark accent blue
CAP_DARK = (0, 80, 152)        # battery cap
WHITE = (255, 255, 255)

SIZES = [256, 128, 96, 64, 48, 40, 32, 24, 20, 16]
SS = 4  # supersampling factor for anti-aliasing


def _rune_lines(s: float):
    """Bluetooth rune segments in unit coordinates scaled by s."""
    a = (0.500 * s, 0.140 * s)   # top of stem
    b = (0.500 * s, 0.860 * s)   # bottom of stem
    c = (0.790 * s, 0.310 * s)   # upper-right tip
    d = (0.790 * s, 0.690 * s)   # lower-right tip
    m = (0.500 * s, 0.500 * s)   # center
    return [(a, b), (a, c), (c, m), (m, d), (d, b)]


def make_icon(px: int) -> Image.Image:
    """Render one icon frame at `px` logical pixels."""
    s = px * SS  # supersampled canvas size
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    dr = ImageDraw.Draw(img)

    # ---- battery body (rounded rect, vertical gradient) ----
    bx0, by0, bx1, by1 = 0.055 * s, 0.215 * s, 0.845 * s, 0.785 * s
    radius = 0.10 * s
    grad = Image.new("RGBA", (int(bx1 - bx0), int(by1 - by0)))
    gdr = ImageDraw.Draw(grad)
    gh = grad.height
    for y in range(gh):
        t = y / max(gh - 1, 1)
        col = tuple(int(ACCENT_TOP[i] + (ACCENT_BOT[i] - ACCENT_TOP[i]) * t) for i in range(3))
        gdr.line([(0, y), (grad.width, y)], fill=col + (255,))
    mask = Image.new("L", grad.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, grad.width - 1, grad.height - 1], radius=radius, fill=255)
    img.paste(grad, (int(bx0), int(by0)), mask)

    # ---- battery cap ----
    dr.rounded_rectangle(
        [0.875 * s, 0.395 * s, 0.955 * s, 0.605 * s],
        radius=0.035 * s,
        fill=CAP_DARK,
    )

    # ---- white bluetooth rune ----
    rw = 0.52 * px  # rune box size (logical px)
    ox = (bx0 + bx1) / 2 - rw / 2 * SS
    oy = (by0 + by1) / 2 - rw / 2 * SS
    lw = max(int(0.085 * s), SS)  # stroke width
    for p, q in _rune_lines(rw * SS):
        dr.line([ox + p[0], oy + p[1], ox + q[0], oy + q[1]],
                fill=WHITE, width=lw)
        r = lw / 2 - 0.5
        for x, y in (p, q):
            dr.ellipse([ox + x - r, oy + y - r, ox + x + r, oy + y + r], fill=WHITE)

    return img.resize((px, px), Image.LANCZOS)


def main() -> None:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("installer/assets/icon.ico")
    out.parent.mkdir(parents=True, exist_ok=True)

    frames = {px: make_icon(px) for px in SIZES}
    frames[256].save(
        out,
        format="ICO",
        sizes=[(px, px) for px in SIZES],
        append_images=[frames[p] for p in SIZES[1:]],
    )
    print(f"icon written: {out} ({out.stat().st_size} bytes)")

    # preview sheet: dark & light rows
    pad, cell = 12, 256
    sheet = Image.new("RGB", ((cell + pad) * 5 + pad, (cell + pad) * 2 + pad + 18), (240, 240, 240))
    d = ImageDraw.Draw(sheet)
    for row, bg in enumerate([(32, 32, 32), (250, 250, 250)]):
        y = pad + row * (cell + pad + 18)
        d.rectangle([pad, y, sheet.width - pad, y + cell + 18], fill=bg)
        for i, px in enumerate([256, 48, 32, 24, 16]):
            ic = frames[px].resize((px, px), Image.NEAREST if px < 48 else Image.LANCZOS)
            sheet.paste(ic, (pad + 30 + i * (cell + pad) // 1, y + 9), ic)
            d.text((pad + 30 + i * (cell + pad) + 4, y + cell - 2), f"{px}px",
                   fill=(180, 180, 180) if row == 0 else (120, 120, 120))
    prev = out.parent / "icon-preview.png"
    sheet.save(prev)
    print(f"preview written: {prev}")


if __name__ == "__main__":
    main()
