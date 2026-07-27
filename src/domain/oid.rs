use std::fmt;
use std::str::FromStr;

/// A Git object identifier (SHA-1).
///
/// Stored as raw bytes rather than a hex `String`: the graph layout keeps one
/// entry per commit in hash maps and adjacency lists, so `Oid` must be `Copy`
/// and allocation-free. A 40-char `String` would cost a heap allocation and a
/// pointer chase per commit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid([u8; 20]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OidParseError {
    InvalidLength { got: usize },
    InvalidHexDigit { at: usize },
}

impl fmt::Display for OidParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength { got } => {
                write!(f, "expected 40 hex characters, got {got}")
            }
            Self::InvalidHexDigit { at } => {
                write!(f, "invalid hex digit at position {at}")
            }
        }
    }
}

impl std::error::Error for OidParseError {}

impl Oid {
    pub const LEN: usize = 20;
    pub const HEX_LEN: usize = 40;

    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// The all-zero id, which Git uses to mean "no object".
    pub const fn zero() -> Self {
        Self([0u8; 20])
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 20]
    }

    pub fn from_hex(hex: &str) -> Result<Self, OidParseError> {
        let bytes = hex.as_bytes();
        if bytes.len() != Self::HEX_LEN {
            return Err(OidParseError::InvalidLength { got: bytes.len() });
        }

        let mut out = [0u8; 20];
        for (i, pair) in bytes.chunks_exact(2).enumerate() {
            let hi = hex_value(pair[0]).ok_or(OidParseError::InvalidHexDigit { at: i * 2 })?;
            let lo = hex_value(pair[1]).ok_or(OidParseError::InvalidHexDigit { at: i * 2 + 1 })?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(Self::HEX_LEN);
        for byte in &self.0 {
            s.push(hex_digit(byte >> 4));
            s.push(hex_digit(byte & 0x0f));
        }
        s
    }

    /// The abbreviated form shown in the UI. `len` is clamped to a full id.
    pub fn to_short_hex(&self, len: usize) -> String {
        let mut s = self.to_hex();
        s.truncate(len.min(Self::HEX_LEN));
        s
    }
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_digit(nibble: u8) -> char {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    DIGITS[nibble as usize] as char
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Debug prints the short form; full hashes make test failures unreadable.
impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.to_short_hex(8))
    }
}

impl FromStr for Oid {
    type Err = OidParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f80910111213";

    #[test]
    fn parses_and_renders_a_full_hex_id() {
        let oid = Oid::from_hex(SAMPLE).expect("valid hex");
        assert_eq!(oid.to_hex(), SAMPLE);
    }

    #[test]
    fn accepts_uppercase_hex() {
        let lower = Oid::from_hex(SAMPLE).unwrap();
        let upper = Oid::from_hex(&SAMPLE.to_uppercase()).unwrap();
        assert_eq!(lower, upper);
    }

    #[test]
    fn renders_lowercase_regardless_of_input_case() {
        let oid = Oid::from_hex(&SAMPLE.to_uppercase()).unwrap();
        assert_eq!(oid.to_hex(), SAMPLE);
    }

    #[test]
    fn rejects_ids_that_are_not_forty_characters() {
        assert_eq!(
            Oid::from_hex("abc"),
            Err(OidParseError::InvalidLength { got: 3 })
        );
        let too_long = format!("{SAMPLE}00");
        assert_eq!(
            Oid::from_hex(&too_long),
            Err(OidParseError::InvalidLength { got: 42 })
        );
    }

    #[test]
    fn rejects_non_hex_characters_and_reports_the_position() {
        let bad = format!("{}z{}", &SAMPLE[..5], &SAMPLE[6..]);
        assert_eq!(
            Oid::from_hex(&bad),
            Err(OidParseError::InvalidHexDigit { at: 5 })
        );
    }

    #[test]
    fn shortens_to_the_requested_length() {
        let oid = Oid::from_hex(SAMPLE).unwrap();
        assert_eq!(oid.to_short_hex(7), "1a2b3c4");
    }

    #[test]
    fn shortening_beyond_the_full_length_yields_the_full_id() {
        let oid = Oid::from_hex(SAMPLE).unwrap();
        assert_eq!(oid.to_short_hex(200), SAMPLE);
    }

    #[test]
    fn zero_is_recognised_and_no_real_id_is_mistaken_for_it() {
        assert!(Oid::zero().is_zero());
        assert!(!Oid::from_hex(SAMPLE).unwrap().is_zero());
    }

    #[test]
    fn round_trips_through_bytes() {
        let oid = Oid::from_hex(SAMPLE).unwrap();
        assert_eq!(Oid::from_bytes(*oid.as_bytes()), oid);
    }

    #[test]
    fn parses_through_from_str() {
        let oid: Oid = SAMPLE.parse().unwrap();
        assert_eq!(oid.to_hex(), SAMPLE);
    }
}
