#!/usr/bin/env python3
"""The Deskveet mark, built from one module so every size and angle follows from a rule.

  canvas 32 x 32, module m = 2, centre C = (16, 16), SVG angles (0 = right, 90 = down)

  planet   radius 3m, centred on C
  wipe     a band 1m wide through the planet's centre, at -45 degrees
  orbit    radius 5.5m around C, stroke 1m, round ends; open for one quarter, centred on -45 degrees,
           so the wipe points straight out through the opening
  dust     three grains leaving through the opening from the orbit's end at -90 degrees:
           radius m * phi^-k and opacity phi^-k (k = 0, 1, 2), each gap 1/2 m measured along the orbit

Writes assets/icon.svg (app icon), assets/tray.svg (menu bar template) and assets/mark.svg (the mark on a
transparent background, for docs and promo material), and prints the sidebar <svg> for index.html. Render with scripts/icons.sh.
"""
from math import cos, sin, radians, degrees, sqrt
from pathlib import Path

M = 2.0
C = 16.0
PHI = (1 + sqrt(5)) / 2
R_PLANET, R_ORBIT, STROKE, BAND, GAP = 3 * M, 5.5 * M, M, M, M / 2
OPEN_FROM, OPEN_TO = 270.0, 360.0           # the open quarter (top right), centred on 315 = -45


def n(v: float) -> str:
    return f"{v:.3f}".rstrip("0").rstrip(".")


def at(r: float, a: float) -> tuple[float, float]:
    return C + r * cos(radians(a)), C + r * sin(radians(a))


def dust() -> list[tuple[float, float, float, float]]:
    """(x, y, radius, opacity) for each grain."""
    out, edge, a = [], STROKE / 2, OPEN_FROM   # start from the round end of the orbit
    for k in range(3):
        r = M * PHI ** -k
        a += degrees((edge + GAP + r) / R_ORBIT)
        x, y = at(R_ORBIT, a)
        out.append((x, y, r, PHI ** -k))
        edge = r
    return out


def mark(planet: str, ink: str, mask_id: str) -> str:
    """The mark in the 32-unit space; `planet` and `ink` are fill/stroke values."""
    w0, w1 = at(20, 135), at(20, 315)
    x0, y0 = at(R_ORBIT, OPEN_TO)
    x1, y1 = at(R_ORBIT, OPEN_FROM)
    grains = "".join(
        f'<circle cx="{n(x)}" cy="{n(y)}" r="{n(r)}" fill="{ink}"{"" if o == 1 else f' fill-opacity="{n(o)}"'}/>'
        for x, y, r, o in dust())
    return (
        f'<mask id="{mask_id}" maskUnits="userSpaceOnUse" x="0" y="0" width="32" height="32">'
        f'<rect width="32" height="32" fill="#fff"/>'
        f'<path d="M{n(w0[0])} {n(w0[1])}L{n(w1[0])} {n(w1[1])}" stroke="#000" stroke-width="{n(BAND)}"/></mask>'
        f'<circle cx="16" cy="16" r="{n(R_PLANET)}" fill="{planet}" mask="url(#{mask_id})"/>'
        # three quarters of the orbit, from 0 degrees clockwise round to 270
        f'<path d="M{n(x0)} {n(y0)}A{n(R_ORBIT)} {n(R_ORBIT)} 0 1 1 {n(x1)} {n(y1)}" fill="none" stroke="{ink}" '
        f'stroke-width="{n(STROKE)}" stroke-linecap="round"/>'
        + grains)


def app_icon() -> str:
    # macOS icon grid: 1024 canvas, 824 tile at 100 with radius 190; the mark's 24-unit box fills 62% of the tile
    s = 824 * 0.62 / 24
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#34332f"/><stop offset="1" stop-color="#1f1e1c"/>
    </linearGradient>
    <linearGradient id="warm" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#ec9a7a"/><stop offset="1" stop-color="#c4603e"/>
    </linearGradient>
  </defs>
  <rect x="100" y="100" width="824" height="824" rx="190" fill="url(#bg)"/>
  <rect x="101.5" y="101.5" width="821" height="821" rx="188.5" fill="none" stroke="#faf9f5" stroke-opacity=".08" stroke-width="3"/>
  <g transform="translate(512 512) scale({n(s)}) translate(-16 -16)">{mark("url(#warm)", "#faf9f5", "wipe")}</g>
</svg>
'''


def tray_icon() -> str:
    # template image: black only, macOS tints it; viewBox trimmed to the mark's 24-unit box plus 1 unit each side
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="3 3 26 26">{mark("#000", "#000", "wipe")}</svg>
'''


def mark_svg() -> str:
    # standalone mark in the dark theme colours, trimmed like the tray icon
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="3 3 26 26">{mark("#d97757", "#faf9f5", "wipe")}</svg>
'''


def sidebar() -> str:
    return (f'<svg viewBox="0 0 32 32" width="26" height="26" aria-hidden="true">'
            f'{mark("currentColor", "var(--text)", "brand-wipe")}</svg>')


if __name__ == "__main__":
    assets = Path(__file__).parent.parent / "assets"
    assets.mkdir(exist_ok=True)
    (assets / "icon.svg").write_text(app_icon())
    (assets / "tray.svg").write_text(tray_icon())
    (assets / "mark.svg").write_text(mark_svg())
    print(sidebar())
