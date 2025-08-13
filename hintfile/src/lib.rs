use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
};

use bitcoin::{BlockHeight, BlockHeightInterval};

pub fn write_compact_size<W: Write>(value: u64, writer: &mut W) -> Result<(), io::Error> {
    match value {
        0..=0xFC => {
            writer.write_all(&[value as u8]) // Cast ok because of match.
        }
        0xFD..=0xFFFF => {
            let v = (value as u16).to_le_bytes(); // Cast ok because of match.
            writer.write_all(&[0xFD, v[0], v[1]])
        }
        0x10000..=0xFFFFFFFF => {
            let v = (value as u32).to_le_bytes(); // Cast ok because of match.
            writer.write_all(&[0xFE, v[0], v[1], v[2], v[3]])
        }
        _ => panic!("unexpected large offset"),
    }
}

pub fn read_compact_size<R: Read>(reader: &mut R) -> Result<u64, io::Error> {
    let mut buf: [u8; 1] = [0; 1];
    reader.read_exact(&mut buf)?;
    let prefix = buf[0];
    match prefix {
        0xFD => {
            let mut buf: [u8; 2] = [0; 2];
            reader.read_exact(&mut buf)?;
            Ok(u16::from_le_bytes(buf) as u64)
        }
        0xFE => {
            let mut buf: [u8; 4] = [0; 4];
            reader.read_exact(&mut buf)?;
            Ok(u32::from_le_bytes(buf) as u64)
        }
        0..=0xFC => Ok(prefix as u64),
        _ => panic!("unexpected large offset"),
    }
}

#[derive(Debug)]
pub struct Hints {
    map: BTreeMap<BlockHeight, Vec<u64>>,
}

impl Hints {
    // # Panics
    //
    // Panics when expected data is not present, or the hintfile overflows the maximum blockheight
    pub fn from_file<R: Read>(reader: &mut R) -> Self {
        let mut map = BTreeMap::new();
        let mut height = BlockHeight::from_u32(1);
        while let Ok(count) = read_compact_size(reader) {
            // panics on 32 bit machines
            let mut offsets = Vec::with_capacity(count as usize);
            for _ in 0..count {
                offsets.push(read_compact_size(reader).expect("unexpected end of hintfile"));
            }
            map.insert(height, offsets);
            height = height
                .checked_add(BlockHeightInterval::from_u32(1))
                .expect("hintfile absurdly large.")
        }
        Self { map }
    }

    /// # Panics
    ///
    /// If there are no offset present at that height, aka an overflow, or the entry has already
    /// been fetched.
    pub fn take_block_offsets(&mut self, height: BlockHeight) -> Vec<u64> {
        self.map.remove(&height).expect("block height overflow")
    }
}

#[cfg(test)]
mod tests {
    use crate::{read_compact_size, write_compact_size};

    #[test]
    fn deser_roundtrip() {
        let mut buf = Vec::new();
        let less: u8 = 0xFB;
        write_compact_size(less as u64, &mut buf).unwrap();
        let read_cs = read_compact_size(&mut buf.as_slice()).unwrap();
        let cast_less = read_cs as u8;
        assert_eq!(less, cast_less);
        let mut buf = Vec::new();
        let median: u16 = 0xFFF;
        write_compact_size(median as u64, &mut buf).unwrap();
        let read_cs = read_compact_size(&mut buf.as_slice()).unwrap();
        let cast_median = read_cs as u16;
        assert_eq!(median, cast_median);
        let mut buf = Vec::new();
        let more: u32 = 0xFFFFF;
        write_compact_size(more as u64, &mut buf).unwrap();
        let read_cs = read_compact_size(&mut buf.as_slice()).unwrap();
        let cast_more = read_cs as u32;
        assert_eq!(more, cast_more);
    }
}
