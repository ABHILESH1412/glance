# Test images

## `raw/`

Camera raw samples from [revelraw-sample-raw-files](https://github.com/tellodaniel/revelraw-sample-raw-files),
released under CC0. `SHA256SUMS.txt` is the upstream manifest; verify with
`sha256sum -c SHA256SUMS.txt --ignore-missing`.

These four were chosen because they exercise genuinely different paths:

| File | Embedded preview | Sensor | What it exercises |
|---|---|---|---|
| `Canon-EOS-R.CR3` | 6720x4480 | 6720x4480 | full-resolution preview, used as-is |
| `Sony-a7III.ARW` | 1616x1080 | 6000x4000 | preview just over the usable threshold |
| `DJI-Mavic-3-Pro.DNG` | 960x720 | 4024x3016 | preview too small, forces a develop |
| `Canon-EOS-D60.CRW` | none | 3088x2056 | no preview at all, develop is the only option |

## `generated/`

Synthetic files, reproducible and free of anyone's personal data.

- `exif-rotated.jpg` — stored 750x500 landscape with EXIF orientation 6. A correct
  viewer shows it as 500x750 portrait, red band at the top, arrow pointing up.
  `exif-expected.png` is what that should look like.
- `large-gradient.png` — 3200x2000, checks scale-to-fit keeps aspect ratio.
- `small-64.png` — 64x64, must NOT be upscaled by the default fit.
- `test.avif` / `test-vector.svg` — the libheif and resvg paths. The SVG has a
  transparent background on purpose: it catches premultiplied-alpha mistakes,
  which show up as dark halos on the anti-aliased edges.
- `broken.png` — PNG magic bytes followed by garbage; exercises the error toast.

Note: raw decoding is far too slow in a debug build. Use `--profile quick`.
