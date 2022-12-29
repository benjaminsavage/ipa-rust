use crate::ff::{self, Error};
use std::any::type_name;
use std::fmt::Debug;
use std::io;
use std::io::ErrorKind;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Not};

use super::ArithmeticOps;

// Trait for primitive integer types used to represent the underlying type for field values
pub trait Int: Sized + Copy + Debug + Into<u128> {
    const BITS: u32;
}

impl Int for u8 {
    const BITS: u32 = u8::BITS;
}

impl Int for u32 {
    const BITS: u32 = u32::BITS;
}

pub trait Field:
    ArithmeticOps
    + From<u128>
    + Into<Self::Integer>
    + Clone
    + Copy
    + PartialEq
    + Debug
    + Send
    + Sync
    + Sized
    + 'static
{
    type Integer: Int;

    const PRIME: Self::Integer;
    /// Additive identity element
    const ZERO: Self;
    /// Multiplicative identity element
    const ONE: Self;
    /// Derived from the size of the backing field, this constant indicates how much
    /// space is required to store this field value
    const SIZE_IN_BYTES: u32 = Self::Integer::BITS / 8;

    /// str repr of the type of the [`Field`]; to be used with `FieldType` to get the size of a
    /// given [`Field`] from this value.
    /// # Instruction For Authors
    /// When creating a new [`Field`] type, modify the `FieldType::serialize` and
    /// `FieldType::deserialize` functions below this trait definition to use the newly created
    /// type
    const TYPE_STR: &'static str;

    /// Blanket implementation to represent the instance of this trait as 16 byte integer.
    /// Uses the fact that such conversion already exists via `Self` -> `Self::Integer` -> `Into<u128>`
    fn as_u128(&self) -> u128 {
        let int: Self::Integer = (*self).into();
        int.into()
    }

    /// Generic implementation to serialize fields into a buffer. Callers need to make sure
    /// there is enough capacity to store the value of this field.
    /// It is less efficient because it operates with generic representation of fields as 16 byte
    /// integers, so consider overriding it for actual field implementations
    ///
    /// ## Errors
    /// Returns an error if buffer did not have enough capacity to store this field value
    fn serialize(&self, buf: &mut [u8]) -> io::Result<()> {
        let raw_value = &self.as_u128().to_le_bytes()[..Self::SIZE_IN_BYTES as usize];

        if buf.len() >= raw_value.len() {
            buf[..Self::SIZE_IN_BYTES as usize].copy_from_slice(raw_value);
            Ok(())
        } else {
            let error_text = format!(
                "Buffer with total capacity {} cannot hold field value {:?} because \
                 it required at least {} bytes available",
                buf.len(),
                self,
                Self::SIZE_IN_BYTES
            );

            Err(io::Error::new(ErrorKind::WriteZero, error_text))
        }
    }

    /// Generic implementation to deserialize fields from buffer.
    /// It is less efficient because it allocates 16 bytes on the stack to accommodate for all
    /// possible field implementations, so consider overriding it for actual field implementations
    ///
    /// In the bright future when we have const generic expressions, this can be changed to provide
    /// zero-cost generic implementation
    ///
    /// ## Errors
    /// Returns an error if buffer did not have enough capacity left to read the field value.
    fn deserialize(buf_from: &[u8]) -> io::Result<Self> {
        if Self::SIZE_IN_BYTES as usize <= buf_from.len() {
            let mut buf_to = [0; 16]; // one day...
            buf_to[..Self::SIZE_IN_BYTES as usize]
                .copy_from_slice(&buf_from[..Self::SIZE_IN_BYTES as usize]);

            Ok(Self::from(u128::from_le_bytes(buf_to)))
        } else {
            let error_text = format!(
                "Buffer is too small to read values of the field type {}. Required at least {} bytes,\
                 got {}", type_name::<Self>(), Self::SIZE_IN_BYTES, buf_from.len()
            );
            Err(io::Error::new(ErrorKind::UnexpectedEof, error_text))
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FieldType {
    Fp2,
    Fp31,
    Fp32BitPrime,
}

impl FieldType {
    #[must_use]
    pub fn size_in_bytes(&self) -> u32 {
        match self {
            Self::Fp2 => ff::Fp2::SIZE_IN_BYTES,
            Self::Fp31 => ff::Fp31::SIZE_IN_BYTES,
            Self::Fp32BitPrime => ff::Fp32BitPrime::SIZE_IN_BYTES,
        }
    }
}

impl AsRef<str> for FieldType {
    fn as_ref(&self) -> &str {
        match self {
            FieldType::Fp2 => ff::Fp2::TYPE_STR,
            FieldType::Fp31 => ff::Fp31::TYPE_STR,
            FieldType::Fp32BitPrime => ff::Fp32BitPrime::TYPE_STR,
        }
    }
}

/// For Authors: when adding a new [`Field`] type, add it to the `serialize` fn below
#[cfg(feature = "enable-serde")]
impl serde::Serialize for FieldType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_ref())
    }
}

/// For Authors: when adding a new [`Field`] type, add it to the `visit_str` fn below
#[cfg(feature = "enable-serde")]
impl<'de> serde::Deserialize<'de> for FieldType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldTypeVisitor;
        impl<'de> serde::de::Visitor<'de> for FieldTypeVisitor {
            type Value = FieldType;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a correctly formatted FieldType")
            }

            fn visit_str<E: serde::de::Error>(
                self,
                field_type_str: &str,
            ) -> Result<Self::Value, E> {
                if field_type_str.eq_ignore_ascii_case(ff::Fp2::TYPE_STR) {
                    Ok(FieldType::Fp2)
                } else if field_type_str.eq_ignore_ascii_case(ff::Fp31::TYPE_STR) {
                    Ok(FieldType::Fp31)
                } else if field_type_str.eq_ignore_ascii_case(ff::Fp32BitPrime::TYPE_STR) {
                    Ok(FieldType::Fp32BitPrime)
                } else {
                    Err(serde::de::Error::custom(Error::UnknownField {
                        type_str: field_type_str.to_string(),
                    }))
                }
            }

            fn visit_string<E: serde::de::Error>(
                self,
                field_type_str: String,
            ) -> Result<Self::Value, E> {
                self.visit_str(&field_type_str)
            }
        }
        deserializer.deserialize_str(FieldTypeVisitor)
    }
}

pub trait BinaryField:
    Field
    + BitAnd<Output = Self>
    + BitAndAssign
    + BitOr<Output = Self>
    + BitOrAssign
    + BitXor<Output = Self>
    + BitXorAssign
    + Not<Output = Self>
{
}

#[cfg(test)]
mod test {
    use super::*;

    #[cfg(feature = "enable-serde")]
    #[test]
    fn field_type_str_is_case_insensitive() {
        let field_type: FieldType = serde_json::from_str("\"fP32bItPrImE\"")
            .expect("FieldType should match regardless of character case");
        assert_eq!(field_type.size_in_bytes(), ff::Fp32BitPrime::SIZE_IN_BYTES);
    }
}
