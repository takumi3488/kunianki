#!/usr/bin/env python3
"""Generate country map SVGs from Natural Earth 1:50m.

Requires Python 3.11+ and Shapely 2: python3 -m venv target/mapgen-venv &&
target/mapgen-venv/bin/pip install 'shapely>=2,<3'. Run with
 target/mapgen-venv/bin/python scripts/gen_maps.py
Natural Earth source: https://github.com/nvkelso/natural-earth-vector
"""

import json
import math
import statistics
import urllib.request
from pathlib import Path

from shapely import affinity
from shapely.geometry import Point, box, shape
from shapely.ops import transform

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_50m_admin_0_countries.geojson"
CACHE = ROOT / "target" / "ne_50m_admin_0_countries.geojson"
OUTPUT = ROOT / "assets" / "maps"
COUNTRIES = """
AD AE AF AG AL AM AO AR AT AU AZ BA BB BD BE BF BG BH BI BJ BN BO BR BS BT BW BY BZ
CA CD CF CG CH CI CL CM CN CO CR CU CV CY CZ DE DJ DK DM DO DZ EC EE EG ER ES ET FI
FJ FM FR GA GB GD GE GH GM GN GQ GR GT GW GY HN HR HT HU ID IE IL IN IQ IR IS IT
JM JO JP KE KG KH KI KM KN KP KR KW KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME
MG MH MK ML MM MN MR MT MU MV MW MX MY MZ NA NE NG NI NL NO NP NR NZ OM PA PE PG
PH PK PL PT PW PY QA RO RS RU RW SA SB SC SD SE SG SI SK SL SM SN SO SR SS ST SV SY
SZ TD TG TH TJ TL TM TN TO TR TT TV TZ UA UG US UY UZ VC VE VN VU WS YE ZA ZM ZW
""".split()
# Known centroids are used when a tiny state has no polygon at 1:50m.
CENTROIDS = {
    "AD": (1.6016, 42.5462), "AG": (-61.7964, 17.0608), "BH": (50.5577, 26.0667),
    "BB": (-59.5432, 13.1939), "BN": (114.7277, 4.5353), "CV": (-23.6052, 15.1201),
    "KM": (43.3333, -11.6455), "DM": (-61.371, 15.415), "GD": (-61.679, 12.1165),
    "FJ": (178.065, -17.7134),
    "KI": (173.0, 1.87), "MV": (73.2207, 3.2028), "MT": (14.3754, 35.9375),
    "MH": (171.1845, 7.1315), "MU": (57.5522, -20.3484), "FM": (150.5508, 7.4256),
    "MC": (7.4246, 43.7384), "NR": (166.9315, -0.5228), "PW": (134.5825, 7.515),
    "KN": (-62.783, 17.3578), "LC": (-60.9789, 13.9094), "LI": (9.535743, 47.136658), "VC": (-61.2872, 12.9843),
    "WS": (-172.1046, -13.759), "SM": (12.4578, 43.9428), "SC": (55.492, -4.6796),
    "SG": (103.8198, 1.3521), "SB": (160.1562, -9.6457), "TO": (-175.1982, -21.179),
    "TV": (177.6493, -7.1095), "VU": (166.9592, -15.3767),
}
# Crop broad territories around their main populated land.
FOCUS = {
    "CL": (-71.5, -36.0, 10.0, 43.0), "EC": (-78.5, -1.5, 8.0, 8.0),
    "ES": (-3.5, 40.0, 13.0, 11.0), "FJ": (178.0, -17.0, 24.0, 18.0),
    "FR": (2.3, 46.5, 14.0, 12.0), "KI": (173.0, 1.0, 22.0, 14.0),
    "NL": (5.3, 52.1, 8.0, 7.0), "NO": (12.0, 63.5, 25.0, 19.0),
    "NZ": (172.0, -41.0, 34.0, 27.0), "PT": (-8.0, 39.5, 7.0, 7.0),
    "RU": (100.0, 63.0, 160.0, 56.0), "US": (-98.0, 39.5, 60.0, 35.0),
    "ZA": (25.0, -29.0, 18.0, 16.0),
}
FOCUS_CONTEXT = {
    # A full-Russia view stays compact with its bordering countries for context.
    "RU": set("NO FI EE LV LT PL BY UA GE AZ KZ CN MN KP US JP".split()),
}
# Keep tiny island states legible, and identify their position even when a polygon exists.
TINY = set(CENTROIDS)


def download_source():
    CACHE.parent.mkdir(parents=True, exist_ok=True)
    if not CACHE.exists():
        print(f"Downloading {SOURCE}")
        urllib.request.urlretrieve(SOURCE, CACHE)
    with CACHE.open(encoding="utf-8") as source:
        return json.load(source)["features"]


def iso_code(properties):
    for key in ("ISO_A2_EH", "ISO_A2"):
        code = properties.get(key)
        if isinstance(code, str) and len(code) == 2 and code != "-99":
            return code
    return None


def coordinates(geometry):
    if geometry.geom_type == "Polygon":
        polygons = (geometry,)
    elif geometry.geom_type == "MultiPolygon":
        polygons = geometry.geoms
    else:
        return
    for polygon in polygons:
        yield from polygon.exterior.coords


def center_of(geometry, code):
    if code in FOCUS:
        return FOCUS[code][:2]
    coords = list(coordinates(geometry))
    lon = math.degrees(math.atan2(
        sum(math.sin(math.radians(point[0])) for point in coords),
        sum(math.cos(math.radians(point[0])) for point in coords),
    ))
    return lon, statistics.median(point[1] for point in coords)


def project_geometry(geometry, center_lon, center_lat, scale_x):
    def project(x, y, z=None):
        if hasattr(x, "__iter__"):
            longitudes = []
            for lon in x:
                unwrapped = lon if not longitudes else lon + 360 * round((longitudes[-1] - lon) / 360)
                longitudes.append(unwrapped)
            shift = 360 * round((center_lon - statistics.mean(longitudes)) / 360)
            xs = [(lon + shift - center_lon) * scale_x for lon in longitudes]
            ys = [lat - center_lat for lat in y]
            return (xs, ys) if z is None else (xs, ys, z)
        px = ((x - center_lon + 180) % 360 - 180) * scale_x
        py = y - center_lat
        return (px, py) if z is None else (px, py, z)
    return transform(project, geometry)


def polygon_paths(geometry):
    if geometry.is_empty:
        return
    if geometry.geom_type == "Polygon":
        polygons = (geometry,)
    elif geometry.geom_type == "MultiPolygon" or geometry.geom_type == "GeometryCollection":
        polygons = geometry.geoms
    else:
        return
    for polygon in polygons:
        if polygon.geom_type != "Polygon" or polygon.area <= 0:
            continue
        rings = [polygon.exterior, *polygon.interiors]
        pieces = []
        for ring in rings:
            points = list(ring.coords)
            if len(points) < 4:
                continue
            pieces.append("M" + " L".join(f"{x:.3f} {y:.3f}" for x, y in points) + " Z")
        if pieces:
            yield " ".join(pieces)


def svg_for(code, features, countries):
    country = countries.get(code)
    context_codes = FOCUS_CONTEXT.get(code)
    fallback = CENTROIDS.get(code)
    if country is None and fallback is None:
        raise RuntimeError(f"No Natural Earth polygon or known centroid for {code}")

    if country is not None:
        center_lon, center_lat = center_of(country, code)
    else:
        center_lon, center_lat = fallback

    scale_x = max(0.16, math.cos(math.radians(center_lat)))
    target = project_geometry(country, center_lon, center_lat, scale_x) if country and code not in FOCUS else Point(0, 0)
    if code in FOCUS:
        cx = cy = 0.0
        width = FOCUS[code][2] * scale_x
        height = FOCUS[code][3]
        side = max(width, height) * 1.25
    elif country:
        min_x, min_y, max_x, max_y = target.bounds
        width, height = max_x - min_x, max_y - min_y
        width, height = max(width, 4.0 * scale_x), max(height, 4.0)
        pad = max(width, height) * 0.16
        cx, cy = (min_x + max_x) / 2, (min_y + max_y) / 2
        side = max(width, height) + 2 * pad
    else:
        cx = cy = 0.0
        side = 5.0
    half = side / 2
    crop = box(cx - half, cy - half, cx + half, cy + half)

    projected = []
    for feature in features:
        props = feature["properties"]
        feature_code = iso_code(props)
        if context_codes and feature_code != code and feature_code not in context_codes:
            continue
        geometry = shape(feature["geometry"])
        if not geometry.is_valid:
            geometry = geometry.buffer(0)
        _, min_lat, _, max_lat = geometry.bounds
        if max_lat - center_lat < crop.bounds[1] or min_lat - center_lat > crop.bounds[3]:
            continue
        min_lon, _, max_lon, _ = geometry.bounds
        if max_lon - min_lon < 180:
            left = ((min_lon - center_lon + 180) % 360 - 180) * scale_x
            right = ((max_lon - center_lon + 180) % 360 - 180) * scale_x
            if left <= right and (right < crop.bounds[0] or left > crop.bounds[2]):
                continue
        projected_geometry = project_geometry(geometry, center_lon, center_lat, scale_x)
        if not projected_geometry.is_valid:
            projected_geometry = projected_geometry.buffer(0)
        if not projected_geometry.intersects(crop):
            continue
        clipped = projected_geometry.intersection(crop).simplify(0.15, preserve_topology=True)
        if not clipped.is_empty:
            projected.append((feature_code, clipped))

    # SVG uses an inverted y axis so paths remain compact and naturally oriented.
    paths = []
    for feature_code, geometry in projected:
        fill = "#e53935" if feature_code == code else "#cfd8dc"
        for d in polygon_paths(geometry):
            paths.append(f'<path d="{d}" fill="{fill}" stroke="#ffffff" stroke-width="0.12"/>')
    marker = ""
    if code in TINY:
        point = Point(0, 0)
        if fallback:
            lon, lat = fallback
            point = Point(((lon - center_lon + 180) % 360 - 180) * scale_x, lat - center_lat)
        marker = f'<circle cx="{point.x:.3f}" cy="{-point.y:.3f}" r="0.42" fill="#e53935" stroke="#ffffff" stroke-width="0.12"/>'
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{cx-half:.3f} {-cy-half:.3f} {side:.3f} {side:.3f}">'
        f'<rect x="{cx-half:.3f}" y="{-cy-half:.3f}" width="{side:.3f}" height="{side:.3f}" fill="#e3f2fd"/>'
        f'<g transform="scale(1,-1)">{"".join(paths)}</g>'
        f'{marker}</svg>\n'
    )


def main():
    features = download_source()
    countries = {}
    for feature in features:
        code = iso_code(feature["properties"])
        if code in COUNTRIES:
            geometry = shape(feature["geometry"])
            if not geometry.is_valid:
                geometry = geometry.buffer(0)
            countries.setdefault(code, geometry)

    missing = sorted(set(COUNTRIES) - countries.keys() - CENTROIDS.keys())
    if missing:
        raise RuntimeError(f"No map polygon or centroid for: {', '.join(missing)}")
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for code in COUNTRIES:
        content = svg_for(code, features, countries)
        path = OUTPUT / f"{code.lower()}.svg"
        path.write_text(content, encoding="utf-8")
        if path.stat().st_size >= 60_000:
            raise RuntimeError(f"{path} exceeds the 60 KB contract limit")
    print(f"Generated {len(COUNTRIES)} maps in {OUTPUT}")


if __name__ == "__main__":
    main()
