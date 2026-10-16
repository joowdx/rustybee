//! GSM-7 / UCS-2 segment math (spec 004). Mirrored in `static/app.js`.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Gsm7,
    Ucs2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Segments {
    pub encoding: Encoding,
    /// Septets for GSM-7, UTF-16 code units for UCS-2.
    pub units: usize,
    pub parts: usize,
    /// Units left in the last part before another part is needed.
    pub remaining: usize,
}

const GSM_BASIC: &str = "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞ\u{1b}ÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà";
const GSM_EXT: &str = "\u{c}^{}\\[~]|€";

fn gsm_septets(c: char) -> Option<usize> {
    if GSM_BASIC.contains(c) {
        Some(1)
    } else if GSM_EXT.contains(c) {
        Some(2)
    } else {
        None
    }
}

pub fn segments(body: &str) -> Segments {
    let gsm: Option<usize> = body.chars().map(gsm_septets).sum();
    let (encoding, units, single, multi) = match gsm {
        Some(n) => (Encoding::Gsm7, n, 160, 153),
        None => (Encoding::Ucs2, body.encode_utf16().count(), 70, 67),
    };
    let (parts, remaining) = if units == 0 {
        (0, single)
    } else if units <= single {
        (1, single - units)
    } else {
        let parts = units.div_ceil(multi);
        (parts, parts * multi - units)
    };
    Segments {
        encoding,
        units,
        parts,
        remaining,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_004_05_gsm_160_is_one_part() {
        let s = segments(&"a".repeat(160));
        assert_eq!(
            (s.encoding, s.units, s.parts, s.remaining),
            (Encoding::Gsm7, 160, 1, 0)
        );
    }

    #[test]
    fn ac_004_05_gsm_161_is_two_parts() {
        let s = segments(&"a".repeat(161));
        assert_eq!(
            (s.encoding, s.parts, s.remaining),
            (Encoding::Gsm7, 2, 306 - 161)
        );
    }

    #[test]
    fn ac_004_05_euro_counts_double() {
        let s = segments("€");
        assert_eq!((s.encoding, s.units), (Encoding::Gsm7, 2));
        let s = segments(&format!("{}€", "a".repeat(159)));
        assert_eq!((s.units, s.parts), (161, 2));
    }

    #[test]
    fn ac_004_05_emoji_is_ucs2() {
        let s = segments("hi 😀");
        assert_eq!(s.encoding, Encoding::Ucs2);
        assert_eq!(s.units, 5);
        assert_eq!(s.parts, 1);
    }

    #[test]
    fn ac_004_05_cyrillic_70_and_71() {
        let s = segments(&"Ж".repeat(70));
        assert_eq!((s.encoding, s.parts, s.remaining), (Encoding::Ucs2, 1, 0));
        let s = segments(&"Ж".repeat(71));
        assert_eq!((s.parts, s.remaining), (2, 134 - 71));
    }

    #[test]
    fn empty_body() {
        let s = segments("");
        assert_eq!((s.parts, s.units), (0, 0));
    }
}
