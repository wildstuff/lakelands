# Brand

Lakelands' visual identity as files: the tokens every figure, page and
post takes its colours and type from, a stylesheet generated from them,
and the mark.

| File | What |
|---|---|
| `tokens.json` | Colours in two themes — **Depth** (`dark`), the brand's home, and **Sand** (`light`), for pages on a light ground — the type families and styles, spacing and radii. |
| `tokens.css` | Generated from `tokens.json`: one custom property per token, `[data-theme="light"]` for what Sand changes, one class per type style. Never edit it by hand. |
| `logos/perch-mark.svg` | The perch on a sand disc, for dark grounds. From 48 px. |
| `logos/perch-mark-on-light.svg` | The perch on a depth disc, for light grounds. From 48 px. |
| `logos/perch-favicon.svg` | Simplified for 16–32 px. |
| `logos/perch-figure.svg` | The perch alone, facing right, for scenes. |

## Changing a token

`tokens.json` is exported from the design system the brand is kept in;
change it there and export it again rather than editing it here. Then
regenerate the stylesheet:

```sh
perl scripts/brand/tokens-css.pl brand/tokens.json > brand/tokens.css
```

The `brand-tokens` gate turns red when `tokens.css` is not what the
generator makes from `tokens.json`.

## Using it

- Take every colour from a token, by name. Copying a hex value into a
  figure makes a second copy nothing holds to this one.
- Depth is the default. A page on a light ground sets
  `data-theme="light"` on its root.
- The fonts are Red Hat Display and Red Hat Mono, from Google Fonts.
- The wordmark is set, not drawn: **lakelands**, Red Hat Display 800,
  lower case, beside the mark.

## The name and the mark

The Apache License covers the code and the on-disk format. It grants no
right to use the name **Lakelands** or the perch mark beyond describing
where the work comes from (section 6 of the licence). Use them to point
at this project; do not use them in a way that suggests a fork, a
product or a service is this project or is endorsed by it.
