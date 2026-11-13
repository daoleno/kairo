//! The setup code `kairo-server` prints for the first password: twelve
//! letters and digits nobody misreads, shown in groups of four
//! (`5SVU-K4B3-G3HA`). The server and the page read what was typed the same
//! way, here: by those characters alone.

/// The characters a code is made of: no I, O, 0 or 1 (they read as each
/// other). 32 of them, so a code is 60 bits.
pub const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// How many characters a code has.
pub const LENGTH: usize = 12;

/// What was typed, as the code's characters only: letters in capitals
/// (full-width ones too, as a CJK keyboard types them), and everything not
/// in [`ALPHABET`] dropped: spaces and dashes of any kind, punctuation, and
/// the I, O, 0 and 1 no code has. `5svu k4b3 g3ha`, `5SVUK4B3G3HA` and
/// `5SVU-K4B3-G3I HA` all read `5SVUK4B3G3HA`.
pub fn normalize(typed: &str) -> String {
    typed
        .chars()
        .map(|c| match c {
            // Full-width forms (U+FF01..U+FF5E) of ASCII.
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .map(|c| c.to_ascii_uppercase())
        .filter(|c| c.is_ascii() && ALPHABET.contains(&(*c as u8)))
        .collect()
}

/// What was typed, as the code is shown: its characters ([`normalize`]) in
/// groups of four (`5SVU-K4B3-G3HA`), at most [`LENGTH`] of them; a partly
/// typed code ends without a dash.
pub fn grouped(typed: &str) -> String {
    let chars: Vec<char> = normalize(typed).chars().take(LENGTH).collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_reads_by_its_characters_alone() {
        for typed in [
            "5SVU-K4B3-G3HA",
            "5SVUK4B3G3HA",
            "5svu k4b3 g3ha",
            " 5svu-k4b3-g3ha \n",
            // What the owner's phone sent: a stray I and a space.
            "5SVU-K4B3-G3I HA",
            // Other dashes and spaces (non-breaking, en dash, minus, full-width).
            "5SVU\u{00A0}K4B3\u{2013}G3HA",
            "5SVU\u{2212}K4B3\u{3000}G3HA",
            "５ＳＶＵ－Ｋ４Ｂ３－Ｇ３ＨＡ",
            // The characters no code has: O, 0, 1, I.
            "5SVU-K4B3-G3HA-O0I1",
        ] {
            assert_eq!(normalize(typed), "5SVUK4B3G3HA", "{typed:?}");
        }
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("ÉÀ-ß"), "");
    }

    #[test]
    fn a_code_is_shown_in_fours() {
        assert_eq!(grouped("5svuk4b3g3ha"), "5SVU-K4B3-G3HA");
        assert_eq!(grouped("5SVU-K4B3-G3I HA"), "5SVU-K4B3-G3HA");
        assert_eq!(grouped("5sv"), "5SV");
        assert_eq!(grouped("5svu"), "5SVU");
        assert_eq!(grouped("5svuk"), "5SVU-K");
        assert_eq!(grouped("5SVU-"), "5SVU");
        // Never more than a code.
        assert_eq!(grouped("5SVUK4B3G3HAXYZ"), "5SVU-K4B3-G3HA");
        // What it shows reads back the same.
        assert_eq!(normalize(&grouped("5svu k4b3 g3ha")), "5SVUK4B3G3HA");
    }

    #[test]
    fn the_alphabet_is_32_characters_none_misread() {
        let mut seen = std::collections::HashSet::new();
        for c in ALPHABET {
            assert!(seen.insert(c));
            assert!(!b"IO01".contains(c));
        }
        assert_eq!(seen.len(), 32);
        // 12 of 32: 60 bits.
        assert_eq!(LENGTH as u32 * ALPHABET.len().ilog2(), 60);
    }
}
