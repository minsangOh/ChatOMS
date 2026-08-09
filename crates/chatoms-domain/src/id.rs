use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use uuid::Uuid;

use crate::DomainError;

fn parse_uuid_v7(value: &str) -> Result<Uuid, DomainError> {
    let uuid = Uuid::parse_str(value).map_err(|_| DomainError::InvalidUuid)?;
    if uuid.get_version_num() != 7 {
        return Err(DomainError::UnsupportedUuidVersion);
    }
    Ok(uuid)
}

macro_rules! uuid_v7_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub(crate) fn from_uuid(uuid: Uuid) -> Result<Self, DomainError> {
                if uuid.get_version_num() != 7 {
                    return Err(DomainError::UnsupportedUuidVersion);
                }
                Ok(Self(uuid))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0.hyphenated())
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::from_uuid(parse_uuid_v7(value)?)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(D::Error::custom)
            }
        }
    };
}

uuid_v7_id!(ProjectId);
uuid_v7_id!(AppProfileId);
uuid_v7_id!(ProviderBindingId);
uuid_v7_id!(TaskId);
uuid_v7_id!(TaskStateTransitionId);
uuid_v7_id!(GitOperationId);
