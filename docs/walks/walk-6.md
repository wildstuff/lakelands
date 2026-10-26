# Walk 6: an agent's graph tasks through the typed query tool and through Cypher

## The tasks

| # | The question, as the prompt puts it | What it covers | Reference answer |
|---|---|---|---|
| 1 | Which orders has the customer Harbor Supplies placed? Give each order's number, date, total and status. | neighbours | O-1002, 2026-08-11, 372.00, shipped; O-1006, 2026-09-09, 142.50, open; O-1011, 2026-10-13, 144.00, cancelled |
| 2 | Which products are on order O-1007, and how many units of each? | two hops | Check valve DN25 (P-21) × 2; O-ring set (P-40) × 8 |
| 3 | Which suppliers make the products on order O-1004, and in which countries are they? | three hops | Nordic Fasteners (S-1), SE; Lago Valvole (S-2), IT; Torino Guarnizioni (S-4), IT |
| 4 | Which open orders contain at least one valve (a product in the category `valves`)? | a filter on a related record | O-1004, O-1007, O-1009 |
| 5 | For each country, how many orders have its customers placed, and what do those orders come to in total? Count every order, whatever its status. | a count and a sum, grouped by a related record's field | AT 26, 146596.80; CZ 106, 675799.74; DE 186, 936721.36; DK 213, 1196491.38; FR 117, 661889.91; GB 173, 905960.42; IE 159, 820741.12; NL 119, 658280.78; PL 251, 1355493.77; PT 150, 833752.68 |
| 6 | Leaving out cancelled orders, how many units of each product category have been ordered? | a group over two steps | bearings 28842, cables 27468, fasteners 30371, fittings 26778, pumps 14, seals 25422, tools 27576, valves 25 |
| 7 | Rank all order lines by quantity, highest first, and give me ranks 6 to 10: each line's id, its order, its product and its quantity. Read the ranking from the store at most five lines at a time. | ordering, and paging past the first page | 6. L-1012-2, O-1012, P-12, 300; 7. L-1006-2, O-1006, P-11, 250; 8. L-1006-1, O-1006, P-10, 200; 9. L-1004-4, O-1004, P-10, 150; 10. L-1012-1, O-1012, P-10, 90 |
| 8 | Kestrel Farms and Ostrava Machining: how are they connected in the data? Give the shortest chain of records that links them. | a path, which the subset cannot spell | the one chain of 6 links: C-4 ← O-1012 ← L-1012-2 → P-12 (Washer M8) ← L-1008-1 → O-1008 → C-5; both bought washers |
| 9 | Which products have never been ordered? | a step that must not match | P-13 Threaded rod M8, P-42 Shaft seal 30mm, P-167, P-168, P-169 |
| 10 | Which customers in Germany have bought at least one product made in Italy? | four hops, a filter at each end, the same customer on many rows | C-1 Delta Metals, C-8, C-20, C-39, C-40, C-53, C-55, C-59 (135 rows before they are told apart) |

The reference answers were fixed before any run: `lake/make_data.py`
prints them from the lists it writes the data from, not from the lake.
