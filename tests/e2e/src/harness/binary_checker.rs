use thiserror::Error;

pub const KNIR_MAGIC: &[u8; 4] = b"KNIR"; // 0x4B 0x4E 0x49 0x52
pub const CURRENT_VERSION: u16 = 1;

#[derive(Error, Debug, PartialEq)]
pub enum BinaryCheckError {
    #[error("File too short: expected at least {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    #[error("Invalid magic header: expected 'KNIR' ([0x4B, 0x4E, 0x49, 0x52]), got {actual:?}")]
    InvalidMagic { actual: [u8; 4] },
    #[error("Unsupported binary version: expected {expected}, got {actual}")]
    UnsupportedVersion { expected: u16, actual: u16 },
    #[error("Invalid instruction word length: stream length not divisible by 8 bytes")]
    UnalignedInstructionStream,
    #[error("Invalid opcode byte {opcode:#04x}")]
    InvalidOpcode { opcode: u8 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedHeader {
    pub magic: [u8; 4],
    pub version: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedInstructionWord {
    pub opcode: u8,
    pub out_type: u8,
    pub dst: u16,
    pub src1: u16,
    pub src2: u16,
}

pub fn encode_header(version: u16) -> [u8; 6] {
    let mut bytes = [0u8; 6];
    bytes[0..4].copy_from_slice(KNIR_MAGIC);
    bytes[4..6].copy_from_slice(&version.to_le_bytes());
    bytes
}

pub fn decode_header(bytes: &[u8]) -> Result<DecodedHeader, BinaryCheckError> {
    if bytes.len() < 6 {
        return Err(BinaryCheckError::TooShort {
            expected: 6,
            actual: bytes.len(),
        });
    }

    let mut magic = [0u8; 4];
    magic.copy_from_slice(&bytes[0..4]);
    if &magic != KNIR_MAGIC {
        return Err(BinaryCheckError::InvalidMagic { actual: magic });
    }

    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != CURRENT_VERSION {
        return Err(BinaryCheckError::UnsupportedVersion {
            expected: CURRENT_VERSION,
            actual: version,
        });
    }

    Ok(DecodedHeader { magic, version })
}

pub fn pack_instruction_word(
    opcode: u8,
    out_type: u8,
    dst: u16,
    src1: u16,
    src2: u16,
) -> [u8; 8] {
    let mut word = [0u8; 8];
    word[0] = opcode;
    word[1] = out_type;
    word[2..4].copy_from_slice(&dst.to_le_bytes());
    word[4..6].copy_from_slice(&src1.to_le_bytes());
    word[6..8].copy_from_slice(&src2.to_le_bytes());
    word
}

pub fn unpack_instruction_word(word: &[u8]) -> Result<DecodedInstructionWord, BinaryCheckError> {
    if word.len() != 8 {
        return Err(BinaryCheckError::TooShort {
            expected: 8,
            actual: word.len(),
        });
    }

    let opcode = word[0];
    let out_type = word[1];
    let dst = u16::from_le_bytes([word[2], word[3]]);
    let src1 = u16::from_le_bytes([word[4], word[5]]);
    let src2 = u16::from_le_bytes([word[6], word[7]]);

    Ok(DecodedInstructionWord {
        opcode,
        out_type,
        dst,
        src1,
        src2,
    })
}
