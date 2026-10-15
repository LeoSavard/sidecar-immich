# Film data

Curves traced from film makers' datasheets, and the scripts that traced them. The datasheets themselves are the
makers'; they are not kept here. Fetch the PDF, then run the scripts from the stock's directory (Python 3 with
Pillow; `pdfimages` from poppler).

## ILFORD HP5 Plus (`hp5-plus/`)

Source: HARMAN technology, *ILFORD HP5 Plus technical information*, Nov 2018 —
<https://www.ilfordphoto.com/amfile/file/download/file/1903/product/691/>.

```bash
pdfimages -f 1 -l 1 -j hp5plus.pdf p1   # p1-002.jpg: wedge spectrogram to tungsten light (2,850 K)
pdfimages -f 5 -l 5 -j hp5plus.pdf p5   # p5-000.jpg: characteristic curve, ILFOTEC HC 1+31, 6½ min, 20 °C
python3 probe.py      # gridlines and ticks, for the calibration in trace.py
python3 trace.py > traced.txt
python3 resample.py   # 10 nm spectrogram and 0.1 log E curve, and the toe's fit
```

The results are in `crates/color/src/spectral_data.rs` (`HP5_PLUS_SPECTROGRAM`) and
`crates/color/src/stock.rs` (`HP5_PLUS_CURVE`, and the fit). How they are read is in `docs/DESIGN.md` §7,
"Stocks".
