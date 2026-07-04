//! Text-file encoding layer for the KiriKiri engine.
//!
//! Unlike TyranoScript (`.ks` are UTF-8) and Ren'Py (`.rpy` are UTF-8), KiriKiri
//! scenario scripts ship in several encodings — most often **Shift-JIS** or
//! **UTF-16LE with a BOM**, sometimes UTF-8. The engine decodes each file to a
//! UTF-8 [`String`] so the shared KAG parser (`engine::tyrano::extract_ks`) can
//! work on it, then re-encodes on inject.
//!
//! Only *stateless* encodings are handled (UTF-8, UTF-16LE/BE, Shift-JIS): each
//! character encodes independently of its neighbours, so re-encoding the whole
//! file is byte-equivalent to splicing per span, and `encode(decode(bytes))`
//! reproduces the original bytes. That is what keeps round-trip identity
//! (`translation == source`) byte-exact.
//!
//! The one wrinkle KiriKiri adds over the UTF-8 engines: a translation may use
//! characters the source encoding cannot represent (e.g. Thai in a Shift-JIS
//! game). When that happens [`encode`] falls back to UTF-16LE, which KiriKiri
//! reads natively via the BOM — so the exported script stays loadable even
//! though it is no longer byte-identical (which is expected: the text changed).

/// A text encoding we can round-trip a KiriKiri `.ks` file through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enc {
    /// UTF-8, no byte-order mark.
    Utf8,
    /// UTF-8 with a leading `EF BB BF` BOM.
    Utf8Bom,
    /// UTF-16 little-endian with a `FF FE` BOM.
    Utf16Le,
    /// UTF-16 big-endian with a `FE FF` BOM.
    Utf16Be,
    /// Shift-JIS (Japanese legacy encoding; no BOM).
    ShiftJis,
}

const BOM_UTF8: [u8; 3] = [0xEF, 0xBB, 0xBF];
const BOM_UTF16LE: [u8; 2] = [0xFF, 0xFE];
const BOM_UTF16BE: [u8; 2] = [0xFE, 0xFF];

/// Sniff the encoding of a `.ks` file from its BOM, falling back to a UTF-8
/// validity check (valid UTF-8, including pure ASCII, is treated as UTF-8;
/// anything else is assumed Shift-JIS, the common KiriKiri default). UTF-16
/// without a BOM is not recognised — KiriKiri UTF-16 scripts carry one.
pub fn detect(bytes: &[u8]) -> Enc {
    if bytes.starts_with(&BOM_UTF8) {
        Enc::Utf8Bom
    } else if bytes.starts_with(&BOM_UTF16LE) {
        Enc::Utf16Le
    } else if bytes.starts_with(&BOM_UTF16BE) {
        Enc::Utf16Be
    } else if std::str::from_utf8(bytes).is_ok() {
        Enc::Utf8
    } else {
        Enc::ShiftJis
    }
}

/// Decode raw file bytes into a UTF-8 string, stripping any BOM.
pub fn decode(bytes: &[u8], enc: Enc) -> String {
    match enc {
        Enc::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Enc::Utf8Bom => String::from_utf8_lossy(&bytes[BOM_UTF8.len()..]).into_owned(),
        Enc::Utf16Le => decode_utf16(&bytes[BOM_UTF16LE.len()..], true),
        Enc::Utf16Be => decode_utf16(&bytes[BOM_UTF16BE.len()..], false),
        Enc::ShiftJis => encoding_rs::SHIFT_JIS
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

/// Encode a UTF-8 string back into `enc`, re-adding the BOM. If `text` contains
/// characters `enc` cannot represent (only possible for Shift-JIS), the file is
/// emitted as UTF-16LE instead, which KiriKiri loads natively.
pub fn encode(text: &str, enc: Enc) -> Vec<u8> {
    match enc {
        Enc::Utf8 => text.as_bytes().to_vec(),
        Enc::Utf8Bom => {
            let mut out = BOM_UTF8.to_vec();
            out.extend_from_slice(text.as_bytes());
            out
        }
        Enc::Utf16Le => {
            let mut out = BOM_UTF16LE.to_vec();
            out.extend(encode_utf16(text, true));
            out
        }
        Enc::Utf16Be => {
            let mut out = BOM_UTF16BE.to_vec();
            out.extend(encode_utf16(text, false));
            out
        }
        Enc::ShiftJis => {
            let (cow, _, had_unmappable) = encoding_rs::SHIFT_JIS.encode(text);
            if had_unmappable {
                // The translation can't be written as Shift-JIS — fall back to
                // UTF-16LE so the exported script is still loadable.
                encode(text, Enc::Utf16Le)
            } else {
                cow.into_owned()
            }
        }
    }
}

/// Decode BOM-stripped UTF-16 bytes (endianness per `le`) into a UTF-8 string.
fn decode_utf16(bytes: &[u8], le: bool) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        let pair = [bytes[i], bytes[i + 1]];
        units.push(if le {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        });
        i += 2;
    }
    String::from_utf16_lossy(&units)
}

/// Encode a UTF-8 string as raw UTF-16 code units (no BOM), endianness per `le`.
/// This is the exact inverse of [`decode_utf16`] for well-formed text, which is
/// what keeps the round-trip byte-identical.
fn encode_utf16(text: &str, le: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for u in text.encode_utf16() {
        let b = if le {
            u.to_le_bytes()
        } else {
            u.to_be_bytes()
        };
        out.extend_from_slice(&b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le_bytes(s: &str) -> Vec<u8> {
        let mut v = BOM_UTF16LE.to_vec();
        v.extend(encode_utf16(s, true));
        v
    }

    #[test]
    fn detect_reads_boms_and_falls_back() {
        assert_eq!(detect("plain ascii".as_bytes()), Enc::Utf8);
        assert_eq!(detect("日本語 utf8".as_bytes()), Enc::Utf8);

        let mut with_bom = BOM_UTF8.to_vec();
        with_bom.extend_from_slice("hi".as_bytes());
        assert_eq!(detect(&with_bom), Enc::Utf8Bom);

        assert_eq!(detect(&utf16le_bytes("森へ")), Enc::Utf16Le);

        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("こんにちは");
        assert_eq!(detect(&sjis), Enc::ShiftJis);
    }

    #[test]
    fn decode_encode_round_trips_each_encoding() {
        let text = "森へ行く。こんにちは[l][r]";
        for enc in [Enc::Utf8, Enc::Utf8Bom, Enc::Utf16Le, Enc::Utf16Be, Enc::ShiftJis] {
            let bytes = encode(text, enc);
            assert_eq!(detect(&bytes), enc, "detect {enc:?}");
            assert_eq!(decode(&bytes, enc), text, "decode {enc:?}");
            // encode(decode(bytes)) == bytes — the identity guarantee.
            assert_eq!(encode(&decode(&bytes, enc), enc), bytes, "re-encode {enc:?}");
        }
    }

    #[test]
    fn shift_jis_falls_back_to_utf16_for_unrepresentable_text() {
        // Thai can't be written as Shift-JIS, so encode() emits UTF-16LE instead.
        let thai = "สวัสดี";
        let bytes = encode(thai, Enc::ShiftJis);
        assert_eq!(detect(&bytes), Enc::Utf16Le);
        assert_eq!(decode(&bytes, Enc::Utf16Le), thai);
    }

    #[test]
    fn utf16_big_endian_round_trips() {
        let text = "あかね";
        let bytes = encode(text, Enc::Utf16Be);
        assert_eq!(&bytes[..2], &BOM_UTF16BE);
        assert_eq!(decode(&bytes, Enc::Utf16Be), text);
    }
}
