use zerocopy::{AsBytes, FromBytes, FromZeroes};

pub mod reader; 

/// Magic bytes 'WAX1' to identify the file format.
pub const WAX_MAGIC: [u8; 4] = [0x57, 0x41, 0x58, 0x31];

#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, AsBytes, FromZeroes)]
pub struct WaxHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub uuid: [u8; 16],
    pub index_offset: u64,
    pub index_length: u64,
    pub compression_type: u8,
    pub padding: [u8; 23], 
}

impl Default for WaxHeader {
    fn default() -> Self {
        Self {
            magic: WAX_MAGIC,
            version: 1,
            uuid: [0; 16],
            index_offset: 0,
            index_length: 0,
            compression_type: 1,
            padding: [0; 23],
        }
    }
}
