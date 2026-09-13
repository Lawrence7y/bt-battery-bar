"""Generate the original BtBatteryBar icon family.

The mark is intentionally not a Bluetooth glyph or a Windows/Microsoft mark.
It combines a battery silhouette with a custom energy-pulse path so the app
remains recognizable at tray size while having its own visual identity.

Outputs:
  - installer/assets/icon.ico (multi-size Win32 icon)
  - msix/layout/Assets/* (MSIX icon scale/target-size variants)
  - installer/assets/icon-preview.png

Usage:
  python tools/make_icon.py [output.ico] [msix-assets-dir]
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

Image.MAX_IMAGE_PIXELS = None

HERE = Path(__file__).resolve().parent.parent

# Blue is used for the app identity, while the white pulse stays readable on
# both light and dark Windows themes. No Microsoft product colors or marks are
# used as a logo element.
BLUE_TOP = (72, 190, 255)
BLUE_BOT = (0, 92, 196)
BLUE_EDGE = (0, 52, 125)
BLUE_CAP = (0, 66, 150)
INK = (6, 32, 72)
WHITE = (255, 255, 255)

ICO_SIZES = [256, 128, 96, 64, 48, 40, 32, 24, 20, 16]
SS = 4  # supersampling factor for anti-aliasing

SCALE_FACTORS = {
    100: 1.00,
    125: 1.25,
    150: 1.50,
    200: 2.00,
    250: 2.50,
    300: 3.00,
    400: 4.00,
}


def _scaled(value: float, s: int) -> int:
    return int(round(value * s))


def _round_line(draw, points, fill, width):
    """Draw a line with round caps (Pillow's line caps vary by version)."""
    draw.line(points, fill=fill, width=width, joint="curve")
    radius = max(width // 2, 1)
    for x, y in (points[0], points[-1]):
        draw.ellipse([x - radius, y - radius, x + radius, y + radius], fill=fill)


def render_mark(px: int) -> Image.Image:
    """Render the transparent app mark at an exact square pixel size."""
    px = max(int(px), 16)
    s = px * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    dr = ImageDraw.Draw(img, "RGBA")

    # Soft edge shadow improves separation from both desktop themes without
    # introducing a solid plate behind the app icon.
    shadow = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow, "RGBA")
    body = [_scaled(v, s) for v in (0.105, 0.235, 0.805, 0.765)]
    sd.rounded_rectangle(body, radius=_scaled(0.105, s), fill=(2, 24, 60, 95))
    shadow = shadow.filter(ImageFilter.GaussianBlur(max(1, _scaled(0.025, s))))
    img.alpha_composite(shadow, (0, _scaled(0.018, s)))

    # Battery body with a restrained vertical blue gradient.
    bx0, by0, bx1, by1 = body
    grad = Image.new("RGBA", (bx1 - bx0 + 1, by1 - by0 + 1))
    gd = ImageDraw.Draw(grad, "RGBA")
    for y in range(grad.height):
        t = y / max(grad.height - 1, 1)
        col = tuple(int(BLUE_TOP[i] + (BLUE_BOT[i] - BLUE_TOP[i]) * t) for i in range(3))
        gd.line([(0, y), (grad.width, y)], fill=col + (255,))
    mask = Image.new("L", grad.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, grad.width - 1, grad.height - 1],
        radius=_scaled(0.105, s),
        fill=255,
    )
    img.paste(grad, (bx0, by0), mask)
    dr = ImageDraw.Draw(img, "RGBA")
    dr.rounded_rectangle(body, radius=_scaled(0.105, s), outline=BLUE_EDGE + (255,), width=max(SS, _scaled(0.018, s)))

    # Battery terminal, deliberately separated from the body for clarity at
    # 16px and below.
    terminal = [_scaled(v, s) for v in (0.835, 0.405, 0.925, 0.595)]
    dr.rounded_rectangle(terminal, radius=_scaled(0.035, s), fill=BLUE_CAP + (255,))

    # Inner cavity gives the custom pulse a consistent silhouette. It is not a
    # Bluetooth rune: the asymmetrical pulse is the product's own symbol.
    cavity = [_scaled(v, s) for v in (0.185, 0.355, 0.735, 0.645)]
    dr.rounded_rectangle(cavity, radius=_scaled(0.045, s), fill=INK + (115,))

    pulse = [
        (_scaled(0.255, s), _scaled(0.535, s)),
        (_scaled(0.375, s), _scaled(0.535, s)),
        (_scaled(0.455, s), _scaled(0.365, s)),
        (_scaled(0.545, s), _scaled(0.635, s)),
        (_scaled(0.635, s), _scaled(0.455, s)),
        (_scaled(0.710, s), _scaled(0.455, s)),
    ]
    pulse_width = max(_scaled(0.075, s), 2 * SS)
    _round_line(dr, pulse, WHITE + (255,), pulse_width)

    # Small cyan connection node: a secondary cue for wireless devices that
    # remains subordinate to the battery/pulse mark.
    node_r = max(_scaled(0.027, s), SS // 2)
    node_x, node_y = _scaled(0.755, s), _scaled(0.295, s)
    dr.ellipse([node_x - node_r, node_y - node_r, node_x + node_r, node_y + node_r], fill=BLUE_TOP + (235,))

    return img.resize((px, px), Image.Resampling.LANCZOS)


def render_tile(width: int, height: int) -> Image.Image:
    """Render a solid, Store-friendly tile using the same original mark."""
    width, height = int(width), int(height)
    sw, sh = width * SS, height * SS
    tile = Image.new("RGBA", (sw, sh), (7, 24, 52, 255))
    td = ImageDraw.Draw(tile, "RGBA")
    # Subtle diagonal glow, not a logo plate copied from another product.
    for y in range(sh):
        t = y / max(sh - 1, 1)
        col = tuple(int((12, 66, 128)[i] * (1 - t) + (5, 30, 72)[i] * t) for i in range(3))
        td.line([(0, y), (sw, y)], fill=col + (255,))
    glow = Image.new("RGBA", (sw, sh), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow, "RGBA")
    gd.ellipse([int(sw * 0.18), int(sh * 0.06), int(sw * 0.88), int(sh * 0.76)], fill=(40, 150, 255, 70))
    tile = Image.alpha_composite(tile, glow.filter(ImageFilter.GaussianBlur(max(1, min(sw, sh) // 12))))

    mark_size = int(min(sw, sh) * 0.70)
    mark = render_mark(mark_size)
    tile.alpha_composite(mark, ((sw - mark.width) // 2, (sh - mark.height) // 2))
    return tile.resize((width, height), Image.Resampling.LANCZOS)


def _write_scaled_family(out_dir: Path, prefix: str, base: int) -> None:
    for factor, multiplier in SCALE_FACTORS.items():
        size = int(round(base * multiplier))
        render_mark(size).save(out_dir / f"{prefix}.scale-{factor}.png")


def emit_msix_assets(out_dir: Path) -> None:
    """Write the MSIX assets required by current Windows icon guidance."""
    out_dir.mkdir(parents=True, exist_ok=True)

    # Manifest base assets and explicit scale variants.
    for prefix, base in (
        ("Square44x44Logo", 44),
        ("Square150x150Logo", 150),
        ("StoreLogo", 50),
    ):
        render_mark(base).save(out_dir / f"{prefix}.png")
        _write_scaled_family(out_dir, prefix, base)

    # Required unplated theme variants are separate files even when their
    # pixels are identical. This lets Windows avoid adding a system backplate.
    target_sizes = [16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256]
    for size in target_sizes:
        mark = render_mark(size)
        mark.save(out_dir / f"AppList.targetsize-{size}.png")
        mark.save(out_dir / f"AppList.targetsize-{size}_altform-unplated.png")
        mark.save(out_dir / f"AppList.targetsize-{size}_altform-lightunplated.png")

    # Optional Windows 10 app-list scale files and tile families. Keeping the
    # set complete makes the package robust across Windows 10/11 scale modes.
    _write_scaled_family(out_dir, "AppList", 44)
    tile_bases = {
        "SmallTile": (71, 71),
        "MedTile": (150, 150),
        "WideTile": (310, 150),
        "LargeTile": (310, 310),
    }
    for prefix, (base_w, base_h) in tile_bases.items():
        for factor, multiplier in SCALE_FACTORS.items():
            w = int(round(base_w * multiplier))
            h = int(round(base_h * multiplier))
            render_tile(w, h).save(out_dir / f"{prefix}.scale-{factor}.png")

    # Backward-compatible aliases used by the previous package layout.
    for size in [16, 24, 32, 48, 256]:
        mark = render_mark(size)
        mark.save(out_dir / f"Square44x44Logo.targetsize-{size}.png")
        mark.save(out_dir / f"Square44x44Logo.targetsize-{size}_altform-unplated.png")
    render_mark(44).save(out_dir / "Square44x44Logo_altform-unplated.png")
    render_mark(150).save(out_dir / "Square150x150Logo_altform-unplated.png")
    render_mark(50).save(out_dir / "StoreLogo_altform-unplated.png")


def write_ico(out: Path) -> dict[int, Image.Image]:
    out.parent.mkdir(parents=True, exist_ok=True)
    frames = {px: render_mark(px) for px in ICO_SIZES}
    frames[256].save(
        out,
        format="ICO",
        sizes=[(px, px) for px in ICO_SIZES],
        append_images=[frames[p] for p in ICO_SIZES[1:]],
    )
    return frames


def write_preview(frames: dict[int, Image.Image], out: Path) -> None:
    pad, cell, label_h = 14, 220, 26
    sheet = Image.new("RGB", ((cell + pad) * 5 + pad, (cell + pad + label_h) * 2 + pad), (238, 240, 244))
    draw = ImageDraw.Draw(sheet)
    for row, bg in enumerate([(23, 28, 38), (250, 251, 253)]):
        y = pad + row * (cell + pad + label_h)
        draw.rectangle([pad, y, sheet.width - pad, y + cell + label_h], fill=bg)
        sizes = [256, 48, 32, 24, 16]
        for col, px in enumerate(sizes):
            preview = frames[px].resize((px, px), Image.Resampling.NEAREST if px < 48 else Image.Resampling.LANCZOS)
            x = pad + 28 + col * (cell + pad)
            sheet.paste(preview, (x, y + 16), preview)
            draw.text((x + 2, y + cell + 2), f"{px}px", fill=(180, 184, 193) if row == 0 else (105, 110, 120))
    out.parent.mkdir(parents=True, exist_ok=True)
    sheet.save(out)


def main() -> None:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "installer/assets/icon.ico"
    if not out.is_absolute():
        out = Path.cwd() / out
    assets_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else HERE / "msix/layout/Assets"
    if not assets_dir.is_absolute():
        assets_dir = Path.cwd() / assets_dir

    frames = write_ico(out)
    print(f"icon written: {out} ({out.stat().st_size} bytes)")
    preview = out.parent / "icon-preview.png"
    write_preview(frames, preview)
    print(f"preview written: {preview}")
    emit_msix_assets(assets_dir)
    print(f"MSIX assets written: {assets_dir}")


if __name__ == "__main__":
    main()
