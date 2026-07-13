"""SDF glyph baker for TMP (matches NTR Soccer's PlaypenSans atlas encoding).
Calibrated: pointSize=90, edge=128, slope≈13 alpha/px (+ inside), tight ink-bbox rect."""
import numpy as np
import freetype
from scipy import ndimage

SLOPE = 13.0          # alpha units per atlas-pixel of signed distance
EDGE = 128.0
POINT_SIZE = 90       # m_FaceInfo.m_PointSize
OVERSAMPLE = 4        # render hi-res then downscale for smoother SDF
MARGIN = 6            # px of falloff room around the ink (atlas-res)

def sdf_glyph(face, unicode):
    """Return (alpha_u8 HxW, metrics dict, rect WxH) for a glyph, or None if empty.
    metrics in px at POINT_SIZE: width,height,bearingX,bearingY,advance."""
    face.set_pixel_sizes(0, POINT_SIZE * OVERSAMPLE)
    face.load_char(unicode, freetype.FT_LOAD_RENDER | freetype.FT_LOAD_TARGET_NORMAL)
    g = face.glyph
    bm = g.bitmap
    adv = g.advance.x / 64.0 / OVERSAMPLE     # advance px @ POINT_SIZE
    if bm.width == 0 or bm.rows == 0:
        return None, {"width": 0.0, "height": 0.0, "bearingX": 0.0,
                      "bearingY": 0.0, "advance": adv}, (0, 0)
    hi = np.array(bm.buffer, dtype=np.uint8).reshape(bm.rows, bm.width)
    # binary mask at hi-res, pad with MARGIN*OVERSAMPLE for falloff room
    pad = MARGIN * OVERSAMPLE
    mask = np.zeros((bm.rows + 2 * pad, bm.width + 2 * pad), dtype=bool)
    mask[pad:pad + bm.rows, pad:pad + bm.width] = hi >= 128
    inside = ndimage.distance_transform_edt(mask)
    outside = ndimage.distance_transform_edt(~mask)
    signed_hi = inside - outside            # + inside, hi-res px
    # downscale to atlas res (divide distance by OVERSAMPLE)
    signed = _block_mean(signed_hi, OVERSAMPLE) / OVERSAMPLE
    alpha = np.clip(EDGE + SLOPE * signed, 0, 255).astype(np.uint8)
    h, w = alpha.shape
    # metrics @ POINT_SIZE (freetype values are @ OVERSAMPLE res -> divide)
    metrics = {
        "width": bm.width / OVERSAMPLE,
        "height": bm.rows / OVERSAMPLE,
        "bearingX": g.bitmap_left / OVERSAMPLE - MARGIN,   # rect starts MARGIN left of ink
        "bearingY": g.bitmap_top / OVERSAMPLE + MARGIN,    # rect top MARGIN above ink
        "advance": adv,
    }
    return alpha, metrics, (w, h)

def _block_mean(a, k):
    h, w = a.shape
    h2, w2 = (h // k) * k, (w // k) * k
    a = a[:h2, :w2].reshape(h2 // k, k, w2 // k, k)
    return a.mean(axis=(1, 3))
