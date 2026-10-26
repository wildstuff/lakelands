#!/usr/bin/env python3
"""Writes walk 8's data files (`make_data.py write DIR`: DIR/*.ndjson) and,
run bare, prints how many records of each type they hold. The trade is
walk 6's types grown past what a few pages hold: 300 suppliers, 2 000
products, 3 000 customers and over 20 000 order lines, drawn with a fixed
seed, with free text an agent searches by. The records walk 8's tasks
name are planted among them; `answers.py` computes the tasks' answers
from the lists here, not from the lake."""
import json
import random
import sys
from datetime import date, timedelta
from decimal import Decimal
from pathlib import Path

rng = random.Random(8)

# Suppliers: a place and a trade word, unique, in the trade word's country.
TRADES = {
    "Pumpen": ("DE", "pumps"), "Armaturen": ("DE", "valves"), "Kabel": ("DE", "cables"),
    "Valvole": ("IT", "valves"), "Guarnizioni": ("IT", "seals"), "Rubinetti": ("IT", "fittings"),
    "Okucia": ("PL", "fasteners"), "Lozyska": ("PL", "bearings"), "Bombas": ("ES", "pumps"),
    "Fasteners": ("SE", "fasteners"), "Bearings": ("GB", "bearings"), "Hoses": ("NL", "hoses"),
    "Tools": ("CZ", "tools"), "Robinetterie": ("FR", "valves"), "Joints": ("FR", "seals"),
    "Lager": ("DE", "bearings"), "Werkzeuge": ("DE", "tools"), "Raccordi": ("IT", "fittings"),
    "Kable": ("PL", "cables"), "Seals": ("GB", "seals"),
}
PLACES = {
    "DE": ["Rhein", "Elbe", "Mosel", "Harz", "Allgau", "Spree", "Main", "Ruhr", "Taunus", "Eifel", "Weser", "Isar", "Neckar", "Saar", "Ems", "Lahn", "Fulda", "Havel", "Donau", "Inn"],
    "IT": ["Milano", "Genova", "Bari", "Lago", "Verona", "Brescia", "Padova", "Bergamo", "Lecco", "Varese", "Parma", "Modena", "Ancona", "Trieste", "Pisa", "Siena", "Udine", "Cremona", "Pavia", "Torino"],
    "PL": ["Krakow", "Poznan", "Lodz", "Lublin", "Opole", "Gliwice", "Radom", "Kielce", "Tarnow", "Rzeszow", "Plock", "Torun", "Zabrze", "Bytom", "Elblag", "Legnica"],
    "ES": ["Bilbao", "Valencia", "Sevilla", "Zaragoza", "Vigo", "Gijon", "Murcia", "Burgos", "Leon", "Cadiz", "Huesca", "Lleida"],
    "SE": ["Nordic", "Malmo", "Umea", "Lulea", "Orebro", "Vasteras", "Boras", "Gavle", "Kalmar", "Vaxjo", "Falun", "Lund"],
    "GB": ["Pennine", "Mersey", "Severn", "Clyde", "Tyne", "Humber", "Solent", "Thames", "Avon", "Trent", "Wear", "Tees"],
    "NL": ["Delta", "Zuider", "Veluwe", "Twente", "Waal", "Maas", "Eems", "Gouda", "Breda", "Zwolle", "Emmen", "Venlo"],
    "CZ": ["Ostrava", "Brno", "Plzen", "Olomouc", "Liberec", "Zlin", "Jihlava", "Kladno", "Most", "Opava", "Tabor", "Cheb"],
    "FR": ["Lyon", "Rhone", "Loire", "Alsace", "Nantes", "Rouen", "Dijon", "Reims", "Tours", "Annecy", "Grenoble", "Vienne"],
}
CITIES = {
    "DE": ["Hamburg", "Bremen", "Duisburg", "Kassel", "Ulm"], "IT": ["Livorno", "Bologna", "Napoli", "Rimini"],
    "PL": ["Szczecin", "Wroclaw", "Katowice"], "ES": ["Tarragona", "Algeciras", "Santander"],
    "SE": ["Goteborg", "Norrkoping", "Helsingborg"], "GB": ["Felixstowe", "Leeds", "Hull"],
    "NL": ["Rotterdam", "Utrecht", "Tilburg"], "CZ": ["Pardubice", "Hradec", "Usti"],
    "FR": ["Marseille", "Le Havre", "Lille"],
}
MODES = ["truck", "sea", "rail", "air freight", "its own vans"]
EXTRAS = [
    "Minimum order {m} units.", "Certified to ISO 9001.", "Invoices in euro only.",
    "Closed in August.", "Prices fixed for {y} months at a time.", "Accepts returns within 30 days.",
    "Short of stock in winter.", "Delivers to the door.", "Offers a cash discount of 2 percent.",
    "Answers the phone in English and German.", "Quotes within two working days.",
]


def notes(country):
    t = [f"Ships from {rng.choice(CITIES[country])} by {rng.choice(MODES)}.",
         f"Lead time {rng.randint(1, 9)} weeks."]
    for e in rng.sample(EXTRAS, rng.randint(0, 3)):
        t.append(e.format(m=rng.choice([50, 100, 250, 500]), y=rng.choice([3, 6, 12])))
    return " ".join(t)


# (id, name, country, notes); the planted three named by walk 8's tasks.
PLANTED_SUPPLIERS = {
    "Torino Guarnizioni": ("IT", "Ships from Bologna by truck. Lead time 4 weeks. Certified to ISO 9001."),
    "Bellagio Valvole": ("IT", "Family firm on Lake Como; repairs and reconditions what it sells. Ships by truck. Lead time 3 weeks."),
    "Baltic Okucia": ("PL", "Ships by rail from Gdansk. Lead time 5 weeks. Minimum order 250 units."),
}
names = [f"{p} {t}" for t, (c, _) in TRADES.items() for p in PLACES[c]]
names = [n for n in names if n not in PLANTED_SUPPLIERS]
names = sorted(rng.sample(names, 300 - len(PLANTED_SUPPLIERS))) + list(PLANTED_SUPPLIERS)
rng.shuffle(names)
SUPPLIERS = []
for i, n in enumerate(names, 1):
    if n in PLANTED_SUPPLIERS:
        c, note = PLANTED_SUPPLIERS[n]
    else:
        c = TRADES[n.split()[-1]][0]
        note = notes(c)
    SUPPLIERS.append((f"S-{i}", n, c, note))
SUP_BY_NAME = {s[1]: s[0] for s in SUPPLIERS}
CATEGORY_OF = {s[0]: TRADES[s[1].split()[-1]][1] for s in SUPPLIERS}

# Products: a name of the category's kinds and sizes, a description of
# the category's sentences, one supplier of the category.
KINDS = {
    "valves": (["Ball", "Gate", "Check", "Butterfly", "Globe", "Needle", "Relief"], "valve", ["DN15", "DN20", "DN25", "DN32", "DN40", "DN50", "DN80"]),
    "pumps": (["Centrifugal", "Submersible", "Multistage", "Diaphragm", "Gear", "Booster", "Dosing"], "pump", ["0.37kW", "0.75kW", "1.5kW", "2.2kW", "4kW", "7.5kW"]),
    "seals": (["O-ring", "Shaft", "Lip", "Flange", "Mechanical", "Piston"], "seal", ["12mm", "20mm", "30mm", "45mm", "60mm"]),
    "bearings": (["Deep groove", "Angular contact", "Needle roller", "Tapered roller", "Thrust", "Self-aligning"], "bearing", ["6201", "6204", "6205", "6206", "6304", "6308"]),
    "fasteners": (["Hex", "Carriage", "Socket", "Flange", "Eye", "Anchor"], "bolt", ["M6", "M8", "M10", "M12", "M16", "M20"]),
    "fittings": (["Elbow", "Tee", "Reducer", "Union", "Nipple", "Coupling"], "fitting", ["1/2in", "3/4in", "1in", "2in"]),
    "hoses": (["Suction", "Air", "Hydraulic", "Garden", "Fuel", "Steam"], "hose", ["10m", "20m", "25m", "50m"]),
    "cables": (["Control", "Power", "Data", "Flexible", "Armoured", "Signal"], "cable", ["3x1.5", "3x2.5", "5x4", "2x0.75"]),
    "tools": (["Torque", "Pipe", "Socket", "Crimping", "Hex", "Strap"], "wrench", ["small", "medium", "large", "set"]),
}
SENTENCES = {
    "valves": ["A {k} {n} for water and oil lines.", "The body is cast brass.", "The body is ductile iron.", "Threaded ends.", "Flanged ends to EN 1092.", "Rated to 16 bar.", "Lever handle.", "Suits heating circuits.", "A {n} for compressed air."],
    "pumps": ["A {k} {n} for clean water.", "The {n} runs on 230 V.", "The {n} runs on 400 V.", "Cast iron housing.", "Stainless steel impeller.", "Quiet in use.", "A {n} for irrigation and garden use.", "Suits boosting the pressure in a building.", "Self-priming.", "Overload protection built in."],
    "seals": ["A {k} {n} of nitrile rubber.", "Of Viton for oils and fuels.", "Of EPDM for water.", "Sold in packs of ten.", "For rotating shafts.", "Holds up to 120 degrees.", "A {n} for hydraulic cylinders."],
    "bearings": ["A {k} {n}, sealed both sides.", "Open, to be greased.", "Chrome steel rings.", "For electric motors.", "For conveyor rollers.", "Low noise.", "A {n} for gearboxes."],
    "fasteners": ["A {k} {n} of zinc-plated steel.", "Of stainless steel A2.", "Of stainless steel A4.", "Strength class 8.8.", "Sold in boxes of 100.", "Hot-dip galvanised.", "For timber and steel."],
    "fittings": ["A {k} {n} of brass.", "Of galvanised malleable iron.", "Press fit.", "Threaded BSP.", "For gas and water.", "Sold singly."],
    "hoses": ["A {k} {n} of reinforced PVC.", "Of rubber with a textile ply.", "With couplings fitted.", "Rated to 10 bar.", "Stays flexible in frost.", "A {n} for workshops."],
    "cables": ["A {k} {n} with copper cores.", "PVC sheath.", "Halogen-free sheath.", "Sold by the drum.", "For fixed installation.", "Oil-resistant jacket."],
    "tools": ["A {k} {n} of chrome vanadium steel.", "In a plastic case.", "Comes with a lifetime warranty.", "For the workshop and site.", "Ratchet head.", "Insulated handles."],
}


def description(cat, kind, noun):
    pool = SENTENCES[cat]
    first = [s for s in pool if "{k}" in s]
    rest = [s for s in pool if "{k}" not in s]
    t = [first[0]] + rng.sample(rest, rng.randint(0, min(5, len(rest))))
    return " ".join(s.format(k=kind.lower(), n=noun) for s in t)


by_cat = {}
for s in SUPPLIERS:
    by_cat.setdefault(CATEGORY_OF[s[0]], []).append(s[0])
PRODUCTS = []  # (sku, name, description, category, unit_price, supplier)
for i in range(2000):
    cat = rng.choice(sorted(KINDS))
    kinds, noun, sizes = KINDS[cat]
    kind = rng.choice(kinds)
    PRODUCTS.append([None, f"{kind} {noun} {rng.choice(sizes)}", description(cat, kind, noun),
                     cat, f"{rng.randint(50, 90000) / 100:.2f}", rng.choice(by_cat[cat])])

# The planted products, each replacing one drawn product of its category
# so the count stays 2 000. A seawater pump the most relevant hit, a long
# pump text that names seawater once, a cable that names it once; the
# cellar pump and the brewery valve found by their descriptions alone.
PLANTED_PRODUCTS = {
    "seawater": ("Seawater lift pump 1.5kW", "Bronze pump for seawater.", "pumps", "1480.00", None),
    "multistage": ("Multistage pump 4kW",
                   "A multistage pump for boosting the pressure in tall buildings. The pump is "
                   "quiet in use and the pump housing is cast iron. It suits clean water and "
                   "rainwater; with the bronze impeller option the pump also handles seawater. "
                   "Every pump is tested at the works before it ships, and a spare seal kit for "
                   "the pump is sold separately.", "pumps", "2310.00", None),
    "cable": ("Flexible cable 3x2.5", "A tinned copper cable for boats; its jacket resists seawater and diesel.", "cables", "3.90", None),
    "cellar": ("Submersible pump 0.75kW", "Sump pump for draining flooded cellars; float switch included.", "pumps", "189.00", None),
    "brewery": ("Butterfly valve DN50", "Sanitary butterfly valve for breweries and dairies; stainless steel, clamp ends.", "valves", "264.00", None),
}
slots = rng.sample(range(2000), len(PLANTED_PRODUCTS))
for (tag, (n, d, cat, u, _)), at in zip(PLANTED_PRODUCTS.items(), slots):
    PRODUCTS[at] = [tag, n, d, cat, u, rng.choice(by_cat[cat])]
# Bellagio Valvole and Baltic Okucia make a known number of products each.
for name, k in (("Bellagio Valvole", 6), ("Baltic Okucia", 8), ("Torino Guarnizioni", 7)):
    sid, cat = SUP_BY_NAME[name], CATEGORY_OF[SUP_BY_NAME[name]]
    for p in PRODUCTS:
        if p[5] == sid:
            p[5] = rng.choice([s for s in by_cat[cat] if s != sid])
    for at in rng.sample([i for i, p in enumerate(PRODUCTS) if p[3] == cat and p[0] is None], k):
        PRODUCTS[at][5] = sid
PLANTED_SKU = {}
for i, p in enumerate(PRODUCTS):
    sku = f"P-{1000 + i}"
    if p[0]:
        PLANTED_SKU[p[0]] = sku
    p[0] = sku
PRODUCTS = [tuple(p) for p in PRODUCTS]

# Customers: a word and a trade, some with a second word; many share a
# word and a few a whole name. Kestrel Farms is the one Kestrel; three
# customers are called Atlas Supplies, beside other Atlas names.
WORDS = ("Alder Amber Anchor Apex Arrow Ash Aspen Aurora Badger Bay Beacon Bear Beech Birch Bluebell Bolt Bramble Bridge "
         "Bright Brook Buzzard Canal Castle Cedar Central Chalk Cherry Cliff Clover Coast Cobalt Comet Copper Coral Crane "
         "Crown Crystal Curlew Cypress Dale Dawn Deer Delta Dune Eagle East Echo Elm Ember Falcon Fern Field Finch Fir "
         "Flint Forest Fox Frost Garnet Glen Gold Granite Green Grove Gull Harbor Harvest Hawk Hazel Heath Heron Hill "
         "Holly Horizon Hunter Iris Iron Ivy Jade Juniper Keel Lake Larch Lark Laurel Lime Linden Lion Lotus Lynx Maple "
         "Marsh Meadow Merlin Mill Mint Moor Moss North Oak Ocean Olive Onyx Orchard Osprey Otter Owl Palm Pearl Pebble "
         "Pine Plover Polar Poplar Prairie Quarry Rain Raven Red Reed Ridge River Robin Rock Rose Rowan Ruby Sage Salmon "
         "Sand Shore Silver Sky Slate Snow Sorrel South Spring Spruce Star Steel Stone Storm Summit Sun Swallow Swan "
         "Teal Thistle Thorn Tide Tiger Timber Union Vale Valley Viking West Willow Wind Wolf Wren Yew").split()
SUFFIXES = ["Farms", "Supplies", "Metals", "Trading", "Hardware", "Utilities", "Machining", "Logistics",
            "Foods", "Engineering", "Builders", "Aerospace", "Plastics", "Textiles", "Motors", "Water"]
COUNTRIES = ["DE", "NL", "GB", "IE", "CZ", "PT", "FR", "PL", "AT", "DK", "IT", "ES", "SE"]
SEGMENTS = ["industry", "retail", "utility", "wholesale"]
PLANTED_CUSTOMERS = [
    ("kestrel", "Kestrel Farms", "IE", "retail"),
    ("atlas-nl", "Atlas Supplies", "NL", "wholesale"),
    ("atlas-pt", "Atlas Supplies", "PT", "retail"),
    ("atlas-gb", "Atlas Supplies", "GB", "industry"),
    ("atlas-metals", "Atlas Metals", "PT", "industry"),
    ("atlas-hardware", "Atlas Hardware", "DE", "retail"),
]
CUSTOMERS = []
for _ in range(3000 - len(PLANTED_CUSTOMERS)):
    w = rng.choice(WORDS)
    if rng.random() < 0.3:
        w = f"{w} {rng.choice(WORDS)}"
    CUSTOMERS.append([None, f"{w} {rng.choice(SUFFIXES)}", rng.choice(COUNTRIES), rng.choice(SEGMENTS)])
for p in PLANTED_CUSTOMERS:
    CUSTOMERS.insert(rng.randint(0, len(CUSTOMERS)), list(p))
PLANTED_CUST = {}
for i, c in enumerate(CUSTOMERS, 1):
    if c[0]:
        PLANTED_CUST[c[0]] = f"C-{i}"
    c[0] = f"C-{i}"
CUSTOMERS = [tuple(c) for c in CUSTOMERS]

# Orders: the planted customers' own orders, written here, and the rest
# drawn; no drawn order is a planted customer's.
day0 = date(2026, 1, 5)
P = PLANTED_SKU
drawn = [p[0] for p in PRODUCTS]
PLANTED_ORDERS = [
    ("kestrel", "2026-02-11", "shipped", [(drawn[17], 40), (drawn[903], 12)]),
    ("kestrel", "2026-04-02", "shipped", [(drawn[244], 6)]),
    ("kestrel", "2026-06-23", "cancelled", [(drawn[1500], 30), (drawn[61], 2)]),
    ("kestrel", "2026-08-30", "open", [(drawn[1222], 15), (drawn[17], 25), (drawn[388], 4)]),
    ("kestrel", "2026-10-08", "open", [(drawn[702], 9)]),
    ("atlas-pt", "2026-03-04", "shipped", [(drawn[512], 30), (drawn[1300], 18)]),
    ("atlas-pt", "2026-05-19", "shipped", [(drawn[512], 25), (drawn[77], 40)]),
    ("atlas-pt", "2026-07-07", "cancelled", [(drawn[1650], 120)]),
    ("atlas-pt", "2026-09-15", "open", [(drawn[77], 20), (drawn[1300], 10), (drawn[808], 5)]),
    ("atlas-nl", "2026-04-28", "shipped", [(drawn[1650], 16)]),
    ("atlas-nl", "2026-09-30", "open", [(drawn[512], 8), (drawn[44], 3)]),
    ("atlas-gb", "2026-06-12", "shipped", [(drawn[1999], 60)]),
    ("atlas-gb", "2026-10-15", "cancelled", [(drawn[400], 7)]),
    ("atlas-metals", "2026-08-20", "shipped", [(drawn[512], 90)]),
    ("atlas-hardware", "2026-09-01", "open", [(drawn[1300], 11)]),
]
planted_keys = set(PLANTED_CUST.values())
others = [c[0] for c in CUSTOMERS if c[0] not in planted_keys]
raw = [(PLANTED_CUST[t], d, s, ls) for t, d, s, ls in PLANTED_ORDERS]
while sum(len(o[3]) for o in raw) < 21000:
    c = rng.choice(others)
    st = rng.choices(["shipped", "open", "cancelled"], [6, 3, 1])[0]
    ls = [(p, rng.randint(1, 80)) for p in rng.sample(drawn, rng.randint(1, 6))]
    raw.append((c, (day0 + timedelta(days=rng.randint(0, 290))).isoformat(), st, ls))
rng.shuffle(raw)
ORDERS = [(f"O-{10001 + i}", c, d, s, ls) for i, (c, d, s, ls) in enumerate(raw)]

price = {p[0]: Decimal(p[4]) for p in PRODUCTS}
lines = []
for o, _, _, _, ls in ORDERS:
    for i, (p, q) in enumerate(ls, 1):
        lines.append((f"L-{o[2:]}-{i}", o, p, q))
total = {o[0]: sum(price[p] * q for p, q in o[4]) for o in ORDERS}


def check():
    """What the tasks lean on holds of the drawn data, or the generator fails."""
    import re
    def words(*texts):
        return [w.lower() for t in texts for w in re.findall(r"\w+", t)]
    def holding(rows, word, *cols):
        return [r[0] for r in rows if word in words(*(r[c] for c in cols))]
    assert holding(PRODUCTS, "seawater", 1, 2) == sorted(P[t] for t in ("seawater", "multistage", "cable")), "seawater"
    for w, t in (("flooded", "cellar"), ("cellars", "cellar"), ("breweries", "brewery"), ("dairies", "brewery")):
        assert holding(PRODUCTS, w, 1, 2) == [P[t]], w
    for w, n in (("como", "Bellagio Valvole"), ("gdansk", "Baltic Okucia")):
        assert holding(SUPPLIERS, w, 1, 3) == [SUP_BY_NAME[n]], w
    assert holding(SUPPLIERS, "guarnizoni", 1, 3) == [] and holding(CUSTOMERS, "kestral", 1) == []
    names = [c[1].lower() for c in CUSTOMERS]
    assert [n for n in names if "kes" in n] == ["kestrel farms"], "Kestrel is the one"
    assert sum(n == "atlas supplies" for n in names) == 3
    assert not any("quil" in n or "illon" in n for n in names), "no customer is like Quillon"
    assert sum("aerospace" in n for n in names) > 50, "Aerospace is a common word"
    assert len(SUPPLIERS) == 300 and len(PRODUCTS) == 2000 and len(CUSTOMERS) == 3000
    assert len(lines) >= 20000


def write(out):
    rows = {
        "suppliers": [{"type": "supplier", "key": s, "fields": {"id": s, "name": n, "country": c, "notes": t}} for s, n, c, t in SUPPLIERS],
        "products": [{"type": "product", "key": k, "fields": {"sku": k, "name": n, "description": d, "category": c, "unit_price": u, "supplier": s}} for k, n, d, c, u, s in PRODUCTS],
        "customers": [{"type": "customer", "key": k, "fields": {"id": k, "name": n, "country": c, "segment": g}} for k, n, c, g in CUSTOMERS],
        "orders": [{"type": "order", "key": o, "fields": {"id": o, "customer": c, "placed_on": d, "total": f"{total[o]:.2f}", "status": s}} for o, c, d, s, _ in ORDERS],
        "order_lines": [{"type": "order_line", "key": k, "fields": {"id": k, "order": o, "product": p, "qty": q}} for k, o, p, q in lines],
    }
    for name, rs in rows.items():
        (out / f"{name}.ndjson").write_text("".join(json.dumps(r) + "\n" for r in rs))


check()
if __name__ == "__main__":
    if sys.argv[1:2] == ["write"]:
        write(Path(sys.argv[2]))
    else:
        n = len(SUPPLIERS) + len(PRODUCTS) + len(CUSTOMERS) + len(ORDERS) + len(lines)
        print(f"records: {len(SUPPLIERS)} suppliers, {len(PRODUCTS)} products, {len(CUSTOMERS)} customers, "
              f"{len(ORDERS)} orders, {len(lines)} order lines; {n} in all")
