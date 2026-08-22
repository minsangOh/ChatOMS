/// The provider-neutral operation vocabulary supported by immutable risk
/// declarations. Phase 5g-2a deliberately supports only provider
/// implementation; later Units must extend this closed enum explicitly.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationRiskKind {
    ProviderImplementation,
}

impl OperationRiskKind {
    pub const ALL: [Self; 1] = [Self::ProviderImplementation];

    #[must_use]
    pub const fn persisted_text(self) -> &'static str {
        match self {
            Self::ProviderImplementation => "ProviderImplementation",
        }
    }

    #[must_use]
    pub fn from_persisted_text(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.persisted_text() == value)
    }
}

/// A content-free SHA-256 digest binding a declaration to one stable target
/// identity. It never contains or exposes a filesystem path.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TargetIdentityDigest([u8; 32]);

impl TargetIdentityDigest {
    #[must_use]
    pub const fn from_digest_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 || !hex.bytes().all(is_lowercase_hex_digit) {
            return None;
        }
        let mut bytes = [0u8; 32];
        let hex_bytes = hex.as_bytes();
        for (index, byte) in bytes.iter_mut().enumerate() {
            let high = hex_nibble(hex_bytes[index * 2])?;
            let low = hex_nibble(hex_bytes[index * 2 + 1])?;
            *byte = (high << 4) | low;
        }
        Some(Self(bytes))
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push_str(&format!("{byte:02x}"));
        }
        output
    }
}

fn is_lowercase_hex_digit(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
