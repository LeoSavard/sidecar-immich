# Film data

Curves read from film makers' datasheets, and the scripts that read them. The datasheets themselves are the
makers'; they are not kept here. Fetch the PDFs, extract their graphs, then run the scripts from a directory
holding them (Python 3 with Pillow; `pdfimages` and `pdftocairo` from poppler). The results are in
`crates/color/src/film_data.rs`; how they are read is in `docs/DESIGN.md` §7, "Stocks".

| Stock | Sheet | Graphs |
|---|---|---|
| ILFORD HP5 Plus | HARMAN, *HP5 Plus technical information*, Nov 2018 — <https://www.ilfordphoto.com/amfile/file/download/file/1903/product/691/> | p. 1 spectrogram, p. 5 curve (images) |
| ILFORD SFX 200 | HARMAN, *SFX 200 technical information*, Nov 2018 | p. 1 spectrogram, p. 2 curve (images) |
| KODAK TRI-X 320 and 400 | Kodak Alaris, *KODAK PROFESSIONAL TRI-X 320 and 400 Films*, F-4017, Feb 2016 | pp. 7–9 (vector paths) |
| EASTMAN DOUBLE-X 5222 | Kodak, *EASTMAN DOUBLE-X Negative Film 5222 / 7222*, H-1-5222, March 2022 | p. 3 (images) |

```bash
# HP5 Plus (hp5-plus/)
pdfimages -f 1 -l 1 -j hp5plus.pdf p1      # p1-002.jpg: spectrogram
pdfimages -f 5 -l 5 -j hp5plus.pdf p5      # p5-000.jpg: characteristic curve
python3 hp5-plus/probe.py && python3 hp5-plus/trace.py > traced.txt

# Kodak and SFX 200 (sheets/)
pdftocairo -svg -f 7 -l 9 trix.pdf trix-p7.svg   # one page at a time: -f 7 -l 7, -f 8 -l 8, -f 9 -l 9
python3 sheets/kodak.py > kodak.txt              # vector paths; sheets/vectors.py lists them
pdfimages -f 3 -l 3 -png doublex.pdf dx && python3 sheets/doublex.py > doublex.txt
pdfimages -f 1 -l 2 -j sfx200.pdf sfx && magick sfx-002.ppm sfx-002.png
python3 sheets/sfx.py > sfx.txt && python3 sheets/sfx_curve.py > sfx_curve.txt
python3 sheets/fit.py      # each curve's toe, straight line and shoulder, as stock.rs takes them
python3 sheets/emit.py     # the Rust tables
```

Each script's header says how its graph was calibrated (gridlines, ticks or the vector frame). Where a trace
needed a correction by eye — Double-X's spectral curve at 660 nm, where it crosses the other curve — `emit.py`
says so.
