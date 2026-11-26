# PaperView protocol fixtures

The `*.ppfb.hex` files are complete, reviewable PPFB v2 Frame messages. Tests
strip comments and ASCII whitespace before decoding them. They pin protocol
headers, frame metadata, Mono1 polarity and row padding, and Gray8 luminance.

`now-playing-viewport.txt` is the shared expected Paperwhite 6 development
viewport used by PaperView configuration tests and PaperSpoon's deterministic
Now Playing renderer test. It contains geometry only. Personal media metadata,
artwork, screenshots, and live Now Playing pixels do not belong here.

The fixture frames are synthetic:

- `mono1-black-white-8x2.ppfb.hex`: black and white polarity endpoints.
- `mono1-odd-width-10x2.ppfb.hex`: MSB-first pixels with per-row padding.
- `gray8-gradient-8x2.ppfb.hex`: exact `0x00` through `0xff` luminance steps.
