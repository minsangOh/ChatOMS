use std::{error::Error, fmt};

use serde::ser::{Impossible, Serializer};

#[derive(Debug)]
pub struct SerializationError(String);

impl fmt::Display for SerializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for SerializationError {}

impl serde::ser::Error for SerializationError {
    fn custom<T>(message: T) -> Self
    where
        T: fmt::Display,
    {
        Self(message.to_string())
    }
}

pub struct StringSerializer;

impl Serializer for StringSerializer {
    type Ok = String;
    type Error = SerializationError;
    type SerializeSeq = Impossible<String, SerializationError>;
    type SerializeTuple = Impossible<String, SerializationError>;
    type SerializeTupleStruct = Impossible<String, SerializationError>;
    type SerializeTupleVariant = Impossible<String, SerializationError>;
    type SerializeMap = Impossible<String, SerializationError>;
    type SerializeStruct = Impossible<String, SerializationError>;
    type SerializeStructVariant = Impossible<String, SerializationError>;

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(value.to_owned())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(variant.to_owned())
    }

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_some<T>(self, _value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_seq(self, _length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_tuple(self, _length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_map(self, _length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(SerializationError("expected string".to_owned()))
    }
}
