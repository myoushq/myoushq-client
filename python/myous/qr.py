"""QR codes for pairing links: SVG for chat apps (works wherever images do
and scales cleanly), text for terminals. Generating them needs only the
pure-Python `qrcode` package; no image library is involved.

Reading a QR code from a photo does need an image decoder, which is attack
surface, so it's a separate optional extra (`myous[qr-read]`, OpenCV).
"""
from __future__ import annotations

import io
import os
import sys

RELEASES = "https://github.com/myoushq/myoushq-client/releases/download"


def install_hint() -> str:
    """The hash-checked way to add QR images to this install: the
    release's lock with `qrcode` added (requirements-qr-python.lock),
    installed into the Python running this client."""
    from myous import __version__

    lock = f"{RELEASES}/v{__version__}/requirements-qr-python.lock"
    python = f'& "{sys.executable}"' if os.name == "nt" else f'"{sys.executable}"'
    return f"{python} -m pip install --require-hashes -r {lock}"


def _code(data: str):
    import qrcode

    qr = qrcode.QRCode(error_correction=qrcode.constants.ERROR_CORRECT_M, border=4)
    qr.add_data(data)
    qr.make(fit=True)
    return qr


def write_svg(data: str, path: str) -> None:
    from qrcode.image.svg import SvgPathImage

    _code(data).make_image(image_factory=SvgPathImage).save(path)


def as_text(data: str) -> str:
    out = io.StringIO()
    _code(data).print_ascii(out=out, invert=True)
    return out.getvalue()


def read_image(path: str) -> str:
    """Decode a QR code from a photo or screenshot."""
    try:
        import cv2
    except ImportError:
        raise RuntimeError("reading QR images needs OpenCV: pip install 'myous[qr-read]'") from None
    image = cv2.imread(path)
    if image is None:
        raise ValueError(f"can't read image {path}")
    data, _, _ = cv2.QRCodeDetector().detectAndDecode(image)
    if not data:
        raise ValueError(f"no QR code found in {path}")
    return data
