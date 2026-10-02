//! Rust implementations of Marketplace Java actions, doing what their Java
//! does in Mendix.
//!
//! A flow that calls a Java action needs a Rust implementation to run in
//! mxrs. These are the ones whose Java is plain enough to restate; each says
//! which Java it follows and where it cannot. An application registers its
//! own after these and replaces any of them by registering the same name.
//!
//! Text is read as UTF-8 where the Java calls `getBytes()`: the platform
//! charset, which is UTF-8 from Java 18 on.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::FlowError;
use crate::engine::{FlowEngine, JavaAction};
use crate::value::FlowValue;

impl FlowEngine {
    /// Registers the Marketplace Java actions mxrs implements.
    pub fn with_marketplace_java_actions(self) -> Self {
        self.with_java_action("CommunityCommons.StringLeftPad", Pad(Side::Left))
            .with_java_action("CommunityCommons.StringRightPad", Pad(Side::Right))
            .with_java_action("CommunityCommons.Hash", Hash)
            .with_java_action("CommunityCommons.RandomString", RandomString)
            .with_java_action("CommunityCommons.Base64Encode", Base64Encode)
            .with_java_action("CommunityCommons.Base64Decode", Base64Decode)
            .with_java_action("CommunityCommons.StringTrim", StringTrim)
            .with_java_action("CommunityCommons.ThrowException", ThrowException)
            .with_java_action("CommunityCommons.RandomHash", RandomHash)
    }
}

fn text(arguments: &BTreeMap<String, FlowValue>, name: &str) -> Option<String> {
    match arguments.get(name) {
        Some(FlowValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// A whole-number argument as the Java reads it: `Long.intValue()`, which
/// keeps the low 32 bits.
fn whole(
    arguments: &BTreeMap<String, FlowValue>,
    action: &str,
    name: &str,
) -> Result<i32, FlowError> {
    match arguments.get(name) {
        #[allow(clippy::cast_possible_truncation)]
        Some(FlowValue::Int(value)) => Ok(*value as i32),
        _ => Err(FlowError::native(format!(
            "{action} needs a number for `{name}`"
        ))),
    }
}

/// The longest text these actions build, in characters: past it a flow
/// fails as Java's `OutOfMemoryError` would, rather than a system that
/// overcommits memory taking the process down later.
const LONGEST: usize = 1 << 27;

/// Room for `count` more elements, or the flow error Java's
/// `OutOfMemoryError` would be — never an aborted process.
fn reserve<T>(vector: &mut Vec<T>, count: usize, action: &str) -> Result<(), FlowError> {
    if count > LONGEST {
        return Err(FlowError::native(format!(
            "{action}: {count} characters is more than a text can hold here ({LONGEST})"
        )));
    }
    vector
        .try_reserve_exact(count)
        .map_err(|_| FlowError::native(format!("{action}: out of memory for {count} characters")))
}

enum Side {
    Left,
    Right,
}

/// `CommunityCommons.StringLeftPad` / `StringRightPad`: commons-lang3's
/// `leftPad`/`rightPad` — `value` padded to `amount` UTF-16 units with
/// `fillCharacter` repeated (a space when it is empty), and `value` itself
/// when already that long. Without a value there is nothing to pad: the
/// result is empty, while an empty text is padded like any other.
struct Pad(Side);

impl JavaAction for Pad {
    fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
        let action = match self.0 {
            Side::Left => "CommunityCommons.StringLeftPad",
            Side::Right => "CommunityCommons.StringRightPad",
        };
        let amount = whole(arguments, action, "amount")?;
        let Some(value) = text(arguments, "value") else {
            return Ok(FlowValue::Empty);
        };
        let fill = text(arguments, "fillCharacter")
            .filter(|fill| !fill.is_empty())
            .unwrap_or_else(|| " ".to_string());
        let value_units: Vec<u16> = value.encode_utf16().collect();
        let fill_units: Vec<u16> = fill.encode_utf16().collect();
        let Some(pads) = usize::try_from(amount)
            .ok()
            .and_then(|amount| amount.checked_sub(value_units.len()))
            .filter(|pads| *pads > 0)
        else {
            return Ok(FlowValue::String(value));
        };
        let mut units = Vec::new();
        reserve(&mut units, pads + value_units.len(), action)?;
        let padding = (0..pads).map(|index| fill_units[index % fill_units.len()]);
        match self.0 {
            Side::Left => {
                units.extend(padding);
                units.extend(value_units);
            }
            Side::Right => {
                units.extend(value_units);
                units.extend(padding);
            }
        }
        Ok(FlowValue::String(String::from_utf16_lossy(&units)))
    }
}

/// `CommunityCommons.Hash`: `StringUtils.hash(value)` — SHA-256 of the
/// UTF-8 bytes, as 64 lowercase hex digits. The action's `length` argument
/// is not read by its Java, and is not read here.
struct Hash;

impl JavaAction for Hash {
    fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
        let value = text(arguments, "value").ok_or_else(|| {
            FlowError::native("CommunityCommons.Hash needs a value to hash".to_string())
        })?;
        let digest = Sha256::digest(value.as_bytes());
        Ok(FlowValue::String(
            digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        ))
    }
}

/// `CommunityCommons.RandomString`: `length` characters drawn uniformly from
/// `A-Z`, `a-z` and `0-9` by a cryptographic generator. A negative length
/// fails, as Java's `IllegalArgumentException` does.
struct RandomString;

const ALPHANUMERIC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

impl JavaAction for RandomString {
    fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
        const ACTION: &str = "CommunityCommons.RandomString";
        let length = usize::try_from(whole(arguments, ACTION, "length")?)
            .map_err(|_| FlowError::native(format!("{ACTION}: the length must not be negative")))?;
        let mut out = Vec::new();
        reserve(&mut out, length, ACTION)?;
        while out.len() < length {
            // A v4 UUID is 122 bits from the system's secure generator. Its
            // version (byte 6) and variant (byte 8) bits are fixed, so those
            // two bytes are skipped; bytes past the largest multiple of 62
            // are dropped, so every character is equally likely.
            for (index, byte) in uuid::Uuid::new_v4().into_bytes().into_iter().enumerate() {
                if index != 6 && index != 8 && byte < 248 && out.len() < length {
                    out.push(ALPHANUMERIC[usize::from(byte) % ALPHANUMERIC.len()]);
                }
            }
        }
        Ok(FlowValue::String(
            String::from_utf8(out).expect("alphanumeric bytes are UTF-8"),
        ))
    }
}

/// `CommunityCommons.Base64Encode`: `Base64.getEncoder()` over the UTF-8
/// bytes; empty stays empty.
struct Base64Encode;

impl JavaAction for Base64Encode {
    fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
        use base64::Engine;
        Ok(match text(arguments, "value") {
            Some(value) => {
                FlowValue::String(base64::engine::general_purpose::STANDARD.encode(value))
            }
            None => FlowValue::Empty,
        })
    }
}

/// `CommunityCommons.Base64Decode`: `Base64.getDecoder()`, read as UTF-8.
/// Like the Java, it accepts text without its padding and stray bits in the
/// last character; text that is not base64 fails, as the Java throws.
struct Base64Decode;

impl JavaAction for Base64Decode {
    fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
        use base64::Engine;
        let Some(encoded) = text(arguments, "encoded") else {
            return Ok(FlowValue::Empty);
        };
        // Java's decoder needs no padding, but padding it is given must be
        // whole; stray bits in the last character it ignores.
        let padding = if encoded.contains('=') {
            base64::engine::DecodePaddingMode::RequireCanonical
        } else {
            base64::engine::DecodePaddingMode::RequireNone
        };
        let engine = base64::engine::GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            base64::engine::GeneralPurposeConfig::new()
                .with_decode_padding_mode(padding)
                .with_decode_allow_trailing_bits(true),
        );
        let bytes = engine.decode(encoded).map_err(|error| {
            FlowError::native(format!("CommunityCommons.Base64Decode: {error}"))
        })?;
        Ok(FlowValue::String(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    }
}

/// `CommunityCommons.StringTrim`: Java's `String.trim()`, which drops the
/// characters up to U+0020 at both ends; empty when there is no value.
struct StringTrim;

impl JavaAction for StringTrim {
    fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
        let value = text(arguments, "value").unwrap_or_default();
        Ok(FlowValue::String(
            value.trim_matches(|c: char| c <= '\u{20}').to_string(),
        ))
    }
}

/// `CommunityCommons.ThrowException`: fails the flow with `message`.
struct ThrowException;

impl JavaAction for ThrowException {
    fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
        Err(FlowError::native(
            text(arguments, "message").unwrap_or_default(),
        ))
    }
}

/// `CommunityCommons.RandomHash`: `UUID.randomUUID().toString()`.
struct RandomHash;

impl JavaAction for RandomHash {
    fn call(&self, _arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
        Ok(FlowValue::String(uuid::Uuid::new_v4().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[(&str, FlowValue)]) -> BTreeMap<String, FlowValue> {
        values
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect()
    }

    fn string(value: FlowValue) -> String {
        match value {
            FlowValue::String(text) => text,
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn padding_follows_commons_lang() {
        let pad = |side: Side, value: &str, amount: i64, fill: &str| {
            string(
                Pad(side)
                    .call(&arguments(&[
                        ("value", FlowValue::String(value.into())),
                        ("amount", FlowValue::Int(amount)),
                        ("fillCharacter", FlowValue::String(fill.into())),
                    ]))
                    .unwrap(),
            )
        };
        assert_eq!(pad(Side::Left, "7", 3, "0"), "007");
        assert_eq!(pad(Side::Left, "bat", 8, "yz"), "yzyzybat");
        assert_eq!(pad(Side::Left, "bat", 5, "yzyzyz"), "yzbat");
        assert_eq!(pad(Side::Left, "bat", 2, "0"), "bat");
        assert_eq!(pad(Side::Left, "bat", 5, ""), "  bat");
        assert_eq!(pad(Side::Right, "bat", 8, "yz"), "batyzyzy");
        let empty = Pad(Side::Left)
            .call(&arguments(&[("amount", FlowValue::Int(3))]))
            .unwrap();
        assert!(matches!(empty, FlowValue::Empty));
        // An empty text is padded; an amount past an `int` wraps as Java's
        // `intValue()` does, here to a negative and so to no padding.
        assert_eq!(pad(Side::Left, "", 3, "0"), "000");
        assert_eq!(pad(Side::Left, "bat", 3_000_000_000, "0"), "bat");
        // A text too long to build fails the flow.
        assert!(
            Pad(Side::Left)
                .call(&arguments(&[
                    ("value", FlowValue::String("a".into())),
                    ("amount", FlowValue::Int(i64::from(i32::MAX))),
                ]))
                .is_err()
        );
    }

    #[test]
    fn strings_are_encoded_trimmed_and_thrown_as_the_java_does() {
        let call = |action: &dyn JavaAction, name: &str, value: &str| {
            action.call(&arguments(&[(name, FlowValue::String(value.into()))]))
        };
        assert_eq!(
            string(call(&Base64Encode, "value", "Mendix ü").unwrap()),
            "TWVuZGl4IMO8"
        );
        assert_eq!(
            string(call(&Base64Decode, "encoded", "TWVuZGl4IMO8").unwrap()),
            "Mendix ü"
        );
        assert!(call(&Base64Decode, "encoded", "not base64!").is_err());
        // Java's decoder needs no padding and ignores stray trailing bits.
        for (encoded, decoded) in [("TWE", "Ma"), ("TQ", "M"), ("TWF=", "Ma")] {
            assert_eq!(
                string(call(&Base64Decode, "encoded", encoded).unwrap()),
                decoded
            );
        }
        // Padding it is given must be whole, as Java's says.
        assert!(call(&Base64Decode, "encoded", "TQ=").is_err());
        assert_eq!(
            string(call(&StringTrim, "value", "\t a b \n").unwrap()),
            "a b"
        );
        // Java's trim leaves a no-break space alone.
        assert_eq!(
            string(call(&StringTrim, "value", "\u{a0}x").unwrap()),
            "\u{a0}x"
        );
        let thrown = call(&ThrowException, "message", "stop").unwrap_err();
        assert!(thrown.to_string().contains("stop"), "{thrown}");
        assert_eq!(string(RandomHash.call(&BTreeMap::new()).unwrap()).len(), 36);
    }

    #[test]
    fn a_hash_is_sha256_in_hex() {
        assert_eq!(
            string(
                Hash.call(&arguments(&[
                    ("value", FlowValue::String("abc".into())),
                    ("length", FlowValue::Int(5)),
                ]))
                .unwrap()
            ),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_random_string_is_alphanumeric_of_the_length_asked() {
        let value = string(
            RandomString
                .call(&arguments(&[("length", FlowValue::Int(40))]))
                .unwrap(),
        );
        assert_eq!(value.len(), 40);
        assert!(value.bytes().all(|byte| ALPHANUMERIC.contains(&byte)));
        let none = string(
            RandomString
                .call(&arguments(&[("length", FlowValue::Int(0))]))
                .unwrap(),
        );
        assert!(none.is_empty());
        assert!(
            RandomString
                .call(&arguments(&[("length", FlowValue::Int(-1))]))
                .is_err()
        );
    }

    #[test]
    fn every_position_of_a_random_string_is_uniform() {
        // Fixed UUID bits once made one position favour 16 letters.
        let mut counts = vec![[0u32; 62]; 16];
        for _ in 0..4_000 {
            let value = string(
                RandomString
                    .call(&arguments(&[("length", FlowValue::Int(16))]))
                    .unwrap(),
            );
            for (position, byte) in value.bytes().enumerate() {
                let index = ALPHANUMERIC.iter().position(|c| *c == byte).unwrap();
                counts[position][index] += 1;
            }
        }
        // 4000 draws over 62 characters is about 65 each; a biased
        // position would put some letter near 250.
        for (position, counts) in counts.iter().enumerate() {
            let most = counts.iter().max().unwrap();
            assert!(*most < 130, "position {position}: {counts:?}");
        }
    }
}
