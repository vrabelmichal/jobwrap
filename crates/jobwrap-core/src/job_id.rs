//! A minimal, dependency-free ULID implementation.
//!
//! A ULID is a 128-bit identifier: a 48-bit millisecond timestamp followed by
//! 80 bits of randomness, encoded in Crockford base32 as a 26-character string.
//! It sorts lexicographically by creation time, which makes job lists stable.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Unix epoch in milliseconds, used by the ULID format.
pub const ULID_EPOCH: u64 = 0x01_0000_0000;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ENCODED_LEN: usize = 26;
const INTERNAL_LEN: usize = 16;

fn crockford_encode(v: u8) -> u8 {
    ALPHABET[(v & 0x1F) as usize]
}

fn crockford_decode(c: u8) -> Option<u8> {
    let v = match c {
        b'0' => 0,
        b'1' => 1,
        b'2' => 2,
        b'3' => 3,
        b'4' => 4,
        b'5' => 5,
        b'6' => 6,
        b'7' => 7,
        b'8' => 8,
        b'9' => 9,
        b'A' | b'a' => 10,
        b'B' | b'b' => 11,
        b'C' | b'c' => 12,
        b'D' | b'd' => 13,
        b'E' | b'e' => 14,
        b'F' | b'f' => 15,
        b'G' | b'g' => 16,
        b'H' | b'h' => 17,
        b'J' | b'j' => 18,
        b'K' | b'k' => 19,
        b'M' | b'm' => 20,
        b'N' | b'n' => 21,
        b'P' | b'p' => 22,
        b'Q' | b'q' => 23,
        b'R' | b'r' => 24,
        b'S' | b's' => 25,
        b'T' | b't' => 26,
        b'V' | b'v' => 27,
        b'W' | b'w' => 28,
        b'X' | b'x' => 29,
        b'Y' | b'y' => 30,
        b'Z' | b'z' => 31,
        _ => return None,
    };
    Some(v)
}

/// A job identifier: a ULID rendered as a 26-character Crockford base32 string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId {
    bytes: [u8; INTERNAL_LEN],
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum JobIdError {
    #[error("a job id must be exactly 26 characters")]
    WrongLength,
    #[error("job id contains an invalid character at position {position}")]
    InvalidCharacter { position: usize },
}

impl JobId {
    /// Generate a new job id from the current system clock and CSPRNG.
    pub fn generate() -> Result<Self, JobIdError> {
        let millis = now_millis();
        let mut rng = [0u8; 10];
        getrandom::getrandom(&mut rng).map_err(|_| JobIdError::InvalidCharacter { position: 0 })?;
        Ok(Self::from_parts(millis, &rng))
    }

    fn from_parts(millis: u64, randomness: &[u8; 10]) -> Self {
        let mut bytes = [0u8; INTERNAL_LEN];
        bytes[0] = (millis >> 40) as u8;
        bytes[1] = (millis >> 32) as u8;
        bytes[2] = (millis >> 24) as u8;
        bytes[3] = (millis >> 16) as u8;
        bytes[4] = (millis >> 8) as u8;
        bytes[5] = millis as u8;
        bytes[6..].copy_from_slice(randomness);
        Self { bytes }
    }

    /// The creation timestamp of the id in milliseconds since the Unix epoch.
    pub fn timestamp_ms(&self) -> u64 {
        let mut millis = 0u64;
        for i in 0..6 {
            millis = (millis << 8) | u64::from(self.bytes[i]);
        }
        millis
    }

    /// The creation timestamp as an RFC 3339 UTC string.
    pub fn timestamp_rfc3339(&self) -> Option<String> {
        let ms = self.timestamp_ms();
        let secs = i64::try_from(ms / 1000).ok()?;
        let nanos = u32::try_from((ms % 1000) * 1_000_000).ok()?;
        chrono::DateTime::from_timestamp(secs, nanos)
            .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    }
}

impl Default for JobId {
    fn default() -> Self {
        // All-zero is not a valid generated id but is a stable value for tests
        // and for "no id" sentinel positions.
        Self {
            bytes: [0; INTERNAL_LEN],
        }
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::with_capacity(ENCODED_LEN);
        // 128 bits do not divide evenly into 26 base32 characters, so the
        // first character carries the top 3 bits and the remaining 25
        // characters carry 5 bits each (128 = 3 + 25 * 5).
        for c in 0..ENCODED_LEN {
            let (base, count) = if c == 0 { (0, 3) } else { (3 + 5 * (c - 1), 5) };
            let mut value: u8 = 0;
            for j in 0..count {
                let bit_pos = base + j;
                let byte_idx = bit_pos / 8;
                let bit_in_byte = 7 - (bit_pos % 8);
                let bit = (self.bytes[byte_idx] >> bit_in_byte) & 1;
                value = (value << 1) | bit;
            }
            out.push(crockford_encode(value) as char);
        }
        debug_assert_eq!(out.len(), ENCODED_LEN);
        f.write_str(&out)
    }
}

impl FromStr for JobId {
    type Err = JobIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != ENCODED_LEN {
            return Err(JobIdError::WrongLength);
        }
        let mut bytes = [0u8; INTERNAL_LEN];
        for (c, ch) in s.bytes().enumerate() {
            let value = crockford_decode(ch).ok_or(JobIdError::InvalidCharacter { position: c })?;
            let (base, count) = if c == 0 { (0, 3) } else { (3 + 5 * (c - 1), 5) };
            for j in 0..count {
                let bit_pos = base + j;
                let byte_idx = bit_pos / 8;
                let bit_in_byte = 7 - (bit_pos % 8);
                if ((value >> (count - 1 - j)) & 1) == 1 {
                    bytes[byte_idx] |= 1 << bit_in_byte;
                }
            }
        }
        Ok(Self { bytes })
    }
}

impl Serialize for JobId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for JobId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

fn now_millis() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn round_trips() {
        let id = JobId::generate().expect("generation succeeds");
        let s = id.to_string();
        assert_eq!(s.len(), 26);
        let parsed = JobId::from_str(&s).expect("parses");
        assert_eq!(parsed, id);
    }

    #[test]
    fn known_vector_round_trip() {
        // 01ARZ3NDEKTSV4RRFFQ69G5FAV is a canonical ULID example.
        let id = JobId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("parses canonical");
        assert_eq!(id.to_string(), "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert!(id.timestamp_ms() > ULID_EPOCH);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(JobId::from_str("short"), Err(JobIdError::WrongLength));
        assert_eq!(
            JobId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAI"),
            Err(JobIdError::InvalidCharacter { position: 25 })
        );
    }

    #[test]
    fn sorts_by_time() {
        let a = JobId::from_parts(1_000, &[0; 10]);
        let b = JobId::from_parts(2_000, &[0; 10]);
        assert!(a < b);
    }
}
