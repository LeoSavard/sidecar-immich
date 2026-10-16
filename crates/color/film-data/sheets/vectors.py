# SPDX-License-Identifier: Apache-2.0
# List the paths of a datasheet page (pdftocairo -svg), in page coordinates: transforms applied, curve
# segments' end points kept. Prints each path's bounding box, point count and dash pattern, so graph frames,
# gridlines and curves can be told apart.
import re
import sys


def matrix(attr):
    m = re.search(r'transform="matrix\(([^)]*)\)"', attr)
    if not m:
        return (1, 0, 0, 1, 0, 0)
    return tuple(float(v) for v in re.split(r"[ ,]+", m.group(1).strip()))


def points(d, t):
    a, b, c, dd, e, f = t
    nums = [float(v) for v in re.findall(r"-?[\d.]+(?:e-?\d+)?", d)]
    cmds = re.findall(r"[MLCZ]", d)
    out, i, cur = [], 0, None
    tr = lambda x, y: (a * x + c * y + e, b * x + dd * y + f)
    for cmd in cmds:
        n = {"M": 2, "L": 2, "C": 6, "Z": 0}[cmd]
        if cmd == "C" and cur is not None:
            # A cubic Bézier: sample it, so curved segments keep their shape.
            p0, p1, p2, p3 = cur, (nums[i], nums[i + 1]), (nums[i + 2], nums[i + 3]), (nums[i + 4], nums[i + 5])
            for k in range(1, 9):
                t = k / 8
                u = 1 - t
                x = u ** 3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t ** 3 * p3[0]
                y = u ** 3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t ** 3 * p3[1]
                out.append(tr(x, y))
            cur = p3
        elif n:
            cur = (nums[i + n - 2], nums[i + n - 1])
            out.append(tr(*cur))
        i += n
    return out


def paths(svg):
    s = open(svg).read()
    # Group transforms: cairo wraps some paths in <g transform=...>; keep it simple and only use the path's own.
    for m in re.finditer(r"<path ([^>]*)/>", s):
        attr = m.group(1)
        d = re.search(r'd="([^"]*)"', attr)
        if not d:
            continue
        dash = re.search(r'stroke-dasharray="([^"]*)"', attr)
        yield points(d.group(1), matrix(attr)), (dash.group(1) if dash else ""), attr


if __name__ == "__main__":
    for i, (pts, dash, attr) in enumerate(paths(sys.argv[1])):
        if not pts:
            continue
        xs, ys = [p[0] for p in pts], [p[1] for p in pts]
        fill = "fill" if 'fill="none"' not in attr else ""
        print(f"{i:4d} n={len(pts):4d} x {min(xs):7.1f}..{max(xs):7.1f} y {min(ys):7.1f}..{max(ys):7.1f} dash={dash[:20]} {fill}")
