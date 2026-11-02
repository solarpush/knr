use crate::error::BinaryError;
use std::io::{Read, Write};

pub const MAGIC: [u8; 4] = [0x4B, 0x4E, 0x49, 0x52]; // "KNIR"
pub const CURRENT_VERSION: u16 = 1;

pub fn write_header(w: &mut impl Write) -> Result<(), BinaryError> {
    w.write_all(&MAGIC).map_err(|e| BinaryError::IoError(e.to_string()))?;
    w.write_all(&CURRENT_VERSION.to_le_bytes()).map_err(|e| BinaryError::IoError(e.to_string()))?;
    Ok(())
}

pub fn read_header(r: &mut impl Read) -> Result<u16, BinaryError> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).map_err(|_| BinaryError::UnexpectedEof { expected: 4, actual: 0 })?;
    if magic != MAGIC {
        return Err(BinaryError::BadMagic(magic));
    }
    let mut ver_bytes = [0u8; 2];
    r.read_exact(&mut ver_bytes).map_err(|_| BinaryError::UnexpectedEof { expected: 2, actual: 0 })?;
    let version = u16::from_le_bytes(ver_bytes);
    if version != CURRENT_VERSION {
        return Err(BinaryError::UnsupportedVersion(version));
    }
    Ok(version)
}
