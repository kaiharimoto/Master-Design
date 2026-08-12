# md-fonts

The typeface Master Design ships with, in the one form that serves both of the jobs it
has to do.

## Why these files are here at all

A design tool has to answer "how wide is this line of text?" the same way twice: once in
the editor, where selection handles and layout depend on it, and once in the exported
page, where a browser answers it. If the two use different fonts, they disagree — text
overflows a box it fitted in the editor, an animation that slid a heading exactly off
screen leaves a sliver behind.

Relying on the system's fonts makes that disagreement the normal case. Android has no
Inter. A CI runner has whatever the base image happened to install. So the tool carries
its own.

## Why `.woff2`, when a measurement engine cannot read one

It can, in a sense. `woff2` is a Brotli-compressed, table-transformed sfnt, and
`md-text` decompresses each file back to a real TrueType face at load time — see
`crates/md-text/src/lib.rs`. That means **one file serves both purposes**: it is measured
in the editor, and it is copied byte-for-byte into `dist/fonts/` for the browser, where
it is exactly the format the browser wants.

The alternative was carrying a `.otf` for measurement *and* a `.woff2` for the web, which
is two files that can drift apart, or carrying only `.otf` and shipping 600 kB per weight
to every visitor. These are the Latin subsets, so the whole family is 216 kB.

## What is here

Inter, six upright weights (300–800) and two italics (400, 700), Latin subset. Enough
that a designer setting Light through ExtraBold gets a real face rather than a synthesised
one, without carrying nine weights of a script most documents will not use.

A document asking for a weight that is not here resolves to the nearest one, the way CSS
does — `md-text` reports which face it actually used, and the exporter turns a substitution
into a warning rather than letting it pass silently.

## Provenance and licence

Inter by Rasmus Andersson, via [Fontsource](https://fontsource.org/fonts/inter) 5.3.0,
under the SIL Open Font License 1.1. The full licence is in `LICENSE-OFL.txt` and must
travel with these files, including into any exported site that embeds them.

To refresh:

```sh
npm pack @fontsource/inter@5
# files/inter-latin-<weight>-<style>.woff2  →  inter/Inter-<weight>[-italic].woff2
```
